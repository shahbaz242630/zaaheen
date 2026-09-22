//! The entitlement check (`SIGNIN-DESIGN.md` §8.26 §4 / §6, §8.27).
//!
//! The account is a stand-in, so nothing here touches Credential Manager, the
//! network or the disk. Time is tokio's paused clock, so the deadline tests
//! are deterministic and take no real seconds.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use vault_account::{
    AccountError, AccountResult, Assessment, Denial, Entitlement, RefreshOutcome, Trigger,
};
use vault_mcp::{AccountNotice, EntitlementCheck, LockReason, Verdict};

use super::{
    daily_period, refresh_if_stale, run_routine_refresh, wants_refresh_at_start, AccountAccess,
    AccountCheck, AccountState, Clock, ModeCheck, TRIAL_NOTICE_WITHIN,
};

const HOUR: i64 = 3_600;
const DAY: i64 = 24 * HOUR;

/// A lease received `elapsed` seconds ago with `remaining` seconds of
/// entitlement left.
const fn leased(
    entitlement: Entitlement,
    trial: bool,
    elapsed: i64,
    remaining: i64,
) -> AccountState {
    AccountState::Leased {
        assessment: Assessment {
            entitlement,
            elapsed,
            remaining,
        },
        trial,
    }
}

/// A paid subscription an hour into a fresh lease, well away from any
/// deadline: nothing to tell anybody.
const ENTITLED: AccountState = leased(
    Entitlement::Entitled {
        payment_failed: false,
    },
    false,
    HOUR,
    20 * DAY,
);
const PAYMENT_FAILED: AccountState = leased(
    Entitlement::Entitled {
        payment_failed: true,
    },
    false,
    HOUR,
    6 * DAY,
);

/// A trial with `remaining` seconds left.
const fn trial_with(remaining: i64) -> AccountState {
    leased(
        Entitlement::Entitled {
            payment_failed: false,
        },
        true,
        HOUR,
        remaining,
    )
}

fn denied(denial: Denial) -> AccountState {
    leased(Entitlement::Denied(denial), false, 31 * DAY, 0)
}

fn trial_ended() -> AccountState {
    denied(Denial::Ended { was_paid: false })
}

fn subscription_ended() -> AccountState {
    denied(Denial::Ended { was_paid: true })
}

/// A clock the test moves by hand.
struct FixedClock(AtomicI64);

impl FixedClock {
    fn at(now: i64) -> Arc<Self> {
        Arc::new(Self(AtomicI64::new(now)))
    }
}

impl Clock for FixedClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// One answer to a `state` call. Not a `Result`, because the account's error
/// type cannot be cloned: an unreadable folder is rebuilt on each call.
enum StateAnswer {
    Reads(AccountState),
    Unreadable,
}

impl StateAnswer {
    fn answer(&self) -> AccountResult<AccountState> {
        match self {
            StateAnswer::Reads(state) => Ok(*state),
            StateAnswer::Unreadable => Err(AccountError::Keychain("access denied".into())),
        }
    }
}

/// What the stand-in account does when asked to refresh.
#[derive(Clone)]
enum OnRefresh {
    /// Answer at once, leaving the state as scripted.
    Answer(Arc<dyn Fn() -> AccountResult<RefreshOutcome> + Send + Sync>),
    /// Take this long first (tokio's paused clock, so no real wait).
    Slow(Duration),
}

/// A stand-in for `vault_account::Account`.
struct FakeAccount {
    /// One answer per `state` call; the last one repeats.
    states: Vec<StateAnswer>,
    reads: AtomicUsize,
    on_refresh: OnRefresh,
    /// Set when a slow refresh ran to the end (it must never be cancelled).
    refresh_finished: Arc<AtomicBool>,
    triggers: Mutex<Vec<Trigger>>,
    uses: Mutex<Vec<i64>>,
}

impl FakeAccount {
    fn new(states: Vec<StateAnswer>, on_refresh: OnRefresh) -> Arc<Self> {
        Arc::new(Self {
            states,
            reads: AtomicUsize::new(0),
            on_refresh,
            refresh_finished: Arc::new(AtomicBool::new(false)),
            triggers: Mutex::new(Vec::new()),
            uses: Mutex::new(Vec::new()),
        })
    }

