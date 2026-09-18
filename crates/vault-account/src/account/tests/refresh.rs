//! The refresh: rotation order, the §8.26 §15 rotation-reuse rule, the single
//! refresher, failures that must never sign anyone out, rate limits, and the
//! 30-days-unused sign-out.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

async fn refresh(world: &World, trigger: Trigger, now: i64) -> RefreshOutcome {
    world.account.refresh(trigger, now).await.unwrap()
}

fn issued_at(outcome: &RefreshOutcome) -> i64 {
    match outcome {
        RefreshOutcome::Refreshed(lease) => lease.issued_at(),
        other => panic!("expected Refreshed, got {other:?}"),
    }
}

// ---- the happy path ----------------------------------------------------------------

#[tokio::test]
async fn a_refresh_rotates_the_token_and_writes_the_new_lease() {
    let world = World::signed_in().await;
    world.set_server_now(T0 + DAY);
    let outcome = refresh(&world, Trigger::Routine, T0 + DAY).await;
    assert_eq!(issued_at(&outcome), T0 + DAY);

    assert_eq!(world.refreshes_sent(), vec!["rt_1".to_string()]);
    assert_eq!(world.token().as_deref(), Some("rt_2"));
    let RefreshOutcome::Refreshed(lease) = outcome else {
        unreachable!()
    };
    assert_eq!(world.lease_bytes().as_deref(), Some(lease.wire()));
    let state = world.dir.read_state();
    assert_eq!(state.lease_issued_at, T0 + DAY);
    assert_eq!(state.floor, T0 + DAY);
    assert_eq!(state.last_refresh_attempt, T0 + DAY);
    assert_eq!(state.last_active_anchor, T0, "activity is not refresh");

    // The lease request carried the NEW access token and this clock.
    let last = world.worker.seen().pop().unwrap();
    assert_eq!(
        last.headers.get("authorization").map(String::as_str),
        Some("Bearer at_2")
    );
    let body: serde_json::Value = serde_json::from_str(&last.body).unwrap();
    assert_eq!(body["client_now"], json!(T0 + DAY));
}

#[tokio::test]
async fn the_new_token_is_stored_before_the_lease_is_fetched() {
    let world = World::signed_in().await;
    let old_lease = world.lease_bytes();
    world.set_worker(WorkerMode::Status(503));
    let err = world
        .account
        .refresh(Trigger::Routine, T0 + DAY)
        .await
        .unwrap_err();
    assert!(err.is_transient(), "{err:?}");
    // The server rotated rt_1 → rt_2; the store has rt_2, so the next
    // refresh does not replay a spent token (which would end the grant).
    assert_eq!(world.token().as_deref(), Some("rt_2"));
    assert_eq!(world.lease_bytes(), old_lease, "the old lease stays");
    assert!(world.has(MARKER_FILE));

    world.set_worker(WorkerMode::Sign(LeaseState::Trial));
    refresh(&world, Trigger::UserAction, T0 + DAY).await;
    assert_eq!(
        world.refreshes_sent(),
        vec!["rt_1".to_string(), "rt_2".to_string()]
    );
}

// ---- the rotation-reuse rule (§8.26 §15) -------------------------------------------

#[tokio::test]
async fn a_rotation_by_another_process_is_not_mistaken_for_sign_out() {
    let world = World::signed_in().await;
    let other_process = world.store.clone();
    world.on_refresh(move |presented| match presented {
        // Another process rotated rt_1 → rt_2 a moment ago, so rt_1 is spent.
        "rt_1" => {
            other_process.save(&token("rt_2")).unwrap();
            invalid_grant()
        }
        "rt_2" => tokens("at_3", "rt_3"),
        _ => invalid_grant(),
    });
    world.set_server_now(T0 + DAY);
    let outcome = refresh(&world, Trigger::Routine, T0 + DAY).await;
    assert_eq!(issued_at(&outcome), T0 + DAY);
    assert_eq!(
        world.refreshes_sent(),
        vec!["rt_1".to_string(), "rt_2".to_string()]
    );
    assert_eq!(world.token().as_deref(), Some("rt_3"));
    assert!(world.has(MARKER_FILE));
}

