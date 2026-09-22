//! What happens to the vault key (ADR-SEC-029, `VAULT-KEY-AND-LOCATION.md`):
//! open it, create it, move it to Local persistence, recover an interrupted
//! move, erase it, and finish an erasure that left files behind.
//!
//! Pure logic over [`KeyStore`] and [`VaultFiles`]. **The caller holds the
//! key lock (K1)** for the whole of every function here; nothing in this
//! module takes a lock, so the tests can drive it step by step.
//!
//! The rules it keeps, each pinned by the tests:
//! - a key is never lost: the spare is written and verified before main is
//!   rewritten, and deleted only once main reads back the same bytes (D5);
//! - a key never comes back once erased: erasure deletes the spare first,
//!   writes the marker only after both are confirmed gone, and no key is
//!   created or restored while a marker exists (E2, E0, C1, D4);
//! - "no key" is believed only after [`Policy::reads`] reads (R1), and even
//!   then no key is made while data sealed under a key exists (D4);
//! - nothing here ever replaces a main that is present (C2).

use std::path::{Path, PathBuf};
use std::time::Duration;

use tracing::{error, info, warn};
use vault_core::{VaultError, VaultKeyFailure, VaultResult};
use zeroize::Zeroizing;

use super::files::{Marker, VaultFiles};
use super::store::{KeyStore, Slot, StoreError, Stored};

/// The 32-byte master key, wiped on drop.
pub(crate) type Key = Zeroizing<[u8; 32]>;

/// How hard to look before believing "no key" (R1).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Policy {
    /// Reads before a `NoEntry` is believed (at least one).
    pub(crate) reads: u32,
    /// Pause between those reads.
    pub(crate) delay: Duration,
}

impl Policy {
    /// ADR-SEC-029 R1: five reads, 100 ms apart.
    pub(crate) const PRODUCTION: Self = Self {
        reads: 5,
        delay: Duration::from_millis(100),
    };
}

/// The store, the files and the policy one operation works with.
pub(crate) struct Ctx<'a> {
    pub(crate) store: &'a dyn KeyStore,
    pub(crate) files: &'a dyn VaultFiles,
    pub(crate) policy: Policy,
}

/// What an erasure did, for `erasure::ErasureOutcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Erased {
    pub(crate) key_destroyed: bool,
    pub(crate) entries_removed: usize,
    pub(crate) undeletable: Vec<PathBuf>,
}

fn folder_unavailable(what: &str, e: &std::io::Error) -> VaultError {
    warn!(target: "vault_app::keychain", error = %e, "{what}");
    VaultError::VaultKey(VaultKeyFailure::FolderUnavailable)
}

/// A stored value as a key; anything but 32 bytes is not one we wrote.
fn key_from(bytes: &[u8]) -> VaultResult<Key> {
    if bytes.len() != 32 {
        warn!(
            target: "vault_app::keychain",
            len = bytes.len(),
            "a stored vault key is not 32 bytes; it is left untouched"
        );
        return Err(VaultError::VaultKey(VaultKeyFailure::Unusable));
    }
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(bytes);
    Ok(key)
}

impl Ctx<'_> {
    /// R1: `Absent` only after `policy.reads` reads say so.
    fn read_believed(&self, slot: Slot) -> Result<Stored, StoreError> {
        let reads = self.policy.reads.max(1);
        for attempt in 1..=reads {
            match self.store.read(slot)? {
                Stored::Absent => {
                    if attempt < reads {
                        std::thread::sleep(self.policy.delay);
                    }
                }
                present => return Ok(present),
            }
        }
        Ok(Stored::Absent)
    }

    /// D2: `slot` holds exactly `key`, with Local persistence.
    fn holds(&self, slot: Slot, key: &[u8; 32]) -> Result<bool, StoreError> {
        match self.read_believed(slot)? {
            Stored::Present(bytes) if bytes.as_slice() == key.as_slice() => {}
            _ => return Ok(false),
        }
        Ok(self.store.is_local(slot)? == Some(true))
    }

    /// D2: write `key` Local, then check it reads back.
    pub(crate) fn write_verified(&self, slot: Slot, key: &[u8; 32]) -> Result<bool, StoreError> {
        self.store.write_local(slot, key)?;
        self.holds(slot, key)
    }

    /// M4: delete the spare and confirm it is gone. Failures are logged;
    /// the next open's recovery finishes the job.
    fn remove_spare(&self) {
        if let Err(e) = self.store.delete(Slot::Spare) {
            warn!(target: "vault_app::keychain", error = %e.0, "the spare key copy could not be deleted yet; the next open removes it");
            return;
        }
        match self.store.read(Slot::Spare) {
            Ok(Stored::Absent) => {}
            Ok(Stored::Present(_)) => {
                warn!(target: "vault_app::keychain", "the spare key copy is still there; the next open removes it")
            }
            Err(e) => {
                warn!(target: "vault_app::keychain", error = %e.0, "could not confirm the spare key copy is gone")
            }
        }
    }
}

