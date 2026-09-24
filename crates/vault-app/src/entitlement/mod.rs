//! The entitlement check the MCP gate asks (ADR-104 + ADR-SEC-022;
//! `SIGNIN-DESIGN.md` §8.26 §4 and §6, and §8.27's contracts for S3).
//!
//! `vault-mcp` owns the gate and the fixed wording; `vault-account` owns the
//! sign-in, the lease and the local record. This module is the join: it is
//! the only place that knows both, which is what the composition root is for
//! (BRD §5.10).
//!
//! **Refresh-then-decide (§8.26 §4).** A call that the lease on disk would
//! refuse is not refused until the account has had a chance to refresh:
//! offline for a day, a lease that is merely stale, a subscription that was
//! paid a minute ago — all of those must serve the call. The refresh is given
//! [`REFRESH_DEADLINE`]; whatever the outcome, the answer then comes from
//! what is on disk.
//!
//! **A refresh is never dropped (§8.27).** `Account::refresh` is not
//! cancel-safe between the server rotating the refresh token and the store
//! writing it, so it runs in its own task and this code stops *waiting* for
//! it rather than cancelling it.
//!
//! **What a denial says (§8.27).** Only a signed `ended` lease says the trial
//! or the subscription has ended; every other denial says the subscription
//! could not be confirmed. A folder that cannot be read, a refresh that
//! failed, a lease that has not arrived yet: none of those may tell the user
//! their trial is over.
//!
//! **What a served call can carry (§8.40).** The last five days of a trial and
//! a failed payment are passed to the agent as an [`AccountNotice`], worked
//! out from the same reading that decided the call — never with a refusal.
//!
//! **The routine refresh (§8.26 §4, §8.40)** is here too, shared by the keeper
//! and the desktop: at start when the lease is stale, then a jittered daily
//! tick that also refreshes only a stale lease, so two processes on one
//! computer still ask about once a day.

mod lock_mode;
#[cfg(test)]
mod tests;
mod unreadable;

pub use lock_mode::{Flip, LockModeCheck};
pub use unreadable::{Rebuild, UnreadableAccount};

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use vault_account::{
    stale_at_start, AccountResult, Assessment, Denial, Entitlement, LeaseState, RefreshOutcome,
    Status, Trigger,
};
use vault_mcp::{AccountNotice, EntitlementCheck, LockReason, Verdict};

/// How long a denied call waits for a refresh before answering from disk
/// (§8.26 §4: "refresh first (<= 5 s), then answer").
pub const REFRESH_DEADLINE: Duration = Duration::from_secs(5);

/// One day in seconds (`vault-account` keeps its own private).
const DAY: i64 = 86_400;

/// A trial with less than this left is in its last days, and the agent is
/// told (§8.26 §6: "`health.warnings` in the last 5 days"). Whole days left
/// run 4 to 0: days 26–30 of 30, counted as the desktop counts its "from day
/// 23" banner.
pub const TRIAL_NOTICE_WITHIN: i64 = 5 * DAY;

/// The clock, injected so the tests can hold it still (no global state,
/// BRD §2.3).
pub trait Clock: Send + Sync {
    /// Seconds since the Unix epoch.
    fn now(&self) -> i64;
}

/// The system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }
}

/// What is on this computer, as the check needs it: [`Status`] without the
/// lease itself, which only [`vault_account`] can build. Keeping the lease
/// out is what lets the tests supply an account of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountState {
    /// Nobody is signed in on this computer.
    SignedOut,
    /// Signed in, but no verified lease yet.
    NoLease,
    /// Signed in with a lease: how it judges now (the verdict, and the time
    /// used and left), and whether it is a trial — what the reminders and the
    /// start-up refresh need (§8.40).
    Leased {
        /// The lease's verdict and times, at the moment of reading.
        assessment: Assessment,
        /// The lease says `trial`.
        trial: bool,
    },
}

impl From<Status> for AccountState {
    fn from(status: Status) -> Self {
        match status {
            Status::SignedOut => AccountState::SignedOut,
            Status::NoLease { .. } => AccountState::NoLease,
            Status::Leased { lease, assessment } => AccountState::Leased {
                assessment,
                trial: lease.state() == LeaseState::Trial,
            },
        }
    }
}