    /// Always answers `state`, and a refresh changes nothing.
    fn steady(state: AccountState) -> Arc<Self> {
        Self::new(
            vec![StateAnswer::Reads(state)],
            OnRefresh::Answer(Arc::new(|| {
                Ok(RefreshOutcome::Skipped(
                    vault_account::SkipReason::RateLimited,
                ))
            })),
        )
    }

    fn refreshes(&self) -> Vec<Trigger> {
        self.triggers.lock().unwrap().clone()
    }

    fn uses(&self) -> Vec<i64> {
        self.uses.lock().unwrap().clone()
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AccountAccess for FakeAccount {
    async fn state(&self, _now: i64) -> AccountResult<AccountState> {
        let n = self.reads.fetch_add(1, Ordering::SeqCst);
        let last = self.states.len() - 1;
        self.states[n.min(last)].answer()
    }

    async fn refresh(&self, trigger: Trigger, _now: i64) -> AccountResult<RefreshOutcome> {
        self.triggers.lock().unwrap().push(trigger);
        match &self.on_refresh {
            OnRefresh::Answer(answer) => answer(),
            OnRefresh::Slow(how_long) => {
                tokio::time::sleep(*how_long).await;
                self.refresh_finished.store(true, Ordering::SeqCst);
                Ok(RefreshOutcome::Skipped(vault_account::SkipReason::LockBusy))
            }
        }
    }

    async fn record_use(&self, now: i64) -> AccountResult<()> {
        self.uses.lock().unwrap().push(now);
        Ok(())
    }
}

fn check_over(account: Arc<FakeAccount>, now: i64) -> AccountCheck {
    AccountCheck::new(account, FixedClock::at(now))
}

// ---------------------------------------------------------------------------
// The happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_live_lease_serves_the_call_without_asking_the_server() {
    let account = FakeAccount::steady(ENTITLED);
    let verdict = check_over(account.clone(), 1_000).check().await;
    assert_eq!(verdict, Verdict::Entitled);
    assert!(
        account.refreshes().is_empty(),
        "a lease that entitles must not wait on a refresh"
    );
}

/// A failed payment still serves the call; the banner is the desktop's job
/// (§8.26 §6 "Messages": payment failed is a warning, not a lock).
#[tokio::test]
async fn a_failed_payment_still_serves_the_call() {
    let account = FakeAccount::steady(PAYMENT_FAILED);
    assert_eq!(
        check_over(account.clone(), 1_000).check().await,
        Verdict::Entitled
    );
    assert!(account.refreshes().is_empty());
}

/// Activity is what the 30-day unused rule counts (§8.26 §4); the account
/// keeps it to once an hour.
#[tokio::test]
async fn using_the_vault_records_the_use() {
    let account = FakeAccount::steady(ENTITLED);
    let _ = check_over(account.clone(), 4_242).check().await;
    assert_eq!(account.uses(), vec![4_242]);
}

#[tokio::test]
async fn a_locked_vault_records_no_use() {
    let account = FakeAccount::steady(trial_ended());
    let _ = check_over(account.clone(), 4_242).check().await;
    assert!(
        account.uses().is_empty(),
        "a refused call must not count as using the vault"
    );
}

// ---------------------------------------------------------------------------
// Refresh-then-decide (§8.26 §4)
// ---------------------------------------------------------------------------

/// The heart of the rule: a lease that has gone stale must not refuse the
/// call until the account has had its chance to refresh.
#[tokio::test]
async fn a_stale_lease_is_refreshed_before_the_call_is_refused() {
    let account = FakeAccount::new(
        vec![
            StateAnswer::Reads(denied(Denial::DeadlinePassed)),
            StateAnswer::Reads(ENTITLED),
        ],
        OnRefresh::Answer(Arc::new(|| {
            Ok(RefreshOutcome::Skipped(vault_account::SkipReason::LockBusy))
        })),
    );
    let verdict = check_over(account.clone(), 1_000).check().await;
    assert_eq!(verdict, Verdict::Entitled);
    assert_eq!(
        account.refreshes(),
        vec![Trigger::Denial],
        "a denied call refreshes as a denial (the account holds the rate limit)"
    );
    assert_eq!(
        account.reads(),
        2,
        "the answer must come from what is on disk after the refresh, not from the first read"
    );
}

#[tokio::test]
async fn the_answer_comes_from_disk_even_when_the_refresh_fails() {
    let account = FakeAccount::new(
        vec![
            StateAnswer::Reads(denied(Denial::OfflineTooLong)),
            StateAnswer::Reads(ENTITLED),
        ],
        OnRefresh::Answer(Arc::new(|| {
            Err(AccountError::Network("no route to host".into()))
        })),
    );
    assert_eq!(
        check_over(account.clone(), 1_000).check().await,
        Verdict::Entitled,
        "another process may have written a fresh lease; the disk decides"
    );
}

/// §8.27: "Account::refresh is not cancel-safe ... run it in its own task and
/// stop waiting, never drop it."
#[tokio::test(start_paused = true)]
async fn a_slow_refresh_is_left_running_and_the_call_answers_from_disk() {
    let account = FakeAccount::new(
        vec![StateAnswer::Reads(trial_ended())],
        OnRefresh::Slow(Duration::from_secs(30)),
    );
    let finished = account.refresh_finished.clone();
    let started = tokio::time::Instant::now();

    let verdict = check_over(account.clone(), 1_000).check().await;

    let waited = started.elapsed();
    assert_eq!(verdict, Verdict::Locked(LockReason::TrialEnded));
    assert!(
        waited <= super::REFRESH_DEADLINE + Duration::from_millis(100),
        "the call waited {waited:?} for a refresh"
    );
    assert!(
        !finished.load(Ordering::SeqCst),
        "the refresh should still be running when the call answers"
    );

    // Give the abandoned refresh its remaining time: it must finish, not die.
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(
        finished.load(Ordering::SeqCst),
        "the refresh was cancelled; a rotated token can be lost that way"
    );
}

// ---------------------------------------------------------------------------
// What a denial says (§8.27)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_trial_that_ended_says_the_trial_ended() {
    let account = FakeAccount::steady(trial_ended());
    assert_eq!(
        check_over(account, 1_000).check().await,
        Verdict::Locked(LockReason::TrialEnded)
    );
}

