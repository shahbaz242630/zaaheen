//! ADR-SEC-029, rule by rule, against the scripted store and files.

use std::path::PathBuf;

use vault_core::{VaultError, VaultKeyFailure};

use super::double::*;
use crate::keychain::lifecycle::{self, erase, open_or_create};

fn open(world: &std::sync::Arc<std::sync::Mutex<World>>) -> Result<[u8; 32], VaultError> {
    let d = Double::new(world);
    open_or_create(&d.ctx()).map(|k| *k)
}

fn main_of(world: &std::sync::Arc<std::sync::Mutex<World>>) -> Option<Cred> {
    world.lock().unwrap().main.clone()
}

fn spare_of(world: &std::sync::Arc<std::sync::Mutex<World>>) -> Option<Cred> {
    world.lock().unwrap().spare.clone()
}

fn local_k() -> Option<Cred> {
    Some(Cred {
        bytes: K.to_vec(),
        local: true,
    })
}

fn enterprise_k() -> Option<Cred> {
    Some(Cred {
        bytes: K.to_vec(),
        local: false,
    })
}

// ── creating (D2, D4) ─────────────────────────────────────────────────────

#[test]
fn a_fresh_install_creates_a_local_key_that_reads_back() {
    let w = shared(World::fresh());
    let key = open(&w).expect("a fresh install gets a key");
    let main = main_of(&w).expect("stored");
    assert_eq!(main.bytes, key.to_vec());
    assert!(main.local, "a new key must never roam");
    assert!(spare_of(&w).is_none());
    // Read back after the write: a Read(Main) follows the Write(Main).
    let log = w.lock().unwrap().log.clone();
    let write = log
        .iter()
        .position(|o| *o == Op::Write(SlotName::Main))
        .unwrap();
    assert!(
        log[write..].contains(&Op::Read(SlotName::Main)),
        "a new key is read back before it is returned"
    );
}

#[test]
fn a_keeper_first_install_with_only_operational_files_creates_a_key() {
    let w = shared(World::fresh().with_operational_files_only());
    open(&w).expect("operational files are not vault data");
    assert!(main_of(&w).is_some());
}

#[test]
fn no_key_is_made_over_existing_vault_data() {
    let w = shared(World::fresh().with_vault_data());
    let err = open(&w).expect_err("a new key could not open that data");
    assert!(
        matches!(err, VaultError::VaultKey(VaultKeyFailure::Missing)),
        "{err}"
    );
    assert_eq!(w.lock().unwrap().writes(), 0, "nothing may be written");
    assert!(main_of(&w).is_none());
}

#[test]
fn a_failed_check_for_vault_data_is_never_read_as_none() {
    let w = shared(World::fresh());
    w.lock().unwrap().fail_always.push(Op::KeyedCheck);
    let err = open(&w).expect_err("an unknown folder state fails closed");
    assert!(
        matches!(
            err,
            VaultError::VaultKey(VaultKeyFailure::FolderUnavailable)
        ),
        "{err}"
    );
    assert_eq!(w.lock().unwrap().writes(), 0);
}

#[test]
fn a_new_key_that_does_not_read_back_is_not_returned() {
    let w = shared(World::fresh());
    w.lock().unwrap().corrupt_writes.push(SlotName::Main);
    let err = open(&w).expect_err("a corrupted write must not pass");
    assert!(matches!(err, VaultError::KeychainProvenance(_)), "{err}");
}

// ── "no key" means NoEntry, believed after R1 (D3, R1) ───────────────────

#[test]
fn a_brief_false_no_entry_is_retried_and_never_creates_a_key() {
    let w = shared(World::fresh().with_local_key().with_vault_data());
    w.lock().unwrap().phantom_absent_reads = 4;
    let key = open(&w).expect("the fifth read finds the key");
    assert_eq!(key, K);
    assert_eq!(
        w.lock().unwrap().writes(),
        0,
        "no new key over the real one"
    );
}

