//! ADR-105 L6's key half: what "Start again in the default place" does about
//! an erased marker. Removed only once main and the spare both read "no
//! entry" after R1 — the key is destroyed, so everything the marker names is
//! dead. With a key (or a copy of one) instead, the marker stays and so does
//! the key. It never creates, writes or deletes a key.

use std::path::PathBuf;

use super::double::*;
use crate::keychain::lifecycle::{settle_marker_to_start_again, MarkerSettled};

fn settle(world: &std::sync::Arc<std::sync::Mutex<World>>) -> Result<MarkerSettled, String> {
    let d = Double::new(world);
    settle_marker_to_start_again(&d.ctx()).map_err(|e| e.to_string())
}

fn lost_folder() -> PathBuf {
    PathBuf::from(if cfg!(windows) {
        r"E:\Zaaheen Memories"
    } else {
        "/e/memories"
    })
}

fn no_key_changes(world: &std::sync::Arc<std::sync::Mutex<World>>) {
    let log = world.lock().unwrap().log.clone();
    assert!(
        !log.iter()
            .any(|op| matches!(op, Op::Write(_) | Op::Delete(_))),
        "starting again never writes or deletes a key: {log:?}"
    );
}

#[test]
fn with_no_marker_nothing_is_decided_and_the_store_is_not_read() {
    let w = shared(World::fresh().with_local_key());
    assert_eq!(settle(&w), Ok(MarkerSettled::NoMarker));
    let log = w.lock().unwrap().log.clone();
    assert!(
        !log.iter().any(|op| matches!(op, Op::Read(_))),
        "no marker, no reason to read the key: {log:?}"
    );
    assert!(w.lock().unwrap().main.is_some());
}

#[test]
fn a_marker_after_a_confirmed_erasure_is_removed() {
    for marker in [MarkerFile::Valid(lost_folder()), MarkerFile::Invalid] {
        let mut world = World::fresh();
        world.marker = Some(marker.clone());
        let w = shared(world);
        assert_eq!(settle(&w), Ok(MarkerSettled::Removed), "{marker:?}");
        assert!(w.lock().unwrap().marker.is_none(), "{marker:?}");
        no_key_changes(&w);
    }
}

/// "No entry" is believed only after R1's reads, for both credentials.
#[test]
fn the_key_is_read_as_r1_asks_before_the_marker_goes() {
    let mut world = World::fresh();
    world.marker = Some(MarkerFile::Valid(lost_folder()));
    let w = shared(world);
    settle(&w).unwrap();
    let log = w.lock().unwrap().log.clone();
    let reads = |slot| log.iter().filter(|op| **op == Op::Read(slot)).count();
    assert_eq!(reads(SlotName::Main), 5, "{log:?}");
    assert_eq!(reads(SlotName::Spare), 5, "{log:?}");
    let removed = log.iter().position(|op| *op == Op::RemoveMarker).unwrap();
    let last_read = log
        .iter()
        .rposition(|op| matches!(op, Op::Read(_)))
        .unwrap();
    assert!(last_read < removed, "every read comes before the removal");
}

#[test]
fn with_a_key_the_marker_and_the_key_both_stay() {
    let mut world = World::fresh().with_local_key();
    world.marker = Some(MarkerFile::Valid(lost_folder()));
    let w = shared(world);
    assert_eq!(settle(&w), Ok(MarkerSettled::KeptWithKey));
    assert!(w.lock().unwrap().marker.is_some());
    assert!(w.lock().unwrap().main.is_some());
    no_key_changes(&w);
}

/// A spare copy of a key is a key: the marker stays.
#[test]
fn with_only_a_spare_copy_the_marker_stays() {
    let mut world = World::fresh();
    world.spare = Some(Cred {
        bytes: K.to_vec(),
        local: true,
    });
    world.marker = Some(MarkerFile::Invalid);
    let w = shared(world);
    assert_eq!(settle(&w), Ok(MarkerSettled::KeptWithKey));
    assert!(w.lock().unwrap().marker.is_some());
    assert!(w.lock().unwrap().spare.is_some());
    no_key_changes(&w);
}

/// The library's documented transient "no entry" never removes a marker
/// while the key is there.
#[test]
fn a_key_that_reads_as_missing_a_few_times_is_still_found() {
    let mut world = World::fresh().with_local_key();
    world.marker = Some(MarkerFile::Valid(lost_folder()));
    world.phantom_absent_reads = 4;
    let w = shared(world);
    assert_eq!(settle(&w), Ok(MarkerSettled::KeptWithKey));
    assert!(w.lock().unwrap().marker.is_some());
}

/// Fail closed: a store that cannot answer, or a marker that cannot be read
/// or removed, decides nothing.
#[test]
fn a_store_or_marker_failure_decides_nothing() {
    for failing in [
        Op::Read(SlotName::Main),
        Op::Read(SlotName::Spare),
        Op::MarkerRead,
        Op::RemoveMarker,
    ] {
        let mut world = World::fresh();
        world.marker = Some(MarkerFile::Valid(lost_folder()));
        world.fail_always.push(failing);
        let w = shared(world);
        assert!(settle(&w).is_err(), "{failing:?}");
        assert!(w.lock().unwrap().marker.is_some(), "{failing:?}");
        no_key_changes(&w);
    }
}
