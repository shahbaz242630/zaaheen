//! The real files side of ADR-SEC-029: which paths count as vault data (D4),
//! the marker's validation (C1), and erasure's bounded file step.

use std::path::{Path, PathBuf};

use crate::keychain::files::{validate_marker, DiskFiles, Marker, VaultFiles};
use crate::keychain::KeyedPaths;

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, b"x").unwrap();
}

#[test]
fn keyed_entries_are_the_sealed_data_and_never_the_operational_files() {
    let root = PathBuf::from("vault-root");
    let entries = KeyedPaths::in_folder(&root).keyed_entries();
    for name in [
        "vault.db",
        "vault.db-wal",
        "vault.db-shm",
        "lance",
        "graph.duckdb",
        "graph.sealed",
        "reports",
    ] {
        assert!(entries.contains(&root.join(name)), "{name} is sealed data");
    }
    for name in [
        ".acl-v1",
        ".vault-host.json",
        "maintenance.json",
        ".keeper",
        "models",
    ] {
        assert!(
            !entries.contains(&root.join(name)),
            "{name} is written before any key exists; it must not count"
        );
    }
    // Every keyed name in the default layout is also one erasure removes,
    // so an erasure can always clear the evidence D4 looks for.
    for path in &entries {
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(
            crate::erasure::VAULT_ENTRIES.contains(&name),
            "{name} counts as vault data but erasure would never remove it"
        );
    }
}

#[test]
fn a_folder_with_only_operational_files_holds_no_vault_data() {
    let tmp = tempfile::tempdir().unwrap();
    for name in [".acl-v1", ".vault-host.json", "maintenance.json"] {
        touch(&tmp.path().join(name));
    }
    std::fs::create_dir_all(tmp.path().join("models")).unwrap();
    let keyed = KeyedPaths::in_folder(tmp.path());
    let files = DiskFiles::new(&keyed, tmp.path().join("m"), Some(tmp.path().to_path_buf()));
    assert!(!files.keyed_data_present().unwrap());
    touch(&tmp.path().join("vault.db-wal"));
    assert!(
        files.keyed_data_present().unwrap(),
        "a WAL alone is vault data"
    );
}

#[test]
fn vault_data_is_looked_for_at_the_paths_the_process_actually_opens() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(elsewhere.join("vectors")).unwrap();
    // `--vault-db root/vault.db --vector-dir elsewhere/vectors`: the vectors
    // alone are vault data, though the root is empty.
    let keyed = KeyedPaths::new(
        root.join("vault.db"),
        elsewhere.join("vectors"),
        root.join("graph.duckdb"),
    );
    let files = DiskFiles::new(&keyed, tmp.path().join("m"), Some(root.clone()));
    assert!(files.keyed_data_present().unwrap());
}

#[test]
fn a_marker_is_trusted_only_for_the_folder_erasure_runs_on() {
    let tmp = tempfile::tempdir().unwrap();
    let erase_root = tmp.path().join("vault");
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&erase_root).unwrap();
    std::fs::create_dir_all(&other).unwrap();

    assert_eq!(
        validate_marker(&erase_root, &erase_root).unwrap(),
        Marker::Valid(erase_root.clone())
    );
    // The same folder written differently is the same folder (a trailing
    // `.` is not even a component to `Path::components`).
    let dotted = erase_root.join(".");
    assert!(matches!(
        validate_marker(&dotted, &erase_root).unwrap(),
        Marker::Valid(_)
    ));
    assert_eq!(
        validate_marker(&other, &erase_root).unwrap(),
        Marker::Invalid,
        "another existing folder is never cleaned"
    );
    // A `..` is refused outright, even when the path resolves to the
    // erasure folder itself: erasure never writes one (the plain-path check
    // must hold on its own, not only through the directory comparison).
    let roundabout = erase_root.join("..").join("vault");
    assert_eq!(
        validate_marker(&roundabout, &erase_root).unwrap(),
        Marker::Invalid
    );
    let escape = erase_root.join("..").join("other");
    assert_eq!(
        validate_marker(&escape, &erase_root).unwrap(),
        Marker::Invalid
    );
    assert_eq!(
        validate_marker(Path::new("relative/vault"), &erase_root).unwrap(),
        Marker::Invalid
    );
    // Amendment 4: a missing folder that is not the erasure folder (a drive
    // that is not plugged in) is not trusted...
    let gone = tmp.path().join("gone");
    assert_eq!(
        validate_marker(&gone, &erase_root).unwrap(),
        Marker::Invalid
    );
    // ...but the erasure folder itself, gone entirely, is: nothing is left
    // to clean.
    let removed_root = tmp.path().join("removed-root");
    assert_eq!(
        validate_marker(&removed_root, &removed_root).unwrap(),
        Marker::Valid(removed_root.clone())
    );
}

