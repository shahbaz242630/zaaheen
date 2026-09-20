//! The entitlement check (`SIGNIN-DESIGN.md` §8.26 §4 / §6, §8.27).
//!
//! The account is a stand-in, so nothing here touches Credential Manager, the
//! network or the disk. Time is tokio's paused clock, so the deadline tests
//! are deterministic and take no real seconds.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use vault_account::{AccountError, AccountResult, Denial, Entitlement, RefreshOutcome, Trigger};
use vault_mcp::{EntitlementCheck, LockReason, Verdict};

use super::{AccountAccess, AccountCheck, AccountState, Clock};

const ENTITLED: AccountState = AccountState::Leased(Entitlement::Entitled {
    payment_failed: false,
});
const PAYMENT_FAILED: AccountState = AccountState::Leased(Entitlement::Entitled {
    payment_failed: true,
});

fn denied(denial: Denial) -> AccountState {
    AccountState::Leased(Entitlement::Denied(denial))
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
