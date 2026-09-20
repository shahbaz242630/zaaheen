//! What the desktop asks the account to do (S3 step 4b).
//!
//! # The one structural rule
//!
//! **This type never holds the vault.** It has no `Application`, no adapter,
//! no key — only the account folder, the credential store, and the two
//! network clients. That is deliberate and it is the whole protection for
//! these commands.
//!
//! The account commands are on §8.26 §6.4's ungated allowlist, because
//! somebody whose trial has ended must still be able to sign in and
//! subscribe — a gate on those would lock a paying customer out of paying.
//! But "ungated" would be frightening if these commands *could* reach a
//! memory. They cannot: they are not given the thing that reads memories, so
//! serving vault data is not a mistake this code is able to make. The
//! compiler enforces it, and
//! `vault_tauri::guard::no_account_command_receives_the_vault` states it
//! where a future reader will see it.
//!
//! (Founder decision, 2026-09-20: *"Never give them the vault"*, chosen over
//! relying on the allowlist plus review.)
//!
//! # What is still missing, said plainly
//!
//! **The signed-in email is only known at sign-in.** `SignedIn` carries a
//! `UserInfo`; the account folder does not persist it (§8.26 §4 lists the
//! four files, and none of them is an identity). So on a later app open this
//! type knows the Clerk subject but not the address, and [`AccountView::email`]
//! is `None`.
//!
//! §8.26 §3 asks for *"Signed in as &lt;email&gt; — not you? Sign out"*, which
//! this satisfies immediately after signing in and not afterwards. Closing it
//! needs either a stored address (a new file, and a §8.26 §4 amendment) or a
//! `userinfo` call when the account panel opens (a network round trip, and a
//! fresh access token). **That is a step 4d decision, not something to
//! improvise here.**

use std::sync::Arc;

use vault_account::{
    Account, AccountConfig, AccountError, CheckoutAnswer, LeaseState, ListenerLimits,
    PendingSignIn, Plan, RefreshOutcome, SignInOutcome, Status, Trigger,
};

/// Re-exported so `vault-tauri` can name a plan without depending on
/// `vault-account` directly: the dependency graph is
/// vault-tauri -> vault-app -> vault-account (SIGNIN-DESIGN 8.26 2), and
/// one import is not a reason to add an edge to it.
pub use vault_account::Plan as SubscriptionPlan;

use crate::entitlement::Clock;
use crate::external_link::{ExternalLink, LinkError};

/// What the desktop shows about the account. Plain data: no token, no lease
/// bytes, no subject — nothing here is a secret, because all of it crosses
/// the Tauri IPC boundary (BRD §11.7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountView {
    /// Is anybody signed in on this computer?
    pub signed_in: bool,
    /// The address, when this process happens to know it — see the module
    /// docs. `None` is normal on a second app open, not an error.
    pub email: Option<String>,
    /// `signed_out`, `no_lease`, `trial`, `active`, `payment_failed`,
    /// `ended`, or `cannot_confirm`. A stable string, mapped to
    /// plain English in the desktop bundle.
    pub state: &'static str,
    /// Whole days of entitlement left, for the trial banner (§8.26 §6:
    /// the banner from day 23, `health.warnings` in the last 5 days).
    /// `None` when there is nothing to count down.
    pub days_left: Option<i64>,
}

/// The stable state strings. One place, so the desktop bundle and the tests
/// agree with each other.
pub mod state {
    pub const SIGNED_OUT: &str = "signed_out";
    pub const NO_LEASE: &str = "no_lease";
    pub const TRIAL: &str = "trial";
    pub const ACTIVE: &str = "active";
    pub const PAYMENT_FAILED: &str = "payment_failed";
    pub const ENDED: &str = "ended";
    pub const CANNOT_CONFIRM: &str = "cannot_confirm";

    /// Every state a view can carry, for the test that pins each one to a
    /// plain-English line in the desktop bundle.
    pub const ALL: &[&str] = &[
        SIGNED_OUT,
        NO_LEASE,
        TRIAL,
        ACTIVE,
        PAYMENT_FAILED,
        ENDED,
        CANNOT_CONFIRM,
    ];
}

/// Why an account operation could not finish. Opaque on purpose: the caller
/// turns it into one of a handful of fixed lines, and the detail goes to the
/// log (BRD §11.7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpsError {
    /// The browser did not come back, or the person closed it.
    SignInDidNotFinish,
    /// Another process holds the account folder.
    Busy,
    /// The network, or the account service, was not reachable.
    Unreachable,
    /// The account service answered something this app will not act on.
    Refused,
    /// A link could not be opened.
    Link(LinkError),
}

impl From<LinkError> for OpsError {
    fn from(e: LinkError) -> Self {
        OpsError::Link(e)
    }
}

impl From<AccountError> for OpsError {
    fn from(e: AccountError) -> Self {
        // Mapped by kind, never by message: an account-service string must
        // not become UI text.
        match e {
            AccountError::Busy => OpsError::Busy,
            AccountError::Network(_) => OpsError::Unreachable,
            _ => OpsError::Refused,
        }
    }
}

