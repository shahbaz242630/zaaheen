//! Where the vault lives (ADR-105 + ADR-SEC-030, `VAULT-KEY-AND-LOCATION.md`).
//!
//! **One record, one resolver, for every process.** The chosen folder is
//! recorded in [`pointer::POINTER_FILE`] in the never-roaming local data
//! folder, with a random ID that is also written into the vault folder as
//! [`VAULT_ID_FILE`]. [`resolve`] is the only way any process finds the vault:
//! the record must be well formed, the folder must exist, and its
//! `.vault-id` must match. Otherwise it answers
//! [`VaultLocationFailure::Unset`] (nothing recorded yet) or
//! [`VaultLocationFailure::Missing`] — **it never creates anything and never
//! falls back to a default**, because that is how a second, empty vault gets
//! made (a different USB stick at the same letter; an unplugged drive).
//!
//! [`prepare`] is the first-run setup (L2): under the key lock K1 and only
//! when nothing is recorded, it adopts an existing install's folder or
//! creates the new-install folder, then records it. Only it and a move
//! (L5) ever create a vault folder.
//!
//! The models never follow the vault ([`models_dir`]).

use std::io;
use std::path::{Path, PathBuf};

use tracing::{info, warn};
use vault_core::{VaultError, VaultLocationFailure, VaultResult};

use crate::keychain::{KeyLocation, KeyedPaths};

pub mod check;
pub mod missing;
pub mod moving;
pub mod pointer;

#[cfg(test)]
mod check_tests;
#[cfg(test)]
mod missing_tests;
#[cfg(test)]
mod tests;

pub use pointer::{PendingCleanup, PendingMove, Pointer, POINTER_FILE};

/// The file inside the vault folder holding the vault's ID. Declared, never
/// erased (`erasure::VAULT_KEEP_FILES`).
pub const VAULT_ID_FILE: &str = ".vault-id";

/// The file inside a move's target folder holding the move's ID
/// (ADR-105 L5). Declared, never erased.
pub const MOVE_ID_FILE: &str = ".move-id";

/// The new-install vault folder's name, inside the local data folder
/// (founder, L2).
pub const NEW_INSTALL_VAULT_DIRNAME: &str = "vault";

/// The two per-user folders the location is decided from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Homes {
    /// `%LOCALAPPDATA%\com.zaaheen.app` — never roams; holds the pointer,
    /// the key lock, the account folder, the logs, and new installs' vaults.
    pub local: PathBuf,
    /// `%APPDATA%\com.zaaheen.app` — where every vault lived before ADR-105,
    /// and where the models stay.
    pub roaming: PathBuf,
}

impl Homes {
    /// This Windows user's folders.
    ///
    /// # Errors
    ///
    /// [`VaultLocationFailure::Missing`] when the environment names neither
    /// folder (a stripped service account): nothing can be found or made.
    pub fn production() -> VaultResult<Self> {
        match (
            crate::install_paths::local_data_dir(),
            crate::install_paths::data_dir(),
        ) {
            (Some(local), Some(roaming)) => Ok(Self { local, roaming }),
            _ => {
                warn!(target: "vault_app::location", "the environment names no app data folders");
                Err(VaultError::VaultLocation(VaultLocationFailure::Missing))
            }
        }
    }

    /// The pointer file.
    pub fn pointer_path(&self) -> PathBuf {
        self.local.join(POINTER_FILE)
    }

    /// Where a brand-new install keeps its vault.
    pub fn new_install_dir(&self) -> PathBuf {
        self.local.join(NEW_INSTALL_VAULT_DIRNAME)
    }
}

/// The resolved vault folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultDir {
    path: PathBuf,
    pointer: Pointer,
}

impl VaultDir {
    /// The vault folder.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The record it was resolved from (for the move's pending fields).
    pub fn pointer(&self) -> &Pointer {
        &self.pointer
    }

    /// The default storage paths inside it.
    pub fn keyed_paths(&self) -> KeyedPaths {
        KeyedPaths::in_folder(&self.path)
    }
}

fn missing(what: &str) -> VaultError {
    warn!(target: "vault_app::location", "{what}");
    VaultError::VaultLocation(VaultLocationFailure::Missing)
}

