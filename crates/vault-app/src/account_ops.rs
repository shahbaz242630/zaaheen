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
//! # The signed-in address (ADR-SEC-027)
//!
//! §8.26 §3 asks for *"Signed in as &lt;email&gt; — not you? Sign out"*. The
//! address is stored beside the refresh token in the credential store at
//! sign-in (founder decision, 2026-09-21), so every view carries it, not only
//! the one returned by the sign-in itself. When it cannot be read the view
//! simply has no address: a label is never worth an error.

use std::sync::Arc;

use vault_account::{
    clock_looks_wrong, Account, AccountConfig, AccountError, CheckoutAnswer, LeaseState,
    ListenerLimits, PendingSignIn, Plan, RefreshOutcome, SignInOutcome, Status, Trigger,
};

/// Which page "Sign in" or "Create an account" opens first (§8.41).
pub use vault_account::SignInEntry;

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
    /// The signed-in address (ADR-SEC-027), or `None` when nobody is signed
    /// in or it could not be read.
    pub email: Option<String>,
    /// `signed_out`, `no_lease`, `trial`, `active`, `payment_failed`,
    /// `ended`, or `cannot_confirm`. A stable string, mapped to
    /// plain English in the desktop bundle.
    pub state: &'static str,
    /// Whole days of entitlement left, for the trial banner (§8.26 §6:
    /// the banner from day 23, `health.warnings` in the last 5 days).
    /// `None` when there is nothing to count down.
    pub days_left: Option<i64>,
    /// The lease on disk says this computer's clock was more than a day off
    /// the server's when it arrived (§8.26 §4): show "Your computer's clock
    /// is wrong. Set it to update automatically." Information only; it
    /// changes nothing about entitlement.
    pub clock_wrong: bool,
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
            Ok(status) => {
                let email = self.account.signed_in_email().await;
                Self::view_of(status, email)
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not read the account folder");
                Self::cannot_confirm()
            }
        }
    }

    /// The view when the account cannot be read at all: "could not confirm",
    /// never "ended" (§8.33), and never "signed out" either — telling
    /// somebody to sign in when the truth is "we could not look" would be the
    /// wrong message.
    #[must_use]
    pub fn cannot_confirm() -> AccountView {
        AccountView {
            signed_in: false,
            email: None,
            state: state::CANNOT_CONFIRM,
            days_left: None,
            clock_wrong: false,
        }
    }

    /// Open the browser, wait for the callback, and finish signing in.
    /// `entry` picks the first page: the sign-in, or the sign-up page that
    /// returns through the same sign-in (§8.41). Everything after the browser
    /// opens is identical.
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
    pub async fn sign_in(&self, entry: SignInEntry) -> Result<AccountView, OpsError> {
        let pending = PendingSignIn::start(&self.config).await?;
        ExternalLink::sign_in(&pending.browser_url(entry))?.open()?;

        match pending.wait(self.limits).await? {
            SignInOutcome::Cancelled => Err(OpsError::SignInDidNotFinish),
            SignInOutcome::Authorized(code) => {
                let now = self.clock.now();
                let signed_in = self.account.complete_sign_in(code, now).await?;
                // The address straight from the sign-in, rather than read back
                // from the store: a store that refused it must not leave the
                // panel blank on the one occasion the address is certainly
                // known (ADR-SEC-027's failure rule).
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

    /// At desktop open, refresh only when §8.26 §4 says to — *"refresh if
    /// the lease is older than 24 h or any deadline is within 3 days"* — so
    /// an ordinary open does not rotate the refresh token. Then once a day
    /// while the app runs, again only when the lease is stale. Never
    /// returns; the caller spawns it.
    ///
    /// Until session 50 the function this replaced had **no production
    /// caller** although §8.37 said it ran; and it refreshed on every open,
    /// which §4 does not ask for (§8.38 retracts both). Since 4d-3 it is the
    /// keeper's routine too, run from one place
    /// ([`crate::entitlement::run_routine_refresh`], §8.40), so the two
    /// processes cannot drift apart — and because each tick refreshes only a
    /// stale lease, the one that refreshes first leaves the other nothing to
    /// do.
    ///
    /// Holds only the account, never the vault.
    pub async fn refresh_at_open_then_daily(self: Arc<Self>) {
        crate::entitlement::run_routine_refresh(
            Arc::clone(&self.account) as Arc<dyn crate::entitlement::AccountAccess>,
            Arc::clone(&self.clock),
        )
        .await;
    }

    /// Map what is on disk to what the desktop shows.
    fn view_of(status: Status, email: Option<String>) -> AccountView {
        match status {
            Status::SignedOut => AccountView {
                signed_in: false,
                email: None,
                state: state::SIGNED_OUT,
                days_left: None,
                clock_wrong: false,
            },
            Status::NoLease { .. } => AccountView {
                signed_in: true,
                email,
                state: state::NO_LEASE,
                days_left: None,
                clock_wrong: false,
            },
            Status::Leased { lease, assessment } => AccountView {
                signed_in: true,
                email,
                state: state_for(lease.state()),
                days_left: Some(days_left(assessment.remaining)),
                clock_wrong: clock_looks_wrong(&lease),
            },
        }
    }
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