/// One read of main, for readers that never create, move or repair
/// (ADR-SEC-019): relays poll, so no retry here.
pub(crate) fn read_main(store: &dyn KeyStore) -> VaultResult<Option<Key>> {
    match store.read(Slot::Main)? {
        Stored::Absent => Ok(None),
        Stored::Present(bytes) => key_from(&bytes).map(Some),
    }
}

/// Open the key, or create one for a fresh install (C1 → C2 → read → D5,
/// else D4).
pub(crate) fn open_or_create(ctx: &Ctx<'_>) -> VaultResult<Key> {
    let (key, erasure) = open_existing(ctx)?;
    match key {
        Some(key) => Ok(key),
        None => create(ctx, erasure),
    }
}

/// C1, C2, then read main; move it to Local if it is not already (D5).
/// Returns the key, if any, and what C1 found.
pub(crate) fn open_existing(ctx: &Ctx<'_>) -> VaultResult<(Option<Key>, ErasureState)> {
    let erasure = finish_erasure(ctx)?;
    // A marker seen at the start of this open means an erasure was
    // confirmed: no spare may be restored in this open, even once C1 has
    // finished and removed the marker.
    recover(ctx, erasure.seen)?;
    match ctx.read_believed(Slot::Main)? {
        Stored::Absent => Ok((None, erasure)),
        Stored::Present(bytes) => {
            let key = key_from(&bytes)?;
            move_to_local(ctx, &key);
            Ok((Some(key), erasure))
        }
    }
}

/// D4: make a new key only when no marker exists and no data sealed under a
/// key does. The caller has already found main and the spare absent.
pub(crate) fn create(ctx: &Ctx<'_>, erasure: ErasureState) -> VaultResult<Key> {
    if erasure.pending {
        warn!(target: "vault_app::keychain", "an earlier erasure is not finished; no new key is made");
        return Err(VaultError::VaultKey(VaultKeyFailure::FolderUnavailable));
    }
    // D4: the spare must also read "no entry" after R1. Recovery read it
    // once. A spare that shows up now is, normally, left for the next open's
    // recovery to restore, never shadowed by a new key. But in an open that
    // began with a confirmed erasure it can only be a copy of the destroyed
    // key (a delete that silently did nothing, then a false "no entry"): it
    // is deleted, never left for a later open to restore.
    if let Stored::Present(_) = ctx.read_believed(Slot::Spare)? {
        if !erasure.seen {
            warn!(target: "vault_app::keychain", "a spare key copy exists; no new key is made");
            return Err(VaultError::KeychainProvenance(
                "a spare copy of the vault key exists; it is restored at the next open".into(),
            ));
        }
        warn!(target: "vault_app::keychain", "a copy of an erased key was still stored; deleting it");
        ctx.store.delete(Slot::Spare)?;
        confirm_gone(ctx, Slot::Spare)?;
    }
    let present = ctx
        .files
        .keyed_data_present()
        .map_err(|e| folder_unavailable("could not check for existing vault data", &e))?;
    if present {
        error!(
            target: "vault_app::keychain",
            "the vault key is missing but vault data exists; no new key is made (it could not open that data)"
        );
        return Err(VaultError::VaultKey(VaultKeyFailure::Missing));
    }
    let mut key = Zeroizing::new([0u8; 32]);
    getrandom::getrandom(&mut *key).map_err(|e| VaultError::Crypto(format!("getrandom: {e}")))?;
    if !ctx.write_verified(Slot::Main, &key)? {
        return Err(VaultError::KeychainProvenance(
            "a newly created vault key did not read back".into(),
        ));
    }
    info!(target: "vault_app::keychain", "a new vault key was created on this computer");
    Ok(key)
}

/// D5: move main to Local persistence through a verified spare. Never fails
/// the open: on any problem the key stays where it is (still readable) and
/// the next open carries on.
pub(crate) fn move_to_local(ctx: &Ctx<'_>, key: &[u8; 32]) {
    match ctx.store.is_local(Slot::Main) {
        Ok(Some(true)) => return,
        Ok(Some(false)) => {}
        Ok(None) => {
            warn!(target: "vault_app::keychain", "the vault key vanished between two reads; not moving it");
            return;
        }
        Err(e) => {
            warn!(target: "vault_app::keychain", error = %e.0, "could not read the vault key's persistence; not moving it");
            return;
        }
    }
    // M2: a verified copy first. On failure, whatever the spare holds stays
    // (a copy of the key, or nothing): no deletion on a failure path.
    if !matches!(ctx.write_verified(Slot::Spare, key), Ok(true)) {
        warn!(target: "vault_app::keychain", "the spare key copy could not be made; the key stays where it is, and the move is retried next open");
        return;
    }
    // M3: rewrite main in place, once more from memory if the first fails.
    let moved = matches!(ctx.write_verified(Slot::Main, key), Ok(true))
        || matches!(ctx.write_verified(Slot::Main, key), Ok(true));
    if !moved {
        error!(target: "vault_app::keychain", "the vault key could not be rewritten as Local; the verified spare copy is kept and the next open repairs it");
        return;
    }
    // M4: only now, with main reading back the same bytes.
    ctx.remove_spare();
    info!(target: "vault_app::keychain", "the vault key now stays on this computer (Local persistence)");
}

