//! The vault's master key: where it is kept, how it is opened, moved,
//! erased, and derived into the at-rest subkeys.
//!
//! **Contract: ADR-SEC-029 in `VAULT-KEY-AND-LOCATION.md`** (with ADR-040,
//! its amendment, and ADR-041 for the derivation tree and the V0.1 bridge).
//! Quote it; the short version:
//! - the key lives in the OS credential store with **Local** persistence, so
//!   it never roams to another computer (D2), through this module's own store
//!   instance, never keyring-core's process-global default (D1);
//! - every operation that writes or deletes it holds **one key lock per
//!   Windows user** (K1), in the never-roaming local data folder;
//! - "no key" means the store's `NoEntry`, believed after five reads (D3,
//!   R1), and **no key is created while data sealed under a key exists**
//!   (D4) or while an erasure is unfinished (E0/C1);
//! - an existing Enterprise key moves to Local through a verified spare copy
//!   (D5), and an interrupted move is repaired at the next open (C2);
//! - erasure deletes the spare, then the key, confirms both gone, and only
//!   then writes the marker that lets the next open finish removing any
//!   files it could not (E2, E0, E3, C1).
//!
//! ## Public surface
//!
//! - [`KeyLocation`] — which key, and where its lock and marker live.
//! - [`KeyedPaths`] — the storage paths a process opens under the key.
//! - [`open_master_key`] — the CLI, keeper and maintenance opener.
//! - [`bridge_or_init_master_key`] — the desktop's opener (ADR-041 bridge).
//! - [`read_existing_master_key`] — read-only, never creates (ADR-SEC-019).
//! - [`derive_sqlcipher_passphrase`], [`derive_at_rest_key`] — the ADR-040
//!   amendment option β derivation tree:
//!
//! ```text
//! master_key             ← 32 bytes from the credential store
//! at_rest_key            = blake3::derive_key("vault memory at-rest sealing v1", &master_key)
//! sqlcipher_passphrase   = hex(blake3::derive_key("vault sqlcipher passphrase v1", &master_key))
//! ```
//!
//! V0.2's key store is Windows Credential Manager; elsewhere every call
//! fails with [`VaultError::KeychainProvenance`] (erasure included — it must
//! never report a success that did not happen).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tracing::warn;
use vault_core::{VaultError, VaultKeyFailure, VaultResult};
use zeroize::Zeroizing;

mod bridge;
mod files;
pub(crate) mod lifecycle;
mod store;

#[cfg(test)]
mod tests;

pub(crate) use lifecycle::Erased;

/// The 32-byte master key, wiped from memory when dropped (BRD §11.5.3).
pub type MasterKey = Zeroizing<[u8; 32]>;

/// Reverse-DNS namespace for production keychain entries — locked at ADR-040.
///
/// V1.0 multi-vault forward-compat: the same namespace holds one entry per
/// vault, told apart by the account-string slot ([`VAULT_ID`]).
pub const PRODUCTION_NAMESPACE: &str = "com.zaaheen.v0.2";

/// V0.2 single-vault account string, shared by every process.
pub const VAULT_ID: &str = "default";

/// BLAKE3 derive_key context for the SqlCipher passphrase subkey
/// (ADR-040 amendment option β). The trailing `v1` allows a later rotation.
const SQLCIPHER_KDF_CONTEXT: &str = "vault sqlcipher passphrase v1";

/// BLAKE3 derive_key context for the at-rest sealing subkey (ADR-008
/// amendment K3 KDF).
const AT_REST_KDF_CONTEXT: &str = "vault memory at-rest sealing v1";

/// File name, under the vault root, of the per-folder key-creation lock
/// that builds before ADR-SEC-029 held. No longer taken (K1 replaced it);
/// still declared in `erasure::VAULT_LOCK_FILES` because existing vaults
/// have the file.
pub const KEY_INIT_LOCKFILE_NAME: &str = ".keyinit.lock";

/// Folder, under the local data folder, holding the key locks and markers.
pub const KEY_LOCK_DIRNAME: &str = "keys";

/// How long to wait for another process holding the key lock (K1).
const KEY_LOCK_WAIT: Duration = Duration::from_secs(10);

/// Which key, where its lock and erasure marker live, and which folder
/// erasure runs on (the only folder a marker may name).
#[derive(Clone, Debug)]
pub struct KeyLocation {
    namespace: String,
    vault_id: String,
    lock_dir: PathBuf,
    erase_root: EraseRoot,
}

/// Where erasure runs, so which folder a marker may name.
#[derive(Clone, Debug)]
enum EraseRoot {
    /// A fixed folder: tests only (production always resolves the recorded
    /// location).
    #[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
    Fixed(PathBuf),
    /// The recorded vault folder (ADR-105), resolved under the key lock each
    /// time it is needed — after the first-run setup has recorded it. When it
    /// cannot be resolved, there is no erasure folder and every marker is
    /// untrusted (ADR-SEC-029 amendment 5).
    Recorded(crate::location::Homes),
}

