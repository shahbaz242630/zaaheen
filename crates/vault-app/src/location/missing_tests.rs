//! ADR-105 L1 and L6 against real temporary folders: telling apart why the
//! memories cannot be found, and "Start again in the default place".

use std::cell::Cell;
use std::path::{Path, PathBuf};

use vault_core::{VaultError, VaultResult};

use super::missing::*;
use super::pointer::{self, PendingCleanup, PendingMove, Pointer};
use super::*;
use crate::keychain::lifecycle::MarkerSettled;
use crate::keychain::test_helpers::test_location;

fn homes(tmp: &Path) -> Homes {
    Homes {
        local: tmp.join("Local").join("com.zaaheen.app"),
        roaming: tmp.join("Roaming").join("com.zaaheen.app"),
    }
}

fn put(path: &Path, content: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// A record naming `folder`, which holds the right ID only if `with_id`.
fn recorded(h: &Homes, folder: &Path, with_id: bool) -> String {
    let id = pointer::new_id().unwrap();
    if with_id {
        put(&folder.join(VAULT_ID_FILE), id.as_bytes());
    }
    pointer::write(
        &h.pointer_path(),
        &Pointer::new(folder.to_path_buf(), id.clone()),
    )
    .unwrap();
    id
}

/// The record on disk, byte for byte (`None`: there is none).
fn record_bytes(h: &Homes) -> Option<Vec<u8>> {
    std::fs::read(h.pointer_path()).ok()
}

/// A stand-in for the key half: answers `settled` and counts the calls.
struct Settle {
    answer: fn() -> VaultResult<MarkerSettled>,
    calls: Cell<u32>,
}

impl Settle {
    fn answering(answer: fn() -> VaultResult<MarkerSettled>) -> Self {
        Self {
            answer,
            calls: Cell::new(0),
        }
    }

    fn run(&self) -> VaultResult<MarkerSettled> {
        self.calls.set(self.calls.get() + 1);
        (self.answer)()
    }
}

fn no_marker() -> VaultResult<MarkerSettled> {
    Ok(MarkerSettled::NoMarker)
}

fn again(h: &Homes, settle: &Settle) -> Result<VaultDir, StartAgainRefusal> {
    start_again_under_lock(h, &|| settle.run())
}

/// A person whose memories were on drive E, which is gone.
fn lost_drive(tmp: &Path) -> (Homes, PathBuf, String) {
    let h = homes(tmp);
    let lost = tmp.join("E").join("Zaaheen Memories");
    let id = recorded(&h, &lost, false);
    (h, lost, id)
}

// ── telling the reasons apart (L1) ───────────────────────────────────────

#[test]
fn nothing_recorded_or_a_location_that_resolves_has_no_problem() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    assert_eq!(
        diagnose(&h),
        None,
        "nothing recorded is the setup's to answer"
    );
    let vault = tmp.path().join("V");
    recorded(&h, &vault, true);
    assert_eq!(diagnose(&h), None);
}

#[test]
fn a_record_that_cannot_be_read_is_told_apart() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    put(&h.pointer_path(), b"not json");
    assert_eq!(diagnose(&h), Some(Problem::RecordUnreadable));
}

#[test]
fn a_folder_that_is_not_there_is_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let (h, lost, _) = lost_drive(tmp.path());
    assert_eq!(diagnose(&h), Some(Problem::FolderAbsent(lost.clone())));
    assert!(!lost.exists(), "diagnosing creates nothing");
}

/// A folder that is there may hold the memories: another stick at the same
/// letter, or a folder whose ID file was lost. Never "absent".
#[test]
fn a_folder_that_is_there_but_is_not_this_vault_is_never_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    let there = tmp.path().join("E").join("Zaaheen Memories");
    recorded(&h, &there, false);
    std::fs::create_dir_all(&there).unwrap();
    assert_eq!(diagnose(&h), Some(Problem::FolderUnusable(there.clone())));
    put(
        &there.join(VAULT_ID_FILE),
        pointer::new_id().unwrap().as_bytes(),
    );
    assert_eq!(diagnose(&h), Some(Problem::FolderUnusable(there)));
}

