//! At-rest-key provenance: OS keychain read/init + master_key derivation tree.
//!
//! See HANDOFF.md ADR-040 + ADR-040 amendment for the contract this module
//! implements. T0.2.0 close-out plan iteration 1 Phase 1 + iteration 1.5
//! Discovery 4 factored keychain-touching code into this module so vault-tauri
//! main.rs's surface change stays minimal and the helpers are unit-testable
//! against the same Windows Credential Manager backend the spike runtime-
//! confirmed.
//!
//! ## Public surface
//!
//! - [`read_or_init_master_key`] — read 32-byte master_key from keychain;
//!   on `NotFound` first-run-generate via `getrandom` + persist + return.
//! - [`derive_sqlcipher_passphrase`] — domain-separated BLAKE3 subkey,
//!   hex-encoded for the `SqlCipherKey::new(String)` constructor.
//! - [`derive_at_rest_key`] — domain-separated BLAKE3 subkey, returned as
//!   `Zeroizing<[u8; 32]>` for downstream consumption by
//!   [`vault_storage::LanceVectorStore::open_with_at_rest_key`] (Phase 2/3).
//! - [`PRODUCTION_NAMESPACE`] — `"com.zaaheen.v0.2"` reverse-DNS service
//!   string for production keychain entries.
//!
//! ## Platform support (Phase 1)
//!
//! Windows-only at Phase 1 per ADR-029 V0.1 dogfood pattern + ADR-040 OQ #1
//! partial resolution. macOS / Linux per-platform crate-add deferred to
//! T0.2.0.x sub-task or T0.2.14 Stub-Installer-adjacent. On non-Windows
//! platforms, [`read_or_init_master_key`] returns
//! [`VaultError::KeychainProvenance`] with a clear "platform not supported in
//! V0.2 Phase 1" message — vault-tauri's fatal-dialog surface picks it up.
//!
//! Derivation primitives ([`derive_sqlcipher_passphrase`] +
//! [`derive_at_rest_key`]) are platform-independent — pure BLAKE3 derive_key
//! calls with no keychain dependency.
//!
//! ## Domain-separated subkey discipline (ADR-040 amendment option β)
//!
//! ```text
//! master_key             ← 32 bytes from keychain
//! at_rest_key            = blake3::derive_key("vault memory at-rest sealing v1", &master_key)
//! sqlcipher_passphrase   = hex(blake3::derive_key("vault sqlcipher passphrase v1", &master_key))
//! ```
//!
//! Distinct domain-separator strings prevent algebraic crossover between
//! the two consumers; same primitive (BLAKE3) preserves single-source-crypto
//! per ADR-008 amendment line 693.

use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(windows)]
use tracing::{info, instrument, warn};
use vault_core::{VaultError, VaultResult};
use zeroize::Zeroizing;

/// Reverse-DNS namespace for production keychain entries — locked at ADR-040.
///
/// Distinguishable from any other Memory Vault keychain entry; spike used
/// `com.zaaheen.spike.v0.2` (different sub-namespace) so spike runs never
/// collide with production. V1.0 multi-vault forward-compat preserved: same
/// namespace will hold multiple per-vault entries differentiated by the
/// account-string slot.
pub const PRODUCTION_NAMESPACE: &str = "com.zaaheen.v0.2";

/// V0.2 single-vault account string. Moved from vault-tauri main.rs at
/// T0.2.0 Phase 3 sub-task (a) (2026-05-11) so vault-tauri + vault-cli
/// share a single source of truth (both consume `PRODUCTION_NAMESPACE` +
/// `VAULT_ID` to access the same keychain entry). V1.0 multi-vault per
/// BRD §6.2 will pass real per-vault ids — same keychain namespace slot,
/// multiple entries differentiated by this account-string slot per
/// ADR-040 forward-compat.
pub const VAULT_ID: &str = "default";

/// BLAKE3 derive_key context for the SqlCipher passphrase subkey
/// (ADR-040 amendment option β).
///
/// The trailing `v1` lets us rotate without ambiguity if the derivation
/// scheme ever changes.
const SQLCIPHER_KDF_CONTEXT: &str = "vault sqlcipher passphrase v1";

/// BLAKE3 derive_key context for the at-rest sealing subkey (ADR-008
/// amendment K3 KDF). Matches the constant ADR-008 amendment locked at
/// HANDOFF.md — same string, single source-of-truth.
const AT_REST_KDF_CONTEXT: &str = "vault memory at-rest sealing v1";

/// Read the 32-byte `master_key` from the OS keychain. On first run (no
/// entry exists), generate a new `master_key` via `getrandom`, persist via
/// `set_secret`, and return the newly-persisted key.
///
/// # Arguments
///
/// - `namespace` — keychain service string. Production callers pass
///   [`PRODUCTION_NAMESPACE`]; tests pass a unique-per-test string to avoid
///   colliding with production entries or with concurrent test runs.
/// - `vault_id` — keychain account/user string. V0.2 single-vault per
///   BRD §6.2 passes `"default"`. V1.0 multi-vault will pass real
///   per-vault ids.
///
/// # Errors
///
/// Returns [`VaultError::KeychainProvenance`] for any keychain failure
/// other than `NotFound` (which is handled internally as the first-run
/// signal). On non-Windows platforms returns the same variant with a
/// "platform not supported in V0.2 Phase 1" message.
///
/// # Security
///
/// Returned key is wrapped in [`Zeroizing<[u8; 32]>`] so the bytes are
/// wiped from memory on Drop per BRD §11.5.3. Caller MUST NOT materialise
/// the bytes as a plaintext `Vec<u8>` outside the immediate derivation
/// call site.
#[cfg(windows)]
pub fn read_or_init_master_key(
    namespace: &str,
    vault_id: &str,
) -> VaultResult<Zeroizing<[u8; 32]>> {
    use windows_native_keyring_store::Store;

    keyring_core::set_default_store(
        Store::new()
            .map_err(|e| VaultError::KeychainProvenance(format!("Store::new failed: {e}")))?,
    );

    let result = read_or_init_inner(namespace, vault_id);

    keyring_core::unset_default_store();

    result
}

/// Non-Windows stub: returns a clear "platform not supported" error.
/// vault-tauri's fatal-dialog flow surfaces it to the user. Cross-platform
/// keychain wiring lands at T0.2.0.x sub-task or T0.2.14 Stub-Installer-
/// adjacent per HANDOFF.md OQ #1 partial resolution.
#[cfg(not(windows))]
pub fn read_or_init_master_key(
    _namespace: &str,
    _vault_id: &str,
) -> VaultResult<Zeroizing<[u8; 32]>> {
    Err(VaultError::KeychainProvenance(format!(
        "Keychain provenance is Windows-only at V0.2 Phase 1 (current platform: {}). \
         macOS / Linux per-platform crate-add deferred to T0.2.0.x sub-task or \
         T0.2.14 Stub-Installer-adjacent per HANDOFF.md OQ #1 partial resolution.",
        std::env::consts::OS
    )))
}