impl KeyLocation {
    /// The installed app's key: [`PRODUCTION_NAMESPACE`] / [`VAULT_ID`], its
    /// lock in `<local data folder>\keys` (never roaming), and erasure's
    /// folder the recorded vault folder (ADR-105).
    ///
    /// # Errors
    ///
    /// [`VaultKeyFailure::FolderUnavailable`] when the environment names no
    /// local data folder or data folder — never a fall-back to the vault
    /// folder, which would split the one lock (K1).
    pub fn production() -> VaultResult<Self> {
        match crate::location::Homes::production() {
            Ok(homes) => Ok(Self {
                namespace: PRODUCTION_NAMESPACE.to_owned(),
                vault_id: VAULT_ID.to_owned(),
                lock_dir: homes.local.join(KEY_LOCK_DIRNAME),
                erase_root: EraseRoot::Recorded(homes),
            }),
            Err(_) => {
                warn!(target: "vault_app::keychain", "no local data folder is named by the environment");
                Err(VaultError::VaultKey(VaultKeyFailure::FolderUnavailable))
            }
        }
    }

    /// An explicit location, for tests. Crate-private, so production code
    /// outside this crate can only use [`KeyLocation::production`] (K1: one
    /// way to find the lock); other crates' tests use
    /// `test_helpers::test_location`.
    #[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
    pub(crate) fn new(
        namespace: impl Into<String>,
        vault_id: impl Into<String>,
        lock_dir: impl Into<PathBuf>,
        erase_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            vault_id: vault_id.into(),
            lock_dir: lock_dir.into(),
            erase_root: EraseRoot::Fixed(erase_root.into()),
        }
    }

    /// The erasure folder, resolved now (call under K1).
    fn erase_root(&self) -> Option<PathBuf> {
        match &self.erase_root {
            EraseRoot::Fixed(p) => Some(p.clone()),
            EraseRoot::Recorded(homes) => crate::location::resolve(homes)
                .ok()
                .map(|dir| dir.path().to_path_buf()),
        }
    }

    /// The credential-store namespace.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The account string.
    pub fn vault_id(&self) -> &str {
        &self.vault_id
    }

    fn lock_file_name(&self) -> String {
        format!("{}.{}.lock", self.namespace, self.vault_id)
    }

    /// The "erased" marker (E0). Also read by a move, which never runs
    /// beside one (ADR-105 L5).
    pub(crate) fn marker_path(&self) -> PathBuf {
        self.lock_dir
            .join(format!("{}.{}.erased", self.namespace, self.vault_id))
    }
}

/// The storage paths a process opens under the key — D4's evidence that a
/// vault already exists. Only data written after a key exists counts;
/// operational files (`.acl-v1`, `.vault-host.json`, `maintenance.json`,
/// lockfiles, `models/`) never do.
#[derive(Clone, Debug)]
pub struct KeyedPaths {
    vault_db: PathBuf,
    vector_dir: PathBuf,
    graph_db: PathBuf,
}

impl KeyedPaths {
    /// The paths this process actually opens (`--vault-db` and friends).
    pub fn new(
        vault_db: impl Into<PathBuf>,
        vector_dir: impl Into<PathBuf>,
        graph_db: impl Into<PathBuf>,
    ) -> Self {
        Self {
            vault_db: vault_db.into(),
            vector_dir: vector_dir.into(),
            graph_db: graph_db.into(),
        }
    }

    /// The default layout inside `data_dir`.
    pub fn in_folder(data_dir: &Path) -> Self {
        Self::new(
            crate::install_paths::vault_db_in(data_dir),
            crate::install_paths::vector_dir_in(data_dir),
            crate::install_paths::graph_db_in(data_dir),
        )
    }

    /// The vault root: the database's folder.
    pub(crate) fn vault_root(&self) -> PathBuf {
        self.vault_db
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }

    /// Every path whose existence means data sealed under a key exists.
    pub(crate) fn keyed_entries(&self) -> Vec<PathBuf> {
        let with_suffix = |suffix: &str| {
            let mut s = self.vault_db.as_os_str().to_owned();
            s.push(suffix);
            PathBuf::from(s)
        };
        vec![
            self.vault_db.clone(),
            with_suffix("-wal"),
            with_suffix("-shm"),
            self.vector_dir.clone(),
            self.graph_db.clone(),
            self.graph_db.with_extension("sealed"),
            self.vault_root().join(vault_storage::REPORTS_DIRNAME),
        ]
    }
}

