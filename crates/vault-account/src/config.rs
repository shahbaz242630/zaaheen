//! The compiled-in account configuration: which issuer to trust and which
//! OAuth client we are.
//!
//! Both values are public (they appear in every authorization URL), but the
//! issuer is also the value the callback's `iss` must equal **by exact string
//! comparison** (RFC 9207, §8.26 §3). So the issuer is held in one canonical
//! form, `https://host[:port]` with no trailing slash, and anything else is
//! refused at construction rather than normalised: a config that differs from
//! the canonical form by one character would otherwise reject every sign-in.

use url::Url;

use crate::error::{AccountError, AccountResult};

/// OAuth scopes the desktop app asks for (§8.26 §2). `offline_access` is what
/// makes Clerk return a refresh token (S0 spike (c)).
pub const SCOPES: &str = "openid profile email offline_access";

/// Longest client id accepted.
const MAX_CLIENT_ID_LEN: usize = 128;

/// Issuer and OAuth client id, validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountConfig {
    issuer: String,
    client_id: String,
}

impl AccountConfig {
    /// Validate and build.
    ///
    /// - `issuer` must be exactly `https://host` or `https://host:port`: no
    ///   path, trailing slash, query, fragment or user info, host in lower
    ///   case (the canonical form a URL parser would print).
    /// - `client_id` must be 1–128 characters of `A-Z a-z 0-9 _ -`.
    ///
    /// # Errors
    ///
    /// [`AccountError::InvalidConfig`] naming the field that failed.
    pub fn new(issuer: &str, client_id: &str) -> AccountResult<Self> {
        validate_origin(issuer, "issuer")?;
        let client_id_ok = !client_id.is_empty()
            && client_id.len() <= MAX_CLIENT_ID_LEN
            && client_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !client_id_ok {
            return Err(invalid(
                "client id must be 1-128 characters of A-Z a-z 0-9 _ -",
            ));
        }
        Ok(Self {
            issuer: issuer.to_owned(),
            client_id: client_id.to_owned(),
        })
    }

    /// The issuer, in canonical form.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// The OAuth client id.
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// `issuer + path`, e.g. `endpoint("/oauth/token")`. Clerk's endpoint
    /// paths are fixed under the issuer (S0 spike, discovery document).
    pub(crate) fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.issuer, path)
    }

    /// A plain-HTTP loopback issuer, for tests that run a fake account
    /// service on this machine. Never reachable outside `cfg(test)`.
    #[cfg(test)]
    pub(crate) fn for_loopback_test(port: u16) -> Self {
        Self {
            issuer: format!("http://127.0.0.1:{port}"),
            client_id: "client_test".into(),
        }
    }
}

fn invalid(reason: &str) -> AccountError {
    AccountError::InvalidConfig(reason.to_owned())
}

/// Accept only the canonical `https://host[:port]` form: parse, rebuild the
/// canonical string, and require the input to be exactly that string. Used
/// for the issuer and for the account Worker's origin; `what` names which.
pub(crate) fn validate_origin(origin: &str, what: &str) -> AccountResult<()> {
    let refuse = || {
        AccountError::InvalidConfig(format!(
            "{what} must be exactly https://host or https://host:port"
        ))
    };
    let parsed = Url::parse(origin).map_err(|_| refuse())?;
    if parsed.scheme() != "https" {
        return Err(refuse());
    }
    let host = parsed.host_str().ok_or_else(refuse)?;
    let bare = parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.path() == "/"
        && parsed.query().is_none()
        && parsed.fragment().is_none();
    let canonical = match parsed.port() {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    };
    if !bare || canonical != origin {
        return Err(refuse());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejects(issuer: &str) {
        match AccountConfig::new(issuer, "client_abc") {
            Err(AccountError::InvalidConfig(_)) => {}
            other => panic!("issuer {issuer:?} should be rejected, got {other:?}"),
        }
    }

    #[test]
    fn accepts_canonical_https_issuers() {
        for issuer in [
            "https://clerk.zaaheen.com",
            "https://example-name-12.clerk.accounts.dev",
            "https://accounts.example:8443",
        ] {
            let config = AccountConfig::new(issuer, "client_abc").unwrap();
            assert_eq!(config.issuer(), issuer);
            assert_eq!(config.client_id(), "client_abc");
        }
    }

    #[test]
    fn rejects_anything_but_the_canonical_https_form() {
        rejects("");
        rejects("clerk.zaaheen.com");
        rejects("http://clerk.zaaheen.com");
        rejects("https://clerk.zaaheen.com/");
        rejects("https://clerk.zaaheen.com/oauth");
        rejects("https://clerk.zaaheen.com?x=1");
        rejects("https://clerk.zaaheen.com#frag");
        rejects("https://user@clerk.zaaheen.com");
        rejects("https://user:pw@clerk.zaaheen.com");
        rejects("https://Clerk.Zaaheen.com");
        rejects(" https://clerk.zaaheen.com");
        rejects("https://clerk.zaaheen.com ");
        rejects("javascript:alert(1)");
    }

    #[test]
    fn client_id_charset_and_length_are_enforced() {
        let ok = "A".repeat(MAX_CLIENT_ID_LEN);
        assert!(AccountConfig::new("https://a.example", &ok).is_ok());
        assert!(AccountConfig::new("https://a.example", "aZ09_-").is_ok());
        for bad in [
            String::new(),
            "A".repeat(MAX_CLIENT_ID_LEN + 1),
            "client id".into(),
            "client&x=1".into(),
            "client\n".into(),
            "clïent".into(),
        ] {
            assert!(
                matches!(
                    AccountConfig::new("https://a.example", &bad),
                    Err(AccountError::InvalidConfig(_))
                ),
                "client id {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn endpoints_hang_directly_off_the_issuer() {
        let config = AccountConfig::new("https://a.example", "c").unwrap();
        assert_eq!(
            config.endpoint("/oauth/token"),
            "https://a.example/oauth/token"
        );
    }
}
