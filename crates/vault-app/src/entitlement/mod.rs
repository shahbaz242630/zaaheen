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

mod lock_mode;
#[cfg(test)]
mod tests;

pub use lock_mode::{Flip, LockModeCheck};

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use vault_account::{AccountResult, Denial, Entitlement, RefreshOutcome, Status, Trigger};
use vault_mcp::{EntitlementCheck, LockReason, Verdict};

/// How long a denied call waits for a refresh before answering from disk
/// (§8.26 §4: "refresh first (<= 5 s), then answer").
pub const REFRESH_DEADLINE: Duration = Duration::from_secs(5);

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
    /// Signed in with a lease, and its verdict now.
    Leased(Entitlement),
}

impl From<Status> for AccountState {
    fn from(status: Status) -> Self {
        match status {
            Status::SignedOut => AccountState::SignedOut,
            Status::NoLease { .. } => AccountState::NoLease,
            Status::Leased { assessment, .. } => AccountState::Leased(assessment.entitlement),
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

    /// What the files on this computer say at `now`. A folder that cannot be
    /// read is "could not confirm": it must never say a trial ended.
    async fn read(&self, now: i64) -> Verdict {
        match self.account.state(now).await {
            Ok(state) => verdict_for(state),
            Err(e) => {
                tracing::warn!(error = %e, "could not read the account folder");
                Verdict::Locked(LockReason::CannotConfirm)
            }
        }
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
        let now = self.clock.now();
        let from_disk = self.read(now).await;
        if from_disk == Verdict::Entitled {
            self.note_use(now).await;
            return from_disk;
        }
        // Nobody is signed in: there is nothing a refresh could change, and
        // the relay's short-circuit (§6.3) counts on this being cheap.
        if from_disk == Verdict::Locked(LockReason::SignedOut) {
            return from_disk;
        }

        // Refresh-then-decide (§8.26 §4): a stale lease, a day offline or a
        // subscription paid a minute ago must not refuse the call.
        self.refresh_briefly(now).await;
        let now = self.clock.now();
        let decided = self.read(now).await;
        if decided == Verdict::Entitled {
            self.note_use(now).await;
        }
        decided
    }
}

#[async_trait]
impl ModeCheck for AccountCheck {
    async fn peek(&self) -> Verdict {
        // `read` is the whole of it: no refresh, and — unlike `check` — no
        // `note_use`. A tick is not somebody using their vault.
        self.read(self.clock.now()).await
    }

    async fn refresh_and_peek(&self) -> Verdict {
        self.refresh_briefly(self.clock.now()).await;
        // A fresh reading of the clock: the refresh may have taken 5 s.
        self.read(self.clock.now()).await
    }
}

/// Map what is on disk to the gate's answer (§8.27: only a signed `ended`
/// says "ended"; everything else says the subscription could not be
/// confirmed).
fn verdict_for(state: AccountState) -> Verdict {
    match state {
        AccountState::SignedOut => Verdict::Locked(LockReason::SignedOut),
        AccountState::NoLease => Verdict::Locked(LockReason::CannotConfirm),
        AccountState::Leased(Entitlement::Entitled { .. }) => Verdict::Entitled,
        AccountState::Leased(Entitlement::Denied(Denial::Ended { was_paid: true })) => {
            Verdict::Locked(LockReason::SubscriptionEnded)
        }
        AccountState::Leased(Entitlement::Denied(Denial::Ended { was_paid: false })) => {
            Verdict::Locked(LockReason::TrialEnded)
        }
        AccountState::Leased(Entitlement::Denied(Denial::DeadlinePassed))
        | AccountState::Leased(Entitlement::Denied(Denial::OfflineTooLong))
        | AccountState::Leased(Entitlement::Denied(Denial::ClockBehind)) => {
            Verdict::Locked(LockReason::CannotConfirm)
        }
    }
}