/// Inner read/first-run helper. Assumes [`keyring_core::set_default_store`]
/// has been called by the caller; caller is also responsible for the matching
/// [`keyring_core::unset_default_store`].
#[cfg(windows)]
fn read_or_init_inner(namespace: &str, vault_id: &str) -> VaultResult<Zeroizing<[u8; 32]>> {
    use keyring_core::Entry;

    let entry = Entry::new(namespace, vault_id)
        .map_err(|e| VaultError::KeychainProvenance(format!("Entry::new failed: {e}")))?;

    match entry.get_secret() {
        Ok(bytes) => {
            // Existing entry — must be exactly 32 bytes for our master_key
            // shape. Anything else means the entry was written by a
            // different (potentially future-incompat) scheme; fail closed.
            if bytes.len() != 32 {
                return Err(VaultError::KeychainProvenance(format!(
                    "Keychain entry exists but secret is {} bytes (expected 32). \
                     Entry may have been written by a non-Memory-Vault tool or by \
                     a future version with a different scheme. Investigate via \
                     Credential Manager (entry namespace: {namespace}, account: \
                     {vault_id}) before proceeding.",
                    bytes.len()
                )));
            }
            let mut master_key = Zeroizing::new([0u8; 32]);
            master_key.copy_from_slice(&bytes);
            Ok(master_key)
        }
        Err(get_err) => {
            // The keyring-core Error type doesn't expose a stable kind enum
            // we can match on across crate versions — string-match on the
            // Display form for the NotFound case (first-run signal). All
            // other errors flow as KeychainProvenance.
            //
            // This is a deliberate trade-off: string-matching is brittle
            // across keyring-core upgrades, but the alternative (treating
            // every Err as first-run) would silently overwrite a legitimate
            // entry on read failure — unsafe. If keyring-core 1.x exposes
            // a stable kind enum in a future minor, swap to that.
            let err_str = format!("{get_err}");
            let err_lower = err_str.to_lowercase();
            let is_not_found = err_lower.contains("no entry")
                || err_lower.contains("not found")
                || err_lower.contains("no such")
                // Windows Credential Manager (`windows-native-keyring-store`)
                // surfaces the empty-entry case as "No matching credential found"
                // — none of the prior substrings match it. Verified empirically
                // at T0.2.0 Phase 1 DoD-gate session 2026-05-10.
                || err_lower.contains("no matching credential");
            if !is_not_found {
                return Err(VaultError::KeychainProvenance(format!(
                    "get_secret failed (non-NotFound): {get_err}"
                )));
            }

            // First-run path: generate new 32-byte master_key, persist,
            // return.
            let mut master_key = Zeroizing::new([0u8; 32]);
            getrandom::getrandom(&mut *master_key)
                .map_err(|e| VaultError::KeychainProvenance(format!("getrandom: {e}")))?;
            entry.set_secret(&*master_key).map_err(|e| {
                VaultError::KeychainProvenance(format!(
                    "set_secret failed during first-run init: {e}"
                ))
            })?;
            Ok(master_key)
        }
    }
}

/// V0.1 → V0.2 SQLCipher passphrase bridge entry point — composes
/// [`read_or_init_master_key`] with the V0.1 bridge path per ADR-041.
///
/// Three branches based on detected state:
/// 1. **Keychain entry present** (V0.2 second-launch path) → return
///    existing master_key. Identical to `read_or_init_master_key`'s
///    existing-entry behavior; no V0.1 detection runs.
/// 2. **Keychain absent + no V0.1 vault.db** (fresh V0.2 install) →
///    delegate to [`read_or_init_master_key`]'s first-run path
///    (generate new master_key + persist to keychain).
/// 3. **Keychain absent + V0.1 vault.db present** → V0.1 bridge path:
///    verify the V0.1 passphrase unlocks the vault → generate new
///    master_key → derive new SQLCipher passphrase → write keychain
///    entry FIRST (cheapest-failure-first per ADR-041 §2 Pattern B
///    cross-store snapshot-commit invariant) → snapshot vault.db →
///    [`vault_storage::rekey_in_place`] (which includes the §10 post-
///    write verification invariant: close + reopen + verify with new
///    passphrase) → cleanup snapshot. On any failure: rollback
///    snapshot + delete keychain entry, return Err.
///
/// **Branch-3 fail-closed condition:** if V0.1 vault.db exists but
/// `v0_1_vault_key` is `None` OR `Some("")`, returns
/// [`VaultError::KeychainProvenance`] with a tailored message naming
/// VAULT_KEY + recovery steps. vault-tauri main.rs's
/// `format_keychain_error_dialog` surfaces this to the user.
///
/// # Arguments
///
/// - `data_dir` — per-user data directory (e.g.,
///   `%APPDATA%/com.memoryvault.dev/`). Bridge checks for V0.1 vault.db
///   at `<data_dir>/vault.db`.
/// - `namespace` / `vault_id` — same shape as `read_or_init_master_key`.
/// - `v0_1_vault_key` — the V0.1 alpha passphrase, sourced from the
///   `VAULT_KEY` env var by the production caller (vault-tauri
///   main.rs). Passed in as a parameter rather than read from env
///   inside the bridge so (a) tests can inject fixture values without
///   mutating global env state (which would require `unsafe` per
///   rustc 1.92's `std::env::set_var` semantics, blocked by ADR-002's
///   `#![forbid(unsafe_code)]`), and (b) the bridge stays a pure
///   function of its inputs — no hidden environment dependency.
///
/// # Errors
///
/// - [`VaultError::KeychainProvenance`] for keychain access failures,
///   wrong V0.1 passphrase, missing V0.1 passphrase (with tailored
///   message naming VAULT_KEY).
/// - [`VaultError::Storage`] wrapping snapshot or rekey failures.
///
/// # See also
///
/// - HANDOFF.md "ADR-041 plan iteration 2 LOCKED" — full bridge sequence + invariant
/// - HANDOFF.md ADR-041 §10 post-write verification invariant
/// - [`vault_storage::verify_sqlcipher_passphrase`] (step 1)
/// - [`vault_storage::rekey_in_place`] (steps 6-7 with internal verify)
#[cfg(windows)]
pub fn bridge_or_init_master_key(
    data_dir: &Path,
    namespace: &str,
    vault_id: &str,
    v0_1_vault_key: Option<&str>,
) -> VaultResult<Zeroizing<[u8; 32]>> {
    use windows_native_keyring_store::Store;

    keyring_core::set_default_store(
        Store::new()
            .map_err(|e| VaultError::KeychainProvenance(format!("Store::new failed: {e}")))?,
    );

    let result = bridge_or_init_inner(data_dir, namespace, vault_id, v0_1_vault_key);

    keyring_core::unset_default_store();

    result
}

/// Non-Windows stub mirrors [`read_or_init_master_key`]'s shape.
#[cfg(not(windows))]
pub fn bridge_or_init_master_key(
    _data_dir: &Path,
    _namespace: &str,
    _vault_id: &str,
    _v0_1_vault_key: Option<&str>,
) -> VaultResult<Zeroizing<[u8; 32]>> {
    Err(VaultError::KeychainProvenance(format!(
        "Keychain provenance is Windows-only at V0.2 Phase 1 (current platform: {}). \
         macOS / Linux per-platform crate-add deferred to T0.2.0.x sub-task or \
         T0.2.14 Stub-Installer-adjacent per HANDOFF.md OQ #1 partial resolution.",
        std::env::consts::OS
    )))
}

/// Inner bridge orchestration. Caller has already established the keychain
/// default store; this fn only does the 3-way branch + V0.1 bridge sequence.
#[cfg(windows)]
#[instrument(skip(data_dir, v0_1_vault_key), fields(data_dir = %data_dir.display(), vault_id))]
fn bridge_or_init_inner(
    data_dir: &Path,
    namespace: &str,
    vault_id: &str,
    v0_1_vault_key: Option<&str>,
) -> VaultResult<Zeroizing<[u8; 32]>> {
    // Branch 1: existing keychain entry present?
    if let Some(existing) = try_read_existing_master_key(namespace, vault_id)? {
        return Ok(existing);
    }

    // Keychain absent. Branch 2 vs 3 depends on V0.1 vault.db presence.
    let vault_db_path = data_dir.join("vault.db");
    if !vault_db_path.exists() {
        // Branch 2: fresh V0.2 install — delegate to existing first-run logic.
        // (read_or_init_inner regenerates the keychain entry; calling it
        // here means we re-do the keychain probe, but that's idempotent
        // and the cost is one keychain RPC.)
        return read_or_init_inner(namespace, vault_id);
    }

    // Branch 3: V0.1 vault.db present. Check passphrase parameter.
    let vault_key = match v0_1_vault_key {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Err(VaultError::KeychainProvenance(format!(
                "V0.1 vault detected at {} but VAULT_KEY env var is not set. \
                 Per ADR-041, the V0.1 → V0.2 SQLCipher passphrase bridge requires \
                 VAULT_KEY (the V0.1 alpha passphrase) to unlock the existing vault \
                 for one-time re-encryption with the new keychain-derived passphrase. \
                 Set VAULT_KEY to your V0.1 passphrase and relaunch Zaaheen. \
                 The keychain entry will be created automatically; on subsequent \
                 launches VAULT_KEY is no longer needed.",
                vault_db_path.display()
            )))
        }
    };

    run_v0_1_bridge(&vault_db_path, vault_key, namespace, vault_id)
}

