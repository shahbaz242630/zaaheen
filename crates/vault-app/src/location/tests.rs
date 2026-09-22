//! ADR-105 L1/L2 against real temporary folders.

use std::path::{Path, PathBuf};

use vault_core::{VaultError, VaultLocationFailure};

use super::pointer::{self, is_plain_local, Pointer};
use super::*;
use crate::keychain::test_helpers::test_location;

/// Two per-user folders under one temporary directory.
fn homes(tmp: &Path) -> Homes {
    Homes {
        local: tmp.join("Local").join("com.zaaheen.app"),
        roaming: tmp.join("Roaming").join("com.zaaheen.app"),
    }
}

fn key(tmp: &Path) -> crate::keychain::KeyLocation {
    test_location("location", tmp)
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"x").unwrap();
}

fn is(err: &VaultError, kind: VaultLocationFailure) -> bool {
    matches!(err, VaultError::VaultLocation(k) if *k == kind)
}

// ── the resolver (L1) ────────────────────────────────────────────────────

#[test]
fn with_nothing_recorded_the_location_is_unset_and_nothing_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    let err = resolve(&h).unwrap_err();
    assert!(is(&err, VaultLocationFailure::Unset), "{err}");
    assert!(
        !h.local.exists() && !h.roaming.exists(),
        "the resolver creates nothing"
    );
}

#[test]
fn a_recorded_folder_with_its_id_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    let vault = tmp.path().join("E").join("Zaaheen Memories");
    std::fs::create_dir_all(&vault).unwrap();
    let id = pointer::new_id().unwrap();
    std::fs::write(vault.join(VAULT_ID_FILE), &id).unwrap();
    pointer::write(&h.pointer_path(), &Pointer::new(vault.clone(), id)).unwrap();
    assert_eq!(resolve(&h).unwrap().path(), vault);
}

/// The founder's must-have: an unplugged drive, a folder removed, a
/// different stick at the same letter — each is "missing", never a new vault.
#[test]
fn a_missing_folder_or_a_wrong_id_is_missing_and_nothing_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    let vault = tmp.path().join("E").join("Zaaheen Memories");
    let id = pointer::new_id().unwrap();
    pointer::write(&h.pointer_path(), &Pointer::new(vault.clone(), id.clone())).unwrap();

    // Folder not there (the drive is out).
    assert!(is(&resolve(&h).unwrap_err(), VaultLocationFailure::Missing));
    assert!(!vault.exists(), "never created by the resolver");

    // Folder there, no ID (another stick with an empty folder of that name).
    std::fs::create_dir_all(&vault).unwrap();
    assert!(is(&resolve(&h).unwrap_err(), VaultLocationFailure::Missing));

    // Folder there with a different vault's ID.
    std::fs::write(vault.join(VAULT_ID_FILE), pointer::new_id().unwrap()).unwrap();
    assert!(is(&resolve(&h).unwrap_err(), VaultLocationFailure::Missing));
    assert!(!vault.join("vault.db").exists());

    // The right ID: found.
    std::fs::write(vault.join(VAULT_ID_FILE), &id).unwrap();
    assert_eq!(resolve(&h).unwrap().path(), vault);
}

#[test]
fn a_damaged_record_is_missing_never_unset() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    std::fs::create_dir_all(&h.local).unwrap();
    for bad in [
        "not json".to_owned(),
        r#"{"version":2,"vault_dir":"C:\\v","vault_id":"00000000000000000000000000000000"}"#.to_owned(),
        r#"{"version":1,"vault_dir":"relative\\v","vault_id":"00000000000000000000000000000000"}"#
            .to_owned(),
        r#"{"version":1,"vault_dir":"C:\\v","vault_id":"short"}"#.to_owned(),
        r#"{"version":1,"vault_dir":"C:\\v","vault_id":"00000000000000000000000000000000","extra":1}"#
            .to_owned(),
    ] {
        std::fs::write(h.pointer_path(), &bad).unwrap();
        let err = resolve(&h).unwrap_err();
        assert!(
            is(&err, VaultLocationFailure::Missing),
            "{bad}: a damaged record must never read as 'nothing recorded' ({err})"
        );
    }
}

#[test]
fn only_plain_local_paths_are_accepted() {
    if cfg!(windows) {
        assert!(is_plain_local(Path::new(r"C:\Users\me\Zaaheen Memories")));
        assert!(is_plain_local(Path::new(r"\\?\C:\Users\me\v")));
        assert!(!is_plain_local(Path::new(r"\\server\share\v")));
        assert!(!is_plain_local(Path::new(r"\\?\UNC\server\share\v")));
        assert!(!is_plain_local(Path::new(r"\\.\PhysicalDrive0")));
        assert!(!is_plain_local(Path::new(r"C:\Users\..\Windows")));
        assert!(!is_plain_local(Path::new(r"relative\v")));
    } else {
        assert!(is_plain_local(Path::new("/home/me/v")));
        assert!(!is_plain_local(Path::new("/home/../etc")));
        assert!(!is_plain_local(Path::new("relative/v")));
    }
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    let refused = pointer::write(
        &h.pointer_path(),
        &Pointer::new(PathBuf::from("relative"), pointer::new_id().unwrap()),
    );
    assert!(refused.is_err(), "a non-plain path is never recorded");
}

