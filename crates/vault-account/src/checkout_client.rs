//! The account Worker's checkout endpoint (§8.26 §5), client side.
//!
//! # The contract S2 deployed
//!
//! ```text
//! POST {api}/v1/checkout
//! Authorization: Bearer <opaque access token>
//! Content-Type: application/json
//! {"plan": "monthly" | "annual"}
//!
//! 200  {"kind":"checkout","txn":"txn_<26>"}   start a new subscription
//! 200  {"kind":"portal","url":"https://..."}  already subscribed: manage it
//! 400  the plan was not one of the two
//! 401  the access token was refused
//! 422  the account has no email to bill
//! 429 / 5xx  try again later
//! ```
//!
//! # Why this module validates so hard
//!
//! Both answers end up **opening something on the user's computer**, so both
//! are attacker-controlled input in the sense that matters: a compromised or
//! impersonated account service could answer with any string it liked. §8.26
//! §5 is explicit — *"The app validates `txn` (`^txn_[a-z0-9]{26}$`) and
//! builds `https://zaaheen.com/pay?_ptxn=…`; portal URLs must be `https` on
//! the allowlisted Paddle host. **Nothing from the server reaches the OS
//! unchecked.**"* — and §8.30: *"The portal link must be `https` on
//! `paddle.com` or one of its subdomains."*
//!
//! So neither value leaves this module as a bare `String`. A
//! [`TransactionId`] and a [`PortalUrl`] can only be built by the checks
//! below, which makes "opened something unvalidated" a thing the caller
//! cannot express (the same shape as `vault_tauri::guard::Entitled`).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::validate_origin;
use crate::error::{AccountError, AccountResult};
use crate::oauth::{https_http, network, read_capped, status_class, AccessToken};

/// How long one checkout request may take end to end. Longer than the lease
/// call: the Worker may make a live Paddle round trip before answering.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// `txn_` plus exactly 26 lowercase alphanumerics (§8.26 §5).
const TXN_PREFIX: &str = "txn_";
const TXN_BODY_LEN: usize = 26;

/// The one host family a portal link may point at (§8.30).
const PADDLE_HOST: &str = "paddle.com";

/// Which plan the person chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    Monthly,
    Annual,
}

impl Plan {
    fn wire(self) -> &'static str {
        match self {
            Plan::Monthly => "monthly",
            Plan::Annual => "annual",
        }
    }
}

/// A Paddle transaction id that has been checked against `^txn_[a-z0-9]{26}$`.
///
/// There is no other way to make one, so a caller cannot put an unchecked
/// string into a URL it then opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionId(String);

