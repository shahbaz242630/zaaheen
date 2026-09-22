//! Moving the memories to a folder the person chose, and finishing or
//! undoing a move that was interrupted (ADR-105 L5, `VAULT-KEY-AND-LOCATION.md`).
//!
//! **The promise:** at every moment exactly one complete vault is named by
//! the location record. The copy is made beside the memories, checked file
//! by file, and only then does the record change (one atomic write); the old
//! copy is removed after that. A start that finds a move half done removes
//! only a folder carrying that move's ID, and tries again; after
//! [`MAX_ATTEMPTS`] the move is given up and the person told why.
//!
//! **Order at the desktop's start, before anything opens the vault or the
//! key** ([`run_pending`]):
//! - `pending_cleanup` → the old copy of a finished move is removed (M6);
//! - `pending_move` → M1 take the vault (the keeper hands over; no window
//!   still has it open), then under the key lock K1: M2 create the folder,
//!   restrict it, write `.move-id`; M3 copy; M4 verify; M5 commit the
//!   record; then M6.
//!
//! Every record write happens under K1, as the first-run setup's does.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info, warn};

use super::check::{self, CheckContext, CheckEnv, Checked, Refusal};
use super::pointer::{self, PendingCleanup, PendingMove, Pointer};
use super::{Homes, MOVE_ID_FILE, VAULT_ID_FILE};
use crate::erasure::VAULT_ENTRIES;
use crate::keeper::exclusive::take_exclusive;
use crate::keeper::intent::{self, IntentGuard};
use crate::keeper::relay::MasterKeySource;
use crate::keychain::{with_key_lock, KeyLocation};
use crate::{ConsolidatorLock, VAULT_LOCKFILE_NAME};
use fs::{Digest, DiskFs, Item, MoveFs};

mod asking;
pub(crate) mod fs;
mod progress;

pub use asking::{check_request, forget_old_copy, status, ForgetRefusal, OldCopy, Status};
pub use progress::{MovePhase, MoveProgress, MoveSnapshot};

#[cfg(test)]
mod asking_tests;
#[cfg(test)]
mod crash_tests;
#[cfg(test)]
mod model;
#[cfg(test)]
mod progress_tests;
#[cfg(test)]
mod start_tests;
#[cfg(test)]
mod tests;

/// Attempts a move gets before it is given up (ADR-105 L5: "after two
/// failures").
pub const MAX_ATTEMPTS: u32 = 2;

/// How long a start waits to have the memories to itself: a keeper serving
/// an AI app lets go in well under a second, a window that is closing is
/// gone in a few; a maintenance run is not interrupted and outlasts this.
const TAKE_WAIT: Duration = Duration::from_secs(30);

/// Bound on each step of asking the keeper to hand over.
const TAKE_STEP: Duration = Duration::from_secs(5);

/// How often a start looks again while it waits.
const POLL: Duration = Duration::from_millis(100);

/// What a move copies (M3): the data sealed under the key, then
/// `maintenance.json`, and `.vault-id` last — never the keeper's files, the
/// folder marker (`.acl-v1`) or the lockfiles. Pinned against the key's list
/// of sealed data by `tests::only_the_memories_move`.
const MOVED: &[&str] = &[
    "vault.db",
    "vault.db-wal",
    "vault.db-shm",
    "lance",
    "graph.duckdb",
    "graph.sealed",
    vault_storage::REPORTS_DIRNAME,
    "maintenance.json",
    VAULT_ID_FILE,
];

/// What a start did about a move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveOutcome {
    /// Nothing was waiting.
    Nothing,
    /// The memories now live in `to`.
    Moved {
        to: PathBuf,
        /// The new folder could not be restricted to this Windows account
        /// (FAT/exFAT drives): the memories stay encrypted (L4.7).
        not_restricted: bool,
        /// The old copy could not all be removed yet; the next start tries
        /// again.
        old_copy_waiting: bool,
    },
    /// An earlier move's old copy is now gone.
    OldCopyRemoved,
    /// An earlier move's old copy still could not be removed.
    OldCopyWaiting,
    /// This attempt did not move anything; the memories are where they
    /// were. `retrying`: the next start tries again; otherwise the move was
    /// given up.
    Failed { reason: MoveFailure, retrying: bool },
    /// The memories could not be had to itself now (an AI app's connection
    /// did not let go, maintenance is running, or a window still has them
    /// open). Nothing was counted; the next start tries again. The memories
    /// open where they are.
    Deferred,
    /// Another Zaaheen window is moving or erasing the memories: this one
    /// must not open them.
    Busy,
}