#[tokio::test]
async fn a_subscription_that_ended_says_the_subscription_ended() {
    let account = FakeAccount::steady(subscription_ended());
    assert_eq!(
        check_over(account, 1_000).check().await,
        Verdict::Locked(LockReason::SubscriptionEnded)
    );
}

/// Only a signed `ended` may say "ended". Everything else is "we could not
/// confirm", which is the honest answer and keeps a wrong clock or a dead
/// network from telling a paying user their trial is over.
#[tokio::test]
async fn every_other_denial_says_it_could_not_be_confirmed() {
    for denial in [
        Denial::DeadlinePassed,
        Denial::OfflineTooLong,
        Denial::ClockBehind,
    ] {
        let account = FakeAccount::steady(denied(denial));
        assert_eq!(
            check_over(account, 1_000).check().await,
            Verdict::Locked(LockReason::CannotConfirm),
            "{denial:?}"
        );
    }
}

#[tokio::test]
async fn a_lease_that_has_not_arrived_yet_says_it_could_not_be_confirmed() {
    let account = FakeAccount::steady(AccountState::NoLease);
    assert_eq!(
        check_over(account, 1_000).check().await,
        Verdict::Locked(LockReason::CannotConfirm)
    );
}

#[tokio::test]
async fn a_folder_that_cannot_be_read_never_says_ended() {
    let account = FakeAccount::new(
        vec![StateAnswer::Unreadable],
        OnRefresh::Answer(Arc::new(|| Err(AccountError::Busy))),
    );
    assert_eq!(
        check_over(account, 1_000).check().await,
        Verdict::Locked(LockReason::CannotConfirm)
    );
}

// ---------------------------------------------------------------------------
// Signed out
// ---------------------------------------------------------------------------