/// What the check needs from the account. Implemented by
/// [`vault_account::Account`]; the tests supply their own, so no test touches
/// Credential Manager or the network.
#[async_trait]
pub trait AccountAccess: Send + Sync + 'static {
    /// What is on disk, judged at `now`. No network, no writes.
    async fn state(&self, now: i64) -> AccountResult<AccountState>;
    /// Refresh the lease (and rotate the refresh token).
    async fn refresh(&self, trigger: Trigger, now: i64) -> AccountResult<RefreshOutcome>;
    /// Record that the vault was used (at most hourly, in server time).
    async fn record_use(&self, now: i64) -> AccountResult<()>;
}

#[async_trait]
impl AccountAccess for vault_account::Account {
    async fn state(&self, now: i64) -> AccountResult<AccountState> {
        vault_account::Account::status(self, now)
            .await
            .map(Into::into)
    }

    async fn refresh(&self, trigger: Trigger, now: i64) -> AccountResult<RefreshOutcome> {
        vault_account::Account::refresh(self, trigger, now).await
    }

    async fn record_use(&self, now: i64) -> AccountResult<()> {
        vault_account::Account::record_use(self, now).await
    }
}

/// What the keeper asks about its own mode, which is not the question a
/// request asks (§8.35).
///
/// A tick wants to know whether entitlement has **flipped**. It must not
/// refresh, must not write, and above all must not record a use: `record_use`
/// feeds `last_active_anchor`, and therefore the 30-day unused sign-out
/// (§8.26 §4). A keeper that recorded a use every `idle_check` would stop
/// that rule ever firing.
#[async_trait]
pub trait ModeCheck: Send + Sync + 'static {
    /// What the files say now: no network, no writes, no use recorded.
    async fn peek(&self) -> Verdict;

    /// One refresh (at most [`REFRESH_DEADLINE`]), then answer from disk
    /// whatever it did. Asked only by a full keeper that has just read a
    /// denial, so a merely stale lease does not unload and reload the models
    /// (§8.26 §6.2).
    async fn refresh_and_peek(&self) -> Verdict;
}

/// The real check: reads the account's own files, refreshes when a call would
/// otherwise be refused, and answers the gate.
pub struct AccountCheck {
    account: Arc<dyn AccountAccess>,
    clock: Arc<dyn Clock>,
    refresh_deadline: Duration,
}

impl AccountCheck {
    /// The shipped check.
    pub fn new(account: Arc<dyn AccountAccess>, clock: Arc<dyn Clock>) -> Self {
        Self {
            account,
            clock,
            refresh_deadline: REFRESH_DEADLINE,
        }
    }

    /// What the files on this computer say at `now`: the verdict, and what
    /// the agent should be told with it (§8.40). A folder that cannot be read
    /// is "could not confirm": it must never say a trial ended.
    async fn read(&self, now: i64) -> (Verdict, Option<AccountNotice>) {
        match self.account.state(now).await {
            Ok(state) => (verdict_for(state), notice_for(state)),
            Err(e) => {
                tracing::warn!(error = %e, "could not read the account folder");
                (Verdict::Locked(LockReason::CannotConfirm), None)
            }
        }
    }

    /// §8.26 §4's routine refresh for this account — at start when the lease
    /// is stale, then daily — which the keeper spawns on its full path
    /// (§8.40). Never returns. Holds only the account, and records no use.
    pub async fn refresh_at_start_then_daily(self: Arc<Self>) {
        run_routine_refresh(Arc::clone(&self.account), Arc::clone(&self.clock)).await;
    }