/// Why an attempt failed. Each has its own words on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveFailure {
    /// The chosen folder can no longer be used (gone, taken, or now refused
    /// by the folder checks).
    FolderNotUsable,
    /// The drive filled up during the copy.
    NotEnoughSpace,
    /// Reading the memories or writing the copy failed.
    CopyFailed,
    /// The copy did not match the memories.
    CopyDidNotMatch,
    /// An earlier attempt stopped part-way (Zaaheen closed, or the computer
    /// went off).
    Interrupted,
    /// "Delete everything" has not finished: there is nothing to move.
    MemoriesErased,
    /// The location record could not be updated.
    RecordFailed,
}

/// Why a move could not be asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestRefusal {
    /// The chosen folder was refused (L4).
    Folder(Refusal),
    /// An earlier move's old copy is still being removed.
    OldCopyWaiting,
    /// A move is already waiting for the next start.
    MoveWaiting,
    /// "Delete everything" has not finished.
    ErasureUnfinished,
    /// Where the memories are now cannot be found or read.
    VaultUnavailable,
    /// The record could not be read or written (or its lock was busy).
    RecordFailed,
}

fn disk(homes: &Homes, key: &KeyLocation) -> DiskFs {
    DiskFs::new(homes.pointer_path(), key.marker_path())
}

/// The names a move copies, for the tests.
#[cfg(test)]
pub(crate) fn moved_names() -> Vec<&'static str> {
    MOVED.to_vec()
}

/// Bytes the memories in `dir` take.
fn vault_size(fs: &dyn MoveFs, dir: &Path) -> io::Result<u64> {
    Ok(fs
        .list(dir, MOVED)?
        .iter()
        .map(|item| match item {
            Item::File { len, .. } => *len,
            Item::Dir(_) => 0,
        })
        .fold(0u64, u64::saturating_add))
}

// ── asking for a move ─────────────────────────────────────────────────────

/// Ask for a move to `chosen` (L5): refused while an old copy is still
/// being removed, a move is waiting, or an erasure is unfinished; otherwise
/// the folder is checked (L4) and the record gets `pending_move`. Nothing
/// else changes until the next start.
///
/// # Errors
///
/// The [`RequestRefusal`] to show.
pub fn request_move(
    homes: &Homes,
    key: &KeyLocation,
    chosen: &Path,
    env: &dyn CheckEnv,
) -> Result<Checked, RequestRefusal> {
    let fs = disk(homes, key);
    match with_key_lock(key, || Ok(request_under_lock(homes, &fs, chosen, env))) {
        Ok(decided) => decided,
        Err(e) => {
            warn!(target: "vault_app::location", error = %e, "a move could not be recorded: the key lock");
            Err(RequestRefusal::RecordFailed)
        }
    }
}

fn request_under_lock(
    homes: &Homes,
    fs: &dyn MoveFs,
    chosen: &Path,
    env: &dyn CheckEnv,
) -> Result<Checked, RequestRefusal> {
    let (checked, mut record) = decide(homes, fs, chosen, env)?;
    let move_id = pointer::new_id().map_err(|_| RequestRefusal::RecordFailed)?;
    record.pending_move = Some(PendingMove {
        to: checked.target.clone(),
        move_id,
        attempts: 0,
    });
    fs.write_pointer(&record)
        .map_err(|_| RequestRefusal::RecordFailed)?;
    info!(target: "vault_app::location", to = %checked.target.display(), "a move of the memories is waiting for the next start");
    Ok(checked)
}

