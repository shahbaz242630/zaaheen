//! ADR-SEC-029 against the real Windows Credential Manager, with throwaway
//! namespaces and temporary folders (never the production key).

use std::path::Path;
use std::time::{Duration, Instant};

use vault_core::{VaultError, VaultKeyFailure};

use crate::keychain::store::{KeyStore, Slot, Stored, WindowsKeyStore};
use crate::keychain::test_helpers::{cleanup_keychain_entry, keychain_test_guard, test_location};
use crate::keychain::{
    bridge_or_init_master_key, open_master_key, read_existing_master_key, KeyLocation, KeyedPaths,
};

/// Removes the test key and its spare even when an assertion fails.
struct Cleanup(KeyLocation);

impl Drop for Cleanup {
    fn drop(&mut self) {
        cleanup_keychain_entry(self.0.namespace(), self.0.vault_id());
    }
}

fn store_for(loc: &KeyLocation) -> WindowsKeyStore {
    WindowsKeyStore::open(loc.namespace(), loc.vault_id()).unwrap()
}

fn stored(store: &WindowsKeyStore, slot: Slot) -> Option<Vec<u8>> {
    match store.read(slot).unwrap() {
        Stored::Absent => None,
        Stored::Present(b) => Some(b.to_vec()),
    }
}

/// Write `bytes` with the library's default (Enterprise) persistence, as
/// every key written before ADR-SEC-029 was.
fn plant_enterprise(loc: &KeyLocation, bytes: &[u8]) {
    let store: std::sync::Arc<keyring_core::CredentialStore> =
        windows_native_keyring_store::Store::new().unwrap();
    let entry = store.build(loc.namespace(), loc.vault_id(), None).unwrap();
    entry.set_secret(bytes).unwrap();
}

#[test]
fn a_new_key_is_stored_local_and_reads_back() {
    let _g = keychain_test_guard();
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_location("live_new_local", tmp.path());
    let _c = Cleanup(loc.clone());
    let key = open_master_key(&loc, &KeyedPaths::in_folder(&tmp.path().join("vault"))).unwrap();
    let store = store_for(&loc);
    assert_eq!(
        store.is_local(Slot::Main).unwrap(),
        Some(true),
        "must not roam"
    );
    assert_eq!(stored(&store, Slot::Main).unwrap(), key.to_vec());
    assert!(stored(&store, Slot::Spare).is_none());
}

#[test]
fn a_real_enterprise_key_moves_to_local_with_identical_bytes() {
    let _g = keychain_test_guard();
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_location("live_move", tmp.path());
    let _c = Cleanup(loc.clone());
    let original = [0x5Au8; 32];
    plant_enterprise(&loc, &original);
    let store = store_for(&loc);
    assert_eq!(
        store.is_local(Slot::Main).unwrap(),
        Some(false),
        "planted as Enterprise"
    );

    let key = open_master_key(&loc, &KeyedPaths::in_folder(&tmp.path().join("vault"))).unwrap();
    assert_eq!(*key, original, "the same key, byte for byte");
    assert_eq!(store.is_local(Slot::Main).unwrap(), Some(true));
    assert_eq!(stored(&store, Slot::Main).unwrap(), original.to_vec());
    assert!(
        stored(&store, Slot::Spare).is_none(),
        "no spare left behind"
    );
}

#[test]
fn erasure_destroys_a_moved_key_and_its_spare() {
    let _g = keychain_test_guard();
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_location("live_erase", tmp.path());
    let _c = Cleanup(loc.clone());
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    open_master_key(&loc, &KeyedPaths::in_folder(&vault)).unwrap();
    let store = store_for(&loc);
    store.write_local(Slot::Spare, &[1u8; 32]).unwrap();
    std::fs::write(vault.join("vault.db"), b"x").unwrap();

    let outcome = crate::erasure::erase_vault(&vault, &loc).unwrap();
    assert!(outcome.key_destroyed);
    assert!(stored(&store, Slot::Main).is_none());
    assert!(stored(&store, Slot::Spare).is_none());
    assert!(!vault.join("vault.db").exists());
}

