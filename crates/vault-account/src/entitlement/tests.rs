//! Time-model tests: the opener's list for this step (forward jump then
//! correction, backwards clock, floor keyed to `lease_issued_at`, activity in
//! server time) plus the expiry cases step 3 handed over. Every time is an
//! explicit epoch second; nothing reads the real clock.

use super::*;
use crate::lease::LeaseState;

/// Server time at issue for the first lease in each scenario.
const T0: i64 = 1_760_000_000;
const HOUR: i64 = 3_600;

fn trial_lease(client_time: i64) -> Lease {
    Lease::for_test(
        LeaseState::Trial,
        T0,
        client_time,
        Some(T0 + 20 * DAY),
        None,
    )
}

fn paid_lease(state: LeaseState, client_time: i64, paid_days: i64) -> Lease {
    Lease::for_test(state, T0, client_time, None, Some(T0 + paid_days * DAY))
}

fn received(lease: &Lease) -> LocalState {
    LocalState::default().on_new_lease(lease)
}

fn verdict(lease: &Lease, state: &LocalState, now: i64) -> Entitlement {
    assess(lease, state, now).entitlement
}

const ENTITLED: Entitlement = Entitlement::Entitled {
    payment_failed: false,
};

// ---- deadlines (the expiry cases) --------------------------------------------------

#[test]
fn a_trial_lease_entitles_until_the_trial_ends_and_not_a_second_longer() {
    let lease = trial_lease(T0);
    let state = received(&lease);
    let start = assess(&lease, &state, T0);
    assert_eq!(start.entitlement, ENTITLED);
    assert_eq!(start.elapsed, 0);
    assert_eq!(start.remaining, 20 * DAY);

    let last = assess(&lease, &state, T0 + 20 * DAY - 1);
    assert_eq!(last.entitlement, ENTITLED);
    assert_eq!(last.remaining, 1);

    let after = assess(&lease, &state, T0 + 20 * DAY);
    assert_eq!(
        after.entitlement,
        Entitlement::Denied(Denial::DeadlinePassed)
    );
    assert_eq!(after.remaining, 0);
}

#[test]
fn a_paid_lease_entitles_until_its_period_ends() {
    let lease = paid_lease(LeaseState::Active, T0, 10);
    let state = received(&lease);
    assert_eq!(verdict(&lease, &state, T0 + 10 * DAY - 1), ENTITLED);
    assert_eq!(
        verdict(&lease, &state, T0 + 10 * DAY),
        Entitlement::Denied(Denial::DeadlinePassed)
    );
}

#[test]
fn a_failed_payment_still_entitles_and_says_so() {
    let lease = paid_lease(LeaseState::PaymentFailed, T0, 7);
    let state = received(&lease);
    assert_eq!(
        verdict(&lease, &state, T0 + DAY),
        Entitlement::Entitled {
            payment_failed: true
        }
    );
    assert_eq!(
        verdict(&lease, &state, T0 + 7 * DAY),
        Entitlement::Denied(Denial::DeadlinePassed)
    );
}

#[test]
fn the_offline_allowance_caps_even_a_long_paid_period() {
    let lease = paid_lease(LeaseState::Active, T0, 365);
    let state = received(&lease);
    let last = assess(&lease, &state, T0 + 30 * DAY - 1);
    assert_eq!(last.entitlement, ENTITLED);
    assert_eq!(
        last.remaining, 1,
        "the allowance, not the year, is what is left"
    );
    assert_eq!(
        verdict(&lease, &state, T0 + 30 * DAY),
        Entitlement::Denied(Denial::OfflineTooLong)
    );
}

#[test]
fn an_ended_lease_never_entitles_and_says_whether_it_was_paid() {
    let never_paid = Lease::for_test(LeaseState::Ended, T0, T0, Some(T0 - DAY), None);
    assert_eq!(
        verdict(&never_paid, &received(&never_paid), T0),
        Entitlement::Denied(Denial::Ended { was_paid: false })
    );
    let lapsed = Lease::for_test(LeaseState::Ended, T0, T0, None, Some(T0 - DAY));
    assert_eq!(
        verdict(&lapsed, &received(&lapsed), T0),
        Entitlement::Denied(Denial::Ended { was_paid: true })
    );
}

