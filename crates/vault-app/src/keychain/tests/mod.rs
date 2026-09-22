//! Tests for the vault key (ADR-SEC-029, ADR-040 amendment, ADR-041).
//!
//! - `double` — a scripted store and files, for every failure and crash
//!   point;
//! - `lifecycle_tests` — each decision of ADR-SEC-029 in turn;
//! - `crash` — the process dies after every single step;
//! - `files_tests` — the real files: vault data, the marker, erasure's step;
//! - `start_again` — ADR-105 L6's key half: the marker when starting again;
//! - `live` — Windows Credential Manager with throwaway keys.

mod crash;
mod double;
mod files_tests;
mod lifecycle_tests;
#[cfg(windows)]
mod live;
mod start_again;

use super::*;

/// D1: the key module never touches keyring-core's process-global default
/// store — a second user of that global (the account token store runs in
/// the same desktop process) would race it.
#[test]
fn the_process_global_default_store_is_never_used() {
    for (name, source) in [
        ("mod.rs", include_str!("../mod.rs")),
        ("store.rs", include_str!("../store.rs")),
        ("lifecycle.rs", include_str!("../lifecycle.rs")),
        ("files.rs", include_str!("../files.rs")),
        ("bridge.rs", include_str!("../bridge.rs")),
    ] {
        for global in [
            "set_default_store",
            "unset_default_store",
            "get_default_store",
            "Entry::new(",
            "Entry::new_with_modifiers(",
        ] {
            assert!(
                !source.contains(global),
                "{name} uses `{global}`, which goes through the process-global store"
            );
        }
    }
}

/// Nothing outside the key lock may create a key: the only creators are the
/// two locked openers.
#[test]
fn keys_are_created_only_under_the_key_lock() {
    let source = include_str!("../mod.rs").replace("\r\n", "\n");
    for opener in [
        "pub fn open_master_key(",
        "pub fn bridge_or_init_master_key(",
    ] {
        let body = source
            .split_once(opener)
            .map(|(_, rest)| rest.split_once("\n}\n").map_or(rest, |(b, _)| b))
            .unwrap_or_else(|| panic!("{opener} is missing"));
        assert!(
            body.contains("with_lifecycle("),
            "{opener} must run under the key lock"
        );
    }
    assert!(
        include_str!("../mod.rs").contains("fn with_lifecycle")
            && include_str!("../mod.rs")
                .split_once("fn with_lifecycle")
                .is_some_and(|(_, rest)| rest.contains("with_key_lock(")),
        "with_lifecycle must take the key lock"
    );
}

#[test]
fn derive_sqlcipher_passphrase_is_deterministic() {
    let master_key = [42u8; 32];
    let expected = hex::encode(blake3::derive_key(SQLCIPHER_KDF_CONTEXT, &master_key));
    assert_eq!(expected.len(), 64);
    let again = hex::encode(blake3::derive_key(SQLCIPHER_KDF_CONTEXT, &master_key));
    assert_eq!(again, expected);
    drop(derive_sqlcipher_passphrase(&master_key));
}

/// ADR-008 amendment K3: the context string is locked verbatim; drift would
/// make every sealed file on disk unreadable.
#[test]
fn derive_at_rest_key_is_deterministic_and_uses_the_k3_context() {
    let master_key = [7u8; 32];
    let k1 = derive_at_rest_key(&master_key);
    let k2 = derive_at_rest_key(&master_key);
    assert_eq!(k1.as_slice(), k2.as_slice());
    let expected = blake3::derive_key("vault memory at-rest sealing v1", &master_key);
    assert_eq!(
        k1.as_slice(),
        expected.as_slice(),
        "K3 KDF context drift. STOP."
    );
}

#[test]
fn sqlcipher_and_at_rest_subkeys_are_domain_separated() {
    let master_key = [99u8; 32];
    assert_ne!(
        blake3::derive_key(SQLCIPHER_KDF_CONTEXT, &master_key),
        blake3::derive_key(AT_REST_KDF_CONTEXT, &master_key),
        "the two consumers' keying material must never coincide"
    );
}

/// K1's lock is in the never-roaming local data folder, and a busy lock is
/// reported as busy.
#[test]
fn a_held_key_lock_is_reported_as_busy_after_the_wait() {
    let tmp = tempfile::tempdir().unwrap();
    let loc = test_helpers::test_location("busy", tmp.path());
    std::fs::create_dir_all(tmp.path().join("keys")).unwrap();
    let _held =
        crate::ConsolidatorLock::try_acquire_named(&tmp.path().join("keys"), &loc.lock_file_name())
            .unwrap();
    let err = with_key_lock_waiting(&loc, Duration::from_millis(150), || Ok(()))
        .expect_err("the lock is held");
    assert!(
        matches!(err, VaultError::VaultKey(VaultKeyFailure::Busy)),
        "{err}"
    );
}

#[test]
fn the_production_lock_lives_in_the_local_data_folder() {
    let Ok(loc) = KeyLocation::production() else {
        return; // an environment with no local data folder: covered above
    };
    let local = crate::install_paths::local_data_dir().unwrap();
    assert_eq!(loc.lock_dir, local.join(KEY_LOCK_DIRNAME));
    assert_eq!(loc.namespace(), PRODUCTION_NAMESPACE);
    assert_eq!(loc.vault_id(), VAULT_ID);
}