#[tokio::test]
async fn a_dead_grant_signs_out_after_one_second_look() {
    let world = World::signed_in().await;
    world.on_refresh(|_| invalid_grant());
    let outcome = refresh(&world, Trigger::Routine, T0 + DAY).await;
    assert_eq!(
        outcome,
        RefreshOutcome::SignedOut(SignOutReason::GrantEnded)
    );
    // The store was re-read, found the same token, and it was NOT replayed.
    assert_eq!(world.refreshes_sent(), vec!["rt_1".to_string()]);
    assert_eq!(world.token(), None);
    for name in [MARKER_FILE, LEASE_FILE, STATE_FILE] {
        assert!(!world.has(name), "{name} left behind");
    }
    assert!(world.revoked().is_empty(), "a dead token is not revoked");
}

#[tokio::test]
async fn a_refusal_of_the_reread_token_too_signs_out() {
    let world = World::signed_in().await;
    let other_process = world.store.clone();
    world.on_refresh(move |presented| {
        if presented == "rt_1" {
            other_process.save(&token("rt_2")).unwrap();
        }
        invalid_grant()
    });
    let outcome = refresh(&world, Trigger::Routine, T0 + DAY).await;
    assert_eq!(
        outcome,
        RefreshOutcome::SignedOut(SignOutReason::GrantEnded)
    );
    assert_eq!(
        world.refreshes_sent(),
        vec!["rt_1".to_string(), "rt_2".to_string()]
    );
    assert!(!world.has(MARKER_FILE));
}

#[tokio::test]
async fn a_missing_token_is_looked_for_twice_then_signs_out() {
    let world = World::signed_in().await;
    world.store.delete().unwrap();
    let outcome = refresh(&world, Trigger::Routine, T0 + DAY).await;
    assert_eq!(outcome, RefreshOutcome::SignedOut(SignOutReason::NoToken));
    assert!(world.refreshes_sent().is_empty());
    assert!(!world.has(MARKER_FILE));
}

// ---- the store refusing the rotated token (found by review, session 44) ------------

#[tokio::test]
async fn a_brief_store_failure_while_saving_the_rotated_token_is_retried() {
    let (world, refuse) = World::flaky().await;
    world.sign_in(T0).await;
    world.on_refresh(clerk_like());
    refuse.store(2, Ordering::SeqCst);
    world.set_server_now(T0 + DAY);
    let outcome = refresh(&world, Trigger::Routine, T0 + DAY).await;
    assert_eq!(issued_at(&outcome), T0 + DAY);
    assert_eq!(world.token().as_deref(), Some("rt_2"));
    assert_eq!(
        refuse.load(Ordering::SeqCst),
        0,
        "both refusals were retried through"
    );
}

#[tokio::test]
async fn a_store_that_keeps_refusing_the_rotated_token_never_signs_out() {
    let (world, refuse) = World::flaky().await;
    world.sign_in(T0).await;
    world.on_refresh(clerk_like());

    // Clerk rotates rt_1 → rt_2 (rt_1 is now spent) and the store refuses
    // rt_2 for longer than the retries.
    refuse.store(usize::MAX, Ordering::SeqCst);
    let err = world
        .account
        .refresh(Trigger::Routine, T0 + DAY)
        .await
        .unwrap_err();
    assert!(err.is_transient(), "{err:?}");
    assert_eq!(
        world.token().as_deref(),
        Some("rt_1"),
        "the store never took rt_2"
    );
    assert!(world.has(MARKER_FILE));

    // The store recovers. The next refresh must use rt_2, never the spent
    // rt_1 (which would end the grant and sign the user out).
    refuse.store(0, Ordering::SeqCst);
    world.set_server_now(T0 + DAY);
    let outcome = refresh(&world, Trigger::UserAction, T0 + DAY + 5).await;
    assert_eq!(issued_at(&outcome), T0 + DAY);
    assert_eq!(
        world.refreshes_sent(),
        vec!["rt_1".to_string(), "rt_2".to_string()]
    );
    assert_eq!(world.token().as_deref(), Some("rt_3"));
    assert!(world.has(MARKER_FILE));
}

