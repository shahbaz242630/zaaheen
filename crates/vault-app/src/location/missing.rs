//! When the memories cannot be found where the record says (ADR-105 L1,
//! L6): telling the reasons apart for the startup screen, and "Start again
//! in the default place".
//!
//! [`diagnose`] only reads. [`start_again`] is offered **only when the
//! recorded folder itself is not there** (a drive that is out, a folder
//! removed): a folder that is there with a missing or different `.vault-id`
//! may hold the memories, so that screen points to support instead. It
//! never deletes anything — it records a new, empty place and lets the next
//! open make the vault there.

use std::path::PathBuf;

use tracing::{info, warn};
use vault_core::{VaultError, VaultKeyFailure, VaultLocationFailure, VaultResult};

use super::pointer::{self, Pointer};
use super::{resolve, Homes, VaultDir, VAULT_ID_FILE};
use crate::keychain::lifecycle::MarkerSettled;
use crate::keychain::{self, KeyLocation, KeyedPaths};

/// Why the recorded location cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The record itself cannot be read (damaged, or written by a version
    /// this one does not know): where the memories are is not known here.
    RecordUnreadable,
    /// The recorded folder is not there at all — a drive that is out, or a
    /// folder removed. The only case "Start again" is offered for (L6).
    FolderAbsent(PathBuf),
    /// Something is at the recorded folder, but it is not this vault (no
    /// `.vault-id`, or another vault's), or it cannot be checked. It may hold
    /// the memories, so only help is offered.
    FolderUnusable(PathBuf),
}

/// Why the recorded location cannot be used; `None` when it resolves, or
/// when nothing is recorded yet (the first-run setup's to answer). Reads
/// only, and agrees with [`resolve`] by construction: it asks it first.
pub fn diagnose(homes: &Homes) -> Option<Problem> {
    match resolve(homes) {
        Ok(_) | Err(VaultError::VaultLocation(VaultLocationFailure::Unset)) => return None,
        Err(_) => {}
    }
    let Ok(Some(record)) = pointer::read(&homes.pointer_path()) else {
        return Some(Problem::RecordUnreadable);
    };
    match record.vault_dir.try_exists() {
        Ok(false) => Some(Problem::FolderAbsent(record.vault_dir)),
        Ok(true) | Err(_) => Some(Problem::FolderUnusable(record.vault_dir)),
    }
}

/// Why "Start again" did not happen. Nothing was changed in any case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartAgainRefusal {
    /// The recorded folder is there (or cannot be checked), or the record
    /// cannot be read: the memories may be there, so starting again is not
    /// offered.
    NotOffered,
    /// The default folder already holds memories: a vault of its own.
    DefaultFolderHoldsMemories,
    /// Whether the key is destroyed could not be checked, or the erased
    /// marker could not be settled: nothing is decided on a guess.
    KeyUnchecked,
    /// Another task holds the key lock (an erasure, say).
    Busy,
    /// A folder or the record could not be checked or written.
    WriteFailed,
}

/// L6 — "Start again in the default place". Under the key lock K1:
/// - only when the recorded folder itself is not there;
/// - never into a default folder that holds memories;
/// - an erased marker is removed only once the key is confirmed destroyed,
///   and kept with the key when one exists (ADR-SEC-029's side, in the key
///   module); this runs before anything is written, so a crash after it
///   leaves "Start again" on offer at the next start;
/// - then a new `.vault-id` in the default folder, and the record — with no
///   pending move or old copy — written last.
///
/// Nothing is ever deleted. The next open creates the new vault (with the
/// existing key, or, with no key and no marker, a new one).
///
/// # Errors
///
/// The [`StartAgainRefusal`] to show.
pub fn start_again(homes: &Homes, key: &KeyLocation) -> Result<VaultDir, StartAgainRefusal> {
    let decided = keychain::with_key_lock(key, || {
        Ok(start_again_under_lock(homes, &|| {
            keychain::settle_marker_to_start_again(key)
        }))
    });
    match decided {
        Ok(result) => result,
        Err(VaultError::VaultKey(VaultKeyFailure::Busy)) => Err(StartAgainRefusal::Busy),
        Err(e) => {
            warn!(target: "vault_app::location", error = %e, "could not start again: the key lock");
            Err(StartAgainRefusal::WriteFailed)
        }
    }
}

/// [`start_again`]'s decision and writes; the caller holds K1.
pub(crate) fn start_again_under_lock(
    homes: &Homes,
    settle_marker: &dyn Fn() -> VaultResult<MarkerSettled>,
) -> Result<VaultDir, StartAgainRefusal> {
    let Some(Problem::FolderAbsent(lost)) = diagnose(homes) else {
        return Err(StartAgainRefusal::NotOffered);
    };
    let target = homes.new_install_dir();
    for entry in KeyedPaths::in_folder(&target).keyed_entries() {
        match entry.try_exists() {
            Ok(false) => {}
            Ok(true) => {
                warn!(target: "vault_app::location", "the default folder holds memories of its own; not starting again there");
                return Err(StartAgainRefusal::DefaultFolderHoldsMemories);
            }
            Err(e) => {
                warn!(target: "vault_app::location", error = %e, "could not check the default folder");
                return Err(StartAgainRefusal::WriteFailed);
            }
        }
    }
    match settle_marker() {
        Ok(settled) => {
            info!(target: "vault_app::location", ?settled, "the erased marker before starting again");
        }
        Err(e) => {
            warn!(target: "vault_app::location", error = %e, "could not check the key before starting again; nothing changed");
            return Err(StartAgainRefusal::KeyUnchecked);
        }
    }
    let written = std::fs::create_dir_all(&target).and_then(|()| {
        let id = pointer::new_id()?;
        pointer::write_atomic(&target.join(VAULT_ID_FILE), id.as_bytes())?;
        pointer::write(&homes.pointer_path(), &Pointer::new(target.clone(), id))
    });
    if let Err(e) = written {
        warn!(target: "vault_app::location", error = %e, "could not record the default folder");
        return Err(StartAgainRefusal::WriteFailed);
    }
    info!(
        target: "vault_app::location",
        lost = %lost.display(),
        folder = %target.display(),
        "started again in the default place; the lost folder's memories stay wherever they are"
    );
    resolve(homes).map_err(|_| StartAgainRefusal::WriteFailed)
}