/// The marker FILE goes through the same checks: a marker naming another
/// existing folder, or a relative path, reads as untrusted.
#[test]
fn a_marker_file_naming_another_folder_or_a_relative_path_is_untrusted() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let keyed = KeyedPaths::in_folder(&vault);
    let marker = tmp.path().join("keys").join("ns.default.erased");
    let files = DiskFiles::new(&keyed, marker.clone(), Some(vault.clone()));
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();

    std::fs::write(&marker, other.to_str().unwrap()).unwrap();
    assert_eq!(files.marker().unwrap(), Marker::Invalid);
    std::fs::write(&marker, "relative/vault").unwrap();
    assert_eq!(files.marker().unwrap(), Marker::Invalid);
    std::fs::write(&marker, vault.to_str().unwrap()).unwrap();
    assert_eq!(files.marker().unwrap(), Marker::Valid(vault.clone()));
}

/// ADR-SEC-029 amendment 5: with no erasure folder resolvable (the vault's
/// location cannot be found), every marker is untrusted.
#[test]
fn with_no_erasure_folder_every_marker_is_untrusted() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let keyed = KeyedPaths::in_folder(&vault);
    let marker = tmp.path().join("keys").join("ns.default.erased");
    let files = DiskFiles::new(&keyed, marker.clone(), None);
    assert_eq!(
        files.marker().unwrap(),
        Marker::None,
        "no marker is no marker"
    );
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, vault.to_str().unwrap()).unwrap();
    assert_eq!(files.marker().unwrap(), Marker::Invalid);
}

#[test]
fn the_marker_round_trips_and_its_removal_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let keyed = KeyedPaths::in_folder(&vault);
    let marker = tmp.path().join("keys").join("ns.default.erased");
    let files = DiskFiles::new(&keyed, marker.clone(), Some(vault.clone()));
    assert_eq!(files.marker().unwrap(), Marker::None);
    files.write_marker(&vault).unwrap();
    assert_eq!(files.marker().unwrap(), Marker::Valid(vault.clone()));
    files.remove_marker().unwrap();
    files.remove_marker().unwrap();
    assert_eq!(files.marker().unwrap(), Marker::None);

    std::fs::write(&marker, [0xFF, 0xFE, 0x00]).unwrap();
    assert_eq!(
        files.marker().unwrap(),
        Marker::Invalid,
        "not text: not trusted"
    );
}

#[test]
fn the_own_folder_is_recognised_by_what_it_is_not_how_it_is_written() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let keyed = KeyedPaths::in_folder(&vault);
    let files = DiskFiles::new(&keyed, tmp.path().join("m"), Some(vault.clone()));
    assert!(files.is_own_folder(&vault.join(".")));
    assert!(!files.is_own_folder(tmp.path()));
}

/// C1 cleans the folder the marker names, never the folder of whichever
/// process happens to finish the erasure.
#[test]
fn cleaning_touches_the_folder_it_is_given_never_the_own_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let erased = tmp.path().join("erased");
    let own = tmp.path().join("own");
    touch(&erased.join("vault.db"));
    touch(&own.join("vault.db"));
    let keyed = KeyedPaths::in_folder(&own);
    let files = DiskFiles::new(&keyed, tmp.path().join("m"), Some(erased.clone()));
    let report = files.clean(&erased);
    assert_eq!(report.removed, 1);
    assert!(!erased.join("vault.db").exists());
    assert!(
        own.join("vault.db").exists(),
        "another folder's vault is untouched"
    );
}

#[test]
fn erasures_file_step_removes_only_the_vaults_own_names() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path();
    touch(&vault.join("vault.db"));
    touch(&vault.join("lance").join("data.lance"));
    touch(&vault.join(".vault-host.json"));
    touch(&vault.join("models").join("model.onnx"));
    touch(&vault.join("my-own-notes.txt"));
    let (removed, left) = crate::erasure::remove_vault_entries(vault);
    assert_eq!(removed, 3);
    assert!(left.is_empty());
    assert!(
        vault.join("models").join("model.onnx").exists(),
        "models survive"
    );
    assert!(
        vault.join("my-own-notes.txt").exists(),
        "nothing outside the list"
    );
}