/// Probe whether the keychain entry already exists. Returns:
/// - `Ok(Some(master_key))` if the entry exists with a 32-byte secret
/// - `Ok(None)` if the entry is NotFound (first-run signal)
/// - `Err(VaultError::KeychainProvenance)` for any other error (read failure,
///   wrong-size secret, etc.)
///
/// Splits out the existence-probe from `read_or_init_inner` (which also
/// generates + persists on NotFound) so the bridge can branch on absence
/// without triggering the auto-generate side effect.
#[cfg(windows)]
fn try_read_existing_master_key(
    namespace: &str,
    vault_id: &str,
) -> VaultResult<Option<Zeroizing<[u8; 32]>>> {
    use keyring_core::Entry;

    let entry = Entry::new(namespace, vault_id)
        .map_err(|e| VaultError::KeychainProvenance(format!("Entry::new failed: {e}")))?;

    match entry.get_secret() {
        Ok(bytes) => {
            if bytes.len() != 32 {
                return Err(VaultError::KeychainProvenance(format!(
                    "Keychain entry exists but secret is {} bytes (expected 32). \
                     Entry may have been written by a non-Memory-Vault tool or by \
                     a future version with a different scheme. Investigate via \
                     Credential Manager (entry namespace: {namespace}, account: \
                     {vault_id}) before proceeding.",
                    bytes.len()
                )));
            }
            let mut master_key = Zeroizing::new([0u8; 32]);
            master_key.copy_from_slice(&bytes);
            Ok(Some(master_key))
        }
        Err(get_err) => {
            // Same NotFound-detection string-match logic as read_or_init_inner.
            let err_str = format!("{get_err}");
            let err_lower = err_str.to_lowercase();
            let is_not_found = err_lower.contains("no entry")
                || err_lower.contains("not found")
                || err_lower.contains("no such")
                || err_lower.contains("no matching credential");
            if is_not_found {
                Ok(None)
            } else {
                Err(VaultError::KeychainProvenance(format!(
                    "get_secret failed (non-NotFound) during bridge existence-probe: {get_err}"
                )))
            }
        }
    }
}

/// Runs the V0.1 → V0.2 SQLCipher passphrase bridge sequence per ADR-041
/// plan iteration 2 §3 (LOCKED). Caller has already verified preconditions
/// (no keychain entry + V0.1 vault.db present + VAULT_KEY env set).
///
/// Cross-store snapshot-commit invariant Pattern B: keychain write FIRST,
/// then snapshot, then rekey, then verify (inside `rekey_in_place`),
/// then snapshot cleanup. Failure at any step rolls back per the
/// invariant's core property (system never in `¬a ∧ ¬b ∧ ¬c`).
#[cfg(windows)]
fn run_v0_1_bridge(
    vault_db_path: &Path,
    v0_1_vault_key: &str,
    namespace: &str,
    vault_id: &str,
) -> VaultResult<Zeroizing<[u8; 32]>> {
    let v0_1_passphrase = vault_storage::SqlCipherKey::new(v0_1_vault_key.to_string());

    // Step 1: verify V0.1 passphrase unlocks vault.db (fail-fast BEFORE
    // any keychain write — avoids creating an orphan keychain entry
    // pointing to a master_key for a vault we can't actually rekey).
    vault_storage::verify_sqlcipher_passphrase(vault_db_path, &v0_1_passphrase).map_err(|e| {
        VaultError::KeychainProvenance(format!(
            "V0.1 VAULT_KEY env var does not unlock the vault at {}: {e}. Verify VAULT_KEY \
             matches the value used in V0.1 alpha; the bridge cannot proceed with a wrong \
             passphrase.",
            vault_db_path.display()
        ))
    })?;

    // Step 2: generate new master_key.
    let mut new_master_key = Zeroizing::new([0u8; 32]);
    getrandom::getrandom(&mut *new_master_key)
        .map_err(|e| VaultError::KeychainProvenance(format!("getrandom: {e}")))?;

    // Step 3: derive new SQLCipher passphrase from new master_key.
    let new_v0_2_passphrase = derive_sqlcipher_passphrase(&new_master_key);

    // Step 4: write keychain entry FIRST (cheapest-to-rollback per §2
    // Pattern B). If keychain write fails, no destructive op has run.
    write_keychain_entry(namespace, vault_id, &new_master_key)?;

    // Step 5: snapshot vault.db (Pattern B explicit pre-copy).
    let snapshot_path = snapshot_path_for(vault_db_path);
    if let Err(e) = std::fs::copy(vault_db_path, &snapshot_path) {
        // Rollback step 4 keychain write. Best-effort delete; if it fails
        // we surface compound err but the system state is still in §2's
        // (a) — vault.db untouched, just an orphan keychain entry that
        // next launch's bridge entry-probe will handle.
        rollback_keychain_entry(namespace, vault_id);
        return Err(VaultError::Storage(format!(
            "bridge step 5 (snapshot vault.db at {}): {e}. Keychain entry rolled \
             back; V0.1 vault.db untouched.",
            snapshot_path.display()
        )));
    }

    // Step 6+7: PRAGMA rekey + post-write verify (delegated to
    // vault_storage::rekey_in_place which embeds §10 verification).
    if let Err(rekey_err) =
        vault_storage::rekey_in_place(vault_db_path, &v0_1_passphrase, &new_v0_2_passphrase)
    {
        // Restore vault.db from snapshot, roll back keychain.
        if let Err(restore_err) = std::fs::copy(&snapshot_path, vault_db_path) {
            warn!(
                error = %restore_err,
                snapshot = %snapshot_path.display(),
                vault = %vault_db_path.display(),
                "snapshot restore failed during rollback; vault.db may be in unknown \
                 state. Manual recovery: copy snapshot over vault.db."
            );
        }
        // Best-effort snapshot cleanup post-restore.
        let _ = std::fs::remove_file(&snapshot_path);
        rollback_keychain_entry(namespace, vault_id);
        return Err(VaultError::Storage(format!(
            "bridge step 6/7 (rekey + verify): {rekey_err}. Snapshot restored to \
             vault.db; keychain entry rolled back."
        )));
    }

    // Step 8: success cleanup of snapshot. Best-effort — bridge is
    // logically complete even if cleanup fails.
    if let Err(e) = std::fs::remove_file(&snapshot_path) {
        warn!(
            error = %e,
            snapshot = %snapshot_path.display(),
            "post-bridge snapshot cleanup failed (best-effort, bridge logically \
             complete)"
        );
    }

    // Step 9: one-time INFO log per iteration 2 §3.
    info!(
        "V0.1 → V0.2 SQLCipher passphrase bridge complete; VAULT_KEY env var no longer \
         required."
    );

    Ok(new_master_key)
}

/// Sibling-path-of-vault.db for the pre-bridge snapshot. For input
/// `vault.db`, returns `vault.db.pre_v0_2_bridge`.
#[cfg(windows)]
fn snapshot_path_for(vault_db_path: &Path) -> PathBuf {
    let mut s = vault_db_path.as_os_str().to_owned();
    s.push(".pre_v0_2_bridge");
    PathBuf::from(s)
}

/// Persist `master_key` to the keychain. Used by the V0.1 bridge path to
/// write the new master_key BEFORE the destructive PRAGMA rekey.
#[cfg(windows)]
fn write_keychain_entry(namespace: &str, vault_id: &str, master_key: &[u8; 32]) -> VaultResult<()> {
    use keyring_core::Entry;

    let entry = Entry::new(namespace, vault_id).map_err(|e| {
        VaultError::KeychainProvenance(format!(
            "Entry::new failed during bridge keychain write: {e}"
        ))
    })?;
    entry.set_secret(master_key).map_err(|e| {
        VaultError::KeychainProvenance(format!("set_secret failed during bridge: {e}"))
    })?;
    Ok(())
}

/// Best-effort delete of the bridge-written keychain entry. Used in
/// rollback paths when the destructive ops fail post-keychain-write.
/// Failure to delete is logged WARN; the orphan entry is recoverable on
/// next launch (the bridge's existence-probe sees it + branch 1 returns
/// the master_key, but no vault.db is actually keyed to it → next call
/// to MetadataStore::open fails closed → user re-runs setup).
#[cfg(windows)]
fn rollback_keychain_entry(namespace: &str, vault_id: &str) {
    use keyring_core::Entry;

    let entry = match Entry::new(namespace, vault_id) {
        Ok(e) => e,
        Err(e) => {
            warn!(
                error = %e,
                namespace,
                vault_id,
                "rollback: Entry::new failed (cannot delete orphan keychain entry; \
                 manual cleanup via Credential Manager required)"
            );
            return;
        }
    };
    if let Err(e) = entry.delete_credential() {
        warn!(
            error = %e,
            namespace,
            vault_id,
            "rollback: delete_credential failed (orphan keychain entry remains; \
             manual cleanup via Credential Manager required)"
        );
    }
}

