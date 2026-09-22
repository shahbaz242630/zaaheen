//! V0.1 → V0.2 SQLCipher passphrase bridge (ADR-041), over the key lifecycle
//! of ADR-SEC-029.
//!
//! Three branches, decided under the key lock after C1 and C2:
//! 1. **A key exists** → return it (moved to Local if needed, D5).
//! 2. **No key, no `vault.db`** → a fresh install; D4 decides whether a key
//!    may be created.
//! 3. **No key, `vault.db` present** → with `VAULT_KEY` set, the one-time
//!    V0.1 bridge (D4's one exception: it creates a key while `vault.db`
//!    exists, by design); without it, the key is missing (U1 message 1).
//!
//! The bridge sequence itself is unchanged from ADR-041 plan iteration 2 §3:
//! verify the V0.1 passphrase → new key → write the key FIRST (now Local and
//! read back, D2) → snapshot → rekey + verify → cleanup; any failure rolls
//! back the snapshot and deletes the key it wrote.

use std::path::{Path, PathBuf};

use tracing::{info, warn};
use vault_core::{VaultError, VaultKeyFailure, VaultResult};
use zeroize::Zeroizing;

use super::lifecycle::{self, Ctx, Key};
use super::store::Slot;

/// The three-branch open for the desktop (see the module docs).
pub(crate) fn bridge_or_init(
    ctx: &Ctx<'_>,
    data_dir: &Path,
    v0_1_vault_key: Option<&str>,
) -> VaultResult<Key> {
    let (existing, erasure) = lifecycle::open_existing(ctx)?;
    if let Some(key) = existing {
        return Ok(key);
    }
    let vault_db = data_dir.join("vault.db");
    let db_present = vault_db.try_exists().map_err(|e| {
        warn!(target: "vault_app::keychain", error = %e, "could not check for the vault database");
        VaultError::VaultKey(VaultKeyFailure::FolderUnavailable)
    })?;
    if !db_present {
        return lifecycle::create(ctx, erasure);
    }
    if erasure.pending {
        return Err(VaultError::VaultKey(VaultKeyFailure::FolderUnavailable));
    }
    match v0_1_vault_key {
        Some(passphrase) if !passphrase.is_empty() => run_v0_1_bridge(ctx, &vault_db, passphrase),
        _ => {
            warn!(
                target: "vault_app::keychain",
                "the vault key is missing but the vault database exists; no new key is made"
            );
            Err(VaultError::VaultKey(VaultKeyFailure::Missing))
        }
    }
}

/// The ADR-041 bridge sequence. Preconditions: no key, `vault.db` present,
/// a non-empty V0.1 passphrase.
fn run_v0_1_bridge(ctx: &Ctx<'_>, vault_db_path: &Path, v0_1_vault_key: &str) -> VaultResult<Key> {
    let v0_1_passphrase = vault_storage::SqlCipherKey::new(v0_1_vault_key.to_string());

    // Step 1: the V0.1 passphrase must unlock the database, before any key
    // is written.
    vault_storage::verify_sqlcipher_passphrase(vault_db_path, &v0_1_passphrase).map_err(|e| {
        VaultError::KeychainProvenance(format!(
            "the V0.1 VAULT_KEY does not unlock the vault at {}: {e}",
            vault_db_path.display()
        ))
    })?;

    // Steps 2–3: a new key and its passphrase.
    let mut new_master_key = Zeroizing::new([0u8; 32]);
    getrandom::getrandom(&mut *new_master_key)
        .map_err(|e| VaultError::Crypto(format!("getrandom: {e}")))?;
    let new_v0_2_passphrase = super::derive_sqlcipher_passphrase(&new_master_key);

    // Step 4: the key FIRST, Local and read back (D2). Nothing destructive
    // has run yet, so a failure only needs the key removed again.
    if !matches!(ctx.write_verified(Slot::Main, &new_master_key), Ok(true)) {
        rollback_key(ctx);
        return Err(VaultError::KeychainProvenance(
            "the bridge's new vault key could not be stored".into(),
        ));
    }

    // Step 5: snapshot.
    let snapshot_path = snapshot_path_for(vault_db_path);
    if let Err(e) = std::fs::copy(vault_db_path, &snapshot_path) {
        rollback_key(ctx);
        return Err(VaultError::Storage(format!(
            "bridge step 5 (snapshot vault.db at {}): {e}. Key rolled back; V0.1 vault.db untouched.",
            snapshot_path.display()
        )));
    }

    // Steps 6–7: rekey + verify.
    if let Err(rekey_err) =
        vault_storage::rekey_in_place(vault_db_path, &v0_1_passphrase, &new_v0_2_passphrase)
    {
        let restored = match std::fs::copy(&snapshot_path, vault_db_path) {
            // Restored: the snapshot has done its job.
            Ok(_) => {
                let _ = std::fs::remove_file(&snapshot_path);
                true
            }
            // Not restored: the snapshot is the only good copy, so it stays.
            Err(restore_err) => {
                warn!(
                    error = %restore_err,
                    snapshot = %snapshot_path.display(),
                    vault = %vault_db_path.display(),
                    "snapshot restore failed during rollback; vault.db may be in unknown \
                     state. Manual recovery: copy the snapshot, which is kept, over vault.db."
                );
                false
            }
        };
        rollback_key(ctx);
        let snapshot = if restored {
            "Snapshot restored to vault.db"
        } else {
            "Snapshot NOT restored; it is kept beside vault.db"
        };
        return Err(VaultError::Storage(format!(
            "bridge step 6/7 (rekey + verify): {rekey_err}. {snapshot}; key rolled back."
        )));
    }

    // Step 8: cleanup, best effort.
    if let Err(e) = std::fs::remove_file(&snapshot_path) {
        warn!(
            error = %e,
            snapshot = %snapshot_path.display(),
            "post-bridge snapshot cleanup failed (best-effort, bridge logically complete)"
        );
    }
    info!("V0.1 → V0.2 SQLCipher passphrase bridge complete; VAULT_KEY is no longer required.");
    Ok(new_master_key)
}

/// Best-effort delete of the key the bridge wrote.
fn rollback_key(ctx: &Ctx<'_>) {
    if let Err(e) = ctx.store.delete(Slot::Main) {
        warn!(
            error = %e.0,
            "rollback: the bridge's key could not be deleted; the next open finds it and \
             the vault it cannot open"
        );
    }
}

/// `vault.db` → `vault.db.pre_v0_2_bridge`.
pub(crate) fn snapshot_path_for(vault_db_path: &Path) -> PathBuf {
    let mut s = vault_db_path.as_os_str().to_owned();
    s.push(".pre_v0_2_bridge");
    PathBuf::from(s)
}