// ── starting again (L6) ──────────────────────────────────────────────────

#[test]
fn starting_again_is_offered_only_when_the_folder_itself_is_not_there() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    let settle = Settle::answering(no_marker);

    // Nothing recorded.
    assert_eq!(
        again(&h, &settle).unwrap_err(),
        StartAgainRefusal::NotOffered
    );
    // A location that resolves.
    let vault = tmp.path().join("V");
    recorded(&h, &vault, true);
    let before = record_bytes(&h);
    assert_eq!(
        again(&h, &settle).unwrap_err(),
        StartAgainRefusal::NotOffered
    );
    assert_eq!(record_bytes(&h), before);
    // A folder that is there without this vault's ID.
    let other = tmp.path().join("E").join("Zaaheen Memories");
    std::fs::create_dir_all(&other).unwrap();
    recorded(&h, &other, false);
    let before = record_bytes(&h);
    assert_eq!(
        again(&h, &settle).unwrap_err(),
        StartAgainRefusal::NotOffered
    );
    assert_eq!(record_bytes(&h), before);
    // A damaged record.
    put(&h.pointer_path(), b"not json");
    assert_eq!(
        again(&h, &settle).unwrap_err(),
        StartAgainRefusal::NotOffered
    );
    assert_eq!(record_bytes(&h), Some(b"not json".to_vec()));

    assert_eq!(settle.calls.get(), 0, "the key is never looked at");
    assert!(!h.new_install_dir().exists(), "nothing is created");
}

#[test]
fn starting_again_records_the_default_folder_with_a_new_id_and_nothing_pending() {
    let tmp = tempfile::tempdir().unwrap();
    let (h, lost, old_id) = lost_drive(tmp.path());
    let mut p = pointer::read(&h.pointer_path()).unwrap().unwrap();
    p.pending_move = Some(PendingMove {
        to: tmp.path().join("F").join("Zaaheen Memories"),
        move_id: pointer::new_id().unwrap(),
        attempts: 1,
    });
    p.pending_cleanup = Some(PendingCleanup {
        from: tmp.path().join("G"),
        move_id: pointer::new_id().unwrap(),
    });
    pointer::write(&h.pointer_path(), &p).unwrap();

    let settle = Settle::answering(no_marker);
    let dir = again(&h, &settle).unwrap();
    assert_eq!(dir.path(), h.new_install_dir());
    assert_eq!(settle.calls.get(), 1);

    let p = pointer::read(&h.pointer_path()).unwrap().unwrap();
    assert_eq!(p.vault_dir, h.new_install_dir());
    assert!(p.pending_move.is_none() && p.pending_cleanup.is_none());
    assert_ne!(p.vault_id, old_id, "a new vault gets a new ID");
    let id = std::fs::read_to_string(h.new_install_dir().join(VAULT_ID_FILE)).unwrap();
    assert_eq!(id.trim(), p.vault_id);
    assert_eq!(resolve(&h).unwrap().path(), h.new_install_dir());
    assert!(!lost.exists(), "the lost folder is never created");
}

/// The next open makes the new vault there: the setup sees a record and
/// only resolves it.
#[test]
fn after_starting_again_the_next_open_finds_the_default_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let (h, _, _) = lost_drive(tmp.path());
    again(&h, &Settle::answering(no_marker)).unwrap();
    let key = test_location("start-again", tmp.path());
    assert_eq!(prepare(&h, &key, &[]).unwrap().path(), h.new_install_dir());
}

/// A default folder holding memories is a vault of its own: never taken
/// over, and nothing about the key is decided.
#[test]
fn a_default_folder_that_holds_memories_is_never_used() {
    let tmp = tempfile::tempdir().unwrap();
    let (h, _, _) = lost_drive(tmp.path());
    for keyed in ["vault.db", "lance/x", "graph.sealed", "reports/r"] {
        let path = h.new_install_dir().join(keyed);
        put(&path, b"someone's memories");
        let before = record_bytes(&h);
        let settle = Settle::answering(no_marker);
        assert_eq!(
            again(&h, &settle).unwrap_err(),
            StartAgainRefusal::DefaultFolderHoldsMemories,
            "{keyed}"
        );
        assert_eq!(settle.calls.get(), 0, "{keyed}: the key is never looked at");
        assert_eq!(record_bytes(&h), before);
        assert_eq!(std::fs::read(&path).unwrap(), b"someone's memories");
        std::fs::remove_dir_all(h.new_install_dir()).unwrap();
    }
}