#[test]
fn a_store_failure_is_never_read_as_no_key() {
    let w = shared(World::fresh().with_local_key());
    w.lock().unwrap().fail_always.push(Op::Read(SlotName::Main));
    let err = open(&w).expect_err("the store failed");
    assert!(matches!(err, VaultError::KeychainProvenance(_)), "{err}");
    assert_eq!(w.lock().unwrap().writes(), 0);
    assert_eq!(main_of(&w), local_k(), "the key is untouched");
}

#[test]
fn a_stored_value_of_the_wrong_size_is_unusable_and_left_alone() {
    let mut world = World::fresh().with_vault_data();
    world.main = Some(Cred {
        bytes: vec![1u8; 31],
        local: false,
    });
    let w = shared(world);
    let err = open(&w).expect_err("31 bytes is not a key");
    assert!(
        matches!(err, VaultError::VaultKey(VaultKeyFailure::Unusable)),
        "{err}"
    );
    assert_eq!(w.lock().unwrap().writes(), 0);
    assert_eq!(main_of(&w).unwrap().bytes.len(), 31);
}

// ── the move (D5) ─────────────────────────────────────────────────────────

#[test]
fn an_enterprise_key_moves_to_local_with_the_same_bytes_and_no_spare_left() {
    let w = shared(World::fresh().with_enterprise_key().with_vault_data());
    assert_eq!(open(&w).unwrap(), K);
    assert_eq!(main_of(&w), local_k());
    assert!(spare_of(&w).is_none());
    // The spare is written before main is rewritten, and deleted after.
    let log = w.lock().unwrap().log.clone();
    let spare_write = log
        .iter()
        .position(|o| *o == Op::Write(SlotName::Spare))
        .unwrap();
    let main_write = log
        .iter()
        .position(|o| *o == Op::Write(SlotName::Main))
        .unwrap();
    let spare_delete = log
        .iter()
        .position(|o| *o == Op::Delete(SlotName::Spare))
        .unwrap();
    assert!(
        spare_write < main_write && main_write < spare_delete,
        "{log:?}"
    );
}

#[test]
fn a_local_key_opens_without_any_write() {
    let w = shared(World::fresh().with_local_key().with_vault_data());
    assert_eq!(open(&w).unwrap(), K);
    assert_eq!(w.lock().unwrap().writes(), 0);
}

#[test]
fn a_spare_that_cannot_be_made_leaves_the_key_where_it_was() {
    let w = shared(World::fresh().with_enterprise_key().with_vault_data());
    w.lock().unwrap().fail_once.push(Op::Write(SlotName::Spare));
    assert_eq!(open(&w).unwrap(), K);
    assert_eq!(main_of(&w), enterprise_k(), "main untouched");
    // Next open: the move completes.
    assert_eq!(open(&w).unwrap(), K);
    assert_eq!(main_of(&w), local_k());
    assert!(spare_of(&w).is_none());
}

#[test]
fn a_spare_that_does_not_read_back_stops_the_move_before_main_is_touched() {
    let w = shared(World::fresh().with_enterprise_key().with_vault_data());
    w.lock().unwrap().corrupt_writes.push(SlotName::Spare);
    assert_eq!(open(&w).unwrap(), K);
    assert_eq!(main_of(&w), enterprise_k());
    assert!(
        !w.lock().unwrap().log.contains(&Op::Write(SlotName::Main)),
        "main must not be rewritten without a verified spare"
    );
}

#[test]
fn a_write_that_does_not_become_local_is_never_counted_as_moved() {
    let w = shared(World::fresh().with_enterprise_key().with_vault_data());
    w.lock().unwrap().writes_stay_enterprise = true;
    assert_eq!(open(&w).unwrap(), K);
    let world = w.lock().unwrap();
    assert!(
        !world.log.contains(&Op::Write(SlotName::Main)),
        "a spare that is not Local is not a verified copy; main must not be touched"
    );
    assert_eq!(world.main, enterprise_k());
}