/// K1: run `f` holding the one key lock for this Windows user.
pub(crate) fn with_key_lock<T>(
    loc: &KeyLocation,
    f: impl FnOnce() -> VaultResult<T>,
) -> VaultResult<T> {
    with_key_lock_waiting(loc, KEY_LOCK_WAIT, f)
}

/// [`with_key_lock`] with an explicit wait (the tests' short one).
fn with_key_lock_waiting<T>(
    loc: &KeyLocation,
    wait: Duration,
    f: impl FnOnce() -> VaultResult<T>,
) -> VaultResult<T> {
    std::fs::create_dir_all(&loc.lock_dir).map_err(|e| {
        warn!(target: "vault_app::keychain", error = %e, "could not create the key lock folder");
        VaultError::VaultKey(VaultKeyFailure::FolderUnavailable)
    })?;
    let name = loc.lock_file_name();
    let deadline = Instant::now() + wait;
    let _guard = loop {
        match crate::ConsolidatorLock::try_acquire_named(&loc.lock_dir, &name) {
            Ok(guard) => break guard,
            Err(VaultError::ConsolidatorBusy(_)) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(VaultError::ConsolidatorBusy(_)) => {
                warn!(target: "vault_app::keychain", "another process held the key lock past the wait");
                return Err(VaultError::VaultKey(VaultKeyFailure::Busy));
            }
            Err(e) => {
                warn!(target: "vault_app::keychain", error = %e, "could not take the key lock");
                return Err(VaultError::VaultKey(VaultKeyFailure::FolderUnavailable));
            }
        }
    };
    f()
}

