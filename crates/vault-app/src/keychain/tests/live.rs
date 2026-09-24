//! ADR-SEC-029 against the real Windows Credential Manager, with throwaway
//! namespaces and temporary folders (never the production key).

use std::time::{Duration, Instant};

use vault_core::{VaultError, VaultKeyFailure};

use crate::keychain::store::{KeyStore, Slot, Stored, WindowsKeyStore};
use crate::keychain::test_helpers::{cleanup_keychain_entry, keychain_test_guard, test_location};
use crate::keychain::{open_master_key, read_existing_master_key, KeyLocation, KeyedPaths};

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