#[test]
fn a_main_rewrite_that_fails_once_is_retried_from_memory() {
    let w = shared(World::fresh().with_enterprise_key().with_vault_data());
    w.lock().unwrap().fail_once.push(Op::Write(SlotName::Main));
    assert_eq!(open(&w).unwrap(), K);
    assert_eq!(main_of(&w), local_k());
    assert!(spare_of(&w).is_none());
}

#[test]
fn a_main_that_will_not_become_local_keeps_its_verified_spare() {
    let w = shared(World::fresh().with_enterprise_key().with_vault_data());
    w.lock()
        .unwrap()
        .fail_always
        .push(Op::Write(SlotName::Main));
    assert_eq!(open(&w).unwrap(), K);
    assert_eq!(main_of(&w), enterprise_k());
    assert_eq!(spare_of(&w), local_k(), "the verified copy stays");
    // Windows recovers: the next open finishes the move.
    w.lock().unwrap().fail_always.clear();
    assert_eq!(open(&w).unwrap(), K);
    assert_eq!(main_of(&w), local_k());
    assert!(spare_of(&w).is_none());
}

#[test]
fn a_spare_that_cannot_be_deleted_is_removed_by_the_next_open() {
    let w = shared(World::fresh().with_enterprise_key().with_vault_data());
    w.lock()
        .unwrap()
        .fail_once
        .push(Op::Delete(SlotName::Spare));
    assert_eq!(open(&w).unwrap(), K);
    assert_eq!(main_of(&w), local_k());
    assert_eq!(spare_of(&w), local_k());
    assert_eq!(open(&w).unwrap(), K);
    assert!(spare_of(&w).is_none());
}

// ── recovery (C2) ────────────────────────────────────────────────────────

#[test]
fn an_interrupted_move_with_main_still_enterprise_is_finished() {
    let mut world = World::fresh().with_enterprise_key().with_vault_data();
    world.spare = local_k();
    let w = shared(world);
    assert_eq!(open(&w).unwrap(), K);
    assert_eq!(main_of(&w), local_k());
    assert!(spare_of(&w).is_none());
}

#[test]
fn a_spare_that_differs_from_the_key_never_replaces_it() {
    let mut world = World::fresh().with_local_key().with_vault_data();
    world.spare = Some(Cred {
        bytes: vec![9u8; 32],
        local: true,
    });
    let w = shared(world);
    assert_eq!(open(&w).unwrap(), K, "main is kept");
    assert_eq!(main_of(&w), local_k());
    assert_eq!(
        spare_of(&w).unwrap().bytes,
        vec![9u8; 32],
        "left for the next erasure"
    );
}

#[test]
fn a_spare_with_main_gone_is_restored_when_no_erasure_is_pending() {
    let mut world = World::fresh().with_vault_data();
    world.spare = local_k();
    let w = shared(world);
    assert_eq!(open(&w).unwrap(), K, "the spare IS the key");
    assert_eq!(main_of(&w), local_k());
    assert!(spare_of(&w).is_none());
}

#[test]
fn a_spare_that_is_not_32_bytes_is_deleted_and_main_is_untouched() {
    let mut world = World::fresh().with_local_key().with_vault_data();
    world.spare = Some(Cred {
        bytes: vec![1u8; 5],
        local: true,
    });
    let w = shared(world);
    assert_eq!(open(&w).unwrap(), K);
    assert!(spare_of(&w).is_none());
    assert_eq!(main_of(&w), local_k());
}

// ── erasure (E1–E3, E0) ──────────────────────────────────────────────────

fn erase_own(w: &std::sync::Arc<std::sync::Mutex<World>>) -> Result<lifecycle::Erased, VaultError> {
    let d = Double::new(w);
    let own = w.lock().unwrap().own.clone();
    erase(&d.ctx(), &own)
}

