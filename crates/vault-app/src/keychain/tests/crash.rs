//! ADR-SEC-029's central promise, checked at every crash point: the process
//! dies after each single step of the move, the recovery, the erasure and
//! the finishing of an erasure, and the next process must find the key
//! either safe (the same bytes, Local, no spare) or — once erasure has
//! deleted it — gone for good.

use super::double::*;
use crate::keychain::lifecycle::{erase, open_or_create};

/// Run `run` in a process that dies after `steps` mutating steps, for every
/// `steps` until a run completes without dying; `check` sees the world after
/// each crash. Returns how many crash points were checked.
fn every_crash_point(
    make: impl Fn() -> World,
    run: impl Fn(&Double),
    check: impl Fn(usize, &std::sync::Arc<std::sync::Mutex<World>>),
) -> usize {
    for steps in 0.. {
        let w = shared(make());
        let d = Double::crashing_after(&w, steps);
        run(&d);
        if !d.died() {
            assert!(steps > 0, "the operation must take at least one step");
            return steps;
        }
        check(steps, &w);
        assert!(steps < 200, "runaway enumeration");
    }
    unreachable!()
}

fn next_open(
    w: &std::sync::Arc<std::sync::Mutex<World>>,
) -> Result<[u8; 32], vault_core::VaultError> {
    let d = Double::new(w);
    open_or_create(&d.ctx()).map(|k| *k)
}

#[test]
fn the_move_never_loses_the_key_at_any_crash_point() {
    let points = every_crash_point(
        || World::fresh().with_enterprise_key().with_vault_data(),
        |d| {
            let _ = open_or_create(&d.ctx());
        },
        |steps, w| {
            let key = next_open(w).unwrap_or_else(|e| panic!("crash after {steps}: {e}"));
            assert_eq!(key, K, "crash after {steps} steps lost the key");
            let world = w.lock().unwrap();
            let main = world.main.clone().expect("main present");
            assert_eq!(main.bytes, K.to_vec());
            assert!(
                main.local,
                "crash after {steps}: the next open finishes the move"
            );
            assert!(world.spare.is_none(), "crash after {steps}: no spare left");
        },
    );
    assert!(
        points >= 3,
        "the move writes the spare, main, then deletes the spare"
    );
}

#[test]
fn restoring_from_a_spare_never_loses_the_key_at_any_crash_point() {
    every_crash_point(
        || {
            let mut world = World::fresh().with_vault_data();
            world.spare = Some(Cred {
                bytes: K.to_vec(),
                local: true,
            });
            world
        },
        |d| {
            let _ = open_or_create(&d.ctx());
        },
        |steps, w| {
            assert_eq!(next_open(w).unwrap(), K, "crash after {steps} lost the key");
            let world = w.lock().unwrap();
            assert!(world.main.as_ref().is_some_and(|c| c.local));
            assert!(world.spare.is_none());
        },
    );
}

#[test]
fn an_erased_key_never_comes_back_at_any_crash_point() {
    every_crash_point(
        || {
            let mut world = World::fresh().with_local_key().with_vault_data();
            // A spare left by an interrupted move makes it harder.
            world.spare = Some(Cred {
                bytes: K.to_vec(),
                local: true,
            });
            world
        },
        |d| {
            let _ = erase(&d.ctx(), &own_folder());
        },
        |steps, w| {
            let main_deleted = w.lock().unwrap().main.is_none();
            let result = next_open(w);
            if main_deleted {
                // Erasure reached the key: K must never be seen again, by
                // this open or by any after a finished erasure.
                if let Ok(key) = result {
                    assert_ne!(key, K, "crash after {steps}: the erased key came back");
                }
                let d = Double::new(w);
                let own = w.lock().unwrap().own.clone();
                erase(&d.ctx(), &own).expect("a retried erasure succeeds");
                if let Ok(key) = next_open(w) {
                    assert_ne!(
                        key, K,
                        "crash after {steps}: the erased key came back after a retry"
                    );
                }
            } else {
                // The crash came before the key was deleted: the user was
                // never told it was erased, and the key must still work.
                assert_eq!(
                    result.unwrap(),
                    K,
                    "crash after {steps}: a live key was lost"
                );
            }
            let world = w.lock().unwrap();
            if world.marker.is_some() {
                assert!(
                    world.main.is_none(),
                    "crash after {steps}: a marker beside a live key"
                );
            }
        },
    );
}

#[test]
fn finishing_an_erasure_never_makes_a_key_before_the_files_are_gone() {
    every_crash_point(
        || {
            let mut world = World::fresh().with_vault_data();
            world.marker = Some(MarkerFile::Valid(world.own.clone()));
            world
        },
        |d| {
            let _ = open_or_create(&d.ctx());
        },
        |steps, w| {
            let world = w.lock().unwrap();
            if world.main.is_some() {
                assert!(
                    world.marker.is_none(),
                    "crash after {steps}: a key made while a marker exists"
                );
                assert!(
                    !world.own_keyed(),
                    "crash after {steps}: a key made over leftover data"
                );
            }
            drop(world);
            next_open(w).unwrap_or_else(|e| panic!("crash after {steps}: {e}"));
            let world = w.lock().unwrap();
            assert!(world.marker.is_none() && !world.own_keyed());
        },
    );
}