/// H6 (ADR-SEC-029 Context): an open SQLite database cannot be deleted on
/// Windows, so "Delete everything" from the desktop leaves `vault.db`; the
/// next open must finish the wipe and start clean, never fail for good.
#[test]
fn an_erasure_that_leaves_an_open_database_is_finished_by_the_next_open() {
    let _g = keychain_test_guard();
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_location("live_h6", tmp.path());
    let _c = Cleanup(loc.clone());
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let paths = KeyedPaths::in_folder(&vault);
    let first = open_master_key(&loc, &paths).unwrap();

    let conn = rusqlite::Connection::open(vault.join("vault.db")).unwrap();
    conn.execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1);")
        .unwrap();
    let outcome = crate::erasure::erase_vault(&vault, &loc).unwrap();
    assert!(
        outcome.undeletable.contains(&vault.join("vault.db")),
        "H6: an open database survives erasure on Windows"
    );
    drop(conn);

    let second = open_master_key(&loc, &paths).expect("the next open finishes the wipe");
    assert_ne!(*second, *first, "the erased key never comes back");
    assert!(!vault.join("vault.db").exists(), "the leftover is gone");
    assert!(!tmp
        .path()
        .join("keys")
        .join(format!("{}.default.erased", loc.namespace()))
        .exists());
}

#[test]
fn one_lock_covers_every_vault_folder_of_a_windows_user() {
    let _g = keychain_test_guard();
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_location("live_one_lock", tmp.path());
    let _c = Cleanup(loc.clone());
    // The key exists first, so the timed open below reads it at once: any
    // wait it shows is the lock's, not R1's retries on a fresh install.
    open_master_key(&loc, &KeyedPaths::in_folder(&tmp.path().join("first"))).unwrap();
    let holder_loc = loc.clone();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        crate::keychain::with_key_lock(&holder_loc, || {
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(400));
            Ok(())
        })
        .unwrap();
    });
    held_rx.recv().unwrap();
    let started = Instant::now();
    // A different vault folder: a per-folder lock would not wait.
    open_master_key(&loc, &KeyedPaths::in_folder(&tmp.path().join("another"))).unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "the open must wait for the one key lock"
    );
    holder.join().unwrap();
}

/// C1 through the real files: a marker naming a folder other than erasure's
/// is never acted on — that folder is untouched and no key is made while
/// the marker stands.
#[test]
fn a_marker_naming_another_folder_touches_nothing_and_makes_no_key() {
    let _g = keychain_test_guard();
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_location("live_foreign_marker", tmp.path());
    let _c = Cleanup(loc.clone());
    let vault = tmp.path().join("vault");
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("vault.db"), b"someone else's").unwrap();
    let keys = tmp.path().join("keys");
    std::fs::create_dir_all(&keys).unwrap();
    std::fs::write(
        keys.join(format!("{}.default.erased", loc.namespace())),
        other.to_str().unwrap(),
    )
    .unwrap();

    let err = open_master_key(&loc, &KeyedPaths::in_folder(&vault))
        .expect_err("no key while an untrusted marker stands");
    assert!(
        matches!(
            err,
            VaultError::VaultKey(VaultKeyFailure::FolderUnavailable)
        ),
        "{err}"
    );
    assert!(
        other.join("vault.db").exists(),
        "the other folder is untouched"
    );
    assert!(stored(&store_for(&loc), Slot::Main).is_none());
}

/// E1: erasure takes the same key lock, so it can never run beside an open
/// that is creating, moving or restoring the key.
#[test]
fn erasure_waits_for_the_key_lock() {
    let _g = keychain_test_guard();
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_location("live_erase_lock", tmp.path());
    let _c = Cleanup(loc.clone());
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    open_master_key(&loc, &KeyedPaths::in_folder(&vault)).unwrap();
    let holder_loc = loc.clone();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        crate::keychain::with_key_lock(&holder_loc, || {
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(400));
            Ok(())
        })
        .unwrap();
    });
    held_rx.recv().unwrap();
    let started = Instant::now();
    crate::erasure::erase_vault(&vault, &loc).unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "erasure must wait for the one key lock"
    );
    holder.join().unwrap();
}