#[test]
fn erasure_deletes_the_spare_before_the_key_and_marks_only_after_both_are_gone() {
    let mut world = World::fresh().with_local_key().with_vault_data();
    world.spare = local_k();
    let w = shared(world);
    let erased = erase_own(&w).expect("erased");
    assert!(erased.key_destroyed);
    assert!(main_of(&w).is_none() && spare_of(&w).is_none());
    let log = w.lock().unwrap().log.clone();
    let del_spare = log
        .iter()
        .position(|o| *o == Op::Delete(SlotName::Spare))
        .unwrap();
    let del_main = log
        .iter()
        .position(|o| *o == Op::Delete(SlotName::Main))
        .unwrap();
    let marker = log.iter().position(|o| *o == Op::WriteMarker).unwrap();
    let first_clean = log.iter().position(|o| *o == Op::Clean).unwrap();
    assert!(del_spare < del_main, "spare first: {log:?}");
    assert!(
        del_main < marker,
        "the marker only after the key is gone: {log:?}"
    );
    assert!(marker < first_clean, "the marker before any file: {log:?}");
    // Everything removed → the marker is removed too.
    assert!(w.lock().unwrap().marker.is_none());
    assert!(!w.lock().unwrap().own_keyed());
}

#[test]
fn erasure_that_cannot_delete_the_key_touches_no_file_and_writes_no_marker() {
    let w = shared(World::fresh().with_local_key().with_vault_data());
    w.lock()
        .unwrap()
        .fail_always
        .push(Op::Delete(SlotName::Main));
    let err = erase_own(&w).expect_err("the key survived");
    assert!(matches!(err, VaultError::KeychainProvenance(_)), "{err}");
    let world = w.lock().unwrap();
    assert!(world.main.is_some());
    assert!(world.marker.is_none(), "no marker while a key is alive");
    assert!(world.own_keyed(), "no file touched");
    assert!(!world.log.contains(&Op::Clean));
}

#[test]
fn erasure_whose_delete_silently_did_nothing_is_reported_as_failed() {
    let w = shared(World::fresh().with_local_key().with_vault_data());
    w.lock().unwrap().sticky_deletes.push(SlotName::Main);
    let err = erase_own(&w).expect_err("the key is still readable");
    assert!(matches!(err, VaultError::KeychainProvenance(_)), "{err}");
    let world = w.lock().unwrap();
    assert!(world.marker.is_none());
    assert!(world.own_keyed());
}

#[test]
fn an_erasure_that_leaves_files_keeps_the_marker_naming_that_folder() {
    let w = shared(World::fresh().with_local_key().with_vault_data());
    {
        let mut world = w.lock().unwrap();
        let db = world.own.join("vault.db");
        world.locked.insert(db);
    }
    let erased = erase_own(&w).expect("the key is gone; a file is left");
    assert_eq!(erased.undeletable.len(), 1);
    let world = w.lock().unwrap();
    assert_eq!(world.marker, Some(MarkerFile::Valid(world.own.clone())));
}

#[test]
fn erasure_is_idempotent() {
    let w = shared(World::fresh().with_local_key().with_vault_data());
    assert!(erase_own(&w).unwrap().key_destroyed);
    assert!(!erase_own(&w).unwrap().key_destroyed);
}

// ── finishing an erasure (C1) ────────────────────────────────────────────

#[test]
fn the_next_open_finishes_an_erasure_then_starts_with_a_new_key() {
    let w = shared(World::fresh().with_local_key().with_vault_data());
    {
        let mut world = w.lock().unwrap();
        let db = world.own.join("vault.db");
        world.locked.insert(db);
    }
    erase_own(&w).unwrap();
    // The desktop closes; the file is no longer held.
    w.lock().unwrap().locked.clear();
    let key = open(&w).expect("finished, then a fresh key");
    assert_ne!(key, K, "the erased key never comes back");
    let world = w.lock().unwrap();
    assert!(world.marker.is_none());
    assert!(!world.own_keyed());
}