// ---- a wrong clock -----------------------------------------------------------------

#[test]
fn a_clock_that_is_simply_wrong_changes_nothing() {
    for skew in [-3 * DAY, -2 * HOUR, 5 * DAY, 400 * DAY] {
        let client_time = T0 + skew;
        let lease = trial_lease(client_time);
        let state = received(&lease);
        assert_eq!(
            verdict(&lease, &state, client_time),
            ENTITLED,
            "skew {skew}"
        );
        assert_eq!(
            verdict(&lease, &state, client_time + 20 * DAY - 1),
            ENTITLED,
            "skew {skew}"
        );
        assert_eq!(
            verdict(&lease, &state, client_time + 20 * DAY),
            Entitlement::Denied(Denial::DeadlinePassed),
            "skew {skew}"
        );
    }
}

#[test]
fn a_clock_more_than_a_day_off_is_reported_as_information() {
    assert!(clock_looks_wrong(&trial_lease(T0 - 2 * DAY)));
    assert!(clock_looks_wrong(&trial_lease(T0 + DAY + 1)));
    assert!(!clock_looks_wrong(&trial_lease(T0 + DAY)));
    assert!(!clock_looks_wrong(&trial_lease(T0 - 3 * HOUR)));
    assert!(!clock_looks_wrong(&trial_lease(T0)));
}

// ---- a moving clock ----------------------------------------------------------------

#[test]
fn a_forward_jump_counts_until_a_fresh_lease_arrives_after_the_fix() {
    let lease = paid_lease(LeaseState::Active, T0, 365);
    let mut state = received(&lease);

    // The clock jumps 40 days ahead: the lease looks used up.
    let jumped = T0 + 40 * DAY;
    assert_eq!(
        verdict(&lease, &state, jumped),
        Entitlement::Denied(Denial::OfflineTooLong)
    );
    state = state.with_floor(&lease, jumped);

    // The clock is put right. The same lease stays used up: the floor
    // remembers, so winding the clock back is not a way to extend a lease.
    let fixed = T0 + DAY;
    assert_eq!(
        verdict(&lease, &state, fixed),
        Entitlement::Denied(Denial::OfflineTooLong)
    );

    // A fresh lease arrives: the jump stops counting.
    let fresh = Lease::for_test(
        LeaseState::Active,
        T0 + DAY + 60,
        fixed,
        None,
        Some(T0 + 365 * DAY),
    );
    state = state.on_new_lease(&fresh);
    assert_eq!(verdict(&fresh, &state, fixed), ENTITLED);
}

#[test]
fn a_clock_moving_backwards_cannot_shrink_elapsed() {
    let lease = paid_lease(LeaseState::Active, T0, 365);
    let state = received(&lease).with_floor(&lease, T0 + 5 * DAY);
    let back = assess(&lease, &state, T0 + 2 * DAY);
    assert_eq!(back.elapsed, 5 * DAY);
}

#[test]
fn a_clock_behind_the_anchor_needs_a_refresh() {
    let lease = paid_lease(LeaseState::Active, T0, 365);
    let state = received(&lease);
    assert_eq!(
        verdict(&lease, &state, T0 - CLOCK_BEHIND_TOLERANCE - 1),
        Entitlement::Denied(Denial::ClockBehind)
    );
    let within = assess(&lease, &state, T0 - CLOCK_BEHIND_TOLERANCE);
    assert_eq!(within.entitlement, ENTITLED);
    assert_eq!(within.elapsed, 0);
}