/// L5's refusals, then L4: where a move to `chosen` would go, and the
/// record it would be added to. Writes nothing; shared by asking for a move
/// and by checking a folder before the person confirms
/// ([`asking::check_request`]), so the two can never disagree.
fn decide(
    homes: &Homes,
    fs: &dyn MoveFs,
    chosen: &Path,
    env: &dyn CheckEnv,
) -> Result<(Checked, Pointer), RequestRefusal> {
    let current = super::resolve(homes).map_err(|_| RequestRefusal::VaultUnavailable)?;
    let record = current.pointer().clone();
    if record.pending_cleanup.is_some() {
        return Err(RequestRefusal::OldCopyWaiting);
    }
    if record.pending_move.is_some() {
        return Err(RequestRefusal::MoveWaiting);
    }
    match fs.marker_exists() {
        Ok(false) => {}
        Ok(true) => return Err(RequestRefusal::ErasureUnfinished),
        Err(_) => return Err(RequestRefusal::RecordFailed),
    }
    let size = vault_size(fs, current.path()).map_err(|_| RequestRefusal::VaultUnavailable)?;
    let models = super::models_dir(homes);
    let ctx = CheckContext {
        homes,
        current_vault: Some(current.path()),
        models_dir: &models,
        vault_size: size,
    };
    let checked = check::check_folder(chosen, &ctx, env).map_err(RequestRefusal::Folder)?;
    Ok((checked, record))
}

// ── at a start ────────────────────────────────────────────────────────────

/// The folder a waiting move goes to, if the record holds one (ADR-105 L-f):
/// only such a start shows the moving screen. No record, one that cannot be
/// read, or only an old copy still to remove: `None`, and that start goes
/// the usual way, where the resolver reports anything wrong.
pub fn waiting_move(homes: &Homes) -> Option<PathBuf> {
    match pointer::read(&homes.pointer_path()) {
        Ok(Some(record)) => record.pending_move.map(|pending| pending.to),
        Ok(None) | Err(_) => None,
    }
}

/// Everything a start must do about a move, before anything opens the vault
/// or the key: remove an old copy, then run a waiting move.
pub async fn run_pending(
    homes: &Homes,
    key: &KeyLocation,
    keys: &dyn MasterKeySource,
) -> MoveOutcome {
    run_pending_reporting(homes, key, keys, &Arc::new(MoveProgress::default())).await
}

/// [`run_pending`], reporting how far a waiting move has got to `progress`
/// (ADR-105 L-f); the outcome is the same either way.
pub async fn run_pending_reporting(
    homes: &Homes,
    key: &KeyLocation,
    keys: &dyn MasterKeySource,
    progress: &Arc<MoveProgress>,
) -> MoveOutcome {
    run_pending_with(
        homes,
        key,
        keys,
        check::SystemEnv,
        TAKE_WAIT,
        TAKE_STEP,
        progress,
    )
    .await
}

/// [`run_pending_reporting`] with the folder checks and the waits given.
pub(crate) async fn run_pending_with<E: CheckEnv + Send + 'static>(
    homes: &Homes,
    key: &KeyLocation,
    keys: &dyn MasterKeySource,
    env: E,
    wait: Duration,
    step: Duration,
    progress: &Arc<MoveProgress>,
) -> MoveOutcome {
    let fs = disk(homes, key);
    let record = match fs.read_pointer() {
        Ok(Some(record)) => record,
        // No record: nothing was ever moved. A damaged one is the
        // resolver's to report.
        Ok(None) | Err(_) => return MoveOutcome::Nothing,
    };
    let mut outcome = MoveOutcome::Nothing;
    if record.pending_cleanup.is_some() {
        let (h, k) = (homes.clone(), key.clone());
        outcome = tokio::task::spawn_blocking(move || clean_old_copy_now(&h, &k))
            .await
            .unwrap_or(MoveOutcome::OldCopyWaiting);
        if outcome != MoveOutcome::OldCopyRemoved {
            return outcome;
        }
    }
    let record = match fs.read_pointer() {
        Ok(Some(record)) if record.pending_move.is_some() => record,
        _ => return outcome,
    };
    let source = record.vault_dir;
    if super::resolve(homes).is_err() {
        warn!(target: "vault_app::location", "a move is waiting but the memories cannot be found");
        return MoveOutcome::Deferred;
    }
    if intent::is_held(&source) {
        info!(target: "vault_app::location", "another window holds the memories; this one waits");
        return MoveOutcome::Busy;
    }

    // M1: the keeper hands the vault over; no keeper starts meanwhile.
    progress.begin(MovePhase::Waiting, 0);
    let deadline = tokio::time::Instant::now() + wait;
    let held = match take_exclusive(&source, keys, wait, step).await {
        Ok(held) => held,
        Err(e) => {
            if intent::is_held(&source) {
                return MoveOutcome::Busy;
            }
            info!(target: "vault_app::location", error = %e, "the memories could not be had for the move; it waits for the next start");
            return MoveOutcome::Deferred;
        }
    };
    // M1, continued: the desktop takes no vault lock, so a window that has
    // not finished closing — or a second one — is seen by its open database.
    loop {
        match fs::database_in_use(&source) {
            Ok(false) => break,
            Ok(true) if tokio::time::Instant::now() < deadline => tokio::time::sleep(POLL).await,
            Ok(true) => {
                info!(target: "vault_app::location", "a window still has the memories open; the move waits for the next start");
                return MoveOutcome::Deferred;
            }
            Err(e) => {
                warn!(target: "vault_app::location", error = %e, "could not check whether the memories are open");
                return MoveOutcome::Deferred;
            }
        }
    }

    let (h, k) = (homes.clone(), key.clone());
    let reporting = disk(homes, key).reporting_to(Arc::clone(progress));
    let moved = tokio::task::spawn_blocking(move || {
        // Held to the end: M6 removes the old copy while no one can open it.
        let _held = held;
        move_holding_source(&h, &k, &reporting, &env)
    })
    .await;
    moved.unwrap_or_else(|e| {
        error!(target: "vault_app::location", error = %e, "the move stopped unexpectedly");
        MoveOutcome::Deferred
    })
}

