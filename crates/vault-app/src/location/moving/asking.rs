//! What the screens ask about a move (ADR-105 L-e): a folder checked
//! before the person confirms, where things stand, and letting go of an old
//! copy on a drive that never comes back (L-d decision 9).

use std::path::{Path, PathBuf};

use tracing::warn;
use vault_core::VaultResult;

use super::check::{CheckEnv, Checked};
use super::fs::MoveFs;
use super::{decide, disk, Homes, RequestRefusal};
use crate::keychain::{with_key_lock, KeyLocation};

/// L5's refusals and L4 for `chosen`, exactly as [`super::request_move`]
/// decides them, **recording nothing**: what the screen shows before the
/// person confirms. Takes no lock (it only reads, and the record is written
/// atomically); `request_move` decides again under the lock.
///
/// # Errors
///
/// The [`RequestRefusal`] to show.
pub fn check_request(
    homes: &Homes,
    key: &KeyLocation,
    chosen: &Path,
    env: &dyn CheckEnv,
) -> Result<Checked, RequestRefusal> {
    decide(homes, &disk(homes, key), chosen, env).map(|(checked, _)| checked)
}

/// Where things stand, for the screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The folder the memories are in now.
    pub folder: PathBuf,
    /// A move asked for and not yet made: where it goes.
    pub move_waiting: Option<PathBuf>,
    /// An earlier move's old copy, still to be removed.
    pub old_copy: Option<OldCopy>,
}

/// An earlier move's old copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OldCopy {
    pub from: PathBuf,
    /// Whether its folder can be seen now. One that cannot (a drive that is
    /// out) is what a start waits for, and what may be forgotten.
    pub connected: bool,
}

/// Where things stand, read from the record. Reads only.
///
/// # Errors
///
/// As [`super::super::resolve`]: the memories cannot be found.
pub fn status(homes: &Homes) -> VaultResult<Status> {
    let dir = super::super::resolve(homes)?;
    let record = dir.pointer();
    Ok(Status {
        folder: dir.path().to_path_buf(),
        move_waiting: record.pending_move.as_ref().map(|m| m.to.clone()),
        old_copy: record.pending_cleanup.as_ref().map(|c| OldCopy {
            from: c.from.clone(),
            connected: matches!(c.from.try_exists(), Ok(true)),
        }),
    })
}

/// Why an old copy was not forgotten. Nothing changed in any case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForgetRefusal {
    /// No old copy is waiting to be removed.
    NothingWaiting,
    /// Its folder can be seen: the next start removes it.
    StillThere,
    /// The record could not be read or written (or its lock was busy).
    RecordFailed,
}

/// Stop waiting for an old copy whose folder cannot be seen — a drive that
/// never comes back would otherwise block every later move (L-d decision
/// 9). Under K1, only `pending_cleanup` is cleared; **nothing is removed**,
/// and the folder is named back so the person can be told to delete it if
/// it turns up (it is encrypted under this computer's key).
///
/// # Errors
///
/// The [`ForgetRefusal`] to show.
pub fn forget_old_copy(homes: &Homes, key: &KeyLocation) -> Result<PathBuf, ForgetRefusal> {
    let fs = disk(homes, key);
    match with_key_lock(key, || Ok(forget_under_lock(&fs))) {
        Ok(decided) => decided,
        Err(e) => {
            warn!(target: "vault_app::location", error = %e, "an old copy could not be forgotten: the key lock");
            Err(ForgetRefusal::RecordFailed)
        }
    }
}

fn forget_under_lock(fs: &dyn MoveFs) -> Result<PathBuf, ForgetRefusal> {
    let mut record = match fs.read_pointer() {
        Ok(Some(record)) => record,
        Ok(None) => return Err(ForgetRefusal::NothingWaiting),
        Err(_) => return Err(ForgetRefusal::RecordFailed),
    };
    let Some(cleanup) = record.pending_cleanup.take() else {
        return Err(ForgetRefusal::NothingWaiting);
    };
    // Exactly what a start waits for rather than removes (`clean_holding_old`):
    // a folder that cannot be seen. One that can is removed, never forgotten.
    if matches!(fs.exists(&cleanup.from), Ok(true)) {
        return Err(ForgetRefusal::StillThere);
    }
    fs.write_pointer(&record)
        .map_err(|_| ForgetRefusal::RecordFailed)?;
    warn!(
        target: "vault_app::location",
        from = %cleanup.from.display(),
        "the person chose to stop waiting for an old copy whose folder cannot be seen; Zaaheen will not remove it"
    );
    Ok(cleanup.from)
}
