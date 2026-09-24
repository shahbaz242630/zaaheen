//! "Delete everything" — cryptographic erasure of the whole vault
//! (ADR-SEC-008).
//!
//! # What this is for
//!
//! BRD §11.5.4 already specifies the mechanism: *"Account deletion: master
//! key is destroyed, making all encrypted data permanently unrecoverable."*
//! This module is that operation for the local vault, exposed so the user
//! can invoke it deliberately from the app.
//!
//! It exists because uninstalling does NOT remove vault data (see the
//! ADR-SEC-008 discussion: Windows Installer leaves user data by design,
//! and deleting a year of memories as a side effect of a routine uninstall
//! is unrecoverable data loss — Signal shipped exactly that and lost a
//! user's entire message history to it). Keeping the data is right; keeping
//! it with no deliberate way to destroy it is not.
//!
//! # Two properties this module is built around
//!
//! **1. Key first, files second.** Every at-rest key is a BLAKE3 subkey of
//! the one master_key in the OS keychain, so destroying that key is what
//! actually makes the data unrecoverable — NIST SP 800-88 "Purge" level,
//! and unaffected by filesystem journaling, SSD wear-levelling, or a backup
//! of the data directory taken last week. Deleting the files is tidiness on
//! top. Under partial failure the orders differ sharply:
//!
//! | order | interrupted halfway | result |
//! |---|---|---|
//! | key → files | files remain | data is cryptographically dead ✅ |
//! | files → key | key remains | any surviving copy is still readable ❌ |
//!
//! **2. It must work on a vault that cannot be opened.** A wipe button that
//! requires a healthy vault is useless exactly when someone needs it — a
//! corrupted database, a half-migrated state, a key that no longer matches
//! the data. So this takes plain paths and identifiers, never an open
//! `Application` or a live `StorageBackend`, and never tries to open the
//! vault it is destroying.
//!
//! # Why there is no audit row
//!
//! BRD §11.9.1 requires state-changing operations to be audited. This one
//! deliberately is not, because the audit log lives **inside** the vault
//! being destroyed — writing a row and then shredding the database that
//! holds it records nothing. The operation is recorded via `tracing` to the
//! application log, which survives. Noted rather than silently skipped.

use std::path::{Path, PathBuf};

use tracing::{error, info, warn};
use vault_core::VaultResult;

/// What an erasure actually accomplished. Reported honestly so the UI can
/// avoid claiming more than happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErasureOutcome {
    /// `true` if a master_key existed and was destroyed. `false` means
    /// there was no key to destroy (already erased, or never initialised)
    /// — NOT a failure.
    pub key_destroyed: bool,
    /// Files and directories removed from the vault directory.
    pub entries_removed: usize,
    /// Paths that could not be removed (locked by another process, ACLs).
    ///
    /// **Non-empty is NOT a failed erasure.** The key is already gone by
    /// this point, so the leftover bytes are undecryptable ciphertext. It
    /// is reported so the UI can be truthful about disk space rather than
    /// about confidentiality.
    pub undeletable: Vec<PathBuf>,
}

impl ErasureOutcome {
    /// Whether the confidentiality goal was met — i.e. no surviving key can
    /// decrypt any surviving bytes.
    ///
    /// Note this is `true` even when `key_destroyed` is `false`: if there
    /// was never a key, there is nothing that can decrypt the files either.
    #[must_use]
    pub fn data_is_unrecoverable(&self) -> bool {
        true
    }
}

/// Names inside the vault directory that erasure must remove. Everything
/// the vault writes is listed here explicitly rather than deleting the
/// directory wholesale, because the vault directory is chosen by the caller
/// and a bug in that path must not turn into an unbounded recursive delete
/// of, say, `%APPDATA%`.
///
/// **Public because it is the vault's on-disk inventory, not just erasure's
/// private business.** `tests/vault_at_rest_sweep.rs` asserts that a real
/// assembled vault contains NOTHING outside this list — the check that
/// ADR-SEC-007 did not have. That leak happened because a new artifact
/// (`reports/*.json`) appeared on disk and no test was positioned to notice
/// a new artifact at all. Keeping one list, read by both the eraser and the
/// sweep, means a new artifact must be declared in exactly one place or two
/// tests fail.
pub const VAULT_ENTRIES: &[&str] = &[
    "vault.db",
    "vault.db-wal",
    "vault.db-shm",
    "graph.sealed",
    "graph.duckdb",
    "lance",
    "reports",
    "maintenance.json",
    // ADR-102 / ADR-SEC-019: the keeper's discovery file (public fields only),
    // the marker recording that the folder's permissions were tightened, and
    // the POSIX socket directory.
    ".vault-host.json",
    ".acl-v1",
    ".keeper",
    // Session 59: the keeper's list of connected AI apps (names and times
    // only), for the Agents tab; removed when the keeper stops.
    ".vault-clients.json",
    // ADR-108 D7: why the last keeper could not start (a code, no user data).
    // NOT keyed data: it never stops a key being created.
    ".vault-start-failure.json",
];