/// Nothing to refresh, so the call is answered at once: the relay's own
/// short-circuit (§6.3) relies on this being cheap.
#[tokio::test]
async fn a_computer_that_is_not_signed_in_is_told_to_sign_in_without_a_refresh() {
    let account = FakeAccount::steady(AccountState::SignedOut);
    let verdict = check_over(account.clone(), 1_000).check().await;
    assert_eq!(verdict, Verdict::Locked(LockReason::SignedOut));
    assert!(account.refreshes().is_empty());
    assert!(account.uses().is_empty());
}

/// A refresh can end the sign-in (the grant died, or 30 days unused): the
/// answer then comes from the cleared files.
#[tokio::test]
async fn a_refresh_that_signs_out_is_told_to_sign_in() {
    let account = FakeAccount::new(
        vec![
            StateAnswer::Reads(trial_ended()),
            StateAnswer::Reads(AccountState::SignedOut),
        ],
        OnRefresh::Answer(Arc::new(|| {
            Ok(RefreshOutcome::SignedOut(
                vault_account::SignOutReason::GrantEnded,
            ))
        })),
    );
    assert_eq!(
        check_over(account, 1_000).check().await,
        Verdict::Locked(LockReason::SignedOut)
    );
}

// ---------------------------------------------------------------------------
// The keeper's own question: `ModeCheck` (§8.26 §6.2, §8.35)
//
// The tick asks whether entitlement has FLIPPED, which is not the same
// question a call asks. It reads, and it must not refresh, must not write,
// and must not record a use: `record_use` feeds the 30-day unused rule, so a
// keeper that recorded one every few seconds would stop that rule ever
// firing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn peek_answers_from_disk_without_refreshing() {
    let account = FakeAccount::steady(trial_ended());
    let verdict = check_over(account.clone(), 1_000).peek().await;
    assert_eq!(verdict, Verdict::Locked(LockReason::TrialEnded));
    assert!(
        account.refreshes().is_empty(),
        "the tick must not reach the network"
    );
}

/// The one that matters: a tick is not a use of the vault. Before this test
/// existed, reusing the serving check for the tick would have recorded a use
/// every `idle_check` and quietly cancelled the 30-day unused sign-out.
#[tokio::test]
async fn peek_never_records_a_use_however_often_it_is_asked() {
    let account = FakeAccount::steady(ENTITLED);
    let check = check_over(account.clone(), 1_000);
    for _ in 0..20 {
        assert_eq!(check.peek().await, Verdict::Entitled);
    }
    assert!(
        account.uses().is_empty(),
        "a mode re-evaluation is not somebody using their vault"
    );
    assert!(account.refreshes().is_empty());
}

/// §8.27 and §8.33: only a signed `ended` may say "ended". A folder that
/// cannot be read must never tell a paying user their trial is over — on the
/// tick path just as on the call path.
#[tokio::test]
async fn peek_calls_an_unreadable_folder_could_not_confirm() {
    let account = FakeAccount::new(
        vec![StateAnswer::Unreadable],
        OnRefresh::Answer(Arc::new(|| {
            Ok(RefreshOutcome::Skipped(
                vault_account::SkipReason::RateLimited,
            ))
        })),
    );
    assert_eq!(
        check_over(account, 1_000).peek().await,
        Verdict::Locked(LockReason::CannotConfirm)
    );
}

/// The tick and a call must never disagree about what the same state means:
/// a keeper that changed mode on a reason a call would not refuse (or the
/// reverse) would flap. Checked over every state the account can report.
#[tokio::test]
async fn peek_and_a_served_call_agree_on_every_state() {
    let states = [
        ENTITLED,
        PAYMENT_FAILED,
        AccountState::SignedOut,
        AccountState::NoLease,
        trial_ended(),
        subscription_ended(),
        denied(Denial::DeadlinePassed),
        denied(Denial::OfflineTooLong),
        denied(Denial::ClockBehind),
    ];
    for state in states {
        let peeked = check_over(FakeAccount::steady(state), 1_000).peek().await;
        // A steady account answers the same after a refresh, so a served
        // call ends on the same verdict the tick read.
        let served = check_over(FakeAccount::steady(state), 1_000).check().await;
        assert_eq!(peeked, served, "{state:?} reads differently on the tick");
    }
}