/// L6 never cleans a folder: files that are not memories stay, and an old
/// ID there is replaced, never reused.
#[test]
fn starting_again_removes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (h, _, _) = lost_drive(tmp.path());
    let default = h.new_install_dir();
    let old_id = pointer::new_id().unwrap();
    put(&default.join(VAULT_ID_FILE), old_id.as_bytes());
    for kept in [".acl-v1", "maintenance.json", ".vault.lock", "notes.txt"] {
        put(&default.join(kept), b"kept");
    }
    let dir = again(&h, &Settle::answering(no_marker)).unwrap();
    assert_ne!(dir.pointer().vault_id, old_id);
    for kept in [".acl-v1", "maintenance.json", ".vault.lock", "notes.txt"] {
        assert_eq!(
            std::fs::read(default.join(kept)).unwrap(),
            b"kept",
            "{kept}"
        );
    }
}

/// Fail closed: when the key cannot be checked, nothing changes.
#[test]
fn when_the_key_cannot_be_checked_nothing_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let (h, _, _) = lost_drive(tmp.path());
    let before = record_bytes(&h);
    let settle = Settle::answering(|| {
        Err(VaultError::KeychainProvenance(
            "the store is failing".into(),
        ))
    });
    assert_eq!(
        again(&h, &settle).unwrap_err(),
        StartAgainRefusal::KeyUnchecked
    );
    assert_eq!(record_bytes(&h), before);
    assert!(!h.new_install_dir().exists());
}

#[test]
fn a_key_kept_or_a_marker_removed_both_let_the_person_start_again() {
    for answer in [
        (|| Ok(MarkerSettled::Removed)) as fn() -> VaultResult<MarkerSettled>,
        || Ok(MarkerSettled::KeptWithKey),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let (h, _, _) = lost_drive(tmp.path());
        let dir = again(&h, &Settle::answering(answer)).unwrap();
        assert_eq!(dir.path(), h.new_install_dir());
    }
}

/// The marker is settled before anything is written, so a crash after it
/// leaves the record naming the lost folder — and "Start again" is offered
/// again at the next start.
#[test]
fn the_key_half_runs_before_the_record_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let (h, lost, _) = lost_drive(tmp.path());
    let seen = Cell::new(false);
    let result = start_again_under_lock(&h, &|| {
        assert_eq!(diagnose(&h), Some(Problem::FolderAbsent(lost.clone())));
        assert!(!h.new_install_dir().join(VAULT_ID_FILE).exists());
        seen.set(true);
        Ok(MarkerSettled::Removed)
    });
    assert!(seen.get());
    assert!(result.is_ok());
}

/// The default folder itself, removed by hand, is made again.
#[test]
fn a_default_folder_removed_by_hand_is_made_again() {
    let tmp = tempfile::tempdir().unwrap();
    let h = homes(tmp.path());
    recorded(&h, &h.new_install_dir(), false);
    assert_eq!(
        diagnose(&h),
        Some(Problem::FolderAbsent(h.new_install_dir()))
    );
    let dir = again(&h, &Settle::answering(no_marker)).unwrap();
    assert_eq!(dir.path(), h.new_install_dir());
    assert!(h.new_install_dir().is_dir());
}

/// The real entry point: under the key lock, and with no marker there is no
/// key store to open (so this runs on every platform CI builds).
#[test]
fn start_again_takes_the_key_lock_and_needs_no_key_store_without_a_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let (h, _, _) = lost_drive(tmp.path());
    let key = test_location("start-again", tmp.path());
    let dir = start_again(&h, &key).unwrap();
    assert_eq!(dir.path(), h.new_install_dir());
}
