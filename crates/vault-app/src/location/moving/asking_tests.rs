//! What the screens ask about a move (ADR-105 L-e), against real temporary
//! folders: a folder checked before the person confirms, where things
//! stand, and letting go of an old copy on a drive that never comes back
//! (L-d decision 9).

use std::path::Path;

use super::tests::{record, requested, snapshot, world, TestEnv, World};
use super::*;
use crate::location::check::Refusal;
use crate::location::pointer::{self, PendingCleanup, PendingMove};
use crate::location::Homes;

/// An earlier move's old copy, waiting to be removed from `from`.
fn with_old_copy(w: &World, from: &Path) {
    let mut p = record(w);
    p.pending_cleanup = Some(PendingCleanup {
        from: from.to_path_buf(),
        move_id: pointer::new_id().unwrap(),
    });
    pointer::write(&w.homes.pointer_path(), &p).unwrap();
}

fn record_bytes(w: &World) -> Vec<u8> {
    std::fs::read(w.homes.pointer_path()).unwrap()
}

// ── checking a folder before the person confirms ─────────────────────────

#[test]
fn checking_a_folder_says_where_the_memories_would_go_and_records_nothing() {
    let w = world();
    let before = record_bytes(&w);
    let checked = check_request(&w.homes, &w.key, &w.chosen, &TestEnv).unwrap();
    assert_eq!(
        checked.target,
        std::fs::canonicalize(&w.chosen)
            .unwrap()
            .join("Zaaheen Memories")
    );
    assert_eq!(record_bytes(&w), before, "nothing is recorded");
    assert!(!checked.target.exists(), "nothing is created");
    // Asking for it afterwards records exactly that folder.
    assert_eq!(requested(&w), checked.target);
}

#[test]
fn checking_refuses_whatever_asking_refuses() {
    let w = world();
    assert_eq!(
        check_request(&w.homes, &w.key, &w.source.join("reports"), &TestEnv),
        Err(RequestRefusal::Folder(Refusal::InsideAppFolders))
    );
    requested(&w);
    assert_eq!(
        check_request(&w.homes, &w.key, &w.chosen, &TestEnv),
        Err(RequestRefusal::MoveWaiting)
    );

    let w = world();
    with_old_copy(&w, &w.tmp.path().join("Old"));
    assert_eq!(
        check_request(&w.homes, &w.key, &w.chosen, &TestEnv),
        Err(RequestRefusal::OldCopyWaiting)
    );

    let w = world();
    let marker = w.key.marker_path();
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, b"C:\\somewhere").unwrap();
    let before = record_bytes(&w);
    assert_eq!(
        check_request(&w.homes, &w.key, &w.chosen, &TestEnv),
        Err(RequestRefusal::ErasureUnfinished)
    );
    assert_eq!(record_bytes(&w), before);
}

#[test]
fn with_nothing_recorded_there_is_nothing_to_check() {
    let tmp = tempfile::tempdir().unwrap();
    let homes = Homes {
        local: tmp.path().join("Local"),
        roaming: tmp.path().join("Roaming"),
    };
    let key = crate::keychain::test_helpers::test_location("ask", tmp.path());
    let chosen = tmp.path().join("E");
    std::fs::create_dir_all(&chosen).unwrap();
    assert_eq!(
        check_request(&homes, &key, &chosen, &TestEnv),
        Err(RequestRefusal::VaultUnavailable)
    );
    assert!(status(&homes).is_err());
    assert!(!homes.pointer_path().exists());
}

// ── where things stand ───────────────────────────────────────────────────

#[test]
fn the_status_names_the_folder_in_use_and_nothing_waiting() {
    let w = world();
    let s = status(&w.homes).unwrap();
    assert_eq!(s.folder, w.source);
    assert_eq!(s.move_waiting, None);
    assert_eq!(s.old_copy, None);
}

#[test]
fn the_status_shows_a_move_waiting_for_the_next_start() {
    let w = world();
    let target = requested(&w);
    let s = status(&w.homes).unwrap();
    assert_eq!(s.folder, w.source, "the memories have not moved yet");
    assert_eq!(s.move_waiting, Some(target));
}

#[test]
fn the_status_says_whether_an_old_copy_can_be_seen() {
    let w = world();
    let old = w.tmp.path().join("F").join("Zaaheen Memories");
    with_old_copy(&w, &old);
    assert_eq!(
        status(&w.homes).unwrap().old_copy,
        Some(OldCopy {
            from: old.clone(),
            connected: false
        })
    );
    std::fs::create_dir_all(&old).unwrap();
    assert_eq!(
        status(&w.homes).unwrap().old_copy,
        Some(OldCopy {
            from: old,
            connected: true
        })
    );
}

// ── letting go of an old copy ────────────────────────────────────────────

#[test]
fn an_old_copy_on_a_drive_that_is_not_there_can_be_forgotten() {
    let w = world();
    let gone = w.tmp.path().join("F").join("Zaaheen Memories");
    with_old_copy(&w, &gone);
    assert_eq!(forget_old_copy(&w.homes, &w.key), Ok(gone));
    let p = record(&w);
    assert!(p.pending_cleanup.is_none());
    assert_eq!(p.vault_dir, w.source);
    assert_eq!(
        snapshot(&w.source),
        w.original,
        "the memories are untouched"
    );
    // The next move is no longer blocked.
    requested(&w);
}

/// A copy that can be seen is removed at the next start: never forgotten,
/// and forgetting never removes anything itself.
#[test]
fn an_old_copy_that_can_be_seen_is_never_forgotten() {
    let w = world();
    let old = w.tmp.path().join("Old");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("vault.db"), b"the old copy").unwrap();
    with_old_copy(&w, &old);
    let before = record_bytes(&w);
    assert_eq!(
        forget_old_copy(&w.homes, &w.key),
        Err(ForgetRefusal::StillThere)
    );
    assert_eq!(record_bytes(&w), before);
    assert_eq!(
        std::fs::read(old.join("vault.db")).unwrap(),
        b"the old copy"
    );
}

#[test]
fn with_no_old_copy_there_is_nothing_to_forget() {
    let w = world();
    let before = record_bytes(&w);
    assert_eq!(
        forget_old_copy(&w.homes, &w.key),
        Err(ForgetRefusal::NothingWaiting)
    );
    assert_eq!(record_bytes(&w), before);
}

/// Only the old copy is forgotten: a move waiting in the same record stays.
#[test]
fn forgetting_an_old_copy_leaves_a_waiting_move_alone() {
    let w = world();
    let gone = w.tmp.path().join("F").join("Zaaheen Memories");
    let mut p = record(&w);
    let waiting = PendingMove {
        to: w.tmp.path().join("G").join("Zaaheen Memories"),
        move_id: pointer::new_id().unwrap(),
        attempts: 1,
    };
    p.pending_move = Some(waiting.clone());
    p.pending_cleanup = Some(PendingCleanup {
        from: gone.clone(),
        move_id: pointer::new_id().unwrap(),
    });
    pointer::write(&w.homes.pointer_path(), &p).unwrap();
    assert_eq!(forget_old_copy(&w.homes, &w.key), Ok(gone));
    let p = record(&w);
    assert!(p.pending_cleanup.is_none());
    assert_eq!(p.pending_move, Some(waiting));
}
