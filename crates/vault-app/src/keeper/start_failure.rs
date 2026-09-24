//! Why a keeper could not start, for the desktop app (ADR-108 D7).
//!
//! Before D4 the desktop opened the key and the vault itself and showed one
//! of ADR-SEC-029's startup messages when that failed. Under D4 the keeper
//! does the opening, so it leaves a note: `<vault>/.vault-start-failure.json`,
//! `{v, pid, at, code}`, written atomically and removed by the next keeper
//! that starts. The desktop believes it only when its `pid` matches the
//! Failed discovery record's, and maps the code to the SAME message text it
//! showed before.
//!
//! The discovery file's format does not change: relays of older builds parse
//! it with `deny_unknown_fields`, so a reason field there would break them.
//!
//! The note is in erasure's `VAULT_ENTRIES` but NOT in the keyed entries
//! that stop a key being created: a failed first start must never block the
//! next one from creating the key (review A-M3).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vault_core::{VaultError, VaultKeyFailure, VaultLocationFailure};

/// The note's file name in the vault folder.
pub const START_FAILURE_FILE: &str = ".vault-start-failure.json";

const FORMAT: u32 = 1;
const MAX_BYTES: u64 = 1024;

/// Why the keeper could not start. A closed list: each maps to one of the
/// startup messages the desktop already had.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartFailureCode {
    /// The key is missing but memories sealed under one exist (message 1).
    KeyMissing,
    /// Windows' credential store could not be used (message 2).
    CredentialStore,
    /// The vault folder or the key's folder could not be checked.
    FolderUnavailable,
    /// A stored key this app could not have written.
    KeyUnusable,
    /// Another task (a deletion) held the key: try again shortly.
    KeyBusy,
    /// The recorded vault folder is missing or is not this vault.
    Location,
    /// Anything else that stopped the vault opening.
    VaultOpenFailed,
    /// A locked computer with no key yet: nothing to open, nothing wrong.
    /// Never written by a current keeper (it exits quietly), but understood
    /// if an older record says so.
    LockedNoKey,
}

impl StartFailureCode {
    /// The code for an error that stopped the vault opening.
    pub fn from_error(err: &VaultError) -> Self {
        match err {
            VaultError::VaultKey(VaultKeyFailure::Missing) => Self::KeyMissing,
            VaultError::VaultKey(VaultKeyFailure::FolderUnavailable) => Self::FolderUnavailable,
            VaultError::VaultKey(VaultKeyFailure::Unusable) => Self::KeyUnusable,
            VaultError::VaultKey(VaultKeyFailure::Busy) => Self::KeyBusy,
            VaultError::KeychainProvenance(_) => Self::CredentialStore,
            VaultError::VaultLocation(_) => Self::Location,
            _ => Self::VaultOpenFailed,
        }
    }

    /// An error of the kind the desktop's dialogs already tell apart, so it
    /// shows the very same words. `None` for a code that needs no dialog.
    pub fn as_error(self) -> Option<VaultError> {
        Some(match self {
            Self::KeyMissing => VaultError::VaultKey(VaultKeyFailure::Missing),
            Self::FolderUnavailable => VaultError::VaultKey(VaultKeyFailure::FolderUnavailable),
            Self::KeyUnusable => VaultError::VaultKey(VaultKeyFailure::Unusable),
            Self::KeyBusy => VaultError::VaultKey(VaultKeyFailure::Busy),
            Self::CredentialStore => {
                VaultError::KeychainProvenance("the credential store could not be used".into())
            }
            Self::Location => VaultError::VaultLocation(VaultLocationFailure::Missing),
            Self::VaultOpenFailed => VaultError::Storage("the memories could not be opened".into()),
            Self::LockedNoKey => return None,
        })
    }

    /// Worth trying again in a moment rather than telling the person.
    pub fn is_transient(self) -> bool {
        matches!(self, Self::KeyBusy)
    }
}

/// The note as written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartFailure {
    pub v: u32,
    pub pid: u32,
    pub at: String,
    pub code: StartFailureCode,
}

fn path_in(vault_root: &Path) -> PathBuf {
    vault_root.join(START_FAILURE_FILE)
}