/// §6.2: a full keeper that finds the user locked refreshes FIRST, because a
/// merely stale lease must not unload and reload 2.86 GB of models.
#[tokio::test]
async fn refresh_and_peek_refreshes_once_then_answers_from_disk() {
    let account = FakeAccount::new(
        // Locked on the first read, entitled once the refresh has landed.
        vec![
            StateAnswer::Reads(denied(Denial::DeadlinePassed)),
            StateAnswer::Reads(ENTITLED),
        ],
        OnRefresh::Answer(Arc::new(|| {
            Ok(RefreshOutcome::Skipped(
                vault_account::SkipReason::RateLimited,
            ))
        })),
    );
    // The keeper's real sequence: the tick reads a denial, and only then is a
    // refresh worth 5 s.
    let check = check_over(account.clone(), 1_000);
    assert_eq!(
        check.peek().await,
        Verdict::Locked(LockReason::CannotConfirm),
        "the tick reads the stale lease"
    );
    assert_eq!(
        check.refresh_and_peek().await,
        Verdict::Entitled,
        "a stale lease that a refresh fixes must not change the keeper's mode"
    );
    assert_eq!(
        account.refreshes(),
        vec![Trigger::Denial],
        "exactly one refresh, as a denial"
    );
}

/// A refresh that fails changes nothing: the answer still comes from disk,
/// and a network error never reports "ended" (§8.33).
#[tokio::test]
async fn refresh_and_peek_answers_from_disk_when_the_refresh_fails() {
    let account = FakeAccount::new(
        vec![StateAnswer::Reads(AccountState::NoLease)],
        OnRefresh::Answer(Arc::new(|| {
            Err(AccountError::Keychain("no network".into()))
        })),
    );
    assert_eq!(
        check_over(account.clone(), 1_000).refresh_and_peek().await,
        Verdict::Locked(LockReason::CannotConfirm)
    );
    assert_eq!(account.refreshes().len(), 1);
}

/// Deciding the keeper's mode is not somebody using their vault either, even
/// when the refresh restored entitlement.
#[tokio::test]
async fn refresh_and_peek_records_no_use() {
    let account = FakeAccount::new(
        vec![
            StateAnswer::Reads(denied(Denial::DeadlinePassed)),
            StateAnswer::Reads(ENTITLED),
        ],
        OnRefresh::Answer(Arc::new(|| {
            Ok(RefreshOutcome::Skipped(
                vault_account::SkipReason::RateLimited,
            ))
        })),
    );
    let _ = check_over(account.clone(), 1_000).refresh_and_peek().await;
    assert!(account.uses().is_empty());
}

// ---------------------------------------------------------------------------
// What a served call carries to the agent (§8.26 §6, §8.40)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_failed_payment_is_served_with_its_notice() {
    let account = FakeAccount::steady(PAYMENT_FAILED);
    assert_eq!(
        check_over(account, 1_000).check_with_notice().await,
        (Verdict::Entitled, Some(AccountNotice::PaymentFailed))
    );
}

/// "`health.warnings` in the last 5 days" (§8.26 §6): whole days left run 4
/// to 0, rounded down like the desktop's banner.
#[tokio::test]
async fn a_trial_in_its_last_five_days_is_served_with_the_days_left() {
    for (remaining, days_left) in [
        (TRIAL_NOTICE_WITHIN - 1, 4),
        (3 * DAY + HOUR, 3),
        (DAY, 1),
        (DAY - 1, 0),
        (1, 0),
    ] {
        let account = FakeAccount::steady(trial_with(remaining));
        assert_eq!(
            check_over(account, 1_000).check_with_notice().await,
            (
                Verdict::Entitled,
                Some(AccountNotice::TrialEnding { days_left })
            ),
            "{remaining}s left"
        );
    }
}

#[tokio::test]
async fn a_trial_with_five_days_or_more_left_carries_nothing() {
    for remaining in [TRIAL_NOTICE_WITHIN, TRIAL_NOTICE_WITHIN + 1, 20 * DAY] {
        let account = FakeAccount::steady(trial_with(remaining));
        assert_eq!(
            check_over(account, 1_000).check_with_notice().await,
            (Verdict::Entitled, None),
            "{remaining}s left"
        );
    }
}