/// C2: finish an interrupted move. Never opens the database, never replaces
/// a main that is present, and deletes the spare only when main holds the
/// same bytes. `erasure_seen`: an erased marker existed when this open
/// began, so a spare is never restored.
pub(crate) fn recover(ctx: &Ctx<'_>, erasure_seen: bool) -> VaultResult<()> {
    let spare = match ctx.store.read(Slot::Spare)? {
        Stored::Absent => return Ok(()),
        Stored::Present(bytes) => bytes,
    };
    if spare.len() != 32 {
        warn!(target: "vault_app::keychain", "a spare key copy is not 32 bytes, so it cannot be a key; deleting it");
        ctx.store.delete(Slot::Spare)?;
        return Ok(());
    }
    let spare = key_from(&spare)?;
    match ctx.read_believed(Slot::Main)? {
        Stored::Present(main) if main.as_slice() == spare.as_slice() => {
            if ctx.store.is_local(Slot::Main)? != Some(true)
                && !ctx.write_verified(Slot::Main, &spare)?
            {
                error!(target: "vault_app::keychain", "the vault key could not be rewritten as Local; the spare copy is kept");
                return Ok(());
            }
            ctx.remove_spare();
            info!(target: "vault_app::keychain", "finished moving the vault key to Local persistence");
            Ok(())
        }
        Stored::Present(_) => {
            error!(target: "vault_app::keychain", "a spare key copy differs from the vault key; keeping the key and leaving the copy");
            Ok(())
        }
        Stored::Absent if erasure_seen => {
            // An erasure was confirmed and not finished: the spare cannot be
            // restored. `finish_erasure` deletes it when it can act; with an
            // untrusted marker it stays, and D4 refuses a new key.
            warn!(target: "vault_app::keychain", "a spare key copy exists while an erasure is unfinished; not restoring it");
            Ok(())
        }
        Stored::Absent => {
            if !ctx.write_verified(Slot::Main, &spare)? {
                error!(target: "vault_app::keychain", "the vault key could not be restored from its spare copy; the copy is kept");
                return Err(VaultError::KeychainProvenance(
                    "the vault key could not be restored from its spare copy".into(),
                ));
            }
            ctx.remove_spare();
            info!(target: "vault_app::keychain", "restored the vault key from its spare copy");
            Ok(())
        }
    }
}

/// What C1 found: whether a marker existed when it started (`seen`), and
/// whether one is still there (`pending`: no key may be created).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ErasureState {
    pub(crate) seen: bool,
    pub(crate) pending: bool,
}

pub(crate) const NO_ERASURE: ErasureState = ErasureState {
    seen: false,
    pending: false,
};
const ERASURE_PENDING: ErasureState = ErasureState {
    seen: true,
    pending: true,
};

/// C1: finish an erasure that left files behind.
pub(crate) fn finish_erasure(ctx: &Ctx<'_>) -> VaultResult<ErasureState> {
    let marker = ctx
        .files
        .marker()
        .map_err(|e| folder_unavailable("could not read the erasure marker", &e))?;
    let folder = match marker {
        Marker::None => return Ok(NO_ERASURE),
        Marker::Invalid => {
            warn!(target: "vault_app::keychain", "an erasure marker cannot be trusted; it is not acted on");
            return Ok(ERASURE_PENDING);
        }
        Marker::Valid(folder) => folder,
    };
    // The marker is written only once the key is confirmed destroyed, so a
    // key here means something outside this design made one (an older
    // build). Delete nothing.
    if let Stored::Present(_) = ctx.read_believed(Slot::Main)? {
        error!(target: "vault_app::keychain", "a vault key exists while an erasure marker is present; nothing is deleted");
        return Ok(ERASURE_PENDING);
    }
    // The erasure was confirmed: a spare cannot be a live key. Deleted and
    // confirmed gone, as E2 does.
    if let Stored::Present(_) = ctx.store.read(Slot::Spare)? {
        ctx.store.delete(Slot::Spare)?;
        confirm_gone(ctx, Slot::Spare)?;
    }
    let report = ctx.files.clean(&folder);
    if !report.left.is_empty() {
        warn!(target: "vault_app::keychain", left = report.left.len(), "an earlier erasure's files cannot all be removed yet");
        if ctx.files.is_own_folder(&folder) {
            return Err(VaultError::VaultKey(VaultKeyFailure::FolderUnavailable));
        }
        return Ok(ERASURE_PENDING);
    }
    ctx.files
        .remove_marker()
        .map_err(|e| folder_unavailable("could not remove the erasure marker", &e))?;
    info!(target: "vault_app::keychain", removed = report.removed, "finished an earlier erasure");
    Ok(ErasureState {
        seen: true,
        pending: false,
    })
}