/// The account operations the desktop offers. **Holds no vault.**
pub struct AccountOps {
    account: Arc<Account>,
    config: AccountConfig,
    clock: Arc<dyn Clock>,
    limits: ListenerLimits,
}

impl AccountOps {
    /// All dependencies injected (BRD §2.3). Note what is absent: there is no
    /// `Application` parameter, and adding one would be the change that makes
    /// these commands dangerous.
    #[must_use]
    pub fn new(account: Arc<Account>, config: AccountConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            account,
            config,
            clock,
            limits: ListenerLimits::default(),
        }
    }

    /// What is on disk. No network, no writes.
    ///
    /// # Errors
    ///
    /// Never: a folder that cannot be read reads as `cannot_confirm`, the
    /// same honest answer §8.33 gives the keeper. Only a signed `ended` ever
    /// says ended.
    pub async fn status(&self) -> AccountView {
        let now = self.clock.now();
        match self.account.status(now).await {
            Ok(status) => Self::view_of(status, None),
            Err(e) => {
                tracing::warn!(error = %e, "could not read the account folder");
                AccountView {
                    signed_in: false,
                    email: None,
                    state: state::CANNOT_CONFIRM,
                    days_left: None,
                }
            }
        }
    }

    /// Open the browser, wait for the callback, and finish signing in.
    ///
    /// The listener uses [`ListenerLimits::default`] — the locked design's
    /// bounds (§8.26 §3), never a loosened set — and nothing about the
    /// callback (the code, the verifier, the redirect) is returned to the
    /// caller.
    ///
    /// # Errors
    ///
    /// [`OpsError`], all opaque.
    #[tracing::instrument(skip_all)]
    pub async fn sign_in(&self) -> Result<AccountView, OpsError> {
        let pending = PendingSignIn::start(&self.config).await?;
        ExternalLink::sign_in(pending.authorize_url())?.open()?;

        match pending.wait(self.limits).await? {
            SignInOutcome::Cancelled => Err(OpsError::SignInDidNotFinish),
            SignInOutcome::Authorized(code) => {
                let now = self.clock.now();
                let signed_in = self.account.complete_sign_in(code, now).await?;
                // The address is known exactly here and nowhere else; see the
                // module docs.
                let email = Some(signed_in.user.email.clone());
                let status = self.account.status(self.clock.now()).await?;
                Ok(Self::view_of(status, email))
            }
        }
    }

    /// Sign out: revoke, delete the token, clear the folder.
    ///
    /// # Errors
    ///
    /// [`OpsError::Busy`] if another process holds the folder.
    #[tracing::instrument(skip_all)]
    pub async fn sign_out(&self) -> Result<AccountView, OpsError> {
        self.account.sign_out().await?;
        Ok(self.status().await)
    }

    /// Subscribe, or manage an existing subscription.
    ///
    /// The call itself happens inside [`Account`], which owns the token
    /// lifecycle: an access token is never handed out here. What comes back
    /// is already validated, and what this does with it is open the link and
    /// return nothing -- so no URL crosses the IPC boundary in either
    /// direction.
    ///
    /// # Errors
    ///
    /// [`OpsError`], all opaque.
    #[tracing::instrument(skip_all)]
    pub async fn subscribe(&self, plan: Plan) -> Result<AccountView, OpsError> {
        match self.account.start_checkout(plan).await {
            Ok(answer) => {
                self.open_checkout(&answer)?;
                Ok(self.status().await)
            }
            Err(e) => {
                // `start_checkout` rotates the token, and a rotation can end
                // with this computer signed out (a dead grant, a missing
                // token) -- it clears the folder as a side effect. Returning
                // only the error would leave the panel showing the person as
                // signed in until something else happened to refresh it, so
                // the caller gets the *current* view either way. Found by
                // step 4b/4c's independent review.
                let view = self.status().await;
                tracing::info!(state = view.state, "a checkout attempt ended in an error");
                Err(e.into())
            }
        }
    }

    /// Open the link a checkout answer points at.
    ///
    /// Takes the **already-validated** answer rather than making the call
    /// itself, and returns nothing: the caller never receives a URL, so no
    /// URL can travel back through the IPC boundary to be opened by
    /// something else.
    ///
    /// # Errors
    ///
    /// [`OpsError::Link`] if the link fails the final `https`-without-
    /// credentials gate.
    #[tracing::instrument(skip_all)]
    pub fn open_checkout(&self, answer: &CheckoutAnswer) -> Result<(), OpsError> {
        let link = match answer {
            CheckoutAnswer::Checkout(txn) => ExternalLink::pay(txn)?,
            CheckoutAnswer::Portal(url) => ExternalLink::portal(url)?,
        };
        link.open()?;
        Ok(())
    }

    /// "I've paid" and the checkout poll: one refresh, then the fresh view.
    ///
    /// # Errors
    ///
    /// Never fails outright — a refresh that could not happen still yields
    /// whatever is on disk, because refusing to answer would look like a
    /// lost subscription.
    #[tracing::instrument(skip_all)]
    pub async fn refresh_now(&self) -> AccountView {
        let now = self.clock.now();
        match self.account.refresh(Trigger::UserAction, now).await {
            Ok(RefreshOutcome::SignedOut(reason)) => {
                tracing::info!(?reason, "the refresh signed this computer out");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "a user-requested refresh did not finish"),
        }
        self.status().await
    }

    /// The background refresh (§8.26 §4): keeper start, desktop open, and the
    /// daily jittered timer. Never triggered by tool activity.
    ///
    /// Failures are logged at debug and swallowed. A routine refresh is
    /// housekeeping — the lease on disk is still good for up to 30 days
    /// offline — so a missed one is not worth telling anybody about, and
    /// `Trigger::Routine` is itself rate-limited to one a minute.
    #[tracing::instrument(skip_all)]
    pub async fn routine_refresh(&self) {
        let now = self.clock.now();
        if let Err(e) = self.account.refresh(Trigger::Routine, now).await {
            tracing::debug!(error = %e, "a routine refresh did not finish");
        }
    }

    /// Refresh on open, then once a day while the app is running (§8.26 §4).
    ///
    /// Discharges the other half of step 3's inherited obligation: until now
    /// `Trigger::Routine` had **no production caller**, so a stale lease was
    /// only noticed when a call was refused — one five-second stall on
    /// somebody's first blocked action, which reads as the app hanging.
    ///
    /// # Why the interval is jittered
    ///
    /// §8.26 §4 says "a jittered daily timer", and the reason is the
    /// Worker's free daily quota: every copy of this app started on the same
    /// morning would otherwise ask at the same moment forever. The jitter is
    /// derived from the account folder's own state rather than a random
    /// number generator, so a given install is consistent across restarts
    /// and the spread across installs is even.
    ///
    /// Returns the task handle so the caller can drop it on shutdown; the
    /// task holds only the account, never the vault.
    pub fn spawn_routine_refresh(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // Desktop open: the first one happens immediately.
            self.routine_refresh().await;

            let period = daily_period(self.clock.now());
            tracing::info!(
                hours = period.as_secs() / 3600,
                "the daily lease refresh is scheduled"
            );
            let mut timer = tokio::time::interval(period);
            // The first tick of an `interval` completes immediately, and the
            // open-refresh above has already covered it.
            timer.tick().await;
            loop {
                timer.tick().await;
                self.routine_refresh().await;
            }
        })
    }

    /// Map what is on disk to what the desktop shows.
    fn view_of(status: Status, email: Option<String>) -> AccountView {
        match status {
            Status::SignedOut => AccountView {
                signed_in: false,
                email: None,
                state: state::SIGNED_OUT,
                days_left: None,
            },
            Status::NoLease { .. } => AccountView {
                signed_in: true,
                email,
                state: state::NO_LEASE,
                days_left: None,
            },
            Status::Leased { lease, assessment } => AccountView {
                signed_in: true,
                email,
                state: state_for(lease.state()),
                days_left: Some(days_left(assessment.remaining)),
            },
        }
    }
}