/// The resolver (L1): the recorded folder, when it is there and is this
/// vault. Never creates, never falls back.
///
/// # Errors
///
/// [`VaultLocationFailure::Unset`] when nothing is recorded;
/// [`VaultLocationFailure::Missing`] for a damaged record, a folder that is
/// not there, or a `.vault-id` that is missing or different.
pub fn resolve(homes: &Homes) -> VaultResult<VaultDir> {
    let pointer = match pointer::read(&homes.pointer_path()) {
        Ok(Some(p)) => p,
        Ok(None) => return Err(VaultError::VaultLocation(VaultLocationFailure::Unset)),
        Err(e) => {
            return Err(missing(&format!(
                "the vault location record cannot be used: {e}"
            )))
        }
    };
    let dir = pointer.vault_dir.clone();
    match dir.try_exists() {
        Ok(true) => {}
        Ok(false) => return Err(missing("the recorded vault folder is not there")),
        Err(e) => {
            return Err(missing(&format!(
                "the recorded vault folder cannot be checked: {e}"
            )))
        }
    }
    match std::fs::read_to_string(dir.join(VAULT_ID_FILE)) {
        Ok(id) if id.trim() == pointer.vault_id => {}
        Ok(_) => return Err(missing("the recorded folder holds a different vault")),
        Err(e) => {
            return Err(missing(&format!(
                "the recorded folder has no vault ID: {e}"
            )))
        }
    }
    Ok(VaultDir { path: dir, pointer })
}

/// The first-run setup (L2), then [`resolve`]. Under the key lock K1: when
/// nothing is recorded yet, an existing install's folder (vault data in the
/// roaming folder or one of `also_existing`) is adopted; otherwise the
/// new-install folder is created. Either way its `.vault-id` and the pointer
/// are written. When a record exists, nothing is written: the resolver's
/// answer is returned as it is.
///
/// Callers take K1 here and release it before `.vault.lock` (lock order
/// `.vault.intent` → `.vault.lock` → K1 is unchanged: this runs before both).
///
/// # Errors
///
/// As [`resolve`]; [`VaultLocationFailure::Missing`] when a folder cannot be
/// checked or written, or the key lock cannot be taken (its own error).
pub fn prepare(
    homes: &Homes,
    key: &KeyLocation,
    also_existing: &[PathBuf],
) -> VaultResult<VaultDir> {
    crate::keychain::with_key_lock(key, || {
        match pointer::read(&homes.pointer_path()) {
            Ok(Some(_)) => return resolve(homes),
            Ok(None) => {}
            Err(e) => {
                return Err(missing(&format!(
                    "the vault location record cannot be used: {e}"
                )))
            }
        }
        let folder = match existing_install(homes, also_existing)? {
            Some(folder) => {
                info!(target: "vault_app::location", folder = %folder.display(), "recording the existing vault's location");
                folder
            }
            None => {
                let folder = homes.new_install_dir();
                std::fs::create_dir_all(&folder)
                    .map_err(|e| missing(&format!("could not create the vault folder: {e}")))?;
                info!(target: "vault_app::location", folder = %folder.display(), "created the vault folder for a new install");
                folder
            }
        };
        record(homes, &folder)
            .map_err(|e| missing(&format!("could not record the vault location: {e}")))?;
        resolve(homes)
    })
}

/// An existing install: the first candidate folder holding vault data — the
/// pre-ADR-105 home, the new-install folder (a record deleted by hand), then
/// any the caller knows of (the desktop's `app_data_dir()`).
fn existing_install(homes: &Homes, also_existing: &[PathBuf]) -> VaultResult<Option<PathBuf>> {
    let new_install = homes.new_install_dir();
    let candidates = std::iter::once(&homes.roaming)
        .chain(std::iter::once(&new_install))
        .chain(also_existing);
    for folder in candidates {
        for entry in KeyedPaths::in_folder(folder).keyed_entries() {
            match entry.try_exists() {
                Ok(true) => return Ok(Some(folder.clone())),
                Ok(false) => {}
                Err(e) => {
                    return Err(missing(&format!(
                        "could not check for an existing vault: {e}"
                    )))
                }
            }
        }
    }
    Ok(None)
}

/// Write `folder`'s `.vault-id` (keeping one already there) and point the
/// record at it.
fn record(homes: &Homes, folder: &Path) -> io::Result<()> {
    let id_path = folder.join(VAULT_ID_FILE);
    let id = match std::fs::read_to_string(&id_path) {
        Ok(existing) if pointer::is_id(existing.trim()) => existing.trim().to_owned(),
        _ => {
            let id = pointer::new_id()?;
            pointer::write_atomic(&id_path, id.as_bytes())?;
            id
        }
    };
    pointer::write(
        &homes.pointer_path(),
        &Pointer::new(folder.to_path_buf(), id),
    )
}

/// Where the models live: where they always have, never following the
/// vault (ADR-105 L3; the models split was cut in review round 2).
pub fn models_dir(homes: &Homes) -> PathBuf {
    crate::install_paths::models_dir_in(&homes.roaming)
}

/// A folder as the person reads it (L-e). The record keeps the form
/// `canonicalize` returns on Windows (`\\?\D:\…`); the screens show `D:\…`.
/// Anything else — a network or device path, which is never recorded — is
/// shown exactly as it is, never dressed up as a local folder.
pub fn display_path(p: &Path) -> String {
    let text = p.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if is_drive_path(rest) => rest.to_owned(),
        _ => text.into_owned(),
    }
}

/// `D:` followed by nothing or a separator.
fn is_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'\\')
}