/// A paid subscription near its renewal is not a trial ending, whatever its
/// countdown says.
#[tokio::test]
async fn a_paid_subscription_near_its_renewal_carries_nothing() {
    let account = FakeAccount::steady(leased(
        Entitlement::Entitled {
            payment_failed: false,
        },
        false,
        HOUR,
        2 * DAY,
    ));
    assert_eq!(
        check_over(account, 1_000).check_with_notice().await,
        (Verdict::Entitled, None)
    );
}

/// SP-4: a refusal says why and nothing else — however close a trial was to
/// its end, or whatever state the account is in.
#[tokio::test]
async fn a_refusal_never_carries_a_notice() {
    for state in [
        AccountState::SignedOut,
        AccountState::NoLease,
        trial_ended(),
        subscription_ended(),
        denied(Denial::DeadlinePassed),
        denied(Denial::OfflineTooLong),
        denied(Denial::ClockBehind),
    ] {
        let (verdict, notice) = check_over(FakeAccount::steady(state), 1_000)
            .check_with_notice()
            .await;
        assert_ne!(verdict, Verdict::Entitled, "{state:?}");
        assert_eq!(notice, None, "{state:?} carried a notice with a refusal");
    }
}

/// The notice, like the verdict, comes from the reading after
/// refresh-then-decide — not from the stale one that asked for the refresh.
#[tokio::test]
async fn the_notice_comes_from_the_reading_after_the_refresh() {
    let account = FakeAccount::new(
        vec![
            StateAnswer::Reads(denied(Denial::DeadlinePassed)),
            StateAnswer::Reads(trial_with(2 * DAY + HOUR)),
        ],
        OnRefresh::Answer(Arc::new(|| {
            Ok(RefreshOutcome::Skipped(vault_account::SkipReason::LockBusy))
        })),
    );
    assert_eq!(
        check_over(account, 1_000).check_with_notice().await,
        (
            Verdict::Entitled,
            Some(AccountNotice::TrialEnding { days_left: 2 })
        )
    );
}

/// The gate asks `check_with_notice` for a tool call and `check` for anything
/// else: both must reach the same verdict, and ask the account the same way.
#[tokio::test]
async fn check_and_check_with_notice_agree_on_every_state() {
    for state in [
        ENTITLED,
        PAYMENT_FAILED,
        trial_with(DAY),
        AccountState::SignedOut,
        AccountState::NoLease,
        trial_ended(),
        subscription_ended(),
        denied(Denial::OfflineTooLong),
    ] {
        let plain = FakeAccount::steady(state);
        let with_notice = FakeAccount::steady(state);
        let verdict = check_over(plain.clone(), 1_000).check().await;
        let (noticed, _) = check_over(with_notice.clone(), 1_000)
            .check_with_notice()
            .await;
        assert_eq!(verdict, noticed, "{state:?}");
        assert_eq!(plain.refreshes(), with_notice.refreshes(), "{state:?}");
        assert_eq!(plain.uses(), with_notice.uses(), "{state:?}");
    }
}

// ---------------------------------------------------------------------------
// The routine refresh at start and daily (§8.26 §4, §8.40) — the keeper's and
// the desktop's, in one place. These replace the desktop's own at-open tests
// from 4d-1 (planted bug d1-08 was the reason those became real tests).
// ---------------------------------------------------------------------------

/// Nothing to refresh for nobody.
#[test]
fn a_signed_out_computer_is_not_refreshed_at_start() {
    assert!(!wants_refresh_at_start(&AccountState::SignedOut));
}

/// A computer signed in without a lease (the first fetch failed at sign-in,
/// or the lease on disk did not verify) always tries: there is nothing on
/// disk to go on.
#[test]
fn a_computer_with_no_lease_always_tries_at_start() {
    assert!(wants_refresh_at_start(&AccountState::NoLease));
}