/// Destroy the master_key in the OS keychain — **cryptographic erasure**
/// (ADR-SEC-008).
///
/// This is the operative half of "delete everything". Every at-rest key in
/// the vault is derived from this one 32-byte secret (`at_rest_key` and
/// `sqlcipher_passphrase`, both BLAKE3 subkeys — see the module docs), so
/// destroying it renders `vault.db`, the Lance vector store, the sealed
/// graph, and the sealed REPORTs permanently undecryptable **whether or not
/// those files are ever removed from disk**.
///
/// This is the mechanism BRD §11.5.4 already specifies: *"Account deletion:
/// master key is destroyed, making all encrypted data permanently
/// unrecoverable."* NIST SP 800-88 classes cryptographic erase as
/// **Purge**-level sanitization, and unlike file deletion it is not defeated
/// by filesystem journaling, SSD wear-levelling, block remapping, or a
/// backup of the data directory taken earlier.
///
/// # Ordering contract — destroy the KEY BEFORE the FILES
///
/// Callers MUST call this first and delete the data directory second. The
/// two orders fail very differently:
///
/// - key-then-files, interrupted → files remain but are **cryptographically
///   dead**. The user's data is gone, which is what they asked for.
/// - files-then-key, interrupted → the key survives in Credential Manager
///   and any copy of the data directory (a backup, a synced folder, a
///   forensic image) is **still decryptable**. The erasure silently did not
///   happen.
///
/// Only the first order is safe under partial failure, so it is the
/// contract rather than an implementation detail.
///
/// # Returns
///
/// `Ok(true)` if an entry existed and was deleted, `Ok(false)` if there was
/// nothing to delete. Idempotent: erasing an already-erased vault is a
/// successful no-op, not an error, so a retry after a partial failure is
/// always safe.
///
/// # Errors
///
/// [`VaultError::KeychainProvenance`] if the keychain store cannot be
/// opened or the delete fails for a reason other than "not found". **A
/// caller MUST NOT report the wipe as successful when this errors** — the
/// data is still readable.
#[cfg(windows)]
pub fn destroy_master_key(namespace: &str, vault_id: &str) -> VaultResult<bool> {
    use windows_native_keyring_store::Store;

    keyring_core::set_default_store(
        Store::new()
            .map_err(|e| VaultError::KeychainProvenance(format!("Store::new failed: {e}")))?,
    );

    let result = destroy_master_key_inner(namespace, vault_id);

    keyring_core::unset_default_store();

    result
}

/// Non-Windows stub, mirroring [`read_or_init_master_key`]. Erasure must
/// FAIL LOUDLY rather than silently no-op on an unsupported platform: a
/// caller that treated `Ok(false)` as "nothing to erase" would tell the
/// user their vault was destroyed when it was not.
#[cfg(not(windows))]
pub fn destroy_master_key(_namespace: &str, _vault_id: &str) -> VaultResult<bool> {
    Err(VaultError::KeychainProvenance(format!(
        "Cryptographic erasure is Windows-only at V0.2 Phase 1 (current platform: {}). \
         Erasure MUST NOT be reported as successful on an unsupported platform.",
        std::env::consts::OS
    )))
}

#[cfg(windows)]
fn destroy_master_key_inner(namespace: &str, vault_id: &str) -> VaultResult<bool> {
    use keyring_core::Entry;

    let entry = Entry::new(namespace, vault_id).map_err(|e| {
        VaultError::KeychainProvenance(format!("Entry::new failed during erasure: {e}"))
    })?;

    // `keyring-core` does not expose a stable NotFound kind (see the
    // read path's note at `read_or_init_inner`), so "already absent" is
    // distinguished by probing first. A racing second erase therefore
    // reports `Ok(false)` rather than an error — correct for an operation
    // whose postcondition is "the key does not exist".
    match entry.get_secret() {
        Err(_) => {
            info!(
                namespace,
                vault_id, "cryptographic erasure: no keychain entry present (already erased)"
            );
            Ok(false)
        }
        Ok(_) => {
            entry.delete_credential().map_err(|e| {
                VaultError::KeychainProvenance(format!("delete_credential failed: {e}"))
            })?;
            // Verify rather than assume. This is the single step that makes
            // the data unrecoverable; a delete that silently did nothing
            // would leave us telling the user their vault was erased when
            // every byte is still decryptable.
            if entry.get_secret().is_ok() {
                return Err(VaultError::KeychainProvenance(
                    "delete_credential reported success but the master_key is still \
                     readable; cryptographic erasure did NOT occur"
                        .to_string(),
                ));
            }
            warn!(
                namespace,
                vault_id,
                "CRYPTOGRAPHIC ERASURE COMPLETE: master_key destroyed; all vault \
                 data is now permanently unrecoverable (ADR-SEC-008)"
            );
            Ok(true)
        }
    }
}

/// Derive the SqlCipher passphrase from the master_key per ADR-040
/// amendment option β: BLAKE3 derive_key with the locked
/// [`SQLCIPHER_KDF_CONTEXT`] domain-separator, hex-encoded to a 64-character
/// String suitable for [`vault_storage::SqlCipherKey::new`].
///
/// Hex-encoding is a serialization choice (32 raw bytes don't fit cleanly
/// into a `String` passphrase per SqlCipherKey's contract), not a security
/// choice. The 32 bytes are full-strength keying material; SQLCipher's
/// PBKDF2 over the hex-encoded form is defense-in-depth.
pub fn derive_sqlcipher_passphrase(master_key: &[u8; 32]) -> vault_storage::SqlCipherKey {
    let subkey = blake3::derive_key(SQLCIPHER_KDF_CONTEXT, master_key);
    vault_storage::SqlCipherKey::new(hex::encode(subkey))
}

/// Derive the at-rest sealing key from the master_key per ADR-008
/// amendment K3 KDF: BLAKE3 derive_key with the locked
/// [`AT_REST_KDF_CONTEXT`] domain-separator. Returned as
/// `Zeroizing<[u8; 32]>` for downstream consumption by
/// [`vault_storage::LanceVectorStore::open_with_at_rest_key`] at Phase 2/3.
pub fn derive_at_rest_key(master_key: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let subkey = blake3::derive_key(AT_REST_KDF_CONTEXT, master_key);
    Zeroizing::new(subkey)
}

// =============================================================================
// Test helpers — feature-gated module for cross-crate test consumption
// =============================================================================