#[tokio::test]
async fn the_newest_unsaved_token_is_used_even_while_the_store_still_refuses() {
    let (world, refuse) = World::flaky().await;
    world.sign_in(T0).await;
    world.on_refresh(clerk_like());
    refuse.store(usize::MAX, Ordering::SeqCst);
    for now in [T0 + DAY, T0 + DAY + 5] {
        let err = world
            .account
            .refresh(Trigger::UserAction, now)
            .await
            .unwrap_err();
        assert!(err.is_transient(), "{err:?}");
    }
    refuse.store(0, Ordering::SeqCst);
    refresh(&world, Trigger::UserAction, T0 + DAY + 10).await;
    assert_eq!(
        world.refreshes_sent(),
        vec!["rt_1".to_string(), "rt_2".to_string(), "rt_3".to_string()],
        "each refresh used the newest token, none a spent one"
    );
    assert_eq!(world.token().as_deref(), Some("rt_4"));
}

#[tokio::test]
async fn sign_out_revokes_the_live_token_even_if_the_store_never_took_it() {
    let (world, refuse) = World::flaky().await;
    world.sign_in(T0).await;
    world.on_refresh(clerk_like());
    refuse.store(usize::MAX, Ordering::SeqCst);
    let _ = world.account.refresh(Trigger::Routine, T0 + DAY).await;
    refuse.store(0, Ordering::SeqCst);
    world.account.sign_out().await.unwrap();
    assert_eq!(world.revoked(), vec!["rt_2".to_string()]);
    assert_eq!(world.token(), None);
    assert!(!world.has(MARKER_FILE));
}

// ---- failures that must never sign anyone out ---------------------------------------

#[tokio::test]
async fn network_trouble_never_signs_out() {
    let world = World::signed_in().await;
    let lease = world.lease_bytes();
    world.on_refresh(|_| json_response(503, r#"{"error":"server_error"}"#));
    let err = world
        .account
        .refresh(Trigger::Routine, T0 + DAY)
        .await
        .unwrap_err();
    assert!(err.is_transient());
    assert_eq!(world.token().as_deref(), Some("rt_1"));
    assert_eq!(world.lease_bytes(), lease);
    assert!(world.has(MARKER_FILE));
    assert_eq!(world.dir.read_state().last_refresh_attempt, T0 + DAY);
}

#[tokio::test]
async fn a_keychain_failure_never_signs_out() {
    let world = World::signed_in().await;
    let lease = world.lease_bytes();
    world.fail_next_keychain_call();
    let err = world
        .account
        .refresh(Trigger::Routine, T0 + DAY)
        .await
        .unwrap_err();
    assert!(matches!(err, AccountError::Keychain(_)), "{err:?}");
    assert!(world.refreshes_sent().is_empty());
    assert_eq!(world.token().as_deref(), Some("rt_1"));
    assert_eq!(world.lease_bytes(), lease);
    assert!(world.has(MARKER_FILE));
}

#[tokio::test]
async fn a_bad_lease_from_the_worker_keeps_the_old_one() {
    for mode in [WorkerMode::Stranger, WorkerMode::OtherUser] {
        let world = World::signed_in().await;
        let lease = world.lease_bytes();
        world.set_worker(mode);
        let err = world
            .account
            .refresh(Trigger::Routine, T0 + DAY)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AccountError::LeaseRejected(_)),
            "{mode:?}: {err:?}"
        );
        assert_eq!(world.lease_bytes(), lease, "{mode:?}");
        assert!(world.has(MARKER_FILE));
    }
}

#[tokio::test]
async fn a_signed_ended_is_written_and_decides() {
    let world = World::signed_in().await;
    world.set_worker(WorkerMode::Sign(LeaseState::Ended));
    world.set_server_now(T0 + DAY);
    let outcome = refresh(&world, Trigger::Routine, T0 + DAY).await;
    assert_eq!(issued_at(&outcome), T0 + DAY);
    match world.account.status(T0 + DAY).await.unwrap() {
        Status::Leased { assessment, .. } => assert_eq!(
            assessment.entitlement,
            Entitlement::Denied(Denial::Ended { was_paid: false })
        ),
        other => panic!("{other:?}"),
    }
    assert!(world.has(MARKER_FILE), "ended is not signed out");
}

// ---- 30 days unused ----------------------------------------------------------------

