//! The signed entitlement lease (§8.26 §4): what the account Worker says
//! about this user, in a form the app can check offline.
//!
//! # Wire (§8.26 §4, quoted)
//!
//! > `b64url(payload) "." b64url(sig)`, `sig = Ed25519(sk,
//! > "zaaheen-lease-v1\0" || payload_bytes)`, verified on the exact bytes
//! > before parsing (dryoc).
//!
//! # What [`LeaseVerifier::verify`] enforces, in this order
//!
//! 1. The wire is at most [`MAX_LEASE_BYTES`] of strict base64url (no
//!    padding) in exactly two parts, and the signature is 64 bytes.
//! 2. The signature verifies over the domain prefix plus the **exact decoded
//!    payload bytes**, under one of the two shipped keys. Nothing is parsed
//!    before this; a lease that fails here is "bad signature" whatever its
//!    payload looks like.
//! 3. Only then is the payload parsed: `v` must be 1, `kid` must name the
//!    key that verified it, `sub` must be the signed-in user, `state` must
//!    be one of the four names, the state's own deadline must be present,
//!    and `offline_days` must be 1–366. Unknown fields are ignored (a newer
//!    Worker may add some); duplicated fields are refused.
//!
//! Whether a lease has **expired** is not decided here: it depends on the
//! clock rules of §8.26 §4 and is the time model's job (S1 step 4).
//!
//! # Keys
//!
//! The app ships two public keys from day one (§8.26 §4): `primary` (its
//! private half in Cloudflare Secrets) and `backup` (generated offline, kept
//! offline by the founder), so a lost or leaked primary can be rotated
//! without stranding anyone. The real keys arrive with S2; tests generate
//! their own.
//!
//! # A recorded property of dryoc 0.7.2
//!
//! Its Ed25519 verify reduces the signature's `S` modulo the group order
//! instead of rejecting a non-canonical `S`, as libsodium does. So a genuine
//! signature has a second valid encoding. That cannot forge or alter a lease
//! (the payload is still authenticated), and nothing here identifies a lease
//! by its signature bytes, so it is recorded rather than patched.

#[cfg(test)]
mod tests;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use dryoc::classic::crypto_sign::crypto_sign_verify_detached;
use serde::Deserialize;

use crate::error::{AccountError, AccountResult};

/// Domain-separation prefix signed ahead of the payload.
pub const LEASE_DOMAIN: &[u8] = b"zaaheen-lease-v1\0";

/// Largest lease accepted, wire form.
pub const MAX_LEASE_BYTES: usize = 4096;

/// The only payload version this app understands.
pub const LEASE_VERSION: u32 = 1;

/// Longest allowance a lease may grant for being offline (§8.26 §4 locks
/// 30 days; the bound only refuses nonsense).
pub const MAX_OFFLINE_DAYS: u32 = 366;

/// One public key the app trusts, with the `kid` leases signed by it carry.
#[derive(Clone, PartialEq, Eq)]
pub struct LeaseKey {
    kid: String,
    public: [u8; 32],
}

impl LeaseKey {
    /// `kid` must be 1–32 characters of `a-z 0-9 -`.
    ///
    /// # Errors
    ///
    /// [`AccountError::InvalidConfig`] for a malformed `kid`.
    pub fn new(kid: &str, public: [u8; 32]) -> AccountResult<Self> {
        let kid_ok = !kid.is_empty()
            && kid.len() <= MAX_KID_LEN
            && kid
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !kid_ok {
            return Err(AccountError::InvalidConfig(
                "lease key id must be 1-32 characters of a-z 0-9 -".into(),
            ));
        }
        Ok(Self {
            kid: kid.to_owned(),
            public,
        })
    }

    /// The key's id.
    pub fn kid(&self) -> &str {
        &self.kid
    }
}

impl std::fmt::Debug for LeaseKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeaseKey")
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

/// Verifies leases against the two shipped keys.
#[derive(Clone, Debug)]
pub struct LeaseVerifier {
    primary: LeaseKey,
    backup: LeaseKey,
}

impl LeaseVerifier {
    /// # Errors
    ///
    /// [`AccountError::InvalidConfig`] if the two keys share a `kid` or a
    /// public key.
    pub fn new(primary: LeaseKey, backup: LeaseKey) -> AccountResult<Self> {
        if primary.kid == backup.kid || primary.public == backup.public {
            return Err(AccountError::InvalidConfig(
                "the primary and backup lease keys must differ in id and key".into(),
            ));
        }
        Ok(Self { primary, backup })
    }

