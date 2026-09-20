#![forbid(unsafe_code)]
//! `vault-account` — the desktop app's Zaaheen account.
//!
//! Implements the client half of **ADR-104 + ADR-SEC-022** (`SIGNIN-DESIGN.md`
//! at the repo root: §8.26, locked 2026-09-17, and its amendment 1, §8.27).
//! Section references like "§8.26 §4" in this crate point there. §8.26 §2
//! gives this crate:
//! PKCE client + loopback listener, token store (own `CredentialStore`, Local
//! persistence), lease verify, entitlement state machine, activity timer,
//! refresh lock, revoke.
//!
//! # What it never touches
//!
//! The vault key. Sign-in proves who the user is and whether they have paid;
//! nothing here reads, derives or stores anything key-derived, so the account
//! service learns identity and billing state only (BRD §11.1, SP-1). The crate
//! depends on `vault-core` and nothing else in the workspace.
//!
//! # S1 build order
//!
//! 1. **Sign-in** — [`AccountConfig`], [`PendingSignIn`] (PKCE + the
//!    `127.0.0.1:0` loopback listener, §8.26 §3) and [`OAuthClient`] (code
//!    exchange, userinfo).
//! 2. **Token store** — [`TokenStore`]: the refresh token in Windows
//!    Credential Manager through this crate's own store, Local persistence.
//! 3. **Lease verify** — [`LeaseVerifier`]: the Worker's Ed25519-signed
//!    lease, checked on its exact bytes against two shipped keys.
//! 4. **The time model** — [`assess`] and [`LocalState`]: entitlement
//!    relative to the lease's signed `client_time`, the lease-keyed floor,
//!    activity in server time.
//! 5. **Refresh, sign-in completion, sign-out** — [`Account`] over
//!    [`AccountDir`] (the four files, `refresh.lock`), [`LeaseClient`] (the
//!    Worker's `/v1/lease`) and the refresh/revoke calls: one refresher under
//!    the lock, the new token stored first, `invalid_grant` re-read once
//!    before it can mean "signed out" (§8.26 §15).
//!
//! # Opening the browser
//!
//! This crate builds the authorization URL; the caller opens it. Keeping the
//! OS hand-off out of here keeps the crate free of UI dependencies, and the URL
//! it hands over is built entirely from local values (issuer, client id,
//! loopback redirect, fresh PKCE and `state`), never from anything a server
//! supplied.

mod account;
mod checkout_client;
mod config;
mod entitlement;
mod error;
mod files;
mod lease;
mod lease_client;
mod oauth;
mod pkce;
mod signin;
#[cfg(test)]
mod test_support;
mod token_store;

pub use account::{
    Account, AccountTimings, RefreshOutcome, SignOutReason, SignedIn, SkipReason, Status, Trigger,
};
pub use checkout_client::{CheckoutAnswer, CheckoutClient, Plan, PortalUrl, TransactionId};
pub use config::{AccountConfig, SCOPES};
pub use entitlement::{
    assess, clock_looks_wrong, refresh_allowed, stale_at_start, unused_too_long, Assessment,
    Denial, Entitlement, LocalState,
};
pub use error::{AccountError, AccountResult};
pub use files::{AccountDir, RefreshLock, LEASE_FILE, LOCK_FILE, MARKER_FILE, STATE_FILE};
pub use lease::{
    Lease, LeaseKey, LeaseState, LeaseVerifier, LEASE_DOMAIN, LEASE_VERSION, MAX_LEASE_BYTES,
    MAX_OFFLINE_DAYS,
};
pub use lease_client::LeaseClient;
pub use oauth::{AccessToken, OAuthClient, RefreshToken, TokenSet, UserInfo};
pub use signin::{AuthorizedCode, ListenerLimits, PendingSignIn, SignInOutcome};
pub use token_store::TokenStore;
