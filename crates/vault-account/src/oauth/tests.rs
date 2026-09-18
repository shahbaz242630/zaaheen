//! OAuth client tests against a fake account service on 127.0.0.1. No test
//! leaves this machine (BRD §2.10).

use std::collections::HashMap;
use std::time::Duration;

use zeroize::Zeroizing;

use super::*;
use crate::test_support::{form, json_response, FakeService};

const SECRET_CODE: &str = "code_SECRET_c0de";
const SECRET_VERIFIER: &str = "verifier-SECRET-abcdefghijklmnopqrstuvwxyz012";
const REDIRECT: &str = "http://127.0.0.1:50072/callback";

type Fake = FakeService;

impl FakeService {
    fn client(&self) -> OAuthClient {
        OAuthClient::for_loopback_test(AccountConfig::for_loopback_test(self.port))
    }
}

async fn fake_raw(response: Option<Vec<u8>>) -> Fake {
    FakeService::fixed(response).await
}

async fn fake_json(status: u16, body: &str) -> Fake {
    FakeService::fixed(Some(json_response(status, body))).await
}

fn authorized_code() -> AuthorizedCode {
    AuthorizedCode {
        code: Zeroizing::new(SECRET_CODE.into()),
        verifier: Zeroizing::new(SECRET_VERIFIER.into()),
        redirect_uri: REDIRECT.into(),
    }
}

const GOOD_TOKENS: &str = r#"{"access_token":"at_ACCESS_1234","refresh_token":"rt_REFRESH_5678","token_type":"Bearer","expires_in":86400,"scope":"openid profile email offline_access","id_token":"x.y.z"}"#;

// ---- the request ----------------------------------------------------------------