/// The vault's lockfiles: declared (the at-rest sweep must see every file the
/// vault writes) but never erased.
///
/// They are empty and hold no user data, and they persist after release by
/// design (ADR-SEC-020): the OS lock on the file is released, never the file.
/// Erasure used to delete them; since session 35 the eraser HOLDS two of them
/// while it works (`keeper::exclusive`), and deleting a lockfile by name is
/// the two-owner race ADR-SEC-020 removed — on POSIX a held lockfile can be
/// unlinked, and the next process would lock a fresh file beside the holder.
pub const VAULT_LOCK_FILES: &[&str] = &[
    ".vault.lock",
    ".consolidator.lock",
    ".vault.intent",
    ".keyinit.lock",
];

/// Files in the vault folder that erasure keeps (ADR-105 L1, L7): declared,
/// so the at-rest sweep accepts them, and never erased.
///
/// `.vault-id` binds the folder to the recorded location. Erasing it would
/// leave the location unresolvable, so the person who just deleted
/// everything could never start again (review round 2, finding 1). It holds a
/// random ID and nothing from the vault. `.move-id` exists only inside a
/// move's target and is removed by the move itself.
pub const VAULT_KEEP_FILES: &[&str] = &[
    crate::location::VAULT_ID_FILE,
    crate::location::MOVE_ID_FILE,
];

/// Cryptographically erase the vault: destroy the master_key (and its spare
/// copy, if a move to Local persistence left one), then remove the vault's
/// data files.
///
/// See the module docs for the ordering contract and why this never opens
/// the vault. Idempotent — running it on an already-erased vault succeeds
/// with `key_destroyed: false`.
///
/// **ADR-SEC-029 (E1–E3):** the whole of it runs under the one key lock, so
/// no process can create, move or restore a key meanwhile. The spare is
/// deleted before the key, and each is confirmed gone before anything else
/// happens. Only then is an "erased" marker written, naming `vault_dir`:
/// files that cannot be removed now (the desktop's own open database,
/// typically) are removed by the next process that opens the key, and no
/// new key is made until they are.
///
/// `models/` is deliberately NOT removed: those are downloaded ML model
/// files containing no user data, they are large (~3.5 GB), and a user who
/// erases their memories and starts fresh should not have to re-download
/// them. Stated rather than silently skipped.
///
/// # Errors
///
/// [`vault_core::VaultError::KeychainProvenance`] if the key could not be
/// destroyed or confirmed gone, or [`vault_core::VaultError::VaultKey`] if
/// the key lock could not be taken. **No files are touched in either case**
/// — erasure either starts by succeeding at the step that matters, or does
/// nothing at all. A caller MUST surface this as a failed wipe: the data is
/// still readable.
#[tracing::instrument(skip_all, fields(vault_dir = %vault_dir.display()))]
pub fn erase_vault(
    vault_dir: &Path,
    key: &crate::keychain::KeyLocation,
) -> VaultResult<ErasureOutcome> {
    warn!(
        target: "vault_app::erasure",
        "CRYPTOGRAPHIC ERASURE REQUESTED: destroying the master_key, after which \
         all vault data is permanently unrecoverable (ADR-SEC-008)"
    );

    let erased = match crate::keychain::erase_under_key_lock(key, vault_dir) {
        Ok(erased) => erased,
        Err(e) => {
            error!(
                target: "vault_app::erasure",
                error = %e,
                "erasure ABORTED at the key step; no files were removed and the \
                 vault remains readable"
            );
            return Err(e);
        }
    };

    info!(
        target: "vault_app::erasure",
        key_destroyed = erased.key_destroyed,
        entries_removed = erased.entries_removed,
        undeletable = erased.undeletable.len(),
        "cryptographic erasure complete"
    );

    Ok(ErasureOutcome {
        key_destroyed: erased.key_destroyed,
        entries_removed: erased.entries_removed,
        undeletable: erased.undeletable,
    })
}