#[tokio::test]
async fn thirty_days_unused_signs_out_and_revokes_the_new_token() {
    let world = World::signed_in().await; // activity anchor = T0
    world.set_server_now(T0 + 31 * DAY);
    let outcome = refresh(&world, Trigger::Routine, T0 + 31 * DAY).await;
    assert_eq!(outcome, RefreshOutcome::SignedOut(SignOutReason::Unused));
    assert_eq!(world.revoked(), vec!["rt_2".to_string()]);
    assert_eq!(world.token(), None);
    for name in [MARKER_FILE, LEASE_FILE, STATE_FILE] {
        assert!(!world.has(name), "{name} left behind");
    }
}

#[tokio::test]
async fn recent_use_keeps_a_long_idle_lease_signed_in() {
    let world = World::signed_in().await;
    world.account.record_use(T0 + 25 * DAY).await.unwrap();
    world.set_server_now(T0 + 31 * DAY);
    let outcome = refresh(&world, Trigger::Routine, T0 + 31 * DAY).await;
    assert_eq!(issued_at(&outcome), T0 + 31 * DAY);
}

// ---- rate limits and the lock ------------------------------------------------------

#[tokio::test]
async fn denial_and_routine_refreshes_are_limited_to_one_a_minute() {
    let world = World::signed_in().await;
    refresh(&world, Trigger::Denial, T0 + 10).await;
    assert_eq!(
        refresh(&world, Trigger::Denial, T0 + 40).await,
        RefreshOutcome::Skipped(SkipReason::RateLimited)
    );
    assert_eq!(
        refresh(&world, Trigger::Routine, T0 + 40).await,
        RefreshOutcome::Skipped(SkipReason::RateLimited)
    );
    assert_eq!(world.refreshes_sent().len(), 1);
    refresh(&world, Trigger::Denial, T0 + 71).await;
    assert_eq!(world.refreshes_sent().len(), 2);
}

#[tokio::test]
async fn after_a_signed_ended_denials_back_off_to_hourly_but_the_user_is_never_blocked() {
    let world = World::signed_in().await;
    world.set_worker(WorkerMode::Sign(LeaseState::Ended));
    refresh(&world, Trigger::Routine, T0 + 10).await;
    assert_eq!(
        refresh(&world, Trigger::Denial, T0 + 200).await,
        RefreshOutcome::Skipped(SkipReason::RateLimited)
    );
    // "I've paid": straight through.
    world.set_worker(WorkerMode::Sign(LeaseState::Active));
    world.set_server_now(T0 + 200);
    let outcome = refresh(&world, Trigger::UserAction, T0 + 200).await;
    assert_eq!(issued_at(&outcome), T0 + 200);
}

#[tokio::test]
async fn a_held_lock_skips_the_refresh_without_touching_the_network() {
    let world = World::signed_in().await;
    let _held = world.dir.lock(Duration::ZERO).unwrap().unwrap();
    let started = std::time::Instant::now();
    assert_eq!(
        refresh(&world, Trigger::Denial, T0 + DAY).await,
        RefreshOutcome::Skipped(SkipReason::LockBusy)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(world.refreshes_sent().is_empty());
}

#[tokio::test]
async fn nobody_signed_in_means_nothing_to_refresh() {
    let world = World::new().await;
    assert_eq!(
        refresh(&world, Trigger::UserAction, T0).await,
        RefreshOutcome::Skipped(SkipReason::NotSignedIn)
    );
    assert!(world.oauth.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_refreshes_at_once_reach_the_server_once() {
    let world = World::signed_in().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    world.on_refresh(move |presented| {
        counted.fetch_add(1, Ordering::SeqCst);
        // Slow enough that the second refresh is waiting on the lock.
        std::thread::sleep(Duration::from_millis(150));
        rotate(presented)
    });
    world.set_server_now(T0 + DAY);
    // Both with the longer user wait, and neither rate-limited, so only the
    // re-read under the lock can stop the second from calling the server.
    let (a, b) = tokio::join!(
        world.account.refresh(Trigger::UserAction, T0 + DAY),
        world.account.refresh(Trigger::UserAction, T0 + DAY),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "one refresher: {a:?} / {b:?}"
    );
    assert_eq!(issued_at(&a), T0 + DAY);
    assert_eq!(issued_at(&b), T0 + DAY);
    assert_eq!(world.token().as_deref(), Some("rt_2"));
}