#[test]
fn deleting_the_record_only_lowers_the_floor_to_now() {
    let lease = paid_lease(LeaseState::Active, T0, 365);
    let kept = received(&lease).with_floor(&lease, T0 + 5 * DAY);
    assert_eq!(assess(&lease, &kept, T0 + 2 * DAY).elapsed, 5 * DAY);
    assert_eq!(
        assess(&lease, &LocalState::default(), T0 + 2 * DAY).elapsed,
        2 * DAY
    );
}

// ---- the floor belongs to one lease ------------------------------------------------

#[test]
fn a_new_lease_read_before_the_record_is_rewritten_ignores_the_old_floor() {
    // The old lease's floor ran 60 days ahead (a clock that jumped), and the
    // clock was then put right: the new lease's client_time is well behind
    // that floor. Reusing the old floor would read as 50 days offline.
    let old = paid_lease(LeaseState::Active, T0, 365);
    let state = received(&old).with_floor(&old, T0 + 60 * DAY);
    // The keeper wrote a new lease; this reader has not seen state.json
    // change yet.
    let new = Lease::for_test(
        LeaseState::Active,
        T0 + 10 * DAY,
        T0 + 10 * DAY,
        None,
        Some(T0 + 400 * DAY),
    );
    let seen = assess(&new, &state, T0 + 10 * DAY);
    assert_eq!(seen.entitlement, ENTITLED);
    assert_eq!(seen.elapsed, 0);
}

#[test]
fn with_floor_on_a_lease_the_record_does_not_know_starts_that_lease_afresh() {
    let old = paid_lease(LeaseState::Active, T0, 365);
    let state = received(&old).with_floor(&old, T0 + 60 * DAY);
    let new = Lease::for_test(
        LeaseState::Active,
        T0 + DAY,
        T0 + DAY,
        None,
        Some(T0 + 365 * DAY),
    );
    let next = state.with_floor(&new, T0 + DAY + 10);
    assert_eq!(next.lease_issued_at, new.issued_at());
    assert_eq!(next.floor, T0 + DAY + 10);
}

// ---- merging two writers' records --------------------------------------------------

fn record(lease_issued_at: i64, floor: i64, active: i64, attempt: i64) -> LocalState {
    LocalState {
        lease_issued_at,
        floor,
        last_active_anchor: active,
        last_refresh_attempt: attempt,
    }
}

#[test]
fn same_lease_merges_everything_with_max() {
    let disk = record(T0, T0 + 50, T0 + 10, T0 + 99);
    let mine = record(T0, T0 + 70, T0 + 5, T0 + 20);
    assert_eq!(
        LocalState::merge(&disk, &mine),
        record(T0, T0 + 70, T0 + 10, T0 + 99)
    );
}

#[test]
fn a_writer_holding_an_older_lease_never_changes_the_floor() {
    let disk = record(T0 + DAY, T0 + DAY + 5, T0 + 10, T0 + 20);
    let mine = record(T0, T0 + 90 * DAY, T0 + 30, T0 + 10);
    assert_eq!(
        LocalState::merge(&disk, &mine),
        record(T0 + DAY, T0 + DAY + 5, T0 + 30, T0 + 20)
    );
}

#[test]
fn a_writer_holding_a_newer_lease_resets_the_floor() {
    let disk = record(T0, T0 + 90 * DAY, T0 + 30, T0 + 40);
    let mine = record(T0 + DAY, T0 + DAY, T0 + 10, T0 + 50);
    assert_eq!(
        LocalState::merge(&disk, &mine),
        record(T0 + DAY, T0 + DAY, T0 + 30, T0 + 50)
    );
}

// ---- receiving a lease -------------------------------------------------------------

#[test]
fn signing_in_seeds_the_activity_anchor_with_server_time() {
    let lease = trial_lease(T0 - 7 * DAY);
    let state = LocalState::default().on_new_lease(&lease);
    assert_eq!(state, record(T0, T0 - 7 * DAY, T0, 0));
}