/// Remove the old copy an earlier move left, if nothing is using it; used at
/// start and after "Delete everything" (L5: an erasure also cleans it).
pub fn clean_old_copy_now(homes: &Homes, key: &KeyLocation) -> MoveOutcome {
    let fs = disk(homes, key);
    let from = match fs.read_pointer() {
        Ok(Some(Pointer {
            pending_cleanup: Some(cleanup),
            ..
        })) => cleanup.from,
        Ok(_) => return MoveOutcome::Nothing,
        Err(_) => return MoveOutcome::OldCopyWaiting,
    };
    // Nothing resolves to it any more, but a window that has not finished
    // closing may still have it open: taken without waiting, or left.
    let Ok(_intent) = IntentGuard::acquire(&from, Duration::ZERO) else {
        return MoveOutcome::OldCopyWaiting;
    };
    let Ok(_lock) = ConsolidatorLock::try_acquire_named(&from, VAULT_LOCKFILE_NAME) else {
        return MoveOutcome::OldCopyWaiting;
    };
    if !matches!(fs::database_in_use(&from), Ok(false)) {
        return MoveOutcome::OldCopyWaiting;
    }
    clean_holding_old(key, &fs)
}

// ── the move ──────────────────────────────────────────────────────────────

/// A committed move whose old copy is still to go.
struct Committed {
    from: PathBuf,
    to: PathBuf,
    move_id: String,
    not_restricted: bool,
}

enum Attempt {
    Done(MoveOutcome),
    Committed(Committed),
}

/// Run a waiting move. The caller holds the vault folder exclusively (M1).
pub(crate) fn move_holding_source(
    homes: &Homes,
    key: &KeyLocation,
    fs: &dyn MoveFs,
    env: &dyn CheckEnv,
) -> MoveOutcome {
    match with_key_lock(key, || Ok(attempt_under_lock(homes, fs, env))) {
        Ok(Attempt::Done(outcome)) => outcome,
        Ok(Attempt::Committed(committed)) => finish(key, fs, committed),
        Err(e) => {
            info!(target: "vault_app::location", error = %e, "the key lock was busy; the move waits for the next start");
            MoveOutcome::Deferred
        }
    }
}

