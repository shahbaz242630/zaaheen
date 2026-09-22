//! The OAuth client: authorization-code exchange and userinfo (§8.26 §3).
//!
//! # The contract with the account service
//!
//! Endpoints hang off the issuer (S0 spike): `/oauth/token`,
//! `/oauth/userinfo`. We are a **public client**: no secret, PKCE only, form
//! bodies (`application/x-www-form-urlencoded`), JSON answers.
//!
//! # What every call enforces
//!
//! - **HTTPS only**, TLS 1.2 floor (§8.26 §8, ADR-SEC-022 divergence 10), the
//!   default native-tls backend and the system proxy.
//! - **Redirects are never followed**, so a code, verifier or token is never
//!   re-sent to a host we did not choose. A redirect is a protocol error.
//! - **Bodies are capped** at [`MAX_BODY_BYTES`] and never logged.
//! - **Errors never quote the server** (see `error.rs`).
//! - The access token lives in memory only (§8.26 §3); the refresh token is
//!   handed to the token store (S1 step 2). Both are wiped on drop and never
//!   printed.

#[cfg(test)]
mod tests;

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use zeroize::Zeroizing;

use crate::config::AccountConfig;
use crate::error::{AccountError, AccountResult};
use crate::signin::AuthorizedCode;

/// Largest response body read from the account service.
pub(crate) const MAX_BODY_BYTES: usize = 64 * 1024;

/// Longest access token accepted. Clerk's is 36 characters (S0 spike (c)).
pub(crate) const MAX_TOKEN_LEN: usize = 4096;

/// Longest refresh token accepted. Clerk's is 48 characters (S0 spike (c)).
/// Tighter than [`MAX_TOKEN_LEN`] because the token must fit in Windows
/// Credential Manager, whose secret limit is 2,560 bytes of UTF-16 (1,280
/// characters): a token we could accept but never store would leave the user
/// signed in for one session and "transiently" failing forever after.
pub(crate) const MAX_REFRESH_TOKEN_LEN: usize = 1024;

/// Talks to the account service's OAuth endpoints.
pub struct OAuthClient {
    config: AccountConfig,
    http: reqwest::Client,
    timeout: Duration,
}

impl OAuthClient {
    /// Build a client for `config`.
    ///
    /// # Errors
    ///
    /// [`AccountError::InvalidConfig`] if the HTTP client cannot be built.
    pub fn new(config: AccountConfig) -> AccountResult<Self> {
        Ok(Self {
            config,
            http: https_http()?,
            timeout: REQUEST_TIMEOUT,
        })
    }

    /// Plain HTTP against a fake account service on this machine, with a 1 s
    /// request timeout. Never reachable outside `cfg(test)`.
    #[cfg(test)]
    pub(crate) fn for_loopback_test(config: AccountConfig) -> Self {
        Self {
            config,
            http: loopback_http(),
            timeout: Duration::from_secs(1),
        }
    }

