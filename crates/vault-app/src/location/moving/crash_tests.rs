//! ADR-105 L5's central promise, checked at every crash point (ADR-SEC-029's
//! method): the process dies after each single changing step of a move, of
//! the start that recovers from it, and of the old copy's removal. After
//! every one of them the location record names exactly one complete vault,
//! and the starts that follow finish or undo the move and leave the
//! memories in one place only.
//!
//! The enumerations run over the in-memory disk ([`super::model`]); the
//! real disk is crashed at chosen points by
//! [`the_real_disk_recovers_at_chosen_crash_points`].

use std::path::{Path, PathBuf};

use super::model::{model, Model, ModelEnv, ModelFs, Node};
use super::tests::*;
use super::*;
use crate::location::{self, pointer, MOVE_ID_FILE, VAULT_ID_FILE};

/// Run `run` in a process that dies after `steps` changing steps, for every
/// `steps` until a run completes without dying; `check` sees the model after
/// each crash. Returns how many crash points were checked.
fn every_crash_point(
    make: impl Fn() -> Model,
    run: impl Fn(&Model, &ModelFs<'_>),
    check: impl Fn(usize, &Model),
) -> usize {
    for steps in 0.. {
        let m = make();
        let fs = ModelFs::crashing_after(&m.disk, steps);
        run(&m, &fs);
        if !fs.died() {
            assert!(steps > 0, "the operation must take at least one step");
            return steps;
        }
        check(steps, &m);
        assert!(steps < 500, "runaway enumeration");
    }
    unreachable!()
}

fn run_move(m: &Model, fs: &ModelFs<'_>) {
    move_holding_source(&m.homes, &m.key, fs, &ModelEnv(&m.disk));
}

#[test]
fn a_move_never_loses_a_memory_at_any_crash_point() {
    let keys = tempfile::tempdir().unwrap();
    let points = every_crash_point(
        || model(keys.path()),
        run_move,
        |steps, m| {
            let at = format!("crash after {steps}");
            m.one_complete_vault(&at);
            m.settle(&at);
        },
    );
    // The count, the folder, restrict, .move-id, 4 folders and 7 files, the
    // switch, then .move-id, the old copy's entries, its .vault-id and the
    // record.
    assert!(
        points >= 25,
        "only {points} crash points: the model is too small"
    );
}

/// A crash, then a crash in the start that recovers from it, at every pair
/// of points: still one complete vault, and the starts after settle it.
#[test]
fn recovering_from_a_crash_is_itself_safe_at_every_pair_of_crash_points() {
    let keys = tempfile::tempdir().unwrap();
    let first_points = every_crash_point(|| model(keys.path()), run_move, |_, _| {});
    let mut pairs = 0;
    for first in 0..first_points {
        let crashed_once = || {
            let m = model(keys.path());
            let fs = ModelFs::crashing_after(&m.disk, first);
            run_move(&m, &fs);
            assert!(fs.died());
            m
        };
        pairs += every_crash_point(
            crashed_once,
            |m, fs| {
                m.start(fs);
            },
            |second, m| {
                let at = format!("crash after {first}, then after {second}");
                m.one_complete_vault(&at);
                m.settle(&at);
            },
        );
    }
    assert!(pairs > first_points, "{pairs} pairs");
}

/// The old copy's removal, from a record that already names the new folder:
/// the new folder is never touched, and the old copy goes in the end.
#[test]
fn removing_the_old_copy_never_touches_the_new_one_at_any_crash_point() {
    let keys = tempfile::tempdir().unwrap();
    // The moment after the switch: a finished move with its old copy put
    // back and the record still saying so.
    let just_switched = || {
        let m = model(keys.path());
        let move_id = m
            .disk
            .borrow()
            .pointer
            .as_ref()
            .unwrap()
            .pending_move
            .as_ref()
            .unwrap()
            .move_id
            .clone();
        assert!(matches!(
            m.start(&ModelFs::new(&m.disk)),
            MoveOutcome::Moved { .. }
        ));
        let mut disk = m.disk.borrow_mut();
        for (rel, node) in m.original.clone() {
            disk.nodes.insert(m.source.join(rel), node);
        }
        disk.nodes.insert(
            m.target.join(MOVE_ID_FILE),
            Node::File(move_id.clone().into_bytes()),
        );
        disk.pointer.as_mut().unwrap().pending_cleanup = Some(pointer::PendingCleanup {
            from: m.source.clone(),
            move_id,
        });
        drop(disk);
        m
    };
    every_crash_point(
        just_switched,
        |m, fs| {
            clean_holding_old(&m.key, fs);
        },
        |steps, m| {
            let at = format!("crash after {steps}");
            let live = m.one_complete_vault(&at);
            assert_eq!(live, m.target, "{at}: the record went back");
            m.settle(&at);
        },
    );
}

/// Two attempts that died: the third start gives the move up at every crash
/// point of its own, and the memories stay complete where they were.
#[test]
fn giving_up_is_safe_at_any_crash_point() {
    let keys = tempfile::tempdir().unwrap();
    let twice_interrupted = || {
        let m = model(keys.path());
        for _ in 0..MAX_ATTEMPTS {
            // Dies part-way through.
            let fs = ModelFs::crashing_after(&m.disk, 8);
            m.start(&fs);
            assert!(fs.died());
        }
        m
    };
    let m = twice_interrupted();
    assert_eq!(
        m.start(&ModelFs::new(&m.disk)),
        MoveOutcome::Failed {
            reason: MoveFailure::Interrupted,
            retrying: false
        }
    );
    every_crash_point(
        twice_interrupted,
        |m, fs| {
            m.start(fs);
        },
        |steps, m| {
            let at = format!("crash after {steps}");
            assert_eq!(m.one_complete_vault(&at), m.source);
            m.settle(&at);
        },
    );
}

// ── the real disk, at chosen points ──────────────────────────────────────

fn with_request() -> World {
    let w = world();
    requested(&w);
    w
}

/// Starts on the real disk until nothing is left to do; then the memories
/// are in exactly one place.
fn settle_on_disk(at: &str, w: &World, target: &Path) {
    for _ in 0..4 {
        if next_start(w, &Scripted::new(w)) == MoveOutcome::Nothing {
            break;
        }
    }
    let live = location::resolve(&w.homes)
        .unwrap_or_else(|e| panic!("{at}: the memories cannot be found: {e}"))
        .path()
        .to_path_buf();
    if live == w.source {
        assert_eq!(snapshot(&w.source), w.original, "{at}");
        assert!(!target.exists(), "{at}: a half-made copy was left");
        assert!(record(w).pending_move.is_none(), "{at}");
    } else {
        assert_moved(w, target);
    }
}

/// A named crash point and the file system that dies there.
type CrashCase = (&'static str, fn(&World) -> Scripted);

/// The real file system crashed after the attempt is counted, with an empty
/// folder made, during the copy, and right after the switch.
#[test]
fn the_real_disk_recovers_at_chosen_crash_points() {
    let cases: [CrashCase; 4] = [
        ("after the count", |w| Scripted::crashing_after(w, 1)),
        ("with an empty folder", |w| Scripted::crashing_after(w, 3)),
        ("during the copy", |w| {
            let mut fs = Scripted::new(w);
            fs.die_copying = Some("graph.sealed");
            fs
        }),
        ("after the switch", |w| {
            let mut fs = Scripted::new(w);
            fs.die_after_commit = true;
            fs
        }),
    ];
    for (at, crashing) in cases {
        let w = with_request();
        let target = record(&w).pending_move.unwrap().to;
        let fs = crashing(&w);
        move_holding_source(&w.homes, &w.key, &fs, &TestEnv);
        assert!(fs.died(), "{at}: the process must die");
        let live = location::resolve(&w.homes)
            .unwrap_or_else(|e| panic!("{at}: the memories cannot be found: {e}"));
        assert_eq!(snapshot(live.path()), w.original, "{at}: incomplete");
        settle_on_disk(at, &w, &target);
    }
}

/// Two interrupted attempts on the real disk: the third start gives the
/// move up, removes the half-made copy and says why.
#[test]
fn a_move_interrupted_twice_is_given_up() {
    let w = with_request();
    let target: PathBuf = record(&w).pending_move.unwrap().to;
    let mid_copy = |w: &World| {
        let mut fs = Scripted::new(w);
        fs.die_copying = Some("graph.sealed");
        move_holding_source(&w.homes, &w.key, &fs, &TestEnv);
        assert!(fs.died());
        assert!(target.join(MOVE_ID_FILE).exists(), "died during the copy");
    };
    mid_copy(&w);
    mid_copy(&w);
    assert_eq!(record(&w).pending_move.unwrap().attempts, MAX_ATTEMPTS);
    assert_eq!(
        next_start(&w, &real_fs(&w)),
        MoveOutcome::Failed {
            reason: MoveFailure::Interrupted,
            retrying: false
        }
    );
    assert!(record(&w).pending_move.is_none());
    assert_stayed(&w, &target);
    assert!(w.source.join(VAULT_ID_FILE).exists());
}

/// The record is written atomically, so a crash can never leave half of
/// one; and a start never believes a record it cannot read.
#[test]
fn a_record_that_cannot_be_read_is_never_acted_on() {
    let w = with_request();
    std::fs::write(w.homes.pointer_path(), b"{ half a rec").unwrap();
    assert_eq!(
        move_holding_source(&w.homes, &w.key, &real_fs(&w), &TestEnv),
        MoveOutcome::Deferred
    );
    assert_eq!(snapshot(&w.source), w.original);
    assert!(
        pointer::read(&w.homes.pointer_path()).is_err(),
        "left as it was"
    );
}