/// Under K1: decide, count the attempt, then M2–M5.
fn attempt_under_lock(homes: &Homes, fs: &dyn MoveFs, env: &dyn CheckEnv) -> Attempt {
    let record = match fs.read_pointer() {
        Ok(Some(record)) => record,
        Ok(None) => return Attempt::Done(MoveOutcome::Nothing),
        Err(e) => {
            warn!(target: "vault_app::location", error = %e, "the location record cannot be read; no move");
            return Attempt::Done(MoveOutcome::Deferred);
        }
    };
    let Some(pending) = record.pending_move.clone() else {
        return Attempt::Done(MoveOutcome::Nothing);
    };
    match fs.marker_exists() {
        Ok(false) => {}
        Ok(true) => {
            return Attempt::Done(give_up(fs, &record, &pending, MoveFailure::MemoriesErased))
        }
        Err(_) => return Attempt::Done(MoveOutcome::Deferred),
    }
    if pending.attempts >= MAX_ATTEMPTS {
        return Attempt::Done(give_up(fs, &record, &pending, MoveFailure::Interrupted));
    }
    // Counted before anything else happens, so a crash counts too.
    let attempt = pending.attempts.saturating_add(1);
    let mut counted = record.clone();
    counted.pending_move = Some(PendingMove {
        attempts: attempt,
        ..pending.clone()
    });
    if fs.write_pointer(&counted).is_err() {
        return Attempt::Done(MoveOutcome::Failed {
            reason: MoveFailure::RecordFailed,
            retrying: true,
        });
    }
    match copy_and_commit(homes, fs, env, &counted, &pending) {
        Ok(committed) => Attempt::Committed(committed),
        Err(reason) => Attempt::Done(after_failure(fs, &pending, attempt, reason)),
    }
}

/// M2–M5: a new folder, the copy, the check, the switch.
fn copy_and_commit(
    homes: &Homes,
    fs: &dyn MoveFs,
    env: &dyn CheckEnv,
    record: &Pointer,
    pending: &PendingMove,
) -> Result<Committed, MoveFailure> {
    let source = &record.vault_dir;
    let to = &pending.to;
    // Whatever an earlier attempt left there.
    remove_half_copy(fs, pending).map_err(|()| MoveFailure::FolderNotUsable)?;
    // The folder, checked again (L4): the drive may have changed since.
    let size = vault_size(fs, source).map_err(|_| MoveFailure::CopyFailed)?;
    let models = super::models_dir(homes);
    let ctx = CheckContext {
        homes,
        current_vault: Some(source),
        models_dir: &models,
        vault_size: size,
    };
    let parent = to.parent().ok_or(MoveFailure::FolderNotUsable)?;
    match check::check_folder(parent, &ctx, env) {
        Ok(checked) if checked.target == *to => {}
        Ok(_) => return Err(MoveFailure::FolderNotUsable),
        Err(Refusal::NotEnoughSpace) => return Err(MoveFailure::NotEnoughSpace),
        Err(_) => return Err(MoveFailure::FolderNotUsable),
    }

    // M2: the folder, restricted, then marked as this move's.
    fs.create_dir(to)
        .map_err(|_| MoveFailure::FolderNotUsable)?;
    let not_restricted = match fs.restrict(to) {
        Ok(()) => false,
        Err(e) => {
            info!(target: "vault_app::location", error = %e, "the new folder could not be restricted (no permissions on this drive)");
            true
        }
    };
    fs.write_file(&to.join(MOVE_ID_FILE), pending.move_id.as_bytes())
        .map_err(|_| MoveFailure::FolderNotUsable)?;

    // M3: the copy.
    let items = fs
        .list(source, MOVED)
        .map_err(|_| MoveFailure::CopyFailed)?;
    let to_copy = items
        .iter()
        .map(|item| match item {
            Item::File { len, .. } => *len,
            Item::Dir(_) => 0,
        })
        .fold(0u64, u64::saturating_add);
    fs.report_phase(MovePhase::Copying, to_copy);
    let mut copied: Vec<(PathBuf, u64, Digest)> = Vec::new();
    for item in &items {
        match item {
            Item::Dir(rel) => fs.create_dir(&to.join(rel)).map_err(|e| copy_failure(&e))?,
            Item::File { rel, len } => {
                let digest = fs
                    .copy_file(&source.join(rel), &to.join(rel))
                    .map_err(|e| copy_failure(&e))?;
                copied.push((rel.clone(), *len, digest));
            }
        }
    }

    // M4: every copied file, read back and checked.
    let to_check = copied
        .iter()
        .map(|(_, len, _)| *len)
        .fold(0u64, u64::saturating_add);
    fs.report_phase(MovePhase::Checking, to_check);
    for (rel, len, digest) in &copied {
        match fs.digest(&to.join(rel)) {
            Ok(got) if got == *digest && got.len == *len => {}
            Ok(_) => return Err(MoveFailure::CopyDidNotMatch),
            Err(_) => return Err(MoveFailure::CopyFailed),
        }
    }

    // M5: the switch — one atomic write of the record.
    fs.report_phase(MovePhase::Finishing, 0);
    let mut committed = record.clone();
    committed.vault_dir = to.clone();
    committed.pending_move = None;
    committed.pending_cleanup = Some(PendingCleanup {
        from: source.clone(),
        move_id: pending.move_id.clone(),
    });
    fs.write_pointer(&committed)
        .map_err(|_| MoveFailure::RecordFailed)?;
    info!(target: "vault_app::location", to = %to.display(), files = copied.len(), "the memories were moved");
    Ok(Committed {
        from: source.clone(),
        to: to.clone(),
        move_id: pending.move_id.clone(),
        not_restricted,
    })
}

