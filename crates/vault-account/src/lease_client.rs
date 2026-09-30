//! The account Worker's lease endpoint (§8.26 §5), client side.
//!
//! # The contract S2 implements
//!
//! ```text
//! POST {api}/v1/lease
//! Authorization: Bearer <opaque access token>
//! Content-Type: application/json
//! {"client_now": <epoch seconds, this computer's clock>, "app_version": "<version>"}
//!
//! 200  {"lease": "<b64url(payload).b64url(sig)>"}
//! 401  the access token was refused
//! 429 / 5xx  try again later (§8.26 §5: an upstream error is 503, never a signed `ended`)
//! ```
//!
//! The first call for a user starts their trial (§8.26 §5). The client sends
//! its clock as it is; the Worker signs it back unclamped as `client_time`
//! (§8.26 §4). The returned wire is checked by [`crate::LeaseVerifier`]; this
//! module only fetches it, with the same transport rules as the OAuth client
//! (HTTPS only, no redirects, capped body, errors that never quote the
//! server).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::validate_origin;
use crate::error::{AccountError, AccountResult};
use crate::lease::MAX_LEASE_BYTES;
use crate::oauth::{https_http, network, read_capped, status_class, AccessToken};

/// Longest app version string sent.
const MAX_APP_VERSION_LEN: usize = 32;

/// Fetches leases from the account Worker.
#[derive(Debug)]
pub struct LeaseClient {
    endpoint: String,
    app_version: String,
    http: reqwest::Client,
    timeout: Duration,
}

impl LeaseClient {
    /// `api_origin` must be exactly `https://host[:port]` (the same rule as
    /// the issuer); `app_version` 1–32 characters of `0-9 A-Z a-z . + -`.
    ///
    /// # Errors
    ///
    /// [`AccountError::InvalidConfig`] for either.
    pub fn new(api_origin: &str, app_version: &str) -> AccountResult<Self> {
        validate_origin(api_origin, "account API origin")?;
        let version_ok = !app_version.is_empty()
            && app_version.len() <= MAX_APP_VERSION_LEN
            && app_version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'));
        if !version_ok {
            return Err(AccountError::InvalidConfig(
                "app version must be 1-32 characters of 0-9 A-Z a-z . + -".into(),
            ));
        }
        Ok(Self {
            endpoint: format!("{api_origin}/v1/lease"),
            app_version: app_version.to_owned(),
            http: https_http()?,
            timeout: REQUEST_TIMEOUT,
        })
    }

    /// Plain HTTP against a fake Worker on this machine, 1 s timeout.
    #[cfg(test)]
    pub(crate) fn for_loopback_test(port: u16) -> Self {
        Self {
            endpoint: format!("http://127.0.0.1:{port}/v1/lease"),
            app_version: "0.3.0".into(),
            http: crate::oauth::loopback_http(),
            timeout: Duration::from_secs(1),
        }
    }

    /// Ask for a lease, sending this computer's clock reading `client_now`.
    /// Returns the wire bytes, unverified, and the newest app version when
    /// the Worker named a plain release number (B6; anything else is `None`
    /// and never fails the lease).
    ///
    /// # Errors
    ///
    /// [`AccountError::Network`] for transport failures, 429 and 5xx;
    /// [`AccountError::Protocol`] for a refused token or a malformed answer.
    #[tracing::instrument(skip_all)]
    pub async fn fetch(&self, access: &AccessToken, client_now: i64) -> AccountResult<Fetched> {
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(access.expose())
            .json(&LeaseRequest {
                client_now,
                app_version: &self.app_version,
            })
            .timeout(self.timeout)
            .send()
            .await
            .map_err(network)?;
        let status = response.status();
        let body = read_capped(response).await?;
        if !status.is_success() {
            tracing::info!(status = status.as_u16(), "lease request refused");
            return Err(status_class(status).unwrap_or_else(|| {
                AccountError::Protocol(format!(
                    "lease request refused with status {}",
                    status.as_u16()
                ))
            }));
        }
        let malformed = || AccountError::Protocol("lease response is not the expected JSON".into());
        let parsed: LeaseResponse = serde_json::from_slice(&body).map_err(|_| malformed())?;
        if parsed.lease.is_empty() || parsed.lease.len() > MAX_LEASE_BYTES {
            return Err(malformed());
        }
        let latest_version = parsed
            .latest_version
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .filter(|v| crate::app_version::parse_release(v).is_some())
            .map(str::to_owned);
        Ok(Fetched {
            wire: parsed.lease.into_bytes(),
            latest_version,
        })
    }
}

/// A lease answer: the wire, unverified, and the newest app version.
#[derive(Debug)]
pub struct Fetched {
    /// The lease wire bytes, for [`crate::LeaseVerifier`].
    pub wire: Vec<u8>,
    /// The newest app version, a plain release number, when the Worker named
    /// one (B6).
    pub latest_version: Option<String>,
}

/// How long one lease request may take end to end.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Serialize)]
struct LeaseRequest<'a> {
    client_now: i64,
    app_version: &'a str,
}