#[test]
fn racing_first_launches_agree_on_one_key() {
    let _g = keychain_test_guard();
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_location("live_race", tmp.path());
    let _c = Cleanup(loc.clone());
    let paths = KeyedPaths::in_folder(&tmp.path().join("vault"));
    let keys: Vec<[u8; 32]> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .map(|_| s.spawn(|| *open_master_key(&loc, &paths).unwrap()))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(
        keys.windows(2).all(|p| p[0] == p[1]),
        "one key for every launcher"
    );
}

#[test]
fn the_read_only_accessor_never_creates_and_returns_the_real_key() {
    let _g = keychain_test_guard();
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_location("live_read_only", tmp.path());
    let _c = Cleanup(loc.clone());
    assert!(read_existing_master_key(loc.namespace(), loc.vault_id())
        .unwrap()
        .is_none());
    assert!(
        read_existing_master_key(loc.namespace(), loc.vault_id())
            .unwrap()
            .is_none(),
        "the first call must not have created one"
    );
    let created = open_master_key(&loc, &KeyedPaths::in_folder(&tmp.path().join("v"))).unwrap();
    let read = read_existing_master_key(loc.namespace(), loc.vault_id())
        .unwrap()
        .unwrap();
    assert_eq!(*created, *read);
}

#[test]
fn a_wrong_size_value_is_unusable_and_never_overwritten() {
    let _g = keychain_test_guard();
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_location("live_malformed", tmp.path());
    let _c = Cleanup(loc.clone());
    crate::keychain::test_helpers::plant_malformed_keychain_entry(loc.namespace(), loc.vault_id());
    let err = open_master_key(&loc, &KeyedPaths::in_folder(&tmp.path().join("v")))
        .expect_err("31 bytes is not a key");
    assert!(
        matches!(err, VaultError::VaultKey(VaultKeyFailure::Unusable)),
        "{err}"
    );
    assert_eq!(stored(&store_for(&loc), Slot::Main).unwrap().len(), 31);
}

#[test]
fn keyring_errors_never_carry_the_stored_bytes() {
    let secret = vec![0xABu8; 32];
    for e in [
        keyring_core::Error::BadEncoding(secret.clone()),
        keyring_core::Error::BadDataFormat(
            secret.clone(),
            Box::new(std::io::Error::other("format")),
        ),
    ] {
        let msg = crate::keychain::store::store_error(e).0;
        for leak in ["171", "ab", "AB"] {
            assert!(!msg.contains(leak), "{msg}");
        }
    }
}

// ── ADR-041 V0.1 → V0.2 bridge, through ADR-SEC-029's store ─────────────

const SQLCIPHER_KDF_CONTEXT: &str = "vault sqlcipher passphrase v1";

fn create_v0_1_sqlcipher_fixture(path: &Path, passphrase: &str, n_rows: usize) {
    let conn = rusqlite::Connection::open(path).expect("fixture open");
    conn.pragma_update(None, "key", passphrase)
        .expect("fixture set key");
    conn.execute_batch(
        "CREATE TABLE memories_v0_1 (id INTEGER PRIMARY KEY, content TEXT NOT NULL);",
    )
    .expect("fixture create table");
    for i in 0..n_rows {
        let content = format!("v0_1_row_{i}_padding_{}", "x".repeat(256));
        conn.execute(
            "INSERT INTO memories_v0_1 (id, content) VALUES (?1, ?2)",
            rusqlite::params![i as i64, content],
        )
        .expect("fixture insert");
    }
}