/// A full drive has its own words; anything else is a failed copy.
fn copy_failure(e: &io::Error) -> MoveFailure {
    // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL; ENOSPC.
    let full: &[i32] = if cfg!(windows) { &[39, 112] } else { &[28] };
    match e.raw_os_error() {
        Some(code) if full.contains(&code) => MoveFailure::NotEnoughSpace,
        _ => MoveFailure::CopyFailed,
    }
}

/// After a failed attempt: the half-made copy goes, and after the last
/// attempt the move is given up. Only while the record still names this
/// move — nothing is removed on the strength of a record that moved on.
fn after_failure(
    fs: &dyn MoveFs,
    pending: &PendingMove,
    attempt: u32,
    reason: MoveFailure,
) -> MoveOutcome {
    warn!(target: "vault_app::location", ?reason, attempt, "a move attempt failed; the memories stay where they were");
    let record = match fs.read_pointer() {
        Ok(Some(record))
            if record.pending_cleanup.is_none()
                && record
                    .pending_move
                    .as_ref()
                    .is_some_and(|m| m.move_id == pending.move_id) =>
        {
            record
        }
        _ => {
            return MoveOutcome::Failed {
                reason,
                retrying: true,
            }
        }
    };
    if attempt >= MAX_ATTEMPTS {
        return give_up(fs, &record, pending, reason);
    }
    if remove_half_copy(fs, pending).is_err() {
        warn!(target: "vault_app::location", to = %pending.to.display(), "the half-made copy could not all be removed yet");
    }
    MoveOutcome::Failed {
        reason,
        retrying: true,
    }
}

/// The move is over without moving: its half-made copy goes and the record
/// forgets it.
fn give_up(
    fs: &dyn MoveFs,
    record: &Pointer,
    pending: &PendingMove,
    reason: MoveFailure,
) -> MoveOutcome {
    if remove_half_copy(fs, pending).is_err() {
        warn!(target: "vault_app::location", to = %pending.to.display(), "a given-up move's folder could not be removed");
    }
    let mut cleared = record.clone();
    cleared.pending_move = None;
    if let Err(e) = fs.write_pointer(&cleared) {
        warn!(target: "vault_app::location", error = %e, "a given-up move could not be cleared from the record");
    }
    warn!(target: "vault_app::location", ?reason, "the move was given up; the memories stay where they were");
    MoveOutcome::Failed {
        reason,
        retrying: false,
    }
}

/// Recovery: the target is removed only when it carries this move's ID
/// (only the vault's own names, `.move-id` last, then the folder itself,
/// which is refused if anything else is in it) or when a crash came before
/// `.move-id` was in place — the folder is empty, or holds nothing but that
/// file still being written, with (the start of) this move's ID. Anything
/// else there is never touched.
fn remove_half_copy(fs: &dyn MoveFs, pending: &PendingMove) -> Result<(), ()> {
    let to = &pending.to;
    match fs.exists(to) {
        Ok(false) => return Ok(()),
        Ok(true) => {}
        Err(_) => return Err(()),
    }
    let id_file = to.join(MOVE_ID_FILE);
    let id_writing = pointer::being_written(&id_file);
    match fs.read_text(&id_file) {
        Ok(Some(id)) if id == pending.move_id => {
            for name in VAULT_ENTRIES.iter().chain(std::iter::once(&VAULT_ID_FILE)) {
                remove_if_there(fs, &to.join(name))?;
            }
            remove_if_there(fs, &id_writing)?;
            fs.remove_entry(&id_file).map_err(|_| ())?;
            fs.remove_dir(to).map_err(|_| ())
        }
        Ok(None) => {
            let names = fs.entry_names(to).map_err(|_| ())?;
            match names.as_slice() {
                [] => {}
                [only] if id_writing.file_name().is_some_and(|w| w == only.as_str()) => {
                    match fs.read_text(&id_writing) {
                        Ok(Some(text)) if pending.move_id.starts_with(&text) => {
                            fs.remove_entry(&id_writing).map_err(|_| ())?;
                        }
                        _ => return Err(()),
                    }
                }
                _ => return Err(()),
            }
            fs.remove_dir(to).map_err(|_| ())
        }
        // Another move's ID, or one that cannot be read: not this move's.
        Ok(Some(_)) | Err(_) => Err(()),
    }
}

