//! The account on this computer: sign-in completion, the one-at-a-time
//! refresh, sign-out, and the bookkeeping between them (§8.26 §3, §4, §7,
//! §15). This is what the keeper and the desktop use (S3); relays only read
//! the `signed-in` marker.
//!
//! # The refresh, and why it is shaped like this (§8.26 §15)
//!
//! Clerk rotates the refresh token on every use, and replaying a spent one
//! "kills the successor too — the whole grant dies" (S0 spike (d)). So:
//!
//! 1. Every refresh holds `refresh.lock`; a process that cannot take it
//!    never refreshes. It re-reads the lease after acquiring: if another
//!    process refreshed while it waited, it uses that lease and makes no
//!    call.
//! 2. The attempt is recorded before the network is touched, so the
//!    one-a-minute limit holds even across a crash.
//! 3. The new refresh token is stored, and the store confirms it, **before**
//!    anything else: before the lease is fetched and before it is written.
//! 4. On `invalid_grant` the store is re-read once and the call retried
//!    with what is there (another process may have rotated a moment
//!    earlier). Only a second refusal, or no token at all after a second
//!    look, signs out.
//! 5. A network error, a 5xx, a keychain error or a bad lease never
//!    discards the lease on disk or signs anyone out; only a signed state
//!    does (§8.26 §4).
//! 6. After a successful refresh, 30 days unused (server time) signs out and
//!    revokes the token just received.
//! 7. If the store refuses the rotated token for longer than a brief retry,
//!    the token is kept in memory: it is the only live one (the server has
//!    spent the stored one), so the next refresh stores it and uses it
//!    first, and sign-out revokes it. Otherwise a keychain blip would sign
//!    the user out one refresh later (found in review, §8.27).
//!
//! # Don't cancel a refresh midway
//!
//! Dropping the future between the server rotating the token and the store
//! writing it loses the new token (the §8.26 §15 residual). Callers that
//! want a deadline (refresh-then-decide, ≤ 5 s) run it in its own task and
//! stop *waiting* for it, rather than dropping it.

#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::checkout_client::{CheckoutAnswer, CheckoutClient, Plan};
use crate::entitlement::{
    assess, clock_looks_wrong, refresh_allowed, unused_too_long, Assessment, LocalState,
};
use crate::error::{AccountError, AccountResult};
use crate::files::{AccountDir, RefreshLock};
use crate::lease::{Lease, LeaseState, LeaseVerifier};
use crate::lease_client::LeaseClient;
use crate::oauth::{AccessToken, OAuthClient, RefreshToken, UserInfo};
use crate::signin::AuthorizedCode;
use crate::token_store::TokenStore;

/// How long to wait for `refresh.lock`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountTimings {
    /// Inside a call (refresh-then-decide, keeper start): §8.26 §4, ≤ 2 s.
    pub call_lock_wait: Duration,
    /// For something the user did (finishing sign-in, signing out, "I've
    /// paid"), which may wait out a refresh in progress.
    pub user_lock_wait: Duration,
}

impl Default for AccountTimings {
    fn default() -> Self {
        Self {
            call_lock_wait: Duration::from_secs(2),
            user_lock_wait: Duration::from_secs(10),
        }
    }
}

/// Why a refresh is being asked for; decides the rate limit (§8.26 §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A call was denied (refresh-then-decide): at most one a minute, hourly
    /// after a signed `ended`.
    Denial,
    /// Keeper start, desktop open, the daily timer: at most one a minute.
    Routine,
    /// Checkout polling, "I've paid": no local limit (the Worker has its own).
    UserAction,
}

/// What is on disk, read without the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// No `signed-in` marker: nobody is signed in on this computer.
    SignedOut,
    /// Signed in, but no valid lease yet (the first fetch failed, or the
    /// lease on disk did not verify). Refresh.
    NoLease {
        /// The signed-in user.
        sub: String,
    },
    /// Signed in with a verified lease.
    Leased {
        /// The lease.
        lease: Lease,
        /// Its verdict now.
        assessment: Assessment,
    },
}