#[test]
fn an_unfinishable_erasure_of_this_folder_fails_closed_and_makes_no_key() {
    let w = shared(World::fresh().with_local_key().with_vault_data());
    {
        let mut world = w.lock().unwrap();
        let db = world.own.join("vault.db");
        world.locked.insert(db);
    }
    erase_own(&w).unwrap();
    let err = open(&w).expect_err("the leftover is still held");
    assert!(
        matches!(
            err,
            VaultError::VaultKey(VaultKeyFailure::FolderUnavailable)
        ),
        "{err}"
    );
    assert!(main_of(&w).is_none());
    assert!(w.lock().unwrap().marker.is_some());
}

#[test]
fn while_an_erasure_is_unfinished_no_key_is_made_in_any_folder() {
    let other = PathBuf::from(if cfg!(windows) {
        r"C:\elsewhere"
    } else {
        "/elsewhere"
    });
    let mut world = World::fresh();
    let mut names = std::collections::BTreeSet::new();
    names.insert("vault.db".to_owned());
    world.folders.insert(other.clone(), names);
    world.locked.insert(other.join("vault.db"));
    world.marker = Some(MarkerFile::Valid(other));
    let w = shared(world);
    let err = open(&w).expect_err("no key while the marker exists");
    assert!(
        matches!(
            err,
            VaultError::VaultKey(VaultKeyFailure::FolderUnavailable)
        ),
        "{err}"
    );
    assert!(main_of(&w).is_none());
}

#[test]
fn a_marker_with_a_live_key_deletes_nothing() {
    let mut world = World::fresh().with_local_key().with_vault_data();
    world.marker = Some(MarkerFile::Valid(world.own.clone()));
    let w = shared(world);
    assert_eq!(open(&w).unwrap(), K);
    let world = w.lock().unwrap();
    assert!(world.own_keyed(), "live data must never be deleted");
    assert!(!world.log.contains(&Op::Clean));
}

#[test]
fn an_untrusted_marker_is_never_acted_on_and_blocks_new_keys() {
    let mut world = World::fresh().with_vault_data();
    world.marker = Some(MarkerFile::Invalid);
    world.spare = local_k();
    let w = shared(world);
    let err = open(&w).expect_err("no key while an untrusted marker exists");
    assert!(
        matches!(
            err,
            VaultError::VaultKey(VaultKeyFailure::FolderUnavailable)
        ),
        "{err}"
    );
    let world = w.lock().unwrap();
    assert!(world.own_keyed(), "nothing deleted");
    assert_eq!(
        world.spare,
        local_k(),
        "the spare is neither restored nor deleted"
    );
    assert!(world.main.is_none());
}

#[test]
fn a_spare_found_during_a_confirmed_erasure_is_deleted_not_restored() {
    let mut world = World::fresh().with_vault_data();
    world.marker = Some(MarkerFile::Valid(world.own.clone()));
    world.spare = local_k();
    let w = shared(world);
    let key = open(&w).expect("cleaned, then a fresh key");
    assert_ne!(
        key, K,
        "the erased key must not come back through its spare"
    );
    assert!(spare_of(&w).is_none());
}

/// Review finding (session 52): a spare delete that reports success but
/// does nothing, during C1, must not let the next step restore it.
#[test]
fn a_spare_that_survives_its_delete_during_an_erasure_never_comes_back() {
    let mut world = World::fresh().with_vault_data();
    world.marker = Some(MarkerFile::Valid(world.own.clone()));
    world.spare = local_k();
    world.sticky_deletes.push(SlotName::Spare);
    let w = shared(world);
    if let Ok(key) = open(&w) {
        assert_ne!(key, K, "the erased key came back through its spare");
    }
    assert!(!matches!(main_of(&w), Some(c) if c.bytes == K.to_vec()));
}