#[test]
fn a_later_lease_keeps_the_activity_and_attempt_times() {
    let first = trial_lease(T0);
    let state = received(&first).with_refresh_attempt(T0 + 5);
    let second = Lease::for_test(
        LeaseState::Trial,
        T0 + DAY,
        T0 + DAY,
        Some(T0 + 20 * DAY),
        None,
    );
    let next = state.on_new_lease(&second);
    assert_eq!(next, record(T0 + DAY, T0 + DAY, T0, T0 + 5));
}

#[test]
fn refresh_attempts_only_move_forward() {
    let state = LocalState::default().with_refresh_attempt(T0 + 100);
    assert_eq!(
        state.with_refresh_attempt(T0 + 50).last_refresh_attempt,
        T0 + 100
    );
    assert_eq!(
        state.with_refresh_attempt(T0 + 200).last_refresh_attempt,
        T0 + 200
    );
}

// ---- activity, in server time ------------------------------------------------------

#[test]
fn activity_is_recorded_in_server_time_whatever_the_local_clock_says() {
    let client_time = T0 + 365 * DAY; // a year fast
    let lease = paid_lease(LeaseState::Active, client_time, 365);
    let state = received(&lease);
    let now = client_time + 2 * HOUR;
    let a = assess(&lease, &state, now);
    let after = state.with_activity(&lease, &a).unwrap();
    assert_eq!(after.last_active_anchor, T0 + 2 * HOUR);
}

#[test]
fn activity_is_written_at_most_hourly() {
    let lease = paid_lease(LeaseState::Active, T0, 365);
    let state = received(&lease); // anchor = T0
    let early = assess(&lease, &state, T0 + HOUR - 1);
    assert_eq!(state.with_activity(&lease, &early), None);
    let later = assess(&lease, &state, T0 + HOUR);
    assert_eq!(
        state
            .with_activity(&lease, &later)
            .map(|s| s.last_active_anchor),
        Some(T0 + HOUR)
    );
}

#[test]
fn a_denied_lease_records_no_activity() {
    let lease = paid_lease(LeaseState::Active, T0, 1);
    let state = received(&lease);
    let a = assess(&lease, &state, T0 + 2 * DAY);
    assert!(matches!(a.entitlement, Entitlement::Denied(_)));
    assert_eq!(state.with_activity(&lease, &a), None);
}

// ---- 30 days unused ----------------------------------------------------------------

fn fresh_issued(at: i64) -> Lease {
    Lease::for_test(LeaseState::Active, at, at, None, Some(at + 30 * DAY))
}

#[test]
fn more_than_30_days_unused_signs_out_after_a_refresh() {
    let state = record(T0, T0, T0, 0);
    assert!(unused_too_long(&state, &fresh_issued(T0 + 30 * DAY + 1)));
    assert!(!unused_too_long(&state, &fresh_issued(T0 + 30 * DAY)));
    assert!(!unused_too_long(&state, &fresh_issued(T0 + 29 * DAY)));
}

#[test]
fn a_missing_or_damaged_activity_record_never_signs_out() {
    for anchor in [0, -5] {
        let state = record(T0, T0, anchor, 0);
        assert!(!unused_too_long(&state, &fresh_issued(T0 + 400 * DAY)));
    }
}

#[test]
fn a_local_clock_jump_cannot_trigger_the_30_day_rule() {
    // Used an hour after sign-in on a clock a year SLOW; the next lease is
    // issued (server time) a day later. Recording activity in local time
    // would put it a year before that lease and sign the user out; in
    // server time it is a day old.
    let client_time = T0 - 365 * DAY;
    let lease = paid_lease(LeaseState::Active, client_time, 365);
    let state = received(&lease);
    let a = assess(&lease, &state, client_time + HOUR);
    let state = state.with_activity(&lease, &a).unwrap();
    assert!(!unused_too_long(&state, &fresh_issued(T0 + DAY)));
}

// ---- when to refresh ---------------------------------------------------------------

