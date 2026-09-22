//! ADR-105 amendment 1 (L-f): a move reports how far it has got, for the
//! "Moving your memories" screen, and ends exactly as it would without.

use std::sync::Arc;
use std::time::Duration;

use super::start_tests::start_reporting;
use super::tests::*;
use super::*;

/// The bytes a move of `w` copies, counted from the test's own snapshot.
fn bytes_to_move(w: &World) -> u64 {
    w.original
        .values()
        .filter_map(Option::as_ref)
        .map(|content| content.len() as u64)
        .sum()
}

#[tokio::test]
async fn a_move_reports_each_phase_in_order_with_every_byte_counted() {
    let w = world();
    let target = requested(&w);
    let total = bytes_to_move(&w);
    assert!(total > 0);
    let progress = Arc::new(MoveProgress::default());

    let outcome = start_reporting(&w, Duration::from_secs(5), &progress).await;
    // The same end as a start with no one watching.
    assert!(matches!(outcome, MoveOutcome::Moved { .. }), "{outcome:?}");
    assert_moved(&w, &target);

    let changes = progress.changes();
    let began: Vec<(MovePhase, u64)> = changes.iter().map(|c| (c.began, c.total)).collect();
    assert_eq!(
        began,
        vec![
            (MovePhase::Waiting, 0),
            (MovePhase::Copying, total),
            (MovePhase::Checking, total),
            (MovePhase::Finishing, 0),
        ]
    );
    // Each counting phase had counted every one of its bytes, once, by the
    // time the next began. `told` is the count before the cap: a chunk
    // counted twice would reach the total halfway, and the cap would hide it.
    assert_eq!(
        changes[2].ended,
        MoveSnapshot {
            phase: MovePhase::Copying,
            done: total,
            total
        }
    );
    assert_eq!(changes[2].told, total, "every byte copied, counted once");
    assert_eq!(
        changes[3].ended,
        MoveSnapshot {
            phase: MovePhase::Checking,
            done: total,
            total
        }
    );
    assert_eq!(changes[3].told, total, "every byte checked, counted once");
    assert_eq!(progress.snapshot().phase, MovePhase::Finishing);
}

#[tokio::test]
async fn a_start_with_no_move_waiting_reports_nothing() {
    let w = world();
    let progress = Arc::new(MoveProgress::default());
    assert_eq!(
        start_reporting(&w, Duration::from_secs(5), &progress).await,
        MoveOutcome::Nothing
    );
    assert!(progress.changes().is_empty());
}

/// The memories cannot be had (a maintenance run holds them): the screen saw
/// the wait, and nothing after it.
#[tokio::test]
async fn a_deferred_move_reports_only_the_wait() {
    let w = world();
    let target = requested(&w);
    let _held =
        crate::ConsolidatorLock::try_acquire_named(&w.source, crate::VAULT_LOCKFILE_NAME).unwrap();
    let progress = Arc::new(MoveProgress::default());
    assert_eq!(
        start_reporting(&w, Duration::from_millis(300), &progress).await,
        MoveOutcome::Deferred
    );
    let began: Vec<MovePhase> = progress.changes().iter().map(|c| c.began).collect();
    assert_eq!(began, vec![MovePhase::Waiting]);
    assert_stayed(&w, &target);
}

#[test]
fn a_count_never_passes_its_total_and_each_phase_starts_from_zero() {
    let progress = MoveProgress::default();
    assert_eq!(progress.snapshot(), MoveSnapshot::default());
    progress.begin(MovePhase::Copying, 10);
    progress.advance(7);
    progress.advance(7);
    assert_eq!(
        progress.snapshot(),
        MoveSnapshot {
            phase: MovePhase::Copying,
            done: 10,
            total: 10
        }
    );
    progress.begin(MovePhase::Checking, 4);
    assert_eq!(
        progress.snapshot(),
        MoveSnapshot {
            phase: MovePhase::Checking,
            done: 0,
            total: 4
        }
    );
    progress.advance(u64::MAX);
    assert_eq!(progress.snapshot().done, 4);
}

/// Only a start with a move waiting takes the moving screen's path.
#[test]
fn only_a_move_waiting_is_a_waiting_move() {
    let empty = tempfile::tempdir().unwrap();
    let nowhere = Homes {
        local: empty.path().join("Local"),
        roaming: empty.path().join("Roaming"),
    };
    assert_eq!(waiting_move(&nowhere), None, "no record");

    let w = world();
    assert_eq!(waiting_move(&w.homes), None, "recorded, nothing asked");
    let target = requested(&w);
    assert_eq!(waiting_move(&w.homes), Some(target));

    // An old copy still to remove, and no move: not a waiting move.
    let mut cleaning = record(&w);
    cleaning.pending_move = None;
    cleaning.pending_cleanup = Some(PendingCleanup {
        from: w.tmp.path().join("old"),
        move_id: pointer::new_id().unwrap(),
    });
    pointer::write(&w.homes.pointer_path(), &cleaning).unwrap();
    assert_eq!(waiting_move(&w.homes), None, "an old copy only");

    // A record that cannot be read goes the usual way.
    std::fs::write(w.homes.pointer_path(), b"{ not a record").unwrap();
    assert_eq!(waiting_move(&w.homes), None, "unreadable");
}