// ── the first-run setup (L2) ─────────────────────────────────────────────

#[test]
fn a_brand_new_install_gets_the_local_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    let dir = prepare(&h, &key(tmp.path()), &[]).unwrap();
    assert_eq!(dir.path(), h.new_install_dir());
    assert!(h.new_install_dir().join(VAULT_ID_FILE).exists());
    assert!(
        !h.roaming.join(VAULT_ID_FILE).exists(),
        "nothing written in Roaming"
    );
    assert_eq!(resolve(&h).unwrap().path(), h.new_install_dir());
}

#[test]
fn an_existing_install_keeps_its_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    touch(&h.roaming.join("vault.db"));
    let dir = prepare(&h, &key(tmp.path()), &[]).unwrap();
    assert_eq!(dir.path(), h.roaming);
    assert!(h.roaming.join(VAULT_ID_FILE).exists());
    assert!(!h.new_install_dir().exists(), "no second vault folder");
}

#[test]
fn an_existing_install_is_found_where_the_desktop_knows_it() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    let app_data = tmp.path().join("TauriAppData");
    touch(&app_data.join("lance").join("x"));
    let dir = prepare(&h, &key(tmp.path()), std::slice::from_ref(&app_data)).unwrap();
    assert_eq!(dir.path(), app_data);
}

#[test]
fn operational_files_alone_are_not_an_existing_install() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    touch(&h.roaming.join(".vault-host.json"));
    touch(&h.roaming.join(".acl-v1"));
    let dir = prepare(&h, &key(tmp.path()), &[]).unwrap();
    assert_eq!(dir.path(), h.new_install_dir());
}

#[test]
fn an_id_already_in_the_folder_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    touch(&h.roaming.join("vault.db"));
    let id = pointer::new_id().unwrap();
    std::fs::write(h.roaming.join(VAULT_ID_FILE), &id).unwrap();
    let dir = prepare(&h, &key(tmp.path()), &[]).unwrap();
    assert_eq!(dir.pointer().vault_id, id);
}

/// Once recorded, the setup never runs again: a missing folder stays
/// missing, never re-adopted and never replaced by a new one.
#[test]
fn a_recorded_location_is_never_redecided() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    let usb = tmp.path().join("E").join("Zaaheen Memories");
    pointer::write(
        &h.pointer_path(),
        &Pointer::new(usb, pointer::new_id().unwrap()),
    )
    .unwrap();
    touch(&h.roaming.join("vault.db"));
    let err = prepare(&h, &key(tmp.path()), &[]).unwrap_err();
    assert!(is(&err, VaultLocationFailure::Missing), "{err}");
    assert!(!h.new_install_dir().exists());
    assert!(!h.roaming.join(VAULT_ID_FILE).exists());
}

/// R1-3: the desktop and a keeper starting together agree on one folder.
#[test]
fn racing_first_runs_agree_on_one_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    let k = key(tmp.path());
    let dirs: Vec<PathBuf> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .map(|_| s.spawn(|| prepare(&h, &k, &[]).unwrap().path().to_path_buf()))
            .collect();
        handles.into_iter().map(|t| t.join().unwrap()).collect()
    });
    assert!(dirs.windows(2).all(|p| p[0] == p[1]));
    let ids: Vec<_> = std::fs::read_dir(&h.local)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().join(VAULT_ID_FILE).exists())
        .collect();
    assert_eq!(ids.len(), 1, "one vault folder");
}

#[test]
fn the_models_never_follow_the_vault() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    prepare(&h, &key(tmp.path()), &[]).unwrap();
    assert_eq!(models_dir(&h), h.roaming.join("models"));
}

// ── showing a folder (L-e) ───────────────────────────────────────────────

/// The record keeps `canonicalize`'s form; the person reads `D:\…`.
#[test]
fn a_folder_is_shown_without_the_prefix_the_record_keeps() {
    assert_eq!(
        display_path(Path::new(r"\\?\D:\Backups\Zaaheen Memories")),
        r"D:\Backups\Zaaheen Memories"
    );
    assert_eq!(display_path(Path::new(r"\\?\e:\x")), r"e:\x");
    assert_eq!(
        display_path(Path::new(r"C:\Users\me\vault")),
        r"C:\Users\me\vault"
    );
    // Never recorded, and never dressed up as a local folder if one were.
    assert_eq!(
        display_path(Path::new(r"\\?\UNC\server\share\v")),
        r"\\?\UNC\server\share\v"
    );
    assert_eq!(display_path(Path::new(r"\\?\")), r"\\?\");
    assert_eq!(display_path(Path::new("/home/me/v")), "/home/me/v");
}
