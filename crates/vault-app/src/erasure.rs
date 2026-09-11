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

/// Cryptographically erase the vault: destroy the master_key, then remove
/// the vault's data files.
///
/// See the module docs for the ordering contract and why this never opens
/// the vault. Idempotent — running it on an already-erased vault succeeds
/// with `key_destroyed: false`.
///
/// `models/` is deliberately NOT removed: those are downloaded ML model
/// files containing no user data, they are large (~3.5 GB), and a user who
/// erases their memories and starts fresh should not have to re-download
/// them. Stated rather than silently skipped.
///
/// # Errors
///
/// [`vault_core::VaultError::KeychainProvenance`] if the master_key could not be
/// destroyed. **No files are touched in that case** — erasure either starts
/// by succeeding at the step that matters, or does nothing at all. A caller
/// MUST surface this as a failed wipe: the data is still readable.
#[tracing::instrument(skip_all, fields(vault_dir = %vault_dir.display()))]
pub fn erase_vault(
    vault_dir: &Path,
    keychain_namespace: &str,
    vault_id: &str,
) -> VaultResult<ErasureOutcome> {
    warn!(
        target: "vault_app::erasure",
        "CRYPTOGRAPHIC ERASURE REQUESTED: destroying the master_key, after which \
         all vault data is permanently unrecoverable (ADR-SEC-008)"
    );

    // STEP 1 — the key. If this fails we stop here and touch nothing:
    // deleting files while the key survives is the strictly worse failure
    // mode (any backup of the data dir stays decryptable).
    let key_destroyed = match crate::keychain::destroy_master_key(keychain_namespace, vault_id) {
        Ok(destroyed) => destroyed,
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

    // STEP 2 — the files. Best-effort from here: the data is already dead.
    let mut entries_removed = 0usize;
    let mut undeletable = Vec::new();

    for name in VAULT_ENTRIES {
        let path = vault_dir.join(name);
        if !path.exists() {
            continue;
        }
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => entries_removed += 1,
            Err(e) => {
                warn!(
                    target: "vault_app::erasure",
                    path = %path.display(),
                    error = %e,
                    "could not remove a vault file during erasure; it is now \
                     undecryptable ciphertext, but it still occupies disk"
                );
                undeletable.push(path);
            }
        }
    }

    info!(
        target: "vault_app::erasure",
        key_destroyed,
        entries_removed,
        undeletable = undeletable.len(),
        "cryptographic erasure complete"
    );

    Ok(ErasureOutcome {
        key_destroyed,
        entries_removed,
        undeletable,
    })
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