#[derive(Deserialize)]
struct LeaseResponse {
    lease: String,
    /// Any JSON: a wrong type is ignored, never a failed lease.
    #[serde(default)]
    latest_version: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{json_response, FakeService};

    fn access() -> AccessToken {
        AccessToken::for_test("at_ACCESS_1234")
    }

    async fn fetch_with(status: u16, body: &str) -> AccountResult<Fetched> {
        let fake = FakeService::fixed(Some(json_response(status, body))).await;
        LeaseClient::for_loopback_test(fake.port)
            .fetch(&access(), 1_760_000_000)
            .await
    }

    #[tokio::test]
    async fn fetch_sends_the_designed_request() {
        let fake = FakeService::fixed(Some(json_response(200, r#"{"lease":"abc.def"}"#))).await;
        let wire = LeaseClient::for_loopback_test(fake.port)
            .fetch(&access(), 1_760_000_123)
            .await
            .unwrap();
        assert_eq!(wire.wire, b"abc.def");
        assert_eq!(wire.latest_version, None);
        let seen = fake.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].path, "/v1/lease");
        assert_eq!(
            seen[0].headers.get("authorization").map(String::as_str),
            Some("Bearer at_ACCESS_1234")
        );
        assert_eq!(
            seen[0].headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        let body: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"client_now": 1_760_000_123_i64, "app_version": "0.3.0"})
        );
    }

    /// B6: the newest version beside the lease is read only when it is a
    /// plain release number; anything else is ignored and the lease stands.
    #[tokio::test]
    async fn the_latest_version_is_read_only_when_plain_and_never_fails_the_lease() {
        let got = fetch_with(200, r#"{"lease":"abc.def","latest_version":"0.3.1"}"#)
            .await
            .unwrap();
        assert_eq!(got.latest_version.as_deref(), Some("0.3.1"));
        for extra in [
            r#""latest_version":5"#,
            r#""latest_version":"v0.3.1""#,
            r#""latest_version":"https://evil.test/""#,
            r#""latest_version":null"#,
            r#""latest_version":{"v":"0.3.1"}"#,
        ] {
            let body = format!(r#"{{"lease":"abc.def",{extra}}}"#);
            let got = fetch_with(200, &body).await.unwrap();
            assert_eq!(got.wire, b"abc.def", "{extra}");
            assert_eq!(got.latest_version, None, "{extra}");
        }
    }

    #[tokio::test]
    async fn busy_or_failing_workers_are_transient() {
        for status in [429, 500, 503] {
            let err = fetch_with(status, r#"{"error":"upstream"}"#)
                .await
                .unwrap_err();
            assert!(err.is_transient(), "{status}: {err:?}");
        }
    }

    #[tokio::test]
    async fn a_refused_token_is_not_transient() {
        let err = fetch_with(401, "{}").await.unwrap_err();
        assert!(matches!(err, AccountError::Protocol(_)), "{err:?}");
    }

    #[tokio::test]
    async fn malformed_answers_are_protocol_errors() {
        let huge = format!(r#"{{"lease":"{}"}}"#, "a".repeat(MAX_LEASE_BYTES + 1));
        for body in [
            "",
            "[]",
            "{}",
            r#"{"lease":5}"#,
            r#"{"lease":""}"#,
            huge.as_str(),
        ] {
            let err = fetch_with(200, body).await.unwrap_err();
            assert!(
                matches!(err, AccountError::Protocol(_)),
                "{body:.40}: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_redirect_is_never_followed() {
        let fake = FakeService::fixed(Some(
            b"HTTP/1.1 307 X\r\nLocation: http://127.0.0.1:1/v1/lease\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec(),
        ))
        .await;
        let err = LeaseClient::for_loopback_test(fake.port)
            .fetch(&access(), 1)
            .await
            .unwrap_err();
        assert!(matches!(err, AccountError::Protocol(_)), "{err:?}");
        assert_eq!(fake.seen().len(), 1);
    }

    #[test]
    fn the_origin_and_version_are_validated() {
        assert!(LeaseClient::new("https://api.zaaheen.com", "0.3.0").is_ok());
        assert!(LeaseClient::new("https://api.zaaheen.com", "1.2.3-beta+4").is_ok());
        for origin in [
            "http://api.zaaheen.com",
            "https://api.zaaheen.com/",
            "https://api.zaaheen.com/v1",
        ] {
            assert!(LeaseClient::new(origin, "0.3.0").is_err(), "{origin}");
        }
        let long = "1".repeat(MAX_APP_VERSION_LEN + 1);
        for version in ["", "0.3 beta", "0.3\n", long.as_str()] {
            assert!(
                LeaseClient::new("https://api.zaaheen.com", version).is_err(),
                "{version:?}"
            );
        }
    }

    #[test]
    fn the_access_token_never_reaches_debug_output() {
        let client = LeaseClient::for_loopback_test(1);
        assert!(!format!("{client:?} {:?}", access()).contains("at_ACCESS_1234"));
    }
}