fn read_row_content(path: &Path, passphrase: &str, id: i64) -> Result<String, String> {
    let conn = rusqlite::Connection::open(path).map_err(|e| format!("open: {e}"))?;
    conn.pragma_update(None, "key", passphrase)
        .map_err(|e| format!("set key: {e}"))?;
    conn.query_row(
        "SELECT content FROM memories_v0_1 WHERE id = ?1",
        rusqlite::params![id],
        |row| row.get::<_, String>(0),
    )
    .map_err(|e| format!("read: {e}"))
}

fn bridge_setup(name: &str) -> (tempfile::TempDir, KeyLocation, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_location(name, tmp.path());
    let data_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&data_dir).unwrap();
    (tmp, loc, data_dir)
}

#[test]
fn bridge_rekeys_a_v0_1_database_and_keeps_its_rows() {
    let _g = keychain_test_guard();
    let (_tmp, loc, data_dir) = bridge_setup("bridge_happy");
    let _c = Cleanup(loc.clone());
    let vault_db = data_dir.join("vault.db");
    create_v0_1_sqlcipher_fixture(&vault_db, "v0_1_test_passphrase", 5);

    let key = bridge_or_init_master_key(&loc, &data_dir, Some("v0_1_test_passphrase")).unwrap();
    let store = store_for(&loc);
    assert_eq!(store.is_local(Slot::Main).unwrap(), Some(true));
    let hex_pass = hex::encode(blake3::derive_key(SQLCIPHER_KDF_CONTEXT, key.as_slice()));
    let row = read_row_content(&vault_db, &hex_pass, 0).expect("readable under the new key");
    assert!(row.starts_with("v0_1_row_0_padding_"));
    assert!(!crate::keychain::bridge::snapshot_path_for(&vault_db).exists());
}

#[test]
fn bridge_with_a_wrong_v0_1_passphrase_changes_nothing() {
    let _g = keychain_test_guard();
    let (_tmp, loc, data_dir) = bridge_setup("bridge_wrong");
    let _c = Cleanup(loc.clone());
    let vault_db = data_dir.join("vault.db");
    create_v0_1_sqlcipher_fixture(&vault_db, "v0_1_correct", 3);
    let before = std::fs::read(&vault_db).unwrap();
    let err = bridge_or_init_master_key(&loc, &data_dir, Some("v0_1_WRONG")).unwrap_err();
    assert!(matches!(err, VaultError::KeychainProvenance(_)), "{err}");
    assert!(
        stored(&store_for(&loc), Slot::Main).is_none(),
        "no key written"
    );
    assert_eq!(std::fs::read(&vault_db).unwrap(), before);
}

#[test]
fn a_database_with_no_key_and_no_v0_1_passphrase_is_a_missing_key() {
    let _g = keychain_test_guard();
    let (_tmp, loc, data_dir) = bridge_setup("bridge_missing");
    let _c = Cleanup(loc.clone());
    create_v0_1_sqlcipher_fixture(&data_dir.join("vault.db"), "v0_1_test", 1);
    for v0_1 in [None, Some("")] {
        let err = bridge_or_init_master_key(&loc, &data_dir, v0_1).unwrap_err();
        assert!(
            matches!(err, VaultError::VaultKey(VaultKeyFailure::Missing)),
            "{v0_1:?}: {err}"
        );
        assert!(stored(&store_for(&loc), Slot::Main).is_none());
    }
}

#[test]
fn bridge_returns_an_existing_key_and_leaves_the_database_alone() {
    let _g = keychain_test_guard();
    let (_tmp, loc, data_dir) = bridge_setup("bridge_existing");
    let _c = Cleanup(loc.clone());
    let pre = open_master_key(&loc, &KeyedPaths::in_folder(&data_dir)).unwrap();
    let vault_db = data_dir.join("vault.db");
    create_v0_1_sqlcipher_fixture(&vault_db, "irrelevant", 1);
    let before = std::fs::read(&vault_db).unwrap();
    let key = bridge_or_init_master_key(&loc, &data_dir, Some("irrelevant")).unwrap();
    assert_eq!(*key, *pre);
    assert_eq!(std::fs::read(&vault_db).unwrap(), before);
}