    /// Exchange an authorization code (`grant_type=authorization_code`) for
    /// tokens. The code is consumed: it is single-use at the server too.
    ///
    /// # Errors
    ///
    /// [`AccountError::InvalidGrant`] if the server refuses the code;
    /// [`AccountError::Network`] for connection failures, timeouts, 429 and
    /// 5xx; [`AccountError::Protocol`] for anything else unexpected, including
    /// a missing refresh token.
    #[tracing::instrument(skip_all)]
    pub async fn exchange(&self, code: AuthorizedCode) -> AccountResult<TokenSet> {
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code.code.as_str()),
            ("redirect_uri", code.redirect_uri.as_str()),
            ("client_id", self.config.client_id()),
            ("code_verifier", code.verifier.as_str()),
        ];
        let response = self
            .http
            .post(self.config.endpoint("/oauth/token"))
            .form(&form)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(network)?;
        let status = response.status();
        let body = read_capped(response).await?;
        if !status.is_success() {
            tracing::info!(status = status.as_u16(), "token exchange refused");
            return Err(token_endpoint_error(status, &body));
        }
        parse_tokens(&body)
    }

    /// Trade a refresh token for new tokens (`grant_type=refresh_token`).
    /// Clerk **rotates** the refresh token on every use and treats reuse of a
    /// spent one as theft, killing the whole grant (§8.26 §15): the caller
    /// must hold `refresh.lock` and store the returned refresh token before
    /// doing anything else.
    ///
    /// # Errors
    ///
    /// As [`OAuthClient::exchange`]; [`AccountError::InvalidGrant`] means the
    /// token is spent, revoked or unknown.
    #[tracing::instrument(skip_all)]
    pub async fn refresh(&self, token: &RefreshToken) -> AccountResult<TokenSet> {
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", token.expose()),
            ("client_id", self.config.client_id()),
        ];
        let response = self
            .http
            .post(self.config.endpoint("/oauth/token"))
            .form(&form)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(network)?;
        let status = response.status();
        let body = read_capped(response).await?;
        if !status.is_success() {
            tracing::info!(status = status.as_u16(), "token refresh refused");
            return Err(token_endpoint_error(status, &body));
        }
        parse_tokens(&body)
    }

    /// Revoke a refresh token at the public revocation endpoint (S0 spike
    /// (f): no secret needed; the grant ends and its access token with it).
    ///
    /// # Errors
    ///
    /// [`AccountError::Network`] for transport failures, 429 and 5xx;
    /// [`AccountError::Protocol`] for other refusals.
    #[tracing::instrument(skip_all)]
    pub async fn revoke(&self, token: &RefreshToken) -> AccountResult<()> {
        let form = [
            ("token", token.expose()),
            ("token_type_hint", "refresh_token"),
            ("client_id", self.config.client_id()),
        ];
        let response = self
            .http
            .post(self.config.endpoint("/oauth/token/revoke"))
            .form(&form)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(network)?;
        let status = response.status();
        // Drain (capped) so the connection closes cleanly; the body is unused.
        let _ = read_capped(response).await?;
        if status.is_success() {
            return Ok(());
        }
        tracing::info!(status = status.as_u16(), "token revocation refused");
        Err(status_class(status).unwrap_or_else(|| {
            AccountError::Protocol(format!(
                "revocation refused with status {}",
                status.as_u16()
            ))
        }))
    }

    /// Who signed in, for "Signed in as <email> — not you? Sign out"
    /// (§8.26 §3).
    ///
    /// # Errors
    ///
    /// As [`OAuthClient::exchange`], except that a refused token is a
    /// [`AccountError::Protocol`] error.
    #[tracing::instrument(skip_all)]
    pub async fn userinfo(&self, access: &AccessToken) -> AccountResult<UserInfo> {
        let response = self
            .http
            .get(self.config.endpoint("/oauth/userinfo"))
            .bearer_auth(access.expose())
            .timeout(self.timeout)
            .send()
            .await
            .map_err(network)?;
        let status = response.status();
        let body = read_capped(response).await?;
        if !status.is_success() {
            tracing::info!(status = status.as_u16(), "userinfo refused");
            return Err(match status_class(status) {
                Some(transient) => transient,
                None => AccountError::Protocol(format!(
                    "userinfo refused with status {}",
                    status.as_u16()
                )),
            });
        }
        parse_userinfo(&body)
    }
}

/// How long to wait for a TCP + TLS connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long one request may take end to end.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest `sub` accepted.
const MAX_SUB_LEN: usize = 256;

/// Longest email accepted (RFC 5321's path limit, rounded up).
const MAX_EMAIL_LEN: usize = 320;

/// The production HTTP client for every account call (OAuth and the
/// Worker): HTTPS only, TLS 1.2 floor (§8.26 §8), no redirects, the default
/// native-tls backend and the system proxy.
pub(crate) fn https_http() -> AccountResult<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("Zaaheen/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AccountError::InvalidConfig(format!("http client: {}", e.without_url())))
}

/// A transport failure. `without_url` keeps the endpoint out of the text;
/// nothing else from the request is in a reqwest error's message.
pub(crate) fn network(e: reqwest::Error) -> AccountError {
    AccountError::Network(e.without_url().to_string())
}

/// Statuses that mean "not now" rather than "no": 429 and 5xx become
/// [`AccountError::Network`]; a redirect (never followed) is a protocol error.
pub(crate) fn status_class(status: reqwest::StatusCode) -> Option<AccountError> {
    if status.is_redirection() {
        return Some(AccountError::Protocol(
            "the account service answered with a redirect".into(),
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Some(AccountError::Network(format!(
            "account service status {}",
            status.as_u16()
        )));
    }
    None
}

/// Map a non-2xx token-endpoint answer. Only the OAuth `error` code is read,
/// and only to recognise `invalid_grant`; nothing the server wrote is quoted.
fn token_endpoint_error(status: reqwest::StatusCode, body: &[u8]) -> AccountError {
    if let Some(err) = status_class(status) {
        return err;
    }
    #[derive(Deserialize)]
    struct OAuthError {
        error: String,
    }
    match serde_json::from_slice::<OAuthError>(body) {
        Ok(e) if e.error == "invalid_grant" => AccountError::InvalidGrant,
        _ => AccountError::Protocol(format!(
            "token request refused with status {}",
            status.as_u16()
        )),
    }
}

/// Read the body, refusing anything over [`MAX_BODY_BYTES`]. The buffer is
/// wiped on drop: it may hold tokens.
pub(crate) async fn read_capped(
    mut response: reqwest::Response,
) -> AccountResult<Zeroizing<Vec<u8>>> {
    let too_large = || AccountError::Protocol("response body too large".into());
    let declared = response.content_length().unwrap_or(0);
    if declared > MAX_BODY_BYTES as u64 {
        return Err(too_large());
    }
    let mut body = Zeroizing::new(Vec::with_capacity(MAX_BODY_BYTES));
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// 1 to [`MAX_TOKEN_LEN`] visible ASCII characters.
fn is_valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= MAX_TOKEN_LEN && token.bytes().all(|b| b.is_ascii_graphic())
}

/// 1 to [`MAX_REFRESH_TOKEN_LEN`] visible ASCII characters: the rule for a
/// refresh token from the server and for one read back from the store.
fn is_valid_refresh_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_REFRESH_TOKEN_LEN
        && token.bytes().all(|b| b.is_ascii_graphic())
}

fn parse_tokens(body: &[u8]) -> AccountResult<TokenSet> {
    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: Option<String>,
        refresh_token: Option<String>,
        token_type: Option<String>,
        expires_in: Option<u64>,
    }
    let shape = || AccountError::Protocol("token response is not the expected JSON".into());
    let parsed: TokenResponse = serde_json::from_slice(body).map_err(|_| shape())?;
    let access = Zeroizing::new(parsed.access_token.ok_or_else(shape)?);
    let refresh = Zeroizing::new(
        parsed
            .refresh_token
            .ok_or_else(|| AccountError::Protocol("token response has no refresh token".into()))?,
    );
    let bearer = parsed
        .token_type
        .is_some_and(|t| t.eq_ignore_ascii_case("bearer"));
    if !bearer || !is_valid_token(&access) || !is_valid_refresh_token(&refresh) {
        return Err(AccountError::Protocol(
            "token response carries a malformed token".into(),
        ));
    }
    Ok(TokenSet {
        access: AccessToken(access),
        refresh: RefreshToken(refresh),
        expires_in: parsed.expires_in.map(Duration::from_secs),
    })
}