impl TransactionId {
    /// # Errors
    ///
    /// [`AccountError::Protocol`] if it is not exactly the locked shape.
    pub fn parse(raw: &str) -> AccountResult<Self> {
        let refused = || AccountError::Protocol("checkout transaction id is malformed".into());
        let body = raw.strip_prefix(TXN_PREFIX).ok_or_else(refused)?;
        // Byte length AND character count.
        //
        // The second half is **redundant today** and is kept on purpose. The
        // charset check below already forces every byte to be ASCII, and for
        // ASCII the two counts are equal — so no input can distinguish them
        // (proved by planted bug 4b05, which removed this half and changed
        // nothing). It is defence in depth (SP-3): it is what would still
        // bound the length if the charset check were ever loosened to admit a
        // non-ASCII character. Said plainly here so nobody mistakes it for a
        // tested guarantee.
        if body.len() != TXN_BODY_LEN || body.chars().count() != TXN_BODY_LEN {
            return Err(refused());
        }
        if !body
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err(refused());
        }
        Ok(Self(raw.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A customer-portal link checked to be `https` on `paddle.com` or a
/// subdomain of it (§8.30).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortalUrl(Url);

impl PortalUrl {
    /// # Errors
    ///
    /// [`AccountError::Protocol`] if it does not parse, is not `https`, or is
    /// not on the allowlisted host.
    pub fn parse(raw: &str) -> AccountResult<Self> {
        let refused = || AccountError::Protocol("portal link is not an allowed address".into());
        let url = Url::parse(raw).map_err(|_| refused())?;

        if url.scheme() != "https" {
            return Err(refused());
        }
        // Credentials in a link are never ours, and browsers treat them as
        // part of the authority -- a classic way to make a hostile URL read
        // as a trusted one.
        if !url.username().is_empty() || url.password().is_some() {
            return Err(refused());
        }

        let host = url.host_str().ok_or_else(refused)?.to_ascii_lowercase();
        // Exactly the host, or a subdomain of it. Written as two checks
        // rather than one `contains`, because `paddle.com.evil.test` and
        // `notpaddle.com` both contain the string and neither is ours.
        let allowed = host == PADDLE_HOST || host.ends_with(&format!(".{PADDLE_HOST}"));
        if !allowed {
            return Err(refused());
        }
        Ok(Self(url))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// What the Worker answered, with both values already checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckoutAnswer {
    /// Nobody is subscribed yet: open the pay page for this transaction.
    Checkout(TransactionId),
    /// Already subscribed: open the billing portal instead.
    Portal(PortalUrl),
}

/// Starts checkouts against the account Worker.
#[derive(Debug)]
pub struct CheckoutClient {
    endpoint: String,
    http: reqwest::Client,
    timeout: Duration,
}

impl CheckoutClient {
    /// `api_origin` must be exactly `https://host[:port]`.
    ///
    /// # Errors
    ///
    /// [`AccountError::InvalidConfig`] if it is not.
    pub fn new(api_origin: &str) -> AccountResult<Self> {
        validate_origin(api_origin, "account API origin")?;
        Ok(Self {
            endpoint: format!("{api_origin}/v1/checkout"),
            http: https_http()?,
            timeout: REQUEST_TIMEOUT,
        })
    }

    /// Plain HTTP against a fake Worker on this machine, 1 s timeout.
    #[cfg(test)]
    pub(crate) fn for_loopback_test(port: u16) -> Self {
        Self {
            endpoint: format!("http://127.0.0.1:{port}/v1/checkout"),
            http: crate::oauth::loopback_http(),
            timeout: Duration::from_secs(1),
        }
    }

    /// Ask for a checkout transaction, or the portal if already subscribed.
    ///
    /// # Errors
    ///
    /// [`AccountError::Network`] for transport failures, 429 and 5xx (worth
    /// retrying); [`AccountError::Protocol`] for a refused token, an answer
    /// that is not one of the two shapes, or a value that fails its check.
    #[tracing::instrument(skip_all, fields(plan = plan.wire()))]
    pub async fn start(&self, access: &AccessToken, plan: Plan) -> AccountResult<CheckoutAnswer> {
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(access.expose())
            .json(&CheckoutRequest { plan: plan.wire() })
            .timeout(self.timeout)
            .send()
            .await
            .map_err(network)?;
        let status = response.status();
        let body = read_capped(response).await?;
        if !status.is_success() {
            // The status only; the server's own words are never quoted back
            // (BRD §11.7.2).
            tracing::info!(status = status.as_u16(), "checkout request refused");
            return Err(status_class(status).unwrap_or_else(|| {
                AccountError::Protocol(format!(
                    "checkout request refused with status {}",
                    status.as_u16()
                ))
            }));
        }

        let malformed =
            || AccountError::Protocol("checkout response is not the expected JSON".into());
        let parsed: CheckoutResponse = serde_json::from_slice(&body).map_err(|_| malformed())?;
        match parsed {
            CheckoutResponse::Checkout { txn } => {
                Ok(CheckoutAnswer::Checkout(TransactionId::parse(&txn)?))
            }
            CheckoutResponse::Portal { url } => Ok(CheckoutAnswer::Portal(PortalUrl::parse(&url)?)),
        }
    }
}

#[derive(Serialize)]
struct CheckoutRequest<'a> {
    plan: &'a str,
}

/// `#[serde(deny_unknown_fields)]` is deliberate: an answer carrying both a
/// `txn` and a `url`, or an unexpected extra, is refused rather than
/// half-understood.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum CheckoutResponse {
    Checkout { txn: String },
    Portal { url: String },
}

#[cfg(test)]
mod tests;