#[test]
fn bridge_on_a_fresh_install_creates_a_local_key() {
    let _g = keychain_test_guard();
    let (_tmp, loc, data_dir) = bridge_setup("bridge_fresh");
    let _c = Cleanup(loc.clone());
    let key = bridge_or_init_master_key(&loc, &data_dir, None).unwrap();
    assert!(key.iter().any(|&b| b != 0));
    assert_eq!(store_for(&loc).is_local(Slot::Main).unwrap(), Some(true));
}

/// ADR-041 §3: the key is written BEFORE the snapshot, so a snapshot failure
/// has a key to roll back — its removal proves the order.
#[test]
fn bridge_writes_the_key_before_the_snapshot_and_rolls_it_back() {
    let _g = keychain_test_guard();
    let (_tmp, loc, data_dir) = bridge_setup("bridge_order");
    let _c = Cleanup(loc.clone());
    let vault_db = data_dir.join("vault.db");
    create_v0_1_sqlcipher_fixture(&vault_db, "v0_1_ordering", 2);
    let blocker = crate::keychain::bridge::snapshot_path_for(&vault_db);
    std::fs::create_dir(&blocker).unwrap();
    assert!(bridge_or_init_master_key(&loc, &data_dir, Some("v0_1_ordering")).is_err());
    assert!(
        stored(&store_for(&loc), Slot::Main).is_none(),
        "rolled back"
    );
    std::fs::remove_dir(&blocker).ok();
}

#[test]
fn bridge_restores_the_snapshot_when_the_rekey_fails() {
    let _g = keychain_test_guard();
    let (_tmp, loc, data_dir) = bridge_setup("bridge_rekey_fail");
    let _c = Cleanup(loc.clone());
    let vault_db = data_dir.join("vault.db");
    let pass = "v0_1_rekey_fail_test";
    create_v0_1_sqlcipher_fixture(&vault_db, pass, 50);
    let size = std::fs::metadata(&vault_db).unwrap().len();
    assert!(size > 8192, "the fixture must span at least two pages");
    let mut bytes = std::fs::read(&vault_db).unwrap();
    bytes[6000] ^= 0xFF;
    std::fs::write(&vault_db, &bytes).unwrap();

    match bridge_or_init_master_key(&loc, &data_dir, Some(pass)) {
        Err(VaultError::Storage(msg)) => assert!(msg.contains("rekey"), "{msg}"),
        other => panic!("expected a rekey failure, got {:?}", other.map(|_| ())),
    }
    assert!(
        stored(&store_for(&loc), Slot::Main).is_none(),
        "key rolled back"
    );
    assert_eq!(std::fs::metadata(&vault_db).unwrap().len(), size);
    assert!(!crate::keychain::bridge::snapshot_path_for(&vault_db).exists());
}

#[test]
fn bridge_against_the_real_v0_1_fixture_keeps_its_five_rows() {
    let _g = keychain_test_guard();
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("vault-storage")
        .join("tests")
        .join("fixtures")
        .join("v0_1_alpha_data_dir")
        .join("vault.db");
    assert!(fixture.exists(), "the captured V0.1 fixture is missing");
    let (_tmp, loc, data_dir) = bridge_setup("bridge_tier_2");
    let _c = Cleanup(loc.clone());
    let vault_db = data_dir.join("vault.db");
    std::fs::copy(&fixture, &vault_db).unwrap();
    let wal = fixture.with_file_name("vault.db-wal");
    if wal.exists() {
        std::fs::copy(&wal, data_dir.join("vault.db-wal")).unwrap();
    }
    let key = bridge_or_init_master_key(
        &loc,
        &data_dir,
        Some("fixture-capture-key-do-not-use-in-prod"),
    )
    .unwrap();
    let hex_pass = hex::encode(blake3::derive_key(SQLCIPHER_KDF_CONTEXT, key.as_slice()));
    let conn = rusqlite::Connection::open_with_flags(
        &vault_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    conn.pragma_update(None, "key", &hex_pass).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 5);
}