/// How a refresh ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// A fresh verified lease is on disk (possibly written by another
    /// process while this one waited for the lock).
    Refreshed(Lease),
    /// This computer is now signed out; everything local is cleared.
    SignedOut(SignOutReason),
    /// Nothing was attempted; decide from what is on disk.
    Skipped(SkipReason),
}

/// What [`Account::rotate_under_lock`] produced. Private: an access token is
/// not something this crate hands out.
enum Rotated {
    /// A live access token, and the refresh token now in the store.
    Tokens {
        access: AccessToken,
        refresh: RefreshToken,
    },
    /// This computer is now signed out; everything local is cleared.
    SignedOut(SignOutReason),
}

/// Why a refresh signed the user out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignOutReason {
    /// The grant is dead: `invalid_grant` twice, the store re-read between.
    GrantEnded,
    /// The marker says signed in, but the store holds no token (twice).
    NoToken,
    /// Unused for more than 30 days, measured in server time.
    Unused,
}

/// Why a refresh was not attempted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// Another process held `refresh.lock` for the whole wait.
    LockBusy,
    /// Too soon after the last attempt.
    RateLimited,
    /// Nobody is signed in.
    NotSignedIn,
}

/// A completed sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    /// Who signed in ("Signed in as <email> — not you? Sign out").
    pub user: UserInfo,
    /// The first lease, when the Worker could be reached (else the keeper
    /// fetches it later).
    pub lease: Option<Lease>,
    /// Show "Your computer's clock is wrong" (information only).
    pub clock_looks_wrong: bool,
}

/// The account on this computer.
#[derive(Debug)]
pub struct Account {
    dir: AccountDir,
    store: TokenStore,
    oauth: OAuthClient,
    api: LeaseClient,
    verifier: LeaseVerifier,
    timings: AccountTimings,
    /// A rotated refresh token the credential store has not accepted yet
    /// (point 7 of the module docs). Newer than the stored one.
    unsaved: Mutex<Option<RefreshToken>>,
    /// Starts checkouts. `None` in the many tests that never subscribe, and
    /// set by [`Account::with_checkout`] in a real build.
    checkout: Option<CheckoutClient>,
}

impl Account {
    /// Assemble from its parts (all injected; no globals, BRD §2.3).
    pub fn new(
        dir: AccountDir,
        store: TokenStore,
        oauth: OAuthClient,
        api: LeaseClient,
        verifier: LeaseVerifier,
        timings: AccountTimings,
    ) -> Self {
        Self {
            dir,
            store,
            oauth,
            api,
            verifier,
            timings,
            unsaved: Mutex::new(None),
            checkout: None,
        }
    }

    /// Give this account the checkout client, so `/v1/checkout` is called
    /// from inside the type that owns the token lifecycle.
    ///
    /// Deliberately a builder rather than a seventh parameter to
    /// [`Account::new`]: the crate's existing tests do not subscribe, and a
    /// signature change would have edited a hundred call sites for nothing.
    #[must_use]
    pub fn with_checkout(mut self, checkout: CheckoutClient) -> Self {
        self.checkout = Some(checkout);
        self
    }

    /// What is on disk now, judged at `now`. No network, no writes.
    ///
    /// # Errors
    ///
    /// I/O failures reading the folder.
    #[tracing::instrument(skip_all)]
    pub async fn status(&self, now: i64) -> AccountResult<Status> {
        let dir = self.dir.clone();
        let verifier = self.verifier.clone();
        blocking(move || {
            let Some(sub) = dir.read_marker()? else {
                return Ok(Status::SignedOut);
            };
            let Some(lease) = read_verified(&dir, &verifier, &sub)? else {
                return Ok(Status::NoLease { sub });
            };
            let assessment = assess(&lease, &dir.read_state(), now);
            Ok(Status::Leased { lease, assessment })
        })
        .await
    }