#[tokio::test]
async fn exchange_sends_the_pkce_form_as_a_public_client() {
    let fake = fake_json(200, GOOD_TOKENS).await;
    fake.client().exchange(authorized_code()).await.unwrap();

    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    let req = &seen[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/oauth/token");
    assert_eq!(
        req.headers.get("content-type").map(String::as_str),
        Some("application/x-www-form-urlencoded")
    );
    assert!(
        !req.headers.contains_key("authorization"),
        "public client: no credentials"
    );

    let fields = form(&req.body);
    let expected: HashMap<String, String> = [
        ("grant_type", "authorization_code"),
        ("code", SECRET_CODE),
        ("redirect_uri", REDIRECT),
        ("client_id", "client_test"),
        ("code_verifier", SECRET_VERIFIER),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(fields, expected, "exactly these fields, no client_secret");
}

// ---- good answers ---------------------------------------------------------------

#[tokio::test]
async fn exchange_returns_both_tokens_and_the_lifetime() {
    let fake = fake_json(200, GOOD_TOKENS).await;
    let tokens = fake.client().exchange(authorized_code()).await.unwrap();
    assert_eq!(tokens.expires_in(), Some(Duration::from_secs(86400)));
    assert_eq!(tokens.access_token().expose(), "at_ACCESS_1234");
    let (access, refresh) = tokens.into_parts();
    assert_eq!(access.expose(), "at_ACCESS_1234");
    assert_eq!(refresh.0.as_str(), "rt_REFRESH_5678");
}

#[tokio::test]
async fn token_type_is_case_insensitive_and_lifetime_is_optional() {
    let body = r#"{"access_token":"at_1","refresh_token":"rt_1","token_type":"bearer"}"#;
    let fake = fake_json(200, body).await;
    let tokens = fake.client().exchange(authorized_code()).await.unwrap();
    assert_eq!(tokens.expires_in(), None);
}

// ---- answers that break the contract ---------------------------------------------

async fn exchange_with(status: u16, body: &str) -> AccountError {
    let fake = fake_json(status, body).await;
    match fake.client().exchange(authorized_code()).await {
        Ok(t) => panic!("expected an error, got {t:?}"),
        Err(e) => e,
    }
}

#[tokio::test]
async fn a_missing_refresh_token_is_a_protocol_error() {
    let body = r#"{"access_token":"at_1","token_type":"Bearer","expires_in":86400}"#;
    assert!(matches!(
        exchange_with(200, body).await,
        AccountError::Protocol(_)
    ));
}

#[tokio::test]
async fn malformed_tokens_are_protocol_errors() {
    let long = "a".repeat(MAX_TOKEN_LEN + 1);
    for (access, refresh, kind) in [
        ("", "rt_1", "Bearer"),
        ("at_1", "", "Bearer"),
        ("at 1", "rt_1", "Bearer"),
        ("at_1", "rt\u{1}1", "Bearer"),
        (long.as_str(), "rt_1", "Bearer"),
        ("at_1", long.as_str(), "Bearer"),
        ("at_1", "rt_1", "mac"),
        ("at_1", "rt_1", ""),
    ] {
        let body = serde_json::json!({
            "access_token": access, "refresh_token": refresh, "token_type": kind
        })
        .to_string();
        assert!(
            matches!(exchange_with(200, &body).await, AccountError::Protocol(_)),
            "{access:?} / {refresh:?} / {kind:?}"
        );
    }
}

#[tokio::test]
async fn a_refresh_token_too_long_for_the_credential_store_is_refused() {
    let body = |refresh: &str| {
        serde_json::json!({
            "access_token": "at_1", "refresh_token": refresh, "token_type": "Bearer"
        })
        .to_string()
    };
    let over = "r".repeat(MAX_REFRESH_TOKEN_LEN + 1);
    assert!(matches!(
        exchange_with(200, &body(&over)).await,
        AccountError::Protocol(_)
    ));
    let at_limit = "r".repeat(MAX_REFRESH_TOKEN_LEN);
    let fake = fake_json(200, &body(&at_limit)).await;
    let tokens = fake.client().exchange(authorized_code()).await.unwrap();
    assert_eq!(tokens.into_parts().1.expose().len(), MAX_REFRESH_TOKEN_LEN);
}

#[test]
fn a_stored_token_passes_the_same_rules_as_a_fresh_one() {
    let ok = RefreshToken::from_stored(Zeroizing::new("rt_REFRESH_5678".into())).unwrap();
    assert_eq!(ok.expose(), "rt_REFRESH_5678");
    for bad in [
        String::new(),
        "rt 1".into(),
        "rt\n1".into(),
        "rt\u{0}1".into(),
        "r".repeat(MAX_REFRESH_TOKEN_LEN + 1),
        "tökén".into(),
    ] {
        assert!(
            RefreshToken::from_stored(Zeroizing::new(bad.clone())).is_none(),
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn non_json_and_wrongly_shaped_json_are_protocol_errors() {
    for body in ["not json", "[]", "null", r#"{"access_token":5}"#, ""] {
        assert!(
            matches!(exchange_with(200, body).await, AccountError::Protocol(_)),
            "{body:?}"
        );
    }
}

#[tokio::test]
async fn an_oversized_body_is_refused() {
    let padding = "x".repeat(MAX_BODY_BYTES + 1);
    let body = format!(
        r#"{{"access_token":"at_1","refresh_token":"rt_1","token_type":"Bearer","pad":"{padding}"}}"#
    );
    assert!(matches!(
        exchange_with(200, &body).await,
        AccountError::Protocol(_)
    ));
}

// ---- error answers -------------------------------------------------------------

#[tokio::test]
async fn invalid_grant_is_its_own_error() {
    let body = r#"{"error":"invalid_grant","error_description":"The authorization code was already used"}"#;
    assert!(matches!(
        exchange_with(400, body).await,
        AccountError::InvalidGrant
    ));
}

#[tokio::test]
async fn other_oauth_errors_are_protocol_errors_that_never_quote_the_server() {
    for (status, error) in [
        (400, "invalid_request"),
        (401, "invalid_client"),
        (400, "unsupported_grant_type"),
    ] {
        let body = serde_json::json!({
            "error": error,
            "error_description": format!("bad {SECRET_CODE} QUOTED_BY_SERVER")
        })
        .to_string();
        let err = exchange_with(status, &body).await;
        assert!(matches!(err, AccountError::Protocol(_)), "{error}: {err:?}");
        let text = err.to_string();
        assert!(!text.contains(SECRET_CODE));
        assert!(!text.contains("QUOTED_BY_SERVER"));
    }
}

#[tokio::test]
async fn busy_or_failing_servers_are_transient() {
    for status in [429, 500, 502, 503] {
        let err = exchange_with(status, r#"{"error":"server_error"}"#).await;
        assert!(matches!(err, AccountError::Network(_)), "{status}: {err:?}");
        assert!(err.is_transient());
    }
}

#[tokio::test]
async fn a_redirect_is_never_followed() {
    let fake = fake_raw(None).await;
    let redirecting = fake_raw(Some(
        format!(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{}/oauth/token\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            fake.port
        )
        .into_bytes(),
    ))
    .await;
    let err = redirecting
        .client()
        .exchange(authorized_code())
        .await
        .unwrap_err();
    assert!(matches!(err, AccountError::Protocol(_)), "{err:?}");
    assert_eq!(redirecting.seen().len(), 1);
    assert!(
        fake.seen().is_empty(),
        "the code was re-sent to the redirect target"
    );
}

#[tokio::test]
async fn an_unreachable_service_is_transient() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let client = OAuthClient::for_loopback_test(AccountConfig::for_loopback_test(port));
    let err = client.exchange(authorized_code()).await.unwrap_err();
    assert!(matches!(err, AccountError::Network(_)), "{err:?}");
}

#[tokio::test]
async fn a_service_that_never_answers_times_out_as_transient() {
    let fake = fake_raw(None).await;
    let err = fake.client().exchange(authorized_code()).await.unwrap_err();
    assert!(matches!(err, AccountError::Network(_)), "{err:?}");
}

// ---- refresh ----------------------------------------------------------------------

fn refresh_token(value: &str) -> RefreshToken {
    RefreshToken::from_stored(Zeroizing::new(value.into())).unwrap()
}

#[tokio::test]
async fn refresh_sends_the_refresh_grant_as_a_public_client() {
    let fake = fake_json(200, GOOD_TOKENS).await;
    let tokens = fake
        .client()
        .refresh(&refresh_token("rt_OLD_1111"))
        .await
        .unwrap();
    assert_eq!(tokens.into_parts().1.expose(), "rt_REFRESH_5678");

    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].path, "/oauth/token");
    assert!(!seen[0].headers.contains_key("authorization"));
    let expected: HashMap<String, String> = [
        ("grant_type", "refresh_token"),
        ("refresh_token", "rt_OLD_1111"),
        ("client_id", "client_test"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(form(&seen[0].body), expected);
}

#[tokio::test]
async fn a_spent_refresh_token_is_invalid_grant() {
    let body =
        r#"{"error":"invalid_grant","error_description":"The refresh token was already used"}"#;
    let fake = fake_json(400, body).await;
    let err = fake
        .client()
        .refresh(&refresh_token("rt_SPENT"))
        .await
        .unwrap_err();
    assert!(matches!(err, AccountError::InvalidGrant), "{err:?}");
}

#[tokio::test]
async fn a_refresh_without_a_new_refresh_token_is_a_protocol_error() {
    // Rotation is the contract (S0 spike (d)); an answer without the next
    // refresh token would strand the user once the old one is spent.
    let body = r#"{"access_token":"at_1","token_type":"Bearer","expires_in":86400}"#;
    let fake = fake_json(200, body).await;
    let err = fake
        .client()
        .refresh(&refresh_token("rt_1"))
        .await
        .unwrap_err();
    assert!(matches!(err, AccountError::Protocol(_)), "{err:?}");
}

#[tokio::test]
async fn refresh_failures_classify_like_the_exchange() {
    for (status, transient) in [(503, true), (429, true), (401, false)] {
        let fake = fake_json(status, r#"{"error":"server_error"}"#).await;
        let err = fake
            .client()
            .refresh(&refresh_token("rt_1"))
            .await
            .unwrap_err();
        assert_eq!(err.is_transient(), transient, "{status}: {err:?}");
        assert!(!matches!(err, AccountError::InvalidGrant));
    }
}

// ---- revoke -----------------------------------------------------------------------

#[tokio::test]
async fn revoke_posts_the_token_to_the_public_endpoint() {
    let fake = fake_json(200, "{}").await;
    fake.client()
        .revoke(&refresh_token("rt_TO_REVOKE"))
        .await
        .unwrap();
    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].path, "/oauth/token/revoke");
    assert!(!seen[0].headers.contains_key("authorization"));
    let expected: HashMap<String, String> = [
        ("token", "rt_TO_REVOKE"),
        ("token_type_hint", "refresh_token"),
        ("client_id", "client_test"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(form(&seen[0].body), expected);
}

#[tokio::test]
async fn revoke_reports_failures() {
    let fake = fake_json(503, "{}").await;
    let err = fake
        .client()
        .revoke(&refresh_token("rt_1"))
        .await
        .unwrap_err();
    assert!(err.is_transient());
    let fake = fake_json(400, r#"{"error":"invalid_request"}"#).await;
    let err = fake
        .client()
        .revoke(&refresh_token("rt_1"))
        .await
        .unwrap_err();
    assert!(matches!(err, AccountError::Protocol(_)));
}

// ---- userinfo -------------------------------------------------------------------

fn access(token: &str) -> AccessToken {
    AccessToken(Zeroizing::new(token.into()))
}

#[tokio::test]
async fn userinfo_sends_the_bearer_token_and_reads_sub_and_email() {
    let body = r#"{"sub":"user_2abc","user_id":"user_2abc","email":"sam@example.com","email_verified":true,"name":"Sam"}"#;
    let fake = fake_json(200, body).await;
    let info = fake
        .client()
        .userinfo(&access("at_ACCESS_1234"))
        .await
        .unwrap();
    assert_eq!(
        info,
        UserInfo {
            sub: "user_2abc".into(),
            email: "sam@example.com".into()
        }
    );
    let seen = fake.seen();
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].path, "/oauth/userinfo");
    assert_eq!(
        seen[0].headers.get("authorization").map(String::as_str),
        Some("Bearer at_ACCESS_1234")
    );
}

#[tokio::test]
async fn userinfo_without_a_usable_sub_or_email_is_a_protocol_error() {
    let long = "a".repeat(400);
    for body in [
        serde_json::json!({ "sub": "user_1" }).to_string(),
        serde_json::json!({ "email": "a@b.c" }).to_string(),
        serde_json::json!({ "sub": "", "email": "a@b.c" }).to_string(),
        serde_json::json!({ "sub": "user_1", "email": "" }).to_string(),
        serde_json::json!({ "sub": "user\n1", "email": "a@b.c" }).to_string(),
        serde_json::json!({ "sub": "user_1", "email": "a@b.c\r\nX: y" }).to_string(),
        serde_json::json!({ "sub": "user_1", "email": long }).to_string(),
    ] {
        let fake = fake_json(200, &body).await;
        let err = fake.client().userinfo(&access("at_1")).await.unwrap_err();
        assert!(matches!(err, AccountError::Protocol(_)), "{body}: {err:?}");
    }
}

#[tokio::test]
async fn a_refused_access_token_is_not_transient() {
    let fake = fake_json(401, r#"{"error":"invalid_token"}"#).await;
    let err = fake.client().userinfo(&access("at_1")).await.unwrap_err();
    assert!(matches!(err, AccountError::Protocol(_)), "{err:?}");
    assert!(!err.is_transient());
}

// ---- nothing secret is ever printed -----------------------------------------------

#[tokio::test]
async fn debug_output_never_contains_a_token() {
    let fake = fake_json(200, GOOD_TOKENS).await;
    let tokens = fake.client().exchange(authorized_code()).await.unwrap();
    let printed = format!("{tokens:?} {:?}", fake.client());
    assert!(!printed.contains("at_ACCESS_1234"));
    assert!(!printed.contains("rt_REFRESH_5678"));
    let code = format!("{:?}", authorized_code());
    assert!(!code.contains(SECRET_CODE));
    assert!(!code.contains(SECRET_VERIFIER));
}

#[test]
fn the_production_client_builds() {
    let config = AccountConfig::new("https://issuer.example", "client_abc").unwrap();
    assert!(OAuthClient::new(config).is_ok());
}