/// Leave the note (best effort: a keeper that cannot write it still
/// publishes Failed, and the desktop then shows its generic message).
pub fn write(vault_root: &Path, pid: u32, code: StartFailureCode) {
    let note = StartFailure {
        v: FORMAT,
        pid,
        at: chrono::Utc::now().to_rfc3339(),
        code,
    };
    let result = serde_json::to_vec(&note)
        .map_err(io::Error::other)
        .and_then(|bytes| {
            let tmp = vault_root.join(format!("{START_FAILURE_FILE}.{pid}.tmp"));
            fs::write(&tmp, bytes)?;
            fs::rename(&tmp, path_in(vault_root))
        });
    if let Err(e) = result {
        tracing::warn!(target: "vault_app::keeper", error = %e, "could not record why the keeper stopped");
    }
}

/// Remove the note: a keeper that started cleanly clears any older one.
pub fn clear(vault_root: &Path) {
    match fs::remove_file(path_in(vault_root)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(target: "vault_app::keeper", error = %e, "could not clear an old start-failure note");
        }
    }
}

/// The note, if there is a well-formed one written by `pid` (the Failed
/// discovery record's pid). Anything else — absent, too big, malformed, or
/// from another process — is not believed.
pub fn read_for(vault_root: &Path, pid: u32) -> Option<StartFailureCode> {
    let path = path_in(vault_root);
    let len = fs::metadata(&path).ok()?.len();
    if len > MAX_BYTES {
        return None;
    }
    let note: StartFailure = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    (note.v == FORMAT && note.pid == pid).then_some(note.code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_is_read_back_only_for_its_own_pid() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 4242, StartFailureCode::KeyMissing);
        assert_eq!(
            read_for(dir.path(), 4242),
            Some(StartFailureCode::KeyMissing)
        );
        assert_eq!(read_for(dir.path(), 4243), None, "another process's note");
        clear(dir.path());
        assert_eq!(read_for(dir.path(), 4242), None);
        clear(dir.path());
    }

    #[test]
    fn a_malformed_or_oversized_note_is_not_believed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(START_FAILURE_FILE), b"{\"v\":1,\"pid\":1}").unwrap();
        assert_eq!(read_for(dir.path(), 1), None);
        fs::write(dir.path().join(START_FAILURE_FILE), vec![b' '; 2048]).unwrap();
        assert_eq!(read_for(dir.path(), 1), None);
    }

    /// Every error the desktop has a message for round-trips to the same kind
    /// of error, so the same words are shown.
    #[test]
    fn each_code_maps_back_to_the_error_its_message_is_chosen_by() {
        let errors = [
            VaultError::VaultKey(VaultKeyFailure::Missing),
            VaultError::VaultKey(VaultKeyFailure::FolderUnavailable),
            VaultError::VaultKey(VaultKeyFailure::Unusable),
            VaultError::VaultKey(VaultKeyFailure::Busy),
            VaultError::KeychainProvenance("x".into()),
            VaultError::VaultLocation(VaultLocationFailure::Missing),
        ];
        for err in errors {
            let code = StartFailureCode::from_error(&err);
            let back = code.as_error().unwrap();
            assert_eq!(
                std::mem::discriminant(&back),
                std::mem::discriminant(&err),
                "{code:?}"
            );
            if let (VaultError::VaultKey(a), VaultError::VaultKey(b)) = (&back, &err) {
                assert_eq!(a, b);
            }
        }
        assert_eq!(
            StartFailureCode::from_error(&VaultError::Storage("x".into())),
            StartFailureCode::VaultOpenFailed
        );
        assert!(StartFailureCode::LockedNoKey.as_error().is_none());
        assert!(StartFailureCode::KeyBusy.is_transient());
    }

    /// The note never counts as keyed data: a failed first start must not
    /// stop the next keeper creating the key (review A-M3).
    #[test]
    fn the_note_is_erased_with_the_vault_but_is_not_keyed_data() {
        assert!(crate::erasure::VAULT_ENTRIES.contains(&START_FAILURE_FILE));
        let keyed = crate::keychain::KeyedPaths::in_folder(Path::new("C:\\vault"));
        assert!(
            !keyed
                .keyed_entries()
                .iter()
                .any(|p| p.ends_with(START_FAILURE_FILE)),
            "the note must not block key creation"
        );
    }
}
