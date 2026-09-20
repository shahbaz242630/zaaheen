//! Tests for what the desktop is told about the account.
//!
//! **What is not here, and why.** Nothing exercises `sign_in`, `sign_out`,
//! `subscribe` or `refresh_now` end to end: each needs a real `Account`, which
//! needs the credential store, the loopback listener and the network. Those
//! paths are covered inside `vault-account`, which owns them and has the test
//! support to drive them (222 tests). What this file covers is the part
//! `vault-app` actually adds — the mapping the desktop sees, and the promise
//! that no account command can reach the vault.

use super::*;

// ------------------------------------------------------------- days left

#[test]
fn a_part_day_always_rounds_down() {
    assert_eq!(days_left(86_400), 1, "exactly one day is one day");
    assert_eq!(
        days_left(86_400 * 2 - 1),
        1,
        "47 hours 59 minutes must read as 1 day, never 2"
    );
    assert_eq!(days_left(86_399), 0, "a part day left is 0 whole days");
    assert_eq!(days_left(86_400 * 30), 30);
}

/// A denied lease has no countdown, and a negative one would read as a
/// negative number of days on the banner.
#[test]
fn a_spent_lease_never_counts_below_zero() {
    assert_eq!(days_left(0), 0);
    assert_eq!(days_left(-1), 0);
    assert_eq!(days_left(-86_400 * 5), 0);
}

/// The trial banner shows from day 23 of 30, i.e. when 7 days remain
/// (§8.26 §6). Pinned so a change to the arithmetic is visible.
#[test]
fn the_trial_banner_boundary_reads_as_expected() {
    assert_eq!(days_left(86_400 * 7), 7, "day 23 of a 30-day trial");
    assert_eq!(days_left(86_400 * 5), 5, "health.warnings start here");
}

// ---------------------------------------------------------------- states

#[test]
fn every_lease_state_maps_to_its_own_string() {
    let mapped = [
        state_for(LeaseState::Trial),
        state_for(LeaseState::Active),
        state_for(LeaseState::PaymentFailed),
        state_for(LeaseState::Ended),
    ];
    let mut unique = mapped.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        mapped.len(),
        "two lease states share a string, so the desktop cannot tell them apart: {mapped:?}"
    );
    for s in mapped {
        assert!(
            state::ALL.contains(&s),
            "{s} is not in state::ALL, so it has no plain-English line"
        );
    }
}

#[test]
fn a_signed_out_computer_reads_as_signed_out_and_keeps_no_address() {
    let view = AccountOps::view_of(Status::SignedOut, Some("someone@example.test".into()));
    assert!(!view.signed_in);
    assert_eq!(view.state, state::SIGNED_OUT);
    assert_eq!(view.days_left, None);
    assert_eq!(
        view.email, None,
        "a signed-out view must not carry an address, whatever it was handed"
    );
}

#[test]
fn signed_in_without_a_lease_keeps_the_address_and_counts_nothing() {
    let view = AccountOps::view_of(
        Status::NoLease {
            sub: "user_123".into(),
        },
        Some("someone@example.test".into()),
    );
    assert!(view.signed_in);
    assert_eq!(view.state, state::NO_LEASE);
    assert_eq!(view.days_left, None, "there is no lease to count down");
    assert_eq!(view.email.as_deref(), Some("someone@example.test"));
}

/// The subject identifies the person to our account service. It has no place
/// in what crosses the IPC boundary (BRD §11.7.2), and `AccountView` has no
/// field for it — this pins that.
#[test]
fn the_view_never_carries_the_account_subject() {
    let view = AccountOps::view_of(
        Status::NoLease {
            sub: "user_SECRET_SUBJECT".into(),
        },
        None,
    );
    let shown = format!("{view:?}");
    assert!(
        !shown.contains("user_SECRET_SUBJECT"),
        "the subject reached the view: {shown}"
    );
}

// ---------------------------------------------------------------- errors

/// An account-service message must never become UI text, so errors are
/// mapped by kind. Network failures in particular must stay distinguishable:
/// "check your connection" and "we could not confirm your subscription" are
/// different things to tell somebody.
#[test]
fn account_errors_map_by_kind_and_never_carry_a_message() {
    assert_eq!(OpsError::from(AccountError::Busy), OpsError::Busy);
    assert_eq!(
        OpsError::from(AccountError::Network("connection reset by peer".into())),
        OpsError::Unreachable
    );
    assert_eq!(
        OpsError::from(AccountError::Protocol("server said something odd".into())),
        OpsError::Refused
    );

    let shown = format!(
        "{:?}",
        OpsError::from(AccountError::Network("connection reset by peer".into()))
    );
    assert!(
        !shown.contains("connection reset"),
        "an upstream message survived into the error: {shown}"
    );
}

#[test]
fn a_refused_link_stays_a_link_error() {
    assert_eq!(
        OpsError::from(LinkError::NotAllowed),
        OpsError::Link(LinkError::NotAllowed)
    );
}

// ----------------------------------------------------- the structural rule

/// The whole protection for the ungated account commands: this type is not
/// given the thing that reads memories.
///
/// A source test, because the guarantee is the *absence* of a field and no
/// runtime assertion can observe an absence. If somebody adds an
/// `Application` here, the account commands stop being safe to leave ungated
/// and this is what says so.
#[test]
fn account_ops_never_holds_the_vault() {
    const SOURCE: &str = include_str!("../account_ops.rs");
    let struct_body = SOURCE
        .split_once("pub struct AccountOps {")
        .expect("the struct is declared in this file")
        .1
        .split_once('}')
        .expect("the struct is closed")
        .0;

    for forbidden in ["Application", "Adapter", "adapter", "MasterKey"] {
        assert!(
            !struct_body.contains(forbidden),
            "AccountOps gained a `{forbidden}` field. These commands run for somebody \
             who is NOT entitled (SIGNIN-DESIGN 8.26 6.4's allowlist), and the only \
             reason that is safe is that they cannot reach a memory. Giving this type \
             the vault removes that guarantee."
        );
    }
}

// ------------------------------------------------------ the daily timer

/// The spread exists so every install does not ask the Worker at the same
/// moment forever (SIGNIN-DESIGN 8.26 4's "jittered daily timer"). Whatever
/// the clock says, the period must stay inside the designed window: never
/// shorter than a day, never longer than a day and six hours.
#[test]
fn the_daily_refresh_period_stays_inside_its_window() {
    const DAY: u64 = 24 * 60 * 60;
    const SPREAD: u64 = 6 * 60 * 60;

    for now in [
        0_i64,
        1,
        1_760_000_000,
        1_760_000_001,
        i64::MAX,
        -1,
        i64::MIN,
    ] {
        let period = daily_period(now).as_secs();
        assert!(
            (DAY..=DAY + SPREAD).contains(&period),
            "a clock reading of {now} produced a {period}s period, outside the              {DAY}s..={}s window",
            DAY + SPREAD
        );
    }
}

/// A negative clock reading (a computer set before 1970) must not panic or
/// wrap into a tiny period that hammers the Worker.
#[test]
fn a_clock_set_before_1970_still_yields_a_sane_period() {
    let period = daily_period(-1_000_000).as_secs();
    assert!(period >= 24 * 60 * 60);
}