/// §4 refreshes at start only when stale, so an ordinary start does not
/// rotate the refresh token.
#[test]
fn a_fresh_lease_is_not_refreshed_at_start() {
    let ok = Entitlement::Entitled {
        payment_failed: false,
    };
    assert!(!wants_refresh_at_start(&leased(ok, false, HOUR, 20 * DAY)));
    assert!(
        !wants_refresh_at_start(&leased(ok, false, DAY - 1, 3 * DAY)),
        "just under a day old, exactly three days left: not yet stale"
    );
}

/// "Refresh if the lease is older than 24 h or any deadline is within
/// 3 days" (§8.26 §4).
#[test]
fn a_stale_lease_is_refreshed_at_start() {
    let ok = Entitlement::Entitled {
        payment_failed: false,
    };
    assert!(
        wants_refresh_at_start(&leased(ok, false, DAY, 20 * DAY)),
        "a day old"
    );
    assert!(
        wants_refresh_at_start(&leased(ok, true, HOUR, 3 * DAY - 1)),
        "a deadline within three days"
    );
}

#[tokio::test]
async fn a_stale_lease_gets_one_routine_refresh_and_no_use() {
    let ok = Entitlement::Entitled {
        payment_failed: false,
    };
    let account = FakeAccount::steady(leased(ok, false, 2 * DAY, 20 * DAY));
    let clock = FixedClock::at(1_000);
    assert!(refresh_if_stale(account.as_ref(), clock.as_ref()).await);
    assert_eq!(account.refreshes(), vec![Trigger::Routine]);
    assert!(
        account.uses().is_empty(),
        "a refresh is not somebody using their vault (§8.35)"
    );
}

#[tokio::test]
async fn a_fresh_lease_or_an_unreadable_folder_gets_no_routine_refresh() {
    let clock = FixedClock::at(1_000);
    let fresh = FakeAccount::steady(ENTITLED);
    assert!(!refresh_if_stale(fresh.as_ref(), clock.as_ref()).await);
    assert!(fresh.refreshes().is_empty());

    let unreadable = FakeAccount::new(
        vec![StateAnswer::Unreadable],
        OnRefresh::Answer(Arc::new(|| Err(AccountError::Busy))),
    );
    assert!(!refresh_if_stale(unreadable.as_ref(), clock.as_ref()).await);
    assert!(unreadable.refreshes().is_empty());
}

/// The routine as the keeper and the desktop run it: once at start when
/// stale, then on each daily tick — again only when stale. Tokio's paused
/// clock, so a day passes in no real time.
#[tokio::test(start_paused = true)]
async fn the_daily_tick_refreshes_a_stale_lease_and_leaves_a_fresh_one_alone() {
    let ok = Entitlement::Entitled {
        payment_failed: false,
    };
    let stale = FakeAccount::steady(leased(ok, false, 2 * DAY, 20 * DAY));
    let fresh = FakeAccount::steady(ENTITLED);
    let clock = FixedClock::at(1_760_000_000);
    let running = [
        tokio::spawn(run_routine_refresh(stale.clone(), clock.clone())),
        tokio::spawn(run_routine_refresh(fresh.clone(), clock.clone())),
    ];

    // Longer than the longest period (a day and six hours).
    tokio::time::sleep(Duration::from_secs(31 * 3_600)).await;
    for task in running {
        task.abort();
    }

    assert_eq!(
        stale.refreshes(),
        vec![Trigger::Routine, Trigger::Routine],
        "one refresh at start and one on the daily tick"
    );
    assert!(
        fresh.refreshes().is_empty(),
        "a fresh lease is refreshed neither at start nor on the tick"
    );
    assert!(fresh.reads() >= 2, "the tick must have looked");
}

/// The spread exists so every install does not ask the Worker at the same
/// moment forever (§8.26 §4's "jittered daily timer"). Whatever the clock
/// says, the period must stay inside the designed window: never shorter than
/// a day, never longer than a day and six hours. (Moved from the desktop's
/// tests with the function, 4d-3.)
#[test]
fn the_daily_refresh_period_stays_inside_its_window() {
    const DAY_SECS: u64 = 24 * 60 * 60;
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
            (DAY_SECS..=DAY_SECS + SPREAD).contains(&period),
            "a clock reading of {now} produced a {period}s period, outside the \
             {DAY_SECS}s..={}s window",
            DAY_SECS + SPREAD
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