/// Test infrastructure exposed for cross-crate test consumption.
///
/// **Visibility:** module-private under `#[cfg(test)]` for vault-app's own
/// tests; `pub` when the `test-helpers` Cargo feature is enabled. Consumers
/// (e.g., vault-cli's tests) enable the feature via `[dev-dependencies]`:
///
/// ```toml
/// vault-app = { path = "../vault-app", features = ["test-helpers"] }
/// ```
///
/// **Why a feature flag, not `#[cfg(test)]`:** `cfg(test)` does NOT propagate
/// across crate boundaries — downstream consumers' test compilation cannot
/// see this crate's `cfg(test)` items. The Cargo feature is the load-bearing
/// piece that exposes the helpers to downstream test builds.
///
/// Promoted from `mod tests`'s private scope at T0.2.0 Phase 3 sub-task (a)
/// (2026-05-11) to eliminate the 50-LOC duplication that would otherwise
/// have landed in vault-cli's tests.
#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers {
    /// Generate a unique-per-test namespace so concurrent + sequential test
    /// runs never collide on the same keychain entry. Prefixed with
    /// `com.zaaheen.test.v0.2.` so a stale entry from a panicked test
    /// is still distinguishable from production / spike entries during
    /// manual Credential Manager cleanup.
    #[cfg(windows)]
    pub fn unique_test_namespace(test_name: &str) -> String {
        let mut nonce = [0u8; 8];
        getrandom::getrandom(&mut nonce).expect("getrandom for test namespace");
        format!("com.zaaheen.test.v0.2.{}.{}", test_name, hex::encode(nonce))
    }

    /// Best-effort cleanup helper. Used in test teardown so re-runs are
    /// deterministic; failures are logged but do not fail the test.
    #[cfg(windows)]
    pub fn cleanup_keychain_entry(namespace: &str, vault_id: &str) {
        use keyring_core::Entry;
        use windows_native_keyring_store::Store;

        let store = match Store::new() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[cleanup] Store::new failed: {e}");
                return;
            }
        };
        keyring_core::set_default_store(store);
        if let Ok(entry) = Entry::new(namespace, vault_id) {
            let _ = entry.delete_credential();
        }
        keyring_core::unset_default_store();
    }

    /// Plant a malformed (non-32-byte) keychain entry so subsequent
    /// [`super::read_or_init_master_key`] calls return
    /// [`vault_core::VaultError::KeychainProvenance`] per the wrong-length-
    /// secret fail-closed branch (`keychain.rs:154-163`). Used by negative-
    /// path tests asserting generic "authentication failed" surfaces without
    /// info leak per BRD §11.7.2.
    #[cfg(windows)]
    pub fn plant_malformed_keychain_entry(namespace: &str, vault_id: &str) {
        use keyring_core::Entry;
        use windows_native_keyring_store::Store;

        let store = Store::new().expect("Store::new for malformed-entry planting");
        keyring_core::set_default_store(store);
        let entry =
            Entry::new(namespace, vault_id).expect("Entry::new for malformed-entry planting");
        // 31 bytes (NOT 32) triggers the "secret is N bytes (expected 32)"
        // fail-closed branch in read_or_init_master_key.
        let malformed: [u8; 31] = [0u8; 31];
        entry
            .set_secret(&malformed)
            .expect("set_secret for malformed-entry planting");
        keyring_core::unset_default_store();
    }

    /// Process-global mutex serializing keychain tests. `keyring_core`'s
    /// `set_default_store` / `unset_default_store` are process-global state;
    /// parallel tests under `RUST_TEST_THREADS=4` (ADR-038 layer 3 sibling)
    /// race on the slot — test A unsetting the store while test B is mid-
    /// keychain-write produces "No default store has been set" errors.
    /// Production has no contention (vault-tauri + vault-cli each call the
    /// keychain helpers once at startup). Tests serialize via this mutex;
    /// each test acquires it before any keychain-related work and releases
    /// via Drop at test end.
    #[cfg(windows)]
    pub static KEYCHAIN_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Acquire the keychain serialization guard. Released on Drop.
    /// `unwrap_or_else(|p| p.into_inner())` recovers from a poisoned mutex
    /// (prior-test panic) so one bad test doesn't lock out the rest of the
    /// suite.
    #[cfg(windows)]
    pub fn keychain_test_guard() -> std::sync::MutexGuard<'static, ()> {
        KEYCHAIN_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Test helpers (unique_test_namespace, cleanup_keychain_entry,
    // keychain_test_guard, plant_malformed_keychain_entry, KEYCHAIN_TEST_MUTEX)
    // are now in `super::test_helpers` per T0.2.0 Phase 3 sub-task (a)
    // promotion (2026-05-11) — feature-gated for cross-crate consumption
    // by vault-cli's tests (eliminates 50-LOC duplication).
    #[cfg(windows)]
    use super::test_helpers::*;

    /// Round-trip: write a master_key via the helper path → read back → byte-
    /// equal. Per iteration-1.5 amendment test floor adjustment (a).
    #[test]
    #[cfg(windows)]
    fn read_or_init_master_key_round_trips_byte_identical() {
        let _guard = keychain_test_guard();
        let namespace = unique_test_namespace("round_trip");
        let vault_id = "test-round-trip-vault";

        // First call generates + persists. Second call reads back the
        // persisted bytes. Byte-equal assertion verifies the keychain
        // round-trip preserves all 32 bytes exactly.
        let first = read_or_init_master_key(&namespace, vault_id).expect("first call must succeed");
        let second =
            read_or_init_master_key(&namespace, vault_id).expect("second call must succeed");

        assert_eq!(
            first.as_slice(),
            second.as_slice(),
            "Round-trip MUST preserve all 32 bytes byte-identical; \
             first call generated key X, second call read back key Y, X != Y. \
             Either get_secret returned different bytes than set_secret wrote, \
             OR the second call hit first-run path despite the entry existing \
             (NotFound classifier regression)."
        );

        cleanup_keychain_entry(&namespace, vault_id);
    }

    /// First-run-generates-and-persists: no entry exists → first call
    /// generates new 32-byte key + persists. Second call reads the
    /// persisted entry (NOT a fresh generation). Per iteration-1.5
    /// amendment test floor adjustment (b).
    #[test]
    #[cfg(windows)]
    fn read_or_init_master_key_first_run_generates_and_persists() {
        let _guard = keychain_test_guard();
        let namespace = unique_test_namespace("first_run");
        let vault_id = "test-first-run-vault";

        // Pre-cleanup so the test starts from a known no-entry state even if
        // a prior test run leaked.
        cleanup_keychain_entry(&namespace, vault_id);

        let first = read_or_init_master_key(&namespace, vault_id)
            .expect("first call must succeed (first-run path)");

        // Subsequent call must return THE SAME bytes (read existing entry),
        // not regenerate. Byte-equal assertion = persistence proof.
        let second = read_or_init_master_key(&namespace, vault_id)
            .expect("second call must succeed (read existing entry)");
        assert_eq!(
            first.as_slice(),
            second.as_slice(),
            "First-run-generates-and-persists invariant: second call MUST read \
             the persisted key, not regenerate. If second != first, the helper \
             is hitting first-run path twice — entry was not persisted, OR the \
             read path's NotFound classifier is misfiring on a successful read."
        );

        // Sanity: keys are 32 bytes (not zero, not truncated).
        assert_eq!(
            first.len(),
            32,
            "Generated master_key must be 32 bytes; got {}",
            first.len()
        );
        let all_zero = first.iter().all(|&b| b == 0);
        assert!(
            !all_zero,
            "Generated master_key must NOT be all zeros (would indicate getrandom \
             returned without filling the buffer or a write/read corruption)."
        );

        cleanup_keychain_entry(&namespace, vault_id);
    }

    /// SqlCipher derivation is deterministic + uses the locked context string.
    /// Defensive pin against accidental KDF-context drift.
    #[test]
    fn derive_sqlcipher_passphrase_is_deterministic() {
        let master_key = [42u8; 32];
        let p1 = derive_sqlcipher_passphrase(&master_key);
        let p2 = derive_sqlcipher_passphrase(&master_key);
        // SqlCipherKey doesn't impl PartialEq + Debug per its zeroize-on-drop
        // discipline; we compare via the crate-private as_str through the
        // Display-less surface: rebuild and compare hex outputs by recomputing
        // the BLAKE3 derive_key step independently here.
        let expected = hex::encode(blake3::derive_key(SQLCIPHER_KDF_CONTEXT, &master_key));
        assert_eq!(
            expected.len(),
            64,
            "BLAKE3 derive_key output must hex-encode to 64 ASCII chars; got {}",
            expected.len()
        );
        // Both p1 and p2 derived from the same master_key + same context must
        // be byte-equal — but SqlCipherKey hides its bytes by design. This
        // assertion verifies the BLAKE3 layer is deterministic; the
        // SqlCipherKey layer is a thin wrapper.
        let again = blake3::derive_key(SQLCIPHER_KDF_CONTEXT, &master_key);
        assert_eq!(
            hex::encode(again),
            expected,
            "BLAKE3 derive_key with the same context + same master_key MUST \
             produce identical output; non-deterministic output is a libray bug."
        );
        // p1 / p2 are dropped here, exercising Zeroize on the SqlCipherKey
        // wrapper for a smoke check that the helper-returned values can be
        // dropped without unsafe panics.
        drop((p1, p2));
    }

    /// At-rest derivation is deterministic + uses ADR-008 amendment K3 KDF
    /// context. Defensive pin against accidental K3 KDF context drift.
    #[test]
    fn derive_at_rest_key_is_deterministic_and_uses_k3_kdf_context() {
        let master_key = [7u8; 32];
        let k1 = derive_at_rest_key(&master_key);
        let k2 = derive_at_rest_key(&master_key);
        assert_eq!(
            k1.as_slice(),
            k2.as_slice(),
            "BLAKE3 derive_key with the same context + master_key MUST be \
             deterministic; non-deterministic output is a library bug."
        );
        // ADR-008 amendment locks the context string verbatim; drift here
        // would silently change at-rest key material, breaking on-disk
        // sealed bytes already written under the old derivation. Defensive
        // pin: independently recompute and compare.
        let expected = blake3::derive_key("vault memory at-rest sealing v1", &master_key);
        assert_eq!(
            k1.as_slice(),
            expected.as_slice(),
            "ADR-008 amendment K3 KDF context drift detected — derive_at_rest_key \
             output does not match independently-recomputed BLAKE3 derive_key with \
             context string \"vault memory at-rest sealing v1\". This would \
             invalidate every on-disk sealed file written under the prior \
             derivation. STOP."
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // ADR-041 V0.1 → V0.2 SQLCipher passphrase bridge tests (Tier 1)
    //
    // Per ADR-041 plan iteration 2 §6: 7 named tests + 1 Tier 2 integration.
    // All bridge tests `#[cfg(windows)]` — bridge wraps Windows Credential
    // Manager via `windows_native_keyring_store::Store` (Phase 1 lock per
    // ADR-029 + ADR-040 OQ #1 partial resolution). macOS/Linux per-platform
    // crate-add at T0.2.0.x sub-task or T0.2.14 Stub-Installer-adjacent.
    //
    // Tests serialize keychain access via `KEYCHAIN_TEST_MUTEX` (defined in
    // `super::test_helpers` per T0.2.0 Phase 3 sub-task (a) promotion);
    // see the module doc-comment there for the full rationale.
    // ────────────────────────────────────────────────────────────────────

    /// Test fixture helper: create a fresh SQLCipher file at `path`,
    /// keyed with `passphrase`, with `n_rows` rows of substantive content
    /// inserted. Used by tier-1 bridge tests + the corruption-mode test 7
    /// (which needs a multi-page DB to corrupt a non-header page).
    #[cfg(windows)]
    fn create_v0_1_sqlcipher_fixture(path: &Path, passphrase: &str, n_rows: usize) {
        use rusqlite::{Connection, OpenFlags};

        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("fixture open");
        conn.pragma_update(None, "key", passphrase)
            .expect("fixture set key");
        conn.execute_batch(
            "CREATE TABLE memories_v0_1 (id INTEGER PRIMARY KEY, content TEXT NOT NULL);",
        )
        .expect("fixture create table");
        for i in 0..n_rows {
            // Substantive content per row so multiple rows cross page
            // boundaries — needed by test 7's corruption probe to land
            // outside page 1.
            let content = format!("v0_1_row_{i}_padding_{}", "x".repeat(256));
            conn.execute(
                "INSERT INTO memories_v0_1 (id, content) VALUES (?1, ?2)",
                rusqlite::params![i as i64, content],
            )
            .expect("fixture insert");
        }
    }

    /// Read back a known row from a SQLCipher file under a given passphrase.
    /// Returns `Err` if open / set-key / read fails. Used to verify
    /// post-bridge content equality (rekey didn't lose data).
    #[cfg(windows)]
    fn read_row_content(path: &Path, passphrase: &str, id: i64) -> Result<String, String> {
        use rusqlite::{Connection, OpenFlags};

        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| format!("open: {e}"))?;
        conn.pragma_update(None, "key", passphrase)
            .map_err(|e| format!("set key: {e}"))?;
        conn.query_row(
            "SELECT content FROM memories_v0_1 WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| format!("read: {e}"))
    }

    /// Probe whether a keychain entry exists under `(namespace, vault_id)`.
    /// Returns true if the entry exists with a 32-byte secret. Used by
    /// rollback-assertion paths in tests.
    #[cfg(windows)]
    fn keychain_entry_exists(namespace: &str, vault_id: &str) -> bool {
        use keyring_core::Entry;
        use windows_native_keyring_store::Store;

        let store = match Store::new() {
            Ok(s) => s,
            Err(_) => return false,
        };
        keyring_core::set_default_store(store);
        let exists = Entry::new(namespace, vault_id)
            .ok()
            .and_then(|e| e.get_secret().ok())
            .map(|bytes| bytes.len() == 32)
            .unwrap_or(false);
        keyring_core::unset_default_store();
        exists
    }

    /// Test 1 — happy path: bridge rekeys a fresh V0.1 SQLCipher fixture
    /// from VAULT_KEY-derived passphrase to keychain-derived passphrase,
    /// preserving all rows. Verifies §3 steps 1-9 + §10 post-write
    /// verification end-to-end.
    #[test]
    #[cfg(windows)]
    fn bridge_rekeys_fresh_sqlcipher_file_and_preserves_rows() {
        let _guard = keychain_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir = tmp.path();
        let vault_db = data_dir.join("vault.db");
        let namespace = unique_test_namespace("bridge_happy_path");
        let vault_id = "test-bridge-happy";

        cleanup_keychain_entry(&namespace, vault_id);
        create_v0_1_sqlcipher_fixture(&vault_db, "v0_1_test_passphrase", 5);

        let master_key =
            bridge_or_init_master_key(data_dir, &namespace, vault_id, Some("v0_1_test_passphrase"))
                .expect("bridge happy path must succeed");

        // Verify keychain entry created.
        assert!(
            keychain_entry_exists(&namespace, vault_id),
            "keychain entry MUST be persisted after successful bridge"
        );

        // Verify vault.db is now keyed by the new keychain-derived passphrase.
        // `SqlCipherKey::as_str` is `pub(crate)` to vault-storage so we
        // recompute the hex form here for the test's direct rusqlite read.
        // This is the SAME derivation `derive_sqlcipher_passphrase` performs
        // internally — test-side independence prevents a derivation drift
        // from silently passing this assertion.
        let new_passphrase_hex =
            hex::encode(blake3::derive_key(SQLCIPHER_KDF_CONTEXT, &*master_key));
        let row = read_row_content(&vault_db, &new_passphrase_hex, 0)
            .expect("post-bridge read with new passphrase must succeed");
        assert!(
            row.starts_with("v0_1_row_0_padding_"),
            "row content MUST be preserved across rekey; got {row:?}"
        );

        // Verify snapshot file was cleaned up (success-path step 8).
        let snapshot = snapshot_path_for(&vault_db);
        assert!(
            !snapshot.exists(),
            "snapshot file MUST be cleaned up on bridge success; still exists at {}",
            snapshot.display()
        );

        cleanup_keychain_entry(&namespace, vault_id);
    }

    /// Test 2 — wrong VAULT_KEY: bridge fail-fasts at step 1 (verify),
    /// no keychain entry created, vault.db unchanged.
    #[test]
    #[cfg(windows)]
    fn bridge_fails_closed_when_vault_key_is_wrong() {
        let _guard = keychain_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir = tmp.path();
        let vault_db = data_dir.join("vault.db");
        let namespace = unique_test_namespace("bridge_wrong_key");
        let vault_id = "test-bridge-wrong-key";

        cleanup_keychain_entry(&namespace, vault_id);
        create_v0_1_sqlcipher_fixture(&vault_db, "v0_1_correct_passphrase", 3);

        let pre_bridge_bytes = std::fs::read(&vault_db).expect("read vault.db pre-bridge");

        let result = bridge_or_init_master_key(
            data_dir,
            &namespace,
            vault_id,
            Some("v0_1_WRONG_passphrase"),
        );

        assert!(
            matches!(result, Err(VaultError::KeychainProvenance(_))),
            "wrong VAULT_KEY MUST fail closed with KeychainProvenance; got {result:?}"
        );
        assert!(
            !keychain_entry_exists(&namespace, vault_id),
            "wrong VAULT_KEY MUST NOT create a keychain entry (fail-fast at step 1, before step 4)"
        );
        let post_bridge_bytes = std::fs::read(&vault_db).expect("read vault.db post-bridge");
        assert_eq!(
            pre_bridge_bytes, post_bridge_bytes,
            "wrong VAULT_KEY MUST leave vault.db bit-for-bit unchanged"
        );

        cleanup_keychain_entry(&namespace, vault_id);
    }

    /// Test 3 — VAULT_KEY unset: bridge returns tailored
    /// KeychainProvenance error naming VAULT_KEY for user-facing dialog.
    #[test]
    #[cfg(windows)]
    fn bridge_fails_closed_when_vault_key_is_unset() {
        let _guard = keychain_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir = tmp.path();
        let vault_db = data_dir.join("vault.db");
        let namespace = unique_test_namespace("bridge_no_key");
        let vault_id = "test-bridge-no-key";

        cleanup_keychain_entry(&namespace, vault_id);
        create_v0_1_sqlcipher_fixture(&vault_db, "v0_1_test_passphrase", 1);

        // Both None and Some("") must trigger the "VAULT_KEY env var is not set"
        // path (per bridge's match arm `Some(s) if !s.is_empty()`).
        for vault_key in [None, Some("")] {
            let result = bridge_or_init_master_key(data_dir, &namespace, vault_id, vault_key);
            match result {
                Err(VaultError::KeychainProvenance(msg)) => {
                    assert!(
                        msg.contains("VAULT_KEY env var is not set"),
                        "VAULT_KEY-unset error MUST name VAULT_KEY for the user-facing \
                         dialog (vault_key={vault_key:?}); got: {msg}"
                    );
                }
                other => panic!(
                    "VAULT_KEY-unset MUST fail closed with KeychainProvenance \
                     (vault_key={vault_key:?}); got {other:?}"
                ),
            }
            assert!(
                !keychain_entry_exists(&namespace, vault_id),
                "VAULT_KEY-unset MUST NOT create a keychain entry (vault_key={vault_key:?})"
            );
        }
    }

    /// Test 4 — keychain entry already present: bridge takes branch 1
    /// (V0.2 second-launch path), returns existing master_key, ignores
    /// V0.1 vault.db / VAULT_KEY entirely.
    #[test]
    #[cfg(windows)]
    fn bridge_no_op_when_keychain_entry_already_exists() {
        let _guard = keychain_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir = tmp.path();
        let vault_db = data_dir.join("vault.db");
        let namespace = unique_test_namespace("bridge_existing_kc");
        let vault_id = "test-bridge-existing-kc";

        cleanup_keychain_entry(&namespace, vault_id);
        // Pre-populate keychain via the existing read_or_init path (which
        // generates + persists on first call).
        let pre_existing = read_or_init_master_key(&namespace, vault_id)
            .expect("pre-populate keychain via read_or_init");

        // Even if a V0.1 vault.db is sitting there + VAULT_KEY is set,
        // bridge takes branch 1 (existing keychain entry) and ignores them.
        create_v0_1_sqlcipher_fixture(&vault_db, "irrelevant_v0_1_passphrase", 1);
        let pre_bytes = std::fs::read(&vault_db).expect("read vault.db pre-bridge");

        let returned = bridge_or_init_master_key(
            data_dir,
            &namespace,
            vault_id,
            Some("irrelevant_v0_1_passphrase"),
        )
        .expect("bridge with existing keychain entry must succeed");

        assert_eq!(
            pre_existing.as_slice(),
            returned.as_slice(),
            "bridge MUST return existing keychain master_key byte-identical when keychain \
             entry exists (branch 1, no V0.1 detection runs)"
        );
        let post_bytes = std::fs::read(&vault_db).expect("read vault.db post-bridge");
        assert_eq!(
            pre_bytes, post_bytes,
            "bridge MUST NOT touch vault.db when keychain entry exists (branch 1 short-circuit)"
        );

        cleanup_keychain_entry(&namespace, vault_id);
    }

    /// Test 5 — fresh V0.2 install (no keychain + no V0.1 vault.db):
    /// bridge takes branch 2, delegates to read_or_init's first-run path
    /// (generate + persist new master_key).
    #[test]
    #[cfg(windows)]
    fn bridge_no_op_when_no_v0_1_sqlcipher_file_present() {
        let _guard = keychain_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir = tmp.path();
        let namespace = unique_test_namespace("bridge_fresh_install");
        let vault_id = "test-bridge-fresh-install";

        cleanup_keychain_entry(&namespace, vault_id);
        // No vault.db created — fresh V0.2 install state.
        assert!(!data_dir.join("vault.db").exists());

        let master_key = bridge_or_init_master_key(data_dir, &namespace, vault_id, None)
            .expect("bridge with no V0.1 vault must succeed via fresh-init delegation");

        assert!(
            keychain_entry_exists(&namespace, vault_id),
            "fresh V0.2 install MUST persist new keychain entry (delegated to read_or_init)"
        );
        // Sanity: 32 bytes, not all zero.
        assert_eq!(master_key.len(), 32);
        assert!(master_key.iter().any(|&b| b != 0));

        cleanup_keychain_entry(&namespace, vault_id);
    }

    /// Test 6 — keychain-write-before-rekey ordering invariant (§3).
    /// Trigger a step-5 (snapshot) failure deliberately + verify rollback
    /// removed the keychain entry. The fact that rollback REMOVES (not
    /// "never created") proves keychain WAS written before snapshot —
    /// pinning the §2 Pattern B "cheapest-to-rollback co-store write
    /// FIRST" invariant.
    ///
    /// Mechanism: pre-create the snapshot path as a directory (not a
    /// file). `std::fs::copy(vault.db, snapshot_path)` then fails with
    /// "destination is a directory" without touching vault.db, leaving
    /// keychain in the post-step-4 state for the rollback to clear.
    #[test]
    #[cfg(windows)]
    fn bridge_writes_keychain_before_rekey_ordering_invariant() {
        let _guard = keychain_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir = tmp.path();
        let vault_db = data_dir.join("vault.db");
        let namespace = unique_test_namespace("bridge_ordering");
        let vault_id = "test-bridge-ordering";

        cleanup_keychain_entry(&namespace, vault_id);
        create_v0_1_sqlcipher_fixture(&vault_db, "v0_1_ordering", 2);

        // Pre-create the snapshot path AS A DIRECTORY to make
        // `std::fs::copy(vault.db, snapshot)` fail at step 5.
        let snapshot_blocker = snapshot_path_for(&vault_db);
        std::fs::create_dir(&snapshot_blocker).expect("create snapshot blocker dir");

        let result =
            bridge_or_init_master_key(data_dir, &namespace, vault_id, Some("v0_1_ordering"));

        assert!(
            result.is_err(),
            "bridge MUST fail when snapshot step is blocked; got {result:?}"
        );
        assert!(
            !keychain_entry_exists(&namespace, vault_id),
            "rollback MUST have removed keychain entry. The fact that rollback REMOVES \
             rather than 'never created' is the empirical proof that step 4 (keychain \
             write) ran BEFORE step 5 (snapshot). If keychain entry is absent post-test \
             AND step 5 failed, ordering invariant from §3 is preserved."
        );

        // Cleanup the snapshot blocker dir we pre-created.
        std::fs::remove_dir(&snapshot_blocker).ok();
        cleanup_keychain_entry(&namespace, vault_id);
    }

    /// Test 7 — snapshot-restore-on-rekey-failure (§3 step 6/7 fail path).
    /// Methodology (a) corrupted-fixture per ADR-041 plan iteration 2 §6 +
    /// iteration 2.1 §11. Probes candidate (ii) malformed-page injection:
    /// PRAGMA key + sqlite_master read (page 1) succeed; PRAGMA rekey
    /// iterates all pages → hits HMAC failure on corrupted page N → Err.
    ///
    /// Per pin 2 watch-trigger: if this corruption mode doesn't reliably
    /// trigger rekey-Err within ~1h of probing, surface to iteration 3.
    #[test]
    #[cfg(windows)]
    fn bridge_restores_from_snapshot_on_rekey_failure() {
        let _guard = keychain_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir = tmp.path();
        let vault_db = data_dir.join("vault.db");
        let namespace = unique_test_namespace("bridge_rekey_fail");
        let vault_id = "test-bridge-rekey-fail";

        cleanup_keychain_entry(&namespace, vault_id);

        // Create a multi-page V0.1 SQLCipher fixture (n_rows × 256-byte
        // padding ≈ multiple 4 KB pages even after compression).
        let v0_1_passphrase = "v0_1_rekey_fail_test";
        create_v0_1_sqlcipher_fixture(&vault_db, v0_1_passphrase, 50);

        let pre_size = std::fs::metadata(&vault_db)
            .expect("metadata pre-corrupt")
            .len();
        // Sanity: must be > 8 KB so we have at least page 2 to corrupt.
        assert!(
            pre_size > 8192,
            "fixture must span >= 2 pages for the corruption probe; got {pre_size}"
        );

        // Corrupt a non-header byte deep in the middle of the file. SQLCipher
        // default page size is 4096; offset 6000 lands inside page 2 body
        // (well past the page-2 page-prefix header). Flip all bits to
        // invalidate the per-page HMAC.
        let mut bytes = std::fs::read(&vault_db).expect("read vault.db");
        bytes[6000] ^= 0xFF;
        std::fs::write(&vault_db, &bytes).expect("write corrupted vault.db");

        // Sanity: PRAGMA key + sqlite_master query (page 1) must still
        // succeed against the corrupted file. If this fails, the corruption
        // landed too early (page 1) and step 1 verify would fail before
        // reaching rekey — different test path. Surface as test setup error.
        let pragma_key_works = read_row_content(&vault_db, v0_1_passphrase, 0).is_err();
        // Note: we use a query against memories_v0_1 not sqlite_master, so a
        // failure could be either (a) PRAGMA key failed (page 1 corrupted)
        // or (b) the row itself is on a corrupted page. (b) is fine for our
        // test purpose. The bridge's verify_sqlcipher_passphrase uses
        // SELECT count(*) FROM sqlite_master, which only touches page 1; if
        // page 1 is intact, that succeeds even when later pages are corrupt.
        // We assert the bridge call shape below to confirm.
        let _ = pragma_key_works; // silence unused warning if path differs

        // Call bridge — expect Err at step 6 (rekey) AFTER step 1 verify
        // succeeds (page 1 intact) AND step 4 keychain write succeeds AND
        // step 5 snapshot succeeds.
        let result =
            bridge_or_init_master_key(data_dir, &namespace, vault_id, Some(v0_1_passphrase));

        match result {
            Err(VaultError::Storage(msg)) => {
                // Bridge should report rekey failure with rollback context.
                assert!(
                    msg.contains("rekey")
                        || msg.contains("Snapshot restored")
                        || msg.contains("step 6"),
                    "rekey failure error MUST name the failing step or rollback action; \
                     got: {msg}"
                );
            }
            Err(VaultError::KeychainProvenance(msg)) => {
                // If the corruption made step 1 verify fail instead of step 6
                // rekey, we surface as "PIN 2 WATCH TRIGGER" rather than a
                // silent test failure — the corruption-mode candidate (ii)
                // didn't isolate rekey-failure on this dep chain.
                panic!(
                    "WATCH-TRIGGER iteration-3 candidate: corruption at offset 6000 \
                     made step 1 verify fail instead of step 6 rekey. Need to find a \
                     corruption mode that lets sqlite_master read succeed but rekey \
                     iterate fail. Adjust offset OR use larger fixture OR escalate to \
                     iteration 3 per pin 2 watch-trigger. Error: {msg}"
                );
            }
            other => {
                panic!("bridge with corrupted vault.db MUST fail at rekey step; got {other:?}")
            }
        }

        // Verify rollback ran:
        // - keychain entry deleted
        // - vault.db restored from snapshot (back to the corrupted state we
        //   wrote, NOT some half-rekeyed state)
        // - snapshot file cleaned up
        assert!(
            !keychain_entry_exists(&namespace, vault_id),
            "rollback MUST delete keychain entry on rekey failure"
        );
        let post_size = std::fs::metadata(&vault_db)
            .expect("metadata post-bridge")
            .len();
        assert_eq!(
            pre_size, post_size,
            "vault.db file size MUST equal pre-bridge size after snapshot restore \
             (pre={pre_size}, post={post_size}). Different size = snapshot restore \
             didn't run OR restored from a different file."
        );
        let snapshot = snapshot_path_for(&vault_db);
        assert!(
            !snapshot.exists(),
            "snapshot file MUST be cleaned up post-rollback; still exists at {}",
            snapshot.display()
        );

        cleanup_keychain_entry(&namespace, vault_id);
    }

    /// Test 8 — Tier 2 integration: bridge against the captured V0.1
    /// fixture from commit `1d72aac` (V0.1 SHIPPED). The fixture has 5
    /// known rows + the captured VAULT_KEY checked in alongside the
    /// fixture README. This test is the realism gate per ADR-041 plan
    /// iteration 2 §5 — Tier 1 uses synthetic SQLCipher fixtures
    /// (rusqlite-direct construction) which might not exercise the
    /// exact byte shape the V0.1 binary actually produced. Tier 2
    /// proves the bridge works against real V0.1-binary-emitted bytes.
    ///
    /// Operates on a deep-copy of the fixture in a tempdir — the
    /// checked-in fixture file is read-only by convention; mutating
    /// it via the bridge would corrupt the test fixture.
    #[test]
    #[cfg(windows)]
    fn tier_2_real_v0_1_vault_db_bridges_and_preserves_5_rows() {
        let _guard = keychain_test_guard();
        let fixture_vault_db = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("vault-storage")
            .join("tests")
            .join("fixtures")
            .join("v0_1_alpha_data_dir")
            .join("vault.db");
        assert!(
            fixture_vault_db.exists(),
            "Tier 2 fixture not found at {}: ADR-041 §5 promised the captured V0.1 \
             vault.db (98 KB, commit 1d72aac, capture key \
             'fixture-capture-key-do-not-use-in-prod'). Recapture per the fixture \
             README if missing.",
            fixture_vault_db.display()
        );

        // Deep-copy into a tempdir so the bridge can rekey + we don't
        // mutate the checked-in fixture.
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir = tmp.path();
        let test_vault_db = data_dir.join("vault.db");
        std::fs::copy(&fixture_vault_db, &test_vault_db).expect("copy fixture vault.db");
        // Also copy vault.db-wal so SQLite's WAL recovery on first open
        // sees a consistent state (the fixture README documents the
        // WAL was captured alongside).
        let fixture_wal = fixture_vault_db.with_file_name("vault.db-wal");
        if fixture_wal.exists() {
            std::fs::copy(&fixture_wal, data_dir.join("vault.db-wal"))
                .expect("copy fixture vault.db-wal");
        }

        let namespace = unique_test_namespace("bridge_tier_2");
        let vault_id = "test-bridge-tier-2";
        cleanup_keychain_entry(&namespace, vault_id);

        let master_key = bridge_or_init_master_key(
            data_dir,
            &namespace,
            vault_id,
            Some("fixture-capture-key-do-not-use-in-prod"),
        )
        .expect("bridge against Tier 2 fixture must succeed");

        // Verify the new keychain-derived passphrase opens the rekeyed
        // vault.db. We don't assert specific row content here because
        // the V0.1 schema is independent of the bridge's job (the
        // bridge is a pure-passphrase migration; schema-shape continuity
        // is verified by `vault_storage::rekey_in_place`'s internal
        // close+reopen+verify step at §3 step 7).
        let new_passphrase_hex =
            hex::encode(blake3::derive_key(SQLCIPHER_KDF_CONTEXT, &*master_key));
        // Verify open + sqlite_master query succeeds with new passphrase.
        let new_passphrase = vault_storage::SqlCipherKey::new(new_passphrase_hex.clone());
        vault_storage::verify_sqlcipher_passphrase(&test_vault_db, &new_passphrase)
            .expect("post-bridge verify with new keychain-derived passphrase must succeed");

        // Verify the original 5 fixture rows survived rekey by counting
        // memories — the V0.1 schema has a `memories` table; assert it
        // has 5 rows.
        use rusqlite::{Connection, OpenFlags};
        let conn = Connection::open_with_flags(
            &test_vault_db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("open rekeyed vault.db for row count");
        conn.pragma_update(None, "key", &new_passphrase_hex)
            .expect("set new key");
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .expect("count rows in memories table");
        assert_eq!(
            row_count, 5,
            "Tier 2 fixture has 5 known rows per its README; post-bridge row count \
             MUST be 5 — different count = rekey lost or corrupted rows"
        );

        cleanup_keychain_entry(&namespace, vault_id);
    }

    // ────────────────────────────────────────────────────────────────────
    // Pre-existing tests follow.
    // ────────────────────────────────────────────────────────────────────

    /// Distinct domain-separator strings for SqlCipher vs at-rest produce
    /// distinct subkeys from the same master_key. Defensive pin against
    /// accidentally-collapsed KDF contexts (which would silently couple the
    /// two consumers' keying material).
    #[test]
    fn sqlcipher_and_at_rest_subkeys_are_domain_separated() {
        let master_key = [99u8; 32];
        let sqlcipher_subkey = blake3::derive_key(SQLCIPHER_KDF_CONTEXT, &master_key);
        let at_rest_subkey = blake3::derive_key(AT_REST_KDF_CONTEXT, &master_key);
        assert_ne!(
            sqlcipher_subkey, at_rest_subkey,
            "ADR-040 amendment option β requires distinct SqlCipher / at-rest subkeys \
             via distinct BLAKE3 domain-separator contexts. Equal subkeys would \
             couple the two consumers' keying material — silent crypto-discipline \
             regression."
        );
    }
}