    /// The address of whoever is signed in now, for "Signed in as <email> —
    /// not you? Sign out" (§8.26 §3, ADR-SEC-027). No network, no writes.
    ///
    /// `None` when nobody is signed in, when no address was stored, when the
    /// stored one belongs to somebody else, or when it cannot be read: a
    /// label is never worth an error, and never worth guessing.
    pub async fn signed_in_email(&self) -> Option<String> {
        let (dir, store) = (self.dir.clone(), self.store.clone());
        let read = blocking(move || {
            let Some(sub) = dir.read_marker()? else {
                return Ok(None);
            };
            store.load_address(&sub)
        })
        .await;
        read.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read the signed-in address");
            None
        })
    }

    /// Refresh the lease (and rotate the refresh token) under the rules in
    /// the module docs.
    ///
    /// # Errors
    ///
    /// Transient failures ([`AccountError::is_transient`]) and a rejected
    /// lease; in every error case the lease on disk and the sign-in are left
    /// as they were.
    #[tracing::instrument(skip_all, fields(trigger = ?trigger))]
    pub async fn refresh(&self, trigger: Trigger, now: i64) -> AccountResult<RefreshOutcome> {
        let wait = match trigger {
            Trigger::UserAction => self.timings.user_lock_wait,
            Trigger::Denial | Trigger::Routine => self.timings.call_lock_wait,
        };
        // Which lease was current when we were asked, before any waiting.
        let seen = self.current_issued_at().await?;
        let Some(lock) = self.lock(wait).await? else {
            return Ok(RefreshOutcome::Skipped(SkipReason::LockBusy));
        };

        // Under the lock, re-read everything (§8.26 §4, §15).
        let (dir, verifier) = (self.dir.clone(), self.verifier.clone());
        let (sub, disk_lease, state) = blocking(move || {
            let Some(sub) = dir.read_marker()? else {
                return Ok((None, None, LocalState::default()));
            };
            let lease = read_verified(&dir, &verifier, &sub)?;
            Ok((Some(sub), lease, dir.read_state()))
        })
        .await?;
        let Some(sub) = sub else {
            return Ok(RefreshOutcome::Skipped(SkipReason::NotSignedIn));
        };
        if let Some(lease) = disk_lease.as_ref() {
            let newer = match seen {
                None => true,
                Some(seen) => lease.issued_at() > seen,
            };
            if newer {
                tracing::debug!("another process refreshed while this one waited");
                return Ok(RefreshOutcome::Refreshed(lease.clone()));
            }
        }
        let after_ended = disk_lease
            .as_ref()
            .is_some_and(|l| l.state() == LeaseState::Ended);
        let allowed = match trigger {
            Trigger::Denial => refresh_allowed(&state, now, after_ended),
            Trigger::Routine => refresh_allowed(&state, now, false),
            Trigger::UserAction => true,
        };
        if !allowed {
            return Ok(RefreshOutcome::Skipped(SkipReason::RateLimited));
        }

        // Record the attempt before touching the network.
        let state = state.with_refresh_attempt(now);
        self.write_state(&lock, state).await?;

        // One path to a live access token, shared with `start_checkout` so
        // the rotation rules below exist exactly once.
        let (access, fresh) = match self.rotate_under_lock(&lock).await? {
            Rotated::SignedOut(reason) => return Ok(RefreshOutcome::SignedOut(reason)),
            Rotated::Tokens { access, refresh } => (access, refresh),
        };

        let wire = self.api.fetch(&access, now).await?;
        let lease = self.verifier.verify(&wire, &sub)?;
        if unused_too_long(&state, &lease) {
            tracing::info!("unused for more than 30 days; signing out");
            self.sign_out_locally(&lock).await;
            if let Err(e) = self.oauth.revoke(&fresh).await {
                tracing::warn!(error = %e, "revocation after the 30-day sign-out failed");
            }
            return Ok(RefreshOutcome::SignedOut(SignOutReason::Unused));
        }

        let (dir, written, lock_for_write) = (self.dir.clone(), lease.clone(), Arc::clone(&lock));
        blocking(move || {
            dir.write_lease(&lock_for_write, written.wire())?;
            dir.write_state(&lock_for_write, &state.on_new_lease(&written))?;
            Ok(())
        })
        .await?;
        tracing::info!(state = ?lease.state(), "lease refreshed");
        Ok(RefreshOutcome::Refreshed(lease))
    }

    /// Finish a browser sign-in: exchange the code, learn who signed in,
    /// store the refresh token, write the marker, start a fresh local record,
    /// and fetch the first lease (which starts the trial).
    ///
    /// # Errors
    ///
    /// Exchange, userinfo and keychain failures; [`AccountError::Busy`] if
    /// the folder stayed locked. A failed first lease fetch is not an error.
    pub async fn complete_sign_in(
        &self,
        code: AuthorizedCode,
        now: i64,
    ) -> AccountResult<SignedIn> {
        let (access, refresh) = self.oauth.exchange(code).await?.into_parts();
        let user = self.oauth.userinfo(&access).await?;
        let Some(lock) = self.lock(self.timings.user_lock_wait).await? else {
            return Err(AccountError::Busy);
        };

        // Token first, then the marker (§8.26 §4: the marker is written
        // "when a refresh token is stored"), then a clean slate. A new grant
        // supersedes any token an earlier refresh could not store.
        self.save_token(refresh).await?;
        self.set_unsaved(None);
        let (dir, sub, l) = (self.dir.clone(), user.sub.clone(), Arc::clone(&lock));
        blocking(move || {
            dir.write_marker(&l, &sub)?;
            dir.remove_lease(&l)?;
            dir.reset_state(&l, &LocalState::default())?;
            Ok(())
        })
        .await?;
        tracing::info!("signed in");

        // The address, so "Signed in as <email>" still has one on the next
        // app open (ADR-SEC-027). A label: a refused save is logged and never
        // fails the sign-in, which is the token, not the label.
        let (store, who) = (self.store.clone(), user.clone());
        if let Err(e) = blocking(move || store.save_address(&who)).await {
            tracing::warn!(error = %e, "the signed-in address was not stored");
        }

        let first = match self.api.fetch(&access, now).await {
            Ok(wire) => self.verifier.verify(&wire, &user.sub),
            Err(e) => Err(e),
        };
        let lease = match first {
            Ok(lease) => {
                let (dir, written, l) = (self.dir.clone(), lease.clone(), Arc::clone(&lock));
                blocking(move || {
                    dir.write_lease(&l, written.wire())?;
                    dir.reset_state(&l, &LocalState::default().on_new_lease(&written))?;
                    Ok(())
                })
                .await?;
                Some(lease)
            }
            Err(e) => {
                tracing::warn!(error = %e, "first lease not fetched; it will be retried");
                None
            }
        };
        Ok(SignedIn {
            clock_looks_wrong: lease.as_ref().is_some_and(clock_looks_wrong),
            user,
            lease,
        })
    }

    /// Sign out: clear the folder (marker first), delete the stored token,
    /// then revoke it at the server (best effort: works offline).
    ///
    /// # Errors
    ///
    /// [`AccountError::Busy`] if the folder stayed locked; folder I/O
    /// failures; a failed token delete (reported after the folder is
    /// cleared).
    #[tracing::instrument(skip_all)]
    pub async fn sign_out(&self) -> AccountResult<()> {
        let Some(lock) = self.lock(self.timings.user_lock_wait).await? else {
            return Err(AccountError::Busy);
        };
        // A token the store never accepted is the live one, so it is the
        // one to revoke.
        let unsaved = self.unsaved_token();
        self.set_unsaved(None);
        let (dir, store) = (self.dir.clone(), self.store.clone());
        let (stored, deleted) = blocking(move || {
            // Read the token for revocation, but never let a store failure
            // keep the folder from being cleared.
            let token = store.load().unwrap_or_else(|e| {
                tracing::warn!(error = %e, "could not read the token to revoke it");
                None
            });
            dir.clear(&lock)?;
            let deleted = store.delete();
            forget_address(&store);
            Ok((token, deleted))
        })
        .await?;
        tracing::info!("signed out");
        if let Some(token) = unsaved.or(stored) {
            if let Err(e) = self.oauth.revoke(&token).await {
                tracing::warn!(error = %e, "revocation failed; signed out on this computer");
            }
        }
        deleted
    }

    /// Start a subscription, or get the portal for one that already exists.
    ///
    /// # Why this lives on `Account` and not on the caller
    ///
    /// `/v1/checkout` needs a Bearer access token, and an access token is
    /// only ever produced by rotating the refresh token under `refresh.lock`
    /// ([`Account::rotate_under_lock`]). Handing that token out would mean a
    /// second way to obtain one -- either duplicating the rotation rules or
    /// bypassing them -- and both fail quietly, as a subscription that
    /// mysteriously signs somebody out. So the call happens here: the token
    /// is used and dropped without leaving the type that owns it.
    ///
    /// The answer is already validated by [`CheckoutClient`]: a
    /// [`TransactionId`](crate::TransactionId) matching the locked shape, or
    /// a [`PortalUrl`](crate::PortalUrl) on an allowed host.
    ///
    /// # Errors
    ///
    /// [`AccountError::InvalidConfig`] if this build has no checkout client;
    /// [`AccountError::Busy`] if the folder stayed locked; whatever the
    /// client reports otherwise.
    #[tracing::instrument(skip_all)]
    pub async fn start_checkout(&self, plan: Plan) -> AccountResult<CheckoutAnswer> {
        let Some(checkout) = self.checkout.as_ref() else {
            return Err(AccountError::InvalidConfig(
                "this build cannot start a checkout".into(),
            ));
        };
        // Subscribing is something the person just asked for, so it waits as
        // long as any other user action rather than giving up quickly.
        let Some(lock) = self.lock(self.timings.user_lock_wait).await? else {
            return Err(AccountError::Busy);
        };
        let dir = self.dir.clone();
        let signed_in = blocking(move || Ok(dir.read_marker()?)).await?;
        if signed_in.is_none() {
            return Err(AccountError::InvalidConfig(
                "nobody is signed in on this computer".into(),
            ));
        }

        let access = match self.rotate_under_lock(&lock).await? {
            Rotated::SignedOut(reason) => {
                tracing::info!(?reason, "checkout found this computer signed out");
                return Err(AccountError::InvalidGrant);
            }
            Rotated::Tokens { access, .. } => access,
        };
        checkout.start(&access, plan).await
    }

    /// Bookkeeping after use (a served call, the desktop opening): raise the
    /// floor (at most once a minute) and record activity in server time (at
    /// most hourly). Skipped silently when the folder is locked or there is
    /// no entitling lease.
    ///
    /// # Errors
    ///
    /// Folder I/O failures.
    pub async fn record_use(&self, now: i64) -> AccountResult<()> {
        let (dir, verifier) = (self.dir.clone(), self.verifier.clone());
        blocking(move || {
            let Some(lock) = dir.lock(Duration::ZERO)? else {
                return Ok(());
            };
            let Some(sub) = dir.read_marker()? else {
                return Ok(());
            };
            let Some(lease) = read_verified(&dir, &verifier, &sub)? else {
                return Ok(());
            };
            let state = dir.read_state();
            let assessment = assess(&lease, &state, now);
            let mut next = state;
            let raised = state.with_floor(&lease, now);
            if raised.lease_issued_at != state.lease_issued_at
                || raised.floor.saturating_sub(state.floor) >= FLOOR_WRITE_EVERY
            {
                next = raised;
            }
            if let Some(active) = next.with_activity(&lease, &assessment) {
                next = active;
            }
            if next != state {
                dir.write_state(&lock, &next)?;
            }
            Ok(())
        })
        .await
    }

    /// `issued_at` of the verified lease on disk, if any.
    async fn current_issued_at(&self) -> AccountResult<Option<i64>> {
        let (dir, verifier) = (self.dir.clone(), self.verifier.clone());
        blocking(move || {
            let Some(sub) = dir.read_marker()? else {
                return Ok(None);
            };
            Ok(read_verified(&dir, &verifier, &sub)?.map(|l| l.issued_at()))
        })
        .await
    }

    /// Take `refresh.lock` off the async runtime (the wait blocks).
    /// Rotate the refresh token under the lock and yield a live access
    /// token. **The one place an access token is produced.**
    ///
    /// Extracted from `refresh` in S3 step 4b so that `start_checkout` gets a
    /// token by the same rules rather than by a second, subtly different
    /// path. Those rules are not incidental:
    ///
    /// * a token the credential store refused earlier is the live one, so it
    ///   is stored again if the store has recovered and used either way;
    /// * `invalid_grant` is looked at **twice**, re-reading the store in
    ///   between, because another process may have rotated a moment ago
    ///   (§8.26 §15). Only the second refusal means "signed out";
    /// * the rotated token is **stored, and confirmed, before anything
    ///   else** -- a token that reached the provider but not the store would
    ///   otherwise be lost.
    ///
    /// The access token is returned, never persisted, and never logged.
    async fn rotate_under_lock(&self, lock: &Arc<RefreshLock>) -> AccountResult<Rotated> {
        // A token the store refused earlier is the live one: store it now if
        // the store has recovered, and use it either way.
        if let Some(pending) = self.unsaved_token() {
            match self.save_token(pending).await {
                Ok(_) => self.set_unsaved(None),
                Err(e) => {
                    tracing::warn!(error = %e, "the credential store still refuses the newer token")
                }
            }
        }
        let token = match self.unsaved_token() {
            Some(pending) => Some(pending),
            None => self.load_token_twice().await?,
        };
        let Some(token) = token else {
            self.sign_out_locally(lock).await;
            return Ok(Rotated::SignedOut(SignOutReason::NoToken));
        };
        let tokens = match self.oauth.refresh(&token).await {
            Ok(tokens) => tokens,
            Err(AccountError::InvalidGrant) => {
                // §8.26 §15: another process may have rotated a moment ago.
                let again = self.load_token().await?;
                let retried = match again {
                    Some(again) if again.expose() != token.expose() => {
                        self.oauth.refresh(&again).await
                    }
                    _ => Err(AccountError::InvalidGrant),
                };
                match retried {
                    Ok(tokens) => tokens,
                    Err(AccountError::InvalidGrant) => {
                        tracing::info!("refresh grant has ended; signing out locally");
                        self.sign_out_locally(lock).await;
                        return Ok(Rotated::SignedOut(SignOutReason::GrantEnded));
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };

        // The rotated token is stored, and confirmed, before anything else.
        let (access, fresh) = tokens.into_parts();
        let refresh = match self.save_token(fresh.clone()).await {
            Ok(saved) => {
                self.set_unsaved(None);
                saved
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "the credential store refused the rotated token; keeping it until it can be stored"
                );
                self.set_unsaved(Some(fresh));
                return Err(e);
            }
        };
        Ok(Rotated::Tokens { access, refresh })
    }

    async fn lock(&self, wait: Duration) -> AccountResult<Option<Arc<RefreshLock>>> {
        let dir = self.dir.clone();
        blocking(move || Ok(dir.lock(wait)?.map(Arc::new))).await
    }

    async fn write_state(&self, lock: &Arc<RefreshLock>, state: LocalState) -> AccountResult<()> {
        let (dir, l) = (self.dir.clone(), Arc::clone(lock));
        blocking(move || Ok(dir.write_state(&l, &state)?)).await
    }

    async fn load_token(&self) -> AccountResult<Option<RefreshToken>> {
        let store = self.store.clone();
        blocking(move || store.load()).await
    }

    /// Load the token; if there is none, look once more before concluding
    /// there really is none (§8.26 §15's second look).
    async fn load_token_twice(&self) -> AccountResult<Option<RefreshToken>> {
        match self.load_token().await? {
            Some(token) => Ok(Some(token)),
            None => self.load_token().await,
        }
    }

    /// Clear the folder and the stored token after the server ended the
    /// grant (or 30 days unused). Failures are logged: the grant is gone
    /// either way, and the marker is removed first.
    async fn sign_out_locally(&self, lock: &Arc<RefreshLock>) {
        self.set_unsaved(None);
        let (dir, store, l) = (self.dir.clone(), self.store.clone(), Arc::clone(lock));
        let cleared = blocking(move || {
            dir.clear(&l)?;
            forget_address(&store);
            store.delete()
        })
        .await;
        if let Err(e) = cleared {
            tracing::warn!(error = %e, "local sign-out was incomplete");
        }
    }

    /// Store a refresh token, retrying briefly: a credential-store blip right
    /// after a rotation must not strand the only live token. Returns the
    /// token, which the caller may still need (to revoke it).
    async fn save_token(&self, token: RefreshToken) -> AccountResult<RefreshToken> {
        let store = self.store.clone();
        blocking(move || {
            let mut attempt = 1;
            loop {
                match store.save(&token) {
                    Ok(()) => return Ok(token),
                    Err(e) if attempt >= SAVE_ATTEMPTS => return Err(e),
                    Err(_) => {
                        attempt += 1;
                        std::thread::sleep(SAVE_RETRY_DELAY);
                    }
                }
            }
        })
        .await
    }

    fn unsaved_token(&self) -> Option<RefreshToken> {
        self.unsaved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_unsaved(&self, token: Option<RefreshToken>) {
        *self.unsaved.lock().unwrap_or_else(PoisonError::into_inner) = token;
    }
}

/// Raise the persisted floor at most this often (§8.26 §4: `state.json`
/// "throttled to at most one write per minute").
const FLOOR_WRITE_EVERY: i64 = 60;

/// Attempts to store a rotated refresh token before keeping it in memory.
const SAVE_ATTEMPTS: u32 = 5;

/// Pause between those attempts (up to ~0.4 s in all).
const SAVE_RETRY_DELAY: Duration = Duration::from_millis(100);

/// The lease on disk, if it verifies for `sub`. A lease that does not verify
/// is treated as absent (and left for a refresh to replace).
fn read_verified(
    dir: &AccountDir,
    verifier: &LeaseVerifier,
    sub: &str,
) -> std::io::Result<Option<Lease>> {
    let Some(wire) = dir.read_lease()? else {
        return Ok(None);
    };
    match verifier.verify(&wire, sub) {
        Ok(lease) => Ok(Some(lease)),
        Err(e) => {
            tracing::warn!(error = %e, "the lease on disk does not verify");
            Ok(None)
        }
    }
}

/// Forget the signed-in address, on every path that clears the marker
/// (ADR-SEC-027). Logged, never fatal: the address is bound to the `sub`, so
/// one left behind by a failed delete can never be shown under anybody's
/// sign-in, and the next sign-in overwrites it.
fn forget_address(store: &TokenStore) {
    if let Err(e) = store.delete_address() {
        tracing::warn!(error = %e, "the signed-in address was not forgotten");
    }
}

/// Run blocking work (file locks, file I/O, the credential store) off the
/// async runtime (BRD §2.8).
async fn blocking<T, F>(work: F) -> AccountResult<T>
where
    F: FnOnce() -> AccountResult<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| AccountError::Io(std::io::Error::other(e.to_string())))?
}