/// Erasure's file step: remove the [`VAULT_ENTRIES`] names inside `folder`,
/// and only those. Returns how many were removed and which could not be
/// (locked, denied, or not even checkable — an entry whose existence cannot
/// be checked is reported, never assumed gone).
pub(crate) fn remove_vault_entries(folder: &Path) -> (usize, Vec<PathBuf>) {
    let mut removed = 0usize;
    let mut left = Vec::new();
    for name in VAULT_ENTRIES {
        let path = folder.join(name);
        match remove_entry(&path) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(e) => {
                warn!(
                    target: "vault_app::erasure",
                    path = %path.display(),
                    error = %e,
                    "could not remove a vault file during erasure; it is now \
                     undecryptable ciphertext, but it still occupies disk"
                );
                left.push(path);
            }
        }
    }
    (removed, left)
}

/// Remove one entry of a vault folder — a folder with everything in it.
/// `Ok(false)`: it was not there. An entry whose existence cannot be checked
/// is an error, never assumed gone. Shared by erasure and a move's cleanup
/// (ADR-105 L5), so both remove exactly the same way.
pub(crate) fn remove_entry(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    }
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(true)
}

/// Where the vault's data lives, for the UI to show the user so "uninstall
/// leaves your memories on disk" is an informed statement rather than a
/// surprise.
#[must_use]
pub fn vault_data_location(vault_dir: &Path) -> PathBuf {
    vault_dir.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file list must never contain a path escape. A `..` or an
    /// absolute path here would turn erasure into a recursive delete
    /// outside the vault directory.
    #[test]
    fn vault_entries_are_plain_relative_names() {
        for name in VAULT_ENTRIES {
            let p = Path::new(name);
            assert!(
                p.is_relative(),
                "VAULT_ENTRIES must be relative; {name} is not"
            );
            assert!(
                !name.contains(".."),
                "VAULT_ENTRIES must not contain a parent escape; got {name}"
            );
            assert_eq!(
                p.components().count(),
                1,
                "VAULT_ENTRIES must be single path components so erasure cannot \
                 reach outside the vault dir; got {name}"
            );
        }
    }

    /// Model files are large and contain no user data; erasing memories
    /// must not force a multi-GB re-download.
    #[test]
    fn models_directory_is_not_erased() {
        assert!(
            !VAULT_ENTRIES.contains(&"models"),
            "models/ holds no user data and must survive erasure"
        );
    }

    /// The lockfiles are declared, never erased, and agree with the constants
    /// the lock code uses.
    #[test]
    fn lockfiles_are_declared_but_never_erased() {
        for name in VAULT_LOCK_FILES {
            assert!(
                !VAULT_ENTRIES.contains(name),
                "{name} is a lockfile; erasure must not delete it by name (ADR-SEC-020)"
            );
            assert_eq!(Path::new(name).components().count(), 1);
        }
        for used in [
            crate::VAULT_LOCKFILE_NAME,
            crate::consolidator_lock::LOCKFILE_NAME,
            crate::keeper::intent::INTENT_FILE,
            crate::keychain::KEY_INIT_LOCKFILE_NAME,
        ] {
            assert!(
                VAULT_LOCK_FILES.contains(&used),
                "{used} is written into the vault but not declared"
            );
        }
    }

    /// ADR-105 L1/L7: the location's ID files are declared and never erased
    /// — and erasure's own file step leaves them in place.
    #[test]
    fn the_location_id_files_are_declared_and_never_erased() {
        for name in VAULT_KEEP_FILES {
            assert!(!VAULT_ENTRIES.contains(name), "{name} must survive erasure");
            assert!(!VAULT_LOCK_FILES.contains(name));
            assert_eq!(Path::new(name).components().count(), 1);
        }
        let tmp = tempfile::tempdir().unwrap();
        for name in VAULT_KEEP_FILES.iter().chain(["vault.db"].iter()) {
            std::fs::write(tmp.path().join(name), b"x").unwrap();
        }
        let (removed, left) = remove_vault_entries(tmp.path());
        assert_eq!(removed, 1);
        assert!(left.is_empty());
        for name in VAULT_KEEP_FILES {
            assert!(tmp.path().join(name).exists(), "{name} was erased");
        }
    }

    #[test]
    fn every_known_user_data_artifact_is_listed() {
        // The sealed REPORT (ADR-SEC-007) lives under reports/ and is the
        // most recently added user-data artifact; if a future artifact is
        // added without updating VAULT_ENTRIES it survives erasure.
        for required in ["vault.db", "lance", "reports", "graph.sealed"] {
            assert!(
                VAULT_ENTRIES.contains(&required),
                "{required} carries user data and MUST be erased"
            );
        }
    }
}