fn remove_if_there(fs: &dyn MoveFs, path: &Path) -> Result<(), ()> {
    match fs.exists(path) {
        Ok(false) => Ok(()),
        Ok(true) => fs.remove_entry(path).map_err(|_| ()),
        Err(_) => Err(()),
    }
}

// ── the old copy (M6) ────────────────────────────────────────────────────

/// M6 of the move that just committed; the caller still holds the old
/// folder.
fn finish(key: &KeyLocation, fs: &dyn MoveFs, c: Committed) -> MoveOutcome {
    let removed =
        remove_old_copy(fs, &c.from, &c.to, &c.move_id) && clear_cleanup(key, fs, &c.move_id);
    MoveOutcome::Moved {
        to: c.to,
        not_restricted: c.not_restricted,
        old_copy_waiting: !removed,
    }
}

/// Remove an earlier move's old copy (M6). The caller holds that folder.
pub(crate) fn clean_holding_old(key: &KeyLocation, fs: &dyn MoveFs) -> MoveOutcome {
    let record = match fs.read_pointer() {
        Ok(Some(record)) => record,
        Ok(None) => return MoveOutcome::Nothing,
        Err(_) => return MoveOutcome::OldCopyWaiting,
    };
    let Some(cleanup) = record.pending_cleanup else {
        return MoveOutcome::Nothing;
    };
    // A folder that cannot be seen (a drive that is out) is waited for,
    // never forgotten: the old copy is still readable on this computer.
    if !matches!(fs.exists(&cleanup.from), Ok(true)) {
        return MoveOutcome::OldCopyWaiting;
    }
    if remove_old_copy(fs, &cleanup.from, &record.vault_dir, &cleanup.move_id)
        && clear_cleanup(key, fs, &cleanup.move_id)
    {
        MoveOutcome::OldCopyRemoved
    } else {
        MoveOutcome::OldCopyWaiting
    }
}

/// M6's file work: `.move-id` out of the new folder, then the old folder's
/// vault entries (erasure's bounded list) and, once they are all gone, its
/// `.vault-id`. **Never the folder the record names.** `true` when nothing
/// of the old copy is left.
fn remove_old_copy(fs: &dyn MoveFs, from: &Path, live: &Path, move_id: &str) -> bool {
    let id_file = live.join(MOVE_ID_FILE);
    if matches!(fs.read_text(&id_file), Ok(Some(ref id)) if id == move_id)
        && fs.remove_entry(&id_file).is_err()
    {
        warn!(target: "vault_app::location", "the move's ID file could not be removed from the new folder");
    }
    if fs.same_dir(from, live) {
        error!(target: "vault_app::location", "the old copy is the folder in use; it is never cleaned");
        return true;
    }
    let mut all = true;
    for name in VAULT_ENTRIES {
        if remove_if_there(fs, &from.join(name)).is_err() {
            all = false;
        }
    }
    if all && remove_if_there(fs, &from.join(VAULT_ID_FILE)).is_err() {
        all = false;
    }
    if !all {
        warn!(target: "vault_app::location", from = %from.display(), "the old copy could not all be removed yet; the next start tries again");
    }
    all
}

/// Under K1: the record forgets the old copy — only this move's.
fn clear_cleanup(key: &KeyLocation, fs: &dyn MoveFs, move_id: &str) -> bool {
    with_key_lock(key, || {
        let Ok(Some(mut record)) = fs.read_pointer() else {
            return Ok(false);
        };
        if !record
            .pending_cleanup
            .as_ref()
            .is_some_and(|c| c.move_id == move_id)
        {
            return Ok(false);
        }
        record.pending_cleanup = None;
        Ok(fs.write_pointer(&record).is_ok())
    })
    .unwrap_or(false)
}