    /// Give a refresh [`Self::refresh_deadline`] to change the answer, then
    /// carry on whatever happened.
    ///
    /// The refresh runs in its own task and is never dropped: cancelling it
    /// between the server rotating the refresh token and the store writing it
    /// loses the live token (§8.27). Waiting stops; the work does not.
    async fn refresh_briefly(&self, now: i64) {
        let account = Arc::clone(&self.account);
        let refreshing = tokio::spawn(async move { account.refresh(Trigger::Denial, now).await });
        match tokio::time::timeout(self.refresh_deadline, refreshing).await {
            Ok(Ok(Ok(outcome))) => tracing::debug!(outcome = ?outcome, "refresh finished"),
            Ok(Ok(Err(e))) => tracing::info!(error = %e, "refresh failed; deciding from disk"),
            Ok(Err(e)) => tracing::warn!(error = %e, "the refresh task ended unexpectedly"),
            Err(_) => tracing::info!("refresh still running; deciding from disk"),
        }
    }

    /// Record that the vault was used. Best effort: the 30-day unused rule is
    /// not worth failing a call over.
    async fn note_use(&self, now: i64) {
        if let Err(e) = self.account.record_use(now).await {
            tracing::debug!(error = %e, "could not record this use of the vault");
        }
    }
}

#[async_trait]
impl EntitlementCheck for AccountCheck {
    async fn check(&self) -> Verdict {
        self.check_with_notice().await.0
    }

    async fn check_with_notice(&self) -> (Verdict, Option<AccountNotice>) {
        let now = self.clock.now();
        let (from_disk, notice) = self.read(now).await;
        if from_disk == Verdict::Entitled {
            self.note_use(now).await;
            return (from_disk, notice);
        }
        // Nobody is signed in: there is nothing a refresh could change, and
        // the relay's short-circuit (§6.3) counts on this being cheap.
        if from_disk == Verdict::Locked(LockReason::SignedOut) {
            return (from_disk, None);
        }

        // Refresh-then-decide (§8.26 §4): a stale lease, a day offline or a
        // subscription paid a minute ago must not refuse the call. The notice,
        // like the verdict, comes from the reading after the refresh.
        self.refresh_briefly(now).await;
        let now = self.clock.now();
        let (decided, notice) = self.read(now).await;
        if decided == Verdict::Entitled {
            self.note_use(now).await;
            return (decided, notice);
        }
        // SP-4: a refusal says why, and nothing else.
        (decided, None)
    }
}

#[async_trait]
impl ModeCheck for AccountCheck {
    async fn peek(&self) -> Verdict {
        // `read` is the whole of it: no refresh, and — unlike `check` — no
        // `note_use`. A tick is not somebody using their vault.
        self.read(self.clock.now()).await.0
    }

    async fn refresh_and_peek(&self) -> Verdict {
        self.refresh_briefly(self.clock.now()).await;
        // A fresh reading of the clock: the refresh may have taken 5 s.
        self.read(self.clock.now()).await.0
    }
}

/// Map what is on disk to the gate's answer (§8.27: only a signed `ended`
/// says "ended"; everything else says the subscription could not be
/// confirmed).
fn verdict_for(state: AccountState) -> Verdict {
    let assessment = match state {
        AccountState::SignedOut => return Verdict::Locked(LockReason::SignedOut),
        AccountState::NoLease => return Verdict::Locked(LockReason::CannotConfirm),
        AccountState::Leased { assessment, .. } => assessment,
    };
    match assessment.entitlement {
        Entitlement::Entitled { .. } => Verdict::Entitled,
        Entitlement::Denied(Denial::Ended { was_paid: true }) => {
            Verdict::Locked(LockReason::SubscriptionEnded)
        }
        Entitlement::Denied(Denial::Ended { was_paid: false }) => {
            Verdict::Locked(LockReason::TrialEnded)
        }
        Entitlement::Denied(Denial::DeadlinePassed)
        | Entitlement::Denied(Denial::OfflineTooLong)
        | Entitlement::Denied(Denial::ClockBehind) => Verdict::Locked(LockReason::CannotConfirm),
    }
}