fn parse_userinfo(body: &[u8]) -> AccountResult<UserInfo> {
    #[derive(Deserialize)]
    struct UserInfoResponse {
        sub: Option<String>,
        email: Option<String>,
    }
    let bad = || AccountError::Protocol("userinfo response lacks a usable sub or email".into());
    let parsed: UserInfoResponse = serde_json::from_slice(body).map_err(|_| bad())?;
    let sub = parsed.sub.ok_or_else(bad)?;
    let email = parsed.email.ok_or_else(bad)?;
    UserInfo::checked(sub, email).ok_or_else(bad)
}

impl fmt::Debug for OAuthClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthClient")
            .field("issuer", &self.config.issuer())
            .finish_non_exhaustive()
    }
}

/// The tokens from a successful exchange.
pub struct TokenSet {
    access: AccessToken,
    refresh: RefreshToken,
    expires_in: Option<Duration>,
}

impl TokenSet {
    /// The access token, for userinfo and (S2) the account Worker.
    pub fn access_token(&self) -> &AccessToken {
        &self.access
    }

    /// The access token's lifetime, when the server stated one.
    pub fn expires_in(&self) -> Option<Duration> {
        self.expires_in
    }

    /// Split into the access token (kept in memory) and the refresh token
    /// (handed to the token store).
    pub fn into_parts(self) -> (AccessToken, RefreshToken) {
        (self.access, self.refresh)
    }
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSet")
            .field("access", &self.access)
            .field("refresh", &self.refresh)
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// An HTTP client for tests against fake services on 127.0.0.1: plain HTTP
/// allowed, redirects off, no proxy.
#[cfg(test)]
pub(crate) fn loopback_http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap()
}

/// An OAuth access token. Memory only; wiped on drop; never printed.
pub struct AccessToken(Zeroizing<String>);

impl AccessToken {
    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(Zeroizing::new(value.to_owned()))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessToken(<redacted>)")
    }
}

/// An OAuth refresh token. Wiped on drop (every clone too); never printed.
/// Only the token store and the refresh call read it.
#[derive(Clone)]
pub struct RefreshToken(Zeroizing<String>);

impl RefreshToken {
    /// Rebuild a token read back from the credential store, applying the same
    /// rules as a freshly issued one. `None` means the stored value is not a
    /// token we could have written.
    pub(crate) fn from_stored(stored: Zeroizing<String>) -> Option<Self> {
        is_valid_refresh_token(&stored).then(|| Self(stored))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RefreshToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshToken(<redacted>)")
    }
}

/// The signed-in user, as the account service reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserInfo {
    /// The account's stable user id (`sub`).
    pub sub: String,
    /// The account's email address, shown so the user can spot a wrong
    /// account at once.
    pub email: String,
}

impl UserInfo {
    /// `sub` and `email`, if both pass the checks applied to every identity
    /// this crate accepts: non-empty, bounded, `sub` printable ASCII, `email`
    /// free of control characters. One place, so the address read back from
    /// the credential store (ADR-SEC-027) is held to exactly the rules it
    /// passed on the way in.
    #[must_use]
    pub fn checked(sub: String, email: String) -> Option<Self> {
        let sub_ok = !sub.is_empty()
            && sub.len() <= MAX_SUB_LEN
            && sub.bytes().all(|b| b.is_ascii_graphic());
        let email_ok = !email.is_empty()
            && email.len() <= MAX_EMAIL_LEN
            && !email.chars().any(char::is_control);
        (sub_ok && email_ok).then_some(Self { sub, email })
    }
}