/// A day, spread over a six-hour window so installs do not all ask the Worker
/// at the same moment forever (§8.26 §4's "jittered daily timer").
///
/// A free function, not a method: building an `AccountOps` needs the
/// credential store and the network clients, and none of that has anything to
/// do with this arithmetic.
///
/// `unsigned_abs` rather than `%` on a signed value: a computer whose clock is
/// set before 1970 gives a negative reading, and a negative remainder would
/// make the period *shorter* than a day — the one outcome that matters here,
/// because it would have that install asking repeatedly.
#[must_use]
fn daily_period(now: i64) -> std::time::Duration {
    const DAY: u64 = 24 * 60 * 60;
    const SPREAD: u64 = 6 * 60 * 60;
    let offset = now.unsigned_abs() % SPREAD;
    std::time::Duration::from_secs(DAY + offset)
}

/// Seconds of entitlement to whole days.
///
/// Rounded **down**, so "1 day left" never shows while there are still 47
/// hours, and a part-day always reads as the smaller number. A negative
/// remaining (a denied lease) reads as 0 rather than a negative countdown.
///
/// Pulled out of `view_of` so it can be tested directly: building a
/// `Status::Leased` needs a verified `Lease`, which only `vault-account` can
/// make (`Lease::for_test` is `pub(crate)`), and widening that boundary just
/// to reach this arithmetic would be the wrong trade.
#[must_use]
fn days_left(remaining_seconds: i64) -> i64 {
    const DAY: i64 = 86_400;
    if remaining_seconds <= 0 {
        return 0;
    }
    remaining_seconds / DAY
}

/// The lease's state as the stable string the desktop reads.
#[must_use]
fn state_for(lease_state: LeaseState) -> &'static str {
    match lease_state {
        LeaseState::Trial => state::TRIAL,
        LeaseState::Active => state::ACTIVE,
        LeaseState::PaymentFailed => state::PAYMENT_FAILED,
        LeaseState::Ended => state::ENDED,
    }
}

#[cfg(test)]
mod tests;