/// E2 → E0 → files → E3: destroy both credentials (spare first), record the
/// erasure, remove the vault's files, then check again.
pub(crate) fn erase(ctx: &Ctx<'_>, vault_dir: &Path) -> VaultResult<Erased> {
    // E2: delete, don't read first. Any failure here stops everything: no
    // marker, no file touched, and the caller reports a failed wipe.
    let spare_removed = ctx.store.delete(Slot::Spare)?;
    confirm_gone(ctx, Slot::Spare)?;
    let main_removed = ctx.store.delete(Slot::Main)?;
    confirm_gone(ctx, Slot::Main)?;

    // E0: only now, with both confirmed gone. Tried twice: without it, a
    // file the erasure cannot remove would block every later start.
    if let Err(e) = ctx
        .files
        .write_marker(vault_dir)
        .or_else(|_| ctx.files.write_marker(vault_dir))
    {
        warn!(target: "vault_app::keychain", error = %e, "the erasure marker could not be written; leftover files would need removing by hand");
    }

    let report = ctx.files.clean(vault_dir);

    // E3: inside the same lock hold.
    for slot in [Slot::Spare, Slot::Main] {
        match ctx.store.read(slot) {
            Ok(Stored::Absent) => {}
            Ok(Stored::Present(_)) => {
                error!(target: "vault_app::keychain", slot = ?slot, "a vault key reappeared during erasure; destroying it again");
                ctx.store.delete(slot)?;
                confirm_gone(ctx, slot)?;
            }
            // E2 already confirmed it gone; a failed re-check is logged, not
            // turned into "nothing was deleted", which would be untrue.
            Err(e) => {
                warn!(target: "vault_app::keychain", error = %e.0, "could not re-check the vault key after erasure")
            }
        }
    }
    if report.left.is_empty() {
        if let Err(e) = ctx.files.remove_marker() {
            warn!(target: "vault_app::keychain", error = %e, "the erasure marker could not be removed; the next open removes it");
        }
    }
    Ok(Erased {
        key_destroyed: spare_removed || main_removed,
        entries_removed: report.removed,
        undeletable: report.left,
    })
}

/// What starting again (ADR-105 L6) did about an erased marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkerSettled {
    /// There was none.
    NoMarker,
    /// Main and the spare both read "no entry" after R1: the key is
    /// destroyed, so everything the marker named is dead. Removed.
    Removed,
    /// A key, or a spare copy of one, exists: the marker stays, and so does
    /// the key.
    KeptWithKey,
}

/// ADR-105 L6's key half, for "Start again in the default place" (the
/// caller holds K1). Any marker counts, trusted or not: which folder it
/// names does not matter once the key is gone. Never creates, writes or
/// deletes a key; any failure decides nothing.
pub(crate) fn settle_marker_to_start_again(ctx: &Ctx<'_>) -> VaultResult<MarkerSettled> {
    let marker = ctx
        .files
        .marker()
        .map_err(|e| folder_unavailable("could not read the erasure marker", &e))?;
    if marker == Marker::None {
        return Ok(MarkerSettled::NoMarker);
    }
    let destroyed = matches!(ctx.read_believed(Slot::Main)?, Stored::Absent)
        && matches!(ctx.read_believed(Slot::Spare)?, Stored::Absent);
    if !destroyed {
        warn!(target: "vault_app::keychain", "starting again while a vault key exists: the erasure marker is kept, and so is the key");
        return Ok(MarkerSettled::KeptWithKey);
    }
    ctx.files
        .remove_marker()
        .map_err(|e| folder_unavailable("could not remove the erasure marker", &e))?;
    info!(target: "vault_app::keychain", "the vault key was confirmed destroyed; the erasure marker is removed to start again");
    Ok(MarkerSettled::Removed)
}

/// E2's confirmation: one read, which must say "no entry".
fn confirm_gone(ctx: &Ctx<'_>, slot: Slot) -> VaultResult<()> {
    match ctx.store.read(slot)? {
        Stored::Absent => Ok(()),
        Stored::Present(_) => Err(VaultError::KeychainProvenance(
            "the vault key was still readable after it was deleted; erasure did NOT happen".into(),
        )),
    }
}