#[test]
fn refresh_attempts_are_limited_to_one_a_minute() {
    let never = LocalState::default();
    assert!(refresh_allowed(&never, T0, false));
    let tried = never.with_refresh_attempt(T0);
    assert!(!refresh_allowed(
        &tried,
        T0 + REFRESH_MIN_INTERVAL - 1,
        false
    ));
    assert!(refresh_allowed(&tried, T0 + REFRESH_MIN_INTERVAL, false));
}

#[test]
fn after_a_signed_ended_refresh_backs_off_to_hourly() {
    let tried = LocalState::default().with_refresh_attempt(T0);
    assert!(!refresh_allowed(&tried, T0 + 30 * 60, true));
    assert!(refresh_allowed(
        &tried,
        T0 + REFRESH_INTERVAL_AFTER_ENDED,
        true
    ));
}

#[test]
fn an_attempt_recorded_in_the_future_does_not_block_refresh() {
    let tried = LocalState::default().with_refresh_attempt(T0 + 10 * DAY);
    assert!(refresh_allowed(&tried, T0, false));
    assert!(refresh_allowed(&tried, T0, true));
}

#[test]
fn a_stale_or_nearly_due_lease_is_refreshed_at_start() {
    let lease = paid_lease(LeaseState::Active, T0, 365);
    let state = received(&lease);
    assert!(!stale_at_start(&assess(&lease, &state, T0 + DAY - 1)));
    assert!(stale_at_start(&assess(&lease, &state, T0 + DAY)));

    let nearly = paid_lease(LeaseState::Active, T0, 3);
    let s = received(&nearly);
    assert!(stale_at_start(&assess(&nearly, &s, T0 + 1)));
    let not_yet = paid_lease(LeaseState::Active, T0, 10);
    assert!(!stale_at_start(&assess(&not_yet, &received(&not_yet), T0)));

    let ended = Lease::for_test(LeaseState::Ended, T0, T0, None, None);
    assert!(stale_at_start(&assess(&ended, &received(&ended), T0)));
}

// ---- robustness --------------------------------------------------------------------

#[test]
fn extreme_values_never_panic() {
    let wild = [i64::MIN, -1, 0, 1, T0, i64::MAX];
    for &deadline in &wild {
        for &client_time in &[1, T0, i64::MAX] {
            for &now in &wild {
                for state in [LeaseState::Trial, LeaseState::Active, LeaseState::Ended] {
                    let lease =
                        Lease::for_test(state, T0, client_time, Some(deadline), Some(deadline));
                    let floor_state = LocalState {
                        lease_issued_at: T0,
                        floor: now,
                        last_active_anchor: deadline,
                        last_refresh_attempt: now,
                    };
                    let a = assess(&lease, &floor_state, now);
                    assert!(a.elapsed >= 0 && a.remaining >= 0);
                    let _ = floor_state.with_activity(&lease, &a);
                    let _ = floor_state.with_floor(&lease, now);
                    let _ = refresh_allowed(&floor_state, now, true);
                    let _ = unused_too_long(&floor_state, &lease);
                    let _ = clock_looks_wrong(&lease);
                    let _ = stale_at_start(&a);
                }
            }
        }
    }
}

// ---- the file format ---------------------------------------------------------------

#[test]
fn the_record_round_trips_with_the_designed_field_names() {
    let state = record(T0, T0 + 1, T0 + 2, T0 + 3);
    let text = serde_json::to_string(&state).unwrap();
    for name in [
        "\"lease_issued_at\":",
        "\"floor\":",
        "\"last_active_anchor\":",
        "\"last_refresh_attempt\":",
    ] {
        assert!(text.contains(name), "{text}");
    }
    assert_eq!(serde_json::from_str::<LocalState>(&text).unwrap(), state);
}

#[test]
fn missing_fields_read_as_absent_and_unknown_fields_are_ignored() {
    assert_eq!(
        serde_json::from_str::<LocalState>("{}").unwrap(),
        LocalState::default()
    );
    let partial: LocalState =
        serde_json::from_str(r#"{"floor": 42, "written_by_a_newer_app": true}"#).unwrap();
    assert_eq!(partial, record(0, 42, 0, 0));
}