/// C1 confirms the spare is gone, as E2 does: a delete that silently did
/// nothing stops the open.
#[test]
fn finishing_an_erasure_confirms_the_spare_is_gone() {
    let mut world = World::fresh();
    world.marker = Some(MarkerFile::Valid(world.own.clone()));
    world.spare = local_k();
    world.sticky_deletes.push(SlotName::Spare);
    let w = shared(world);
    let d = Double::new(&w);
    let err = lifecycle::finish_erasure(&d.ctx()).expect_err("the spare survived its delete");
    assert!(matches!(err, VaultError::KeychainProvenance(_)), "{err}");
    assert!(
        w.lock().unwrap().marker.is_some(),
        "the erasure is not finished"
    );
}

/// D4: no new key is ever made while a spare exists; the next open's
/// recovery restores it instead.
#[test]
fn no_new_key_is_made_beside_a_spare() {
    let mut world = World::fresh();
    world.spare = local_k();
    let w = shared(world);
    let d = Double::new(&w);
    let err = lifecycle::create(&d.ctx(), lifecycle::NO_ERASURE).expect_err("a spare exists");
    assert!(matches!(err, VaultError::KeychainProvenance(_)), "{err}");
    assert_eq!(w.lock().unwrap().writes(), 0);
}

/// Review note (session 52): in an open that began with a confirmed
/// erasure, a spare that still shows up at D4 is a copy of the destroyed
/// key — deleted, never left for a later open (which has no marker) to
/// restore.
#[test]
fn a_spare_met_at_d4_after_an_erasure_is_deleted_not_deferred() {
    let mut world = World::fresh();
    world.spare = local_k();
    let w = shared(world);
    let d = Double::new(&w);
    let seen = lifecycle::ErasureState {
        seen: true,
        pending: false,
    };
    let key = lifecycle::create(&d.ctx(), seen).expect("the dead copy goes, a new key comes");
    assert_ne!(*key, K);
    assert!(spare_of(&w).is_none(), "the erased key's copy is deleted");
    // And the next open never brings K back.
    assert_ne!(open(&w).unwrap(), K);
}

/// Even once C1 has finished and removed the marker, the same open never
/// restores a spare.
#[test]
fn no_spare_is_restored_in_the_open_that_finished_an_erasure() {
    let mut world = World::fresh();
    world.marker = Some(MarkerFile::Valid(world.own.clone()));
    let w = shared(world);
    let d = Double::new(&w);
    let state = lifecycle::finish_erasure(&d.ctx()).unwrap();
    assert!(state.seen && !state.pending);
    // A spare appearing now (out of design) is not restored by this open.
    w.lock().unwrap().spare = local_k();
    lifecycle::recover(&d.ctx(), state.seen).unwrap();
    assert!(
        main_of(&w).is_none(),
        "no restore in an open that saw an erasure"
    );
}

#[test]
fn a_marker_that_cannot_be_removed_fails_closed_and_makes_no_key() {
    let mut world = World::fresh().with_vault_data();
    world.marker = Some(MarkerFile::Valid(world.own.clone()));
    let w = shared(world);
    w.lock().unwrap().fail_always.push(Op::RemoveMarker);
    let err = open(&w).expect_err("the marker must not outlive a new key");
    assert!(
        matches!(
            err,
            VaultError::VaultKey(VaultKeyFailure::FolderUnavailable)
        ),
        "{err}"
    );
    assert!(main_of(&w).is_none());
}

// ── after an erasure, the old key never returns ──────────────────────────

#[test]
fn an_erased_key_never_returns_to_a_process_that_opened_before_it() {
    let w = shared(World::fresh().with_enterprise_key().with_vault_data());
    // A creator opens (and moves) the key...
    assert_eq!(open(&w).unwrap(), K);
    // ...erasure runs (under the same lock, so strictly after)...
    erase_own(&w).unwrap();
    // ...and the next open of any process never sees K again.
    let next = open(&w).expect("a fresh start");
    assert_ne!(next, K);
    assert!(spare_of(&w).is_none());
}