/// What the agent is told with a served call (§8.26 §6, §8.40): a failed
/// payment, or a trial in its last five days. Nothing for a refusal, a paid
/// subscription in good standing, or a computer with no lease.
fn notice_for(state: AccountState) -> Option<AccountNotice> {
    let AccountState::Leased { assessment, trial } = state else {
        return None;
    };
    match assessment.entitlement {
        Entitlement::Entitled {
            payment_failed: true,
        } => Some(AccountNotice::PaymentFailed),
        Entitlement::Entitled {
            payment_failed: false,
        } if trial && assessment.remaining < TRIAL_NOTICE_WITHIN => {
            Some(AccountNotice::TrialEnding {
                days_left: whole_days(assessment.remaining),
            })
        }
        Entitlement::Entitled { .. } | Entitlement::Denied(_) => None,
    }
}

/// Seconds to whole days, rounded down (a part day is 0), never negative.
fn whole_days(seconds: i64) -> u32 {
    u32::try_from(seconds.max(0) / DAY).unwrap_or(u32::MAX)
}

/// §8.26 §4 at start (keeper start, desktop open): "refresh if the lease is
/// older than 24 h or any deadline is within 3 days". Nobody signed in has
/// nothing to refresh; a computer with no lease yet always tries (the first
/// fetch may have failed at sign-in).
fn wants_refresh_at_start(state: &AccountState) -> bool {
    match state {
        AccountState::SignedOut => false,
        AccountState::NoLease => true,
        AccountState::Leased { assessment, .. } => stale_at_start(assessment),
    }
}

/// A day, spread over a six-hour window so installs do not all ask the Worker
/// at the same moment forever (§8.26 §4's "jittered daily timer"). Moved here
/// from the desktop's account operations in 4d-3, so the keeper and the
/// desktop share it (§8.40).
///
/// `unsigned_abs` rather than `%` on a signed value: a computer whose clock is
/// set before 1970 gives a negative reading, and a negative remainder would
/// make the period *shorter* than a day — the one outcome that matters here,
/// because it would have that install asking repeatedly.
#[must_use]
fn daily_period(now: i64) -> Duration {
    const DAY_SECS: u64 = 24 * 60 * 60;
    const SPREAD: u64 = 6 * 60 * 60;
    let offset = now.unsigned_abs() % SPREAD;
    Duration::from_secs(DAY_SECS + offset)
}

/// One routine refresh, only when §4 asks for one (see
/// [`wants_refresh_at_start`]). Returns whether it refreshed.
///
/// Records no use: a refresh is not somebody using their vault (§8.35), and
/// recording one here would keep the 30-days-unused sign-out from ever firing.
/// Failures are logged at debug and swallowed — the lease on disk is still
/// good for up to 30 days offline.
async fn refresh_if_stale(account: &dyn AccountAccess, clock: &dyn Clock) -> bool {
    let now = clock.now();
    match account.state(now).await {
        Ok(state) if wants_refresh_at_start(&state) => {
            if let Err(e) = account.refresh(Trigger::Routine, now).await {
                tracing::debug!(error = %e, "a routine refresh did not finish");
            }
            true
        }
        Ok(_) => {
            tracing::debug!("the lease is fresh; no routine refresh");
            false
        }
        Err(e) => {
            tracing::debug!(error = %e, "could not read the account for a routine refresh");
            false
        }
    }
}

/// §8.26 §4's routine for one account: [`refresh_if_stale`] at start, then
/// once a day on a jittered timer, refreshing again only a stale lease. The
/// keeper (through [`AccountCheck::refresh_at_start_then_daily`]) and the
/// desktop (`AccountOps::refresh_at_open_then_daily`) both run this one
/// (§8.40). Never returns; the caller spawns it.
pub async fn run_routine_refresh(account: Arc<dyn AccountAccess>, clock: Arc<dyn Clock>) {
    refresh_if_stale(account.as_ref(), clock.as_ref()).await;

    let period = daily_period(clock.now());
    tracing::info!(
        hours = period.as_secs() / 3600,
        "the daily lease refresh is scheduled"
    );
    let mut timer = tokio::time::interval(period);
    // The first tick of an `interval` completes immediately, and the start
    // check above has already covered it.
    timer.tick().await;
    loop {
        timer.tick().await;
        refresh_if_stale(account.as_ref(), clock.as_ref()).await;
    }
}