/// Run `f` with the production store and the files for `paths`, under K1.
fn with_lifecycle<T>(
    loc: &KeyLocation,
    paths: &KeyedPaths,
    f: impl FnOnce(&lifecycle::Ctx<'_>) -> VaultResult<T>,
) -> VaultResult<T> {
    with_key_lock(loc, || {
        let store = store::platform_store(&loc.namespace, &loc.vault_id)?;
        // Resolved here, under K1, after any first-run setup has recorded the
        // location (ADR-105 L3).
        let files = files::DiskFiles::new(paths, loc.marker_path(), loc.erase_root());
        let ctx = lifecycle::Ctx {
            store: &*store,
            files: &files,
            policy: lifecycle::Policy::PRODUCTION,
        };
        f(&ctx)
    })
}

/// Open the master key, or create one for a fresh install — the CLI's,
/// keeper's and maintenance runner's opener.
///
/// # Errors
///
/// [`VaultError::VaultKey`] for a missing key over existing data, a folder
/// that cannot be checked, an unusable stored value, or a lock held too
/// long; [`VaultError::KeychainProvenance`] when the credential store fails.
pub fn open_master_key(loc: &KeyLocation, paths: &KeyedPaths) -> VaultResult<Zeroizing<[u8; 32]>> {
    with_lifecycle(loc, paths, lifecycle::open_or_create)
}

/// The desktop's opener: [`open_master_key`] plus the V0.1 → V0.2 bridge
/// (ADR-041) for a V0.1 `vault.db` with `VAULT_KEY` set. `data_dir` is the
/// folder holding the default layout.
///
/// # Errors
///
/// As [`open_master_key`]; a V0.1 bridge failure is
/// [`VaultError::KeychainProvenance`] or [`VaultError::Storage`], rolled
/// back.
pub fn bridge_or_init_master_key(
    loc: &KeyLocation,
    data_dir: &Path,
    v0_1_vault_key: Option<&str>,
) -> VaultResult<Zeroizing<[u8; 32]>> {
    let paths = KeyedPaths::in_folder(data_dir);
    with_lifecycle(loc, &paths, |ctx| {
        bridge::bridge_or_init(ctx, data_dir, v0_1_vault_key)
    })
}

/// Read the master key WITHOUT ever creating, moving or repairing it
/// (ADR-SEC-019): for relays and the locked keeper. Takes no lock. `Ok(None)`
/// means there is no key (one read; callers that wait, poll).
///
/// # Errors
///
/// [`VaultError::KeychainProvenance`] when the store fails;
/// [`VaultKeyFailure::Unusable`] for a stored value that is not 32 bytes.
pub fn read_existing_master_key(
    namespace: &str,
    vault_id: &str,
) -> VaultResult<Option<Zeroizing<[u8; 32]>>> {
    let store = store::platform_store(namespace, vault_id)?;
    lifecycle::read_main(&*store)
}

/// ADR-105 L6's key half, for `location::missing::start_again`; **the
/// caller holds K1**. With no marker there is nothing to settle and no store
/// to open.
pub(crate) fn settle_marker_to_start_again(
    loc: &KeyLocation,
) -> VaultResult<lifecycle::MarkerSettled> {
    match loc.marker_path().try_exists() {
        Ok(false) => return Ok(lifecycle::MarkerSettled::NoMarker),
        Ok(true) => {}
        Err(e) => {
            warn!(target: "vault_app::keychain", error = %e, "could not check for an erasure marker");
            return Err(VaultError::VaultKey(VaultKeyFailure::FolderUnavailable));
        }
    }
    let store = store::platform_store(&loc.namespace, &loc.vault_id)?;
    // Only the marker is read or removed here; the keyed paths are never
    // looked at, and with no erasure folder every marker counts as present.
    let paths = KeyedPaths::in_folder(&loc.lock_dir);
    let files = files::DiskFiles::new(&paths, loc.marker_path(), None);
    lifecycle::settle_marker_to_start_again(&lifecycle::Ctx {
        store: &*store,
        files: &files,
        policy: lifecycle::Policy::PRODUCTION,
    })
}

/// Erasure's key half and file step under one K1 hold (E1): used by
/// `erasure::erase_vault`.
pub(crate) fn erase_under_key_lock(loc: &KeyLocation, vault_dir: &Path) -> VaultResult<Erased> {
    let paths = KeyedPaths::in_folder(vault_dir);
    with_lifecycle(loc, &paths, |ctx| lifecycle::erase(ctx, vault_dir))
}

/// Derive the SqlCipher passphrase from the master_key per ADR-040
/// amendment option β: BLAKE3 derive_key with the locked
/// [`SQLCIPHER_KDF_CONTEXT`], hex-encoded to 64 characters for
/// [`vault_storage::SqlCipherKey::new`].
pub fn derive_sqlcipher_passphrase(master_key: &[u8; 32]) -> vault_storage::SqlCipherKey {
    let subkey = blake3::derive_key(SQLCIPHER_KDF_CONTEXT, master_key);
    vault_storage::SqlCipherKey::new(hex::encode(subkey))
}

/// Derive the at-rest sealing key from the master_key per ADR-008 amendment
/// K3 KDF: BLAKE3 derive_key with the locked [`AT_REST_KDF_CONTEXT`].
pub fn derive_at_rest_key(master_key: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let subkey = blake3::derive_key(AT_REST_KDF_CONTEXT, master_key);
    Zeroizing::new(subkey)
}

/// Test infrastructure for this crate's tests and, behind the
/// `test-helpers` feature, other crates' tests (`cfg(test)` does not cross
/// crate boundaries).
#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers {
    use std::path::Path;

    use super::KeyLocation;

    /// A unique-per-test namespace, prefixed `com.zaaheen.test.v0.2.` so a
    /// stale entry from a panicked test is recognisable in Credential
    /// Manager.
    pub fn unique_test_namespace(test_name: &str) -> String {
        let mut nonce = [0u8; 8];
        getrandom::getrandom(&mut nonce).expect("getrandom for test namespace");
        format!("com.zaaheen.test.v0.2.{}.{}", test_name, hex::encode(nonce))
    }

    /// A throwaway key location: a unique namespace, its lock and marker in
    /// `dir/keys`, and `dir/vault` as erasure's folder.
    pub fn test_location(test_name: &str, dir: &Path) -> KeyLocation {
        KeyLocation::new(
            unique_test_namespace(test_name),
            "default",
            dir.join("keys"),
            dir.join("vault"),
        )
    }

    /// Best-effort removal of a test key and its spare, through this
    /// module's own store (never the process-global default, D1).
    #[cfg(windows)]
    pub fn cleanup_keychain_entry(namespace: &str, vault_id: &str) {
        if let Ok(store) = super::store::WindowsKeyStore::open(namespace, vault_id) {
            use super::store::{KeyStore, Slot};
            let _ = store.delete(Slot::Spare);
            let _ = store.delete(Slot::Main);
        }
    }

    /// Plant a 31-byte value as the key, so an open fails as unusable.
    #[cfg(windows)]
    pub fn plant_malformed_keychain_entry(namespace: &str, vault_id: &str) {
        let store: std::sync::Arc<keyring_core::CredentialStore> =
            windows_native_keyring_store::Store::new().expect("open the credential store");
        let modifiers = std::collections::HashMap::from([("persistence", "Local")]);
        let entry = store
            .build(namespace, vault_id, Some(&modifiers))
            .expect("build the entry");
        entry.set_secret(&[0u8; 31]).expect("plant a 31-byte value");
    }

    /// Serialises tests that touch Windows Credential Manager, so a run
    /// under `RUST_TEST_THREADS=4` does not load it with parallel writes (the
    /// library warns that operations on one entry from several threads are
    /// not reliably ordered). Recovers from a poisoned mutex.
    pub fn keychain_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static KEYCHAIN_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
        KEYCHAIN_TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