    /// Verify `wire` and return the lease it carries, which must be about
    /// `expected_sub` (the signed-in user).
    ///
    /// # Errors
    ///
    /// [`AccountError::LeaseRejected`] for anything that fails the rules in
    /// the module docs. Never transient: a rejected lease is discarded.
    pub fn verify(&self, wire: &[u8], expected_sub: &str) -> AccountResult<Lease> {
        // 1. Shape, before any decoding work.
        if wire.len() > MAX_LEASE_BYTES {
            return Err(rejected("lease too large"));
        }
        let mut parts = wire.split(|&b| b == b'.');
        let (Some(payload_b64), Some(sig_b64), None) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(rejected("lease is not two dot-separated parts"));
        };
        let payload = URL_SAFE_NO_PAD
            .decode(payload_b64)
            .ok()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| rejected("payload is not base64url"))?;
        let signature: [u8; 64] = URL_SAFE_NO_PAD
            .decode(sig_b64)
            .ok()
            .and_then(|s| <[u8; 64]>::try_from(s.as_slice()).ok())
            .ok_or_else(|| rejected("signature is not 64 bytes of base64url"))?;

        // 2. The signature, over the domain prefix and the exact payload bytes.
        let mut message = Vec::with_capacity(LEASE_DOMAIN.len() + payload.len());
        message.extend_from_slice(LEASE_DOMAIN);
        message.extend_from_slice(&payload);
        let signer = [&self.primary, &self.backup]
            .into_iter()
            .find(|key| crypto_sign_verify_detached(&signature, &message, &key.public).is_ok())
            .ok_or_else(|| rejected("bad signature"))?;

        // 3. Only now, the payload.
        let raw: RawPayload =
            serde_json::from_slice(&payload).map_err(|_| rejected("malformed payload"))?;
        if raw.v != LEASE_VERSION {
            return Err(rejected("unsupported lease version"));
        }
        if raw.kid != signer.kid {
            return Err(rejected("key id does not match the signing key"));
        }
        let sub_ok = !raw.sub.is_empty()
            && raw.sub.len() <= MAX_SUB_LEN
            && raw.sub.bytes().all(|b| b.is_ascii_graphic());
        if !sub_ok || raw.sub != expected_sub {
            return Err(rejected("lease is for a different user"));
        }
        if raw.issued_at <= 0 || raw.client_time <= 0 {
            return Err(rejected("lease times must be positive"));
        }
        if !(1..=MAX_OFFLINE_DAYS).contains(&raw.offline_days) {
            return Err(rejected("offline allowance out of range"));
        }
        let has_deadline = match raw.state {
            LeaseState::Trial => raw.trial_ends_at.is_some(),
            LeaseState::Active | LeaseState::PaymentFailed => raw.active_until.is_some(),
            LeaseState::Ended => true,
        };
        if !has_deadline {
            return Err(rejected("lease state is missing its deadline"));
        }
        Ok(Lease {
            wire: wire.to_vec(),
            kid: raw.kid,
            sub: raw.sub,
            state: raw.state,
            trial_ends_at: raw.trial_ends_at,
            active_until: raw.active_until,
            issued_at: raw.issued_at,
            client_time: raw.client_time,
            offline_days: raw.offline_days,
        })
    }
}

/// Longest key id.
const MAX_KID_LEN: usize = 32;

/// Longest `sub` accepted in a lease (as for userinfo).
const MAX_SUB_LEN: usize = 256;

/// A rejection. Reasons are fixed text: nothing from the lease is quoted.
fn rejected(why: &str) -> AccountError {
    AccountError::LeaseRejected(why.to_owned())
}

/// The payload as signed. Unknown fields are ignored (forward
/// compatibility); a duplicated known field fails the parse (serde's derive
/// refuses duplicates); a missing or `null` deadline is `None`.
#[derive(Deserialize)]
struct RawPayload {
    v: u32,
    kid: String,
    sub: String,
    state: LeaseState,
    trial_ends_at: Option<i64>,
    active_until: Option<i64>,
    issued_at: i64,
    client_time: i64,
    offline_days: u32,
}

/// The account's state as the Worker signed it (§8.26 §4). Wire names are
/// `trial`, `active`, `payment_failed`, `ended`, exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    /// In the 30-day free trial.
    Trial,
    /// Paying (or comped).
    Active,
    /// Paying, but the last charge failed; still entitled until `active_until`.
    PaymentFailed,
    /// Trial or subscription over.
    Ended,
}

/// A verified lease. Not secret: it is written to disk as-is (§8.26 §4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    wire: Vec<u8>,
    kid: String,
    sub: String,
    state: LeaseState,
    trial_ends_at: Option<i64>,
    active_until: Option<i64>,
    issued_at: i64,
    client_time: i64,
    offline_days: u32,
}

impl Lease {
    /// A lease with the given terms, skipping the signature, for tests of
    /// the time model. Never reachable outside `cfg(test)`.
    #[cfg(test)]
    pub(crate) fn for_test(
        state: LeaseState,
        issued_at: i64,
        client_time: i64,
        trial_ends_at: Option<i64>,
        active_until: Option<i64>,
    ) -> Self {
        Self {
            wire: Vec::new(),
            kid: "primary".into(),
            sub: "user_test".into(),
            state,
            trial_ends_at,
            active_until,
            issued_at,
            client_time,
            offline_days: 30,
        }
    }

    /// The exact bytes that verified, for the `lease` file.
    pub fn wire(&self) -> &[u8] {
        &self.wire
    }

    /// Which shipped key signed it.
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// The user it is about.
    pub fn sub(&self) -> &str {
        &self.sub
    }

    /// The signed state.
    pub fn state(&self) -> LeaseState {
        self.state
    }

    /// End of the trial, epoch seconds (always present when `state` is
    /// [`LeaseState::Trial`]).
    pub fn trial_ends_at(&self) -> Option<i64> {
        self.trial_ends_at
    }

    /// End of the paid period, epoch seconds (always present when `state` is
    /// [`LeaseState::Active`] or [`LeaseState::PaymentFailed`]).
    pub fn active_until(&self) -> Option<i64> {
        self.active_until
    }

    /// Server time at issue, epoch seconds.
    pub fn issued_at(&self) -> i64 {
        self.issued_at
    }

    /// The client's own clock reading, as the client sent it and the server
    /// signed it unclamped: the anchor for local elapsed time (§8.26 §4).
    pub fn client_time(&self) -> i64 {
        self.client_time
    }

    /// How many days this lease may be used without reaching the server.
    pub fn offline_days(&self) -> u32 {
        self.offline_days
    }
}
