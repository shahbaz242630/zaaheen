//! Socket-level tests of the loopback listener, including the adversarial
//! cases the session-44 opener names: wrong `state`, wrong `iss`, replayed
//! callback, second request, oversized request, `error=` callback, and a
//! squatter's noise on the port. Every test keeps well under 5 s by
//! shortening the per-connection timeout and the window.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use url::Url;

use super::*;
use crate::config::SCOPES;
use crate::pkce::s256_challenge;

const ISSUER: &str = "https://issuer.example";
const ISS_ENC: &str = "https%3A%2F%2Fissuer.example";

fn config() -> AccountConfig {
    AccountConfig::new(ISSUER, "client_test123").unwrap()
}

fn fast() -> ListenerLimits {
    ListenerLimits {
        max_request_bytes: 8 * 1024,
        per_connection: Duration::from_millis(400),
        window: Duration::from_secs(4),
    }
}

fn param(url: &Url, name: &str) -> String {
    let values: Vec<String> = url
        .query_pairs()
        .filter(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
        .collect();
    assert_eq!(values.len(), 1, "{name} must appear exactly once");
    values[0].clone()
}

struct Harness {
    port: u16,
    state: String,
    flow: JoinHandle<AccountResult<SignInOutcome>>,
}

async fn start(limits: ListenerLimits) -> Harness {
    let pending = PendingSignIn::start(&config()).await.unwrap();
    let url = pending.authorize_url().clone();
    let state = param(&url, "state");
    let redirect = Url::parse(&param(&url, "redirect_uri")).unwrap();
    let port = redirect.port().unwrap();
    let flow = tokio::spawn(pending.wait(limits));
    Harness { port, state, flow }
}

impl Harness {
    fn callback(&self, code: &str) -> String {
        format!("/callback?code={code}&state={}&iss={ISS_ENC}", self.state)
    }

    async fn get(&self, target: &str) -> String {
        let req = format!(
            "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            self.port
        );
        raw(self.port, req.as_bytes()).await
    }

    /// The flow's result, which must arrive promptly.
    async fn outcome(self) -> AccountResult<SignInOutcome> {
        timeout(Duration::from_secs(3), self.flow)
            .await
            .expect("flow did not finish")
            .unwrap()
    }

    /// The flow must still be waiting after `ms`.
    async fn still_waiting(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        assert!(
            !self.flow.is_finished(),
            "the flow ended when it should have kept waiting"
        );
    }
}

/// Send `bytes`, then read whatever comes back until the server closes.
/// Connection errors read as an empty answer.
async fn raw(port: u16, bytes: &[u8]) -> String {
    let Ok(Ok(mut stream)) = timeout(
        Duration::from_secs(3),
        TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    else {
        return String::new();
    };
    if stream.write_all(bytes).await.is_err() {
        return String::new();
    }
    let mut out = Vec::new();
    let _ = timeout(Duration::from_secs(2), stream.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

fn code_of(outcome: AccountResult<SignInOutcome>) -> String {
    match outcome {
        Ok(SignInOutcome::Authorized(a)) => a.code.to_string(),
        other => panic!("expected a code, got {other:?}"),
    }
}

// ---- the page the browser opens (§8.41) -------------------------------------

#[tokio::test]
async fn sign_in_opens_the_authorize_url_itself() {
    let pending = PendingSignIn::start(&config()).await.unwrap();
    assert_eq!(
        pending.browser_url(SignInEntry::SignIn),
        *pending.authorize_url()
    );
}

/// "Create an account": the hosted sign-up page, sent back through the very
/// same authorize URL (same `state`, same PKCE challenge, same loopback), so
/// the sign-in that finishes is this one.
#[tokio::test]
async fn create_an_account_opens_sign_up_then_returns_through_the_same_sign_in() {
    let config = AccountConfig::new(
        "https://example-name-12.clerk.accounts.dev",
        "client_test123",
    )
    .unwrap();
    let pending = PendingSignIn::start(&config).await.unwrap();
    let url = pending.browser_url(SignInEntry::SignUp);
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("example-name-12.accounts.dev"));
    assert_eq!(url.path(), "/sign-up");
    assert_eq!(url.query_pairs().count(), 1, "only the way back: {url}");
    assert_eq!(
        param(&url, "redirect_url"),
        pending.authorize_url().as_str()
    );
}

/// No known sign-up page: the ordinary sign-in, whose own "Sign up" link
/// comes back the same way — never a guessed address.
#[tokio::test]
async fn without_a_known_sign_up_page_create_an_account_opens_the_sign_in() {
    let pending = PendingSignIn::start(&config()).await.unwrap();
    assert_eq!(
        pending.browser_url(SignInEntry::SignUp),
        *pending.authorize_url()
    );
}

// ---- the authorization request ------------------------------------------------

#[tokio::test]
async fn the_authorize_url_carries_exactly_the_pkce_request() {
    let pending = PendingSignIn::start(&config()).await.unwrap();
    let url = pending.authorize_url().clone();

    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("issuer.example"));
    assert_eq!(url.path(), "/oauth/authorize");
    assert_eq!(param(&url, "response_type"), "code");
    assert_eq!(param(&url, "client_id"), "client_test123");
    assert_eq!(param(&url, "scope"), SCOPES);
    assert_eq!(param(&url, "code_challenge_method"), "S256");
    assert_eq!(
        param(&url, "code_challenge"),
        s256_challenge(pending.pkce.verifier())
    );
    assert_eq!(param(&url, "state"), pending.state.as_str());
    assert_eq!(param(&url, "state").len(), 43);

    let redirect = param(&url, "redirect_uri");
    let port = pending.listener.local_addr().unwrap().port();
    assert_ne!(port, 0);
    assert_eq!(redirect, format!("http://127.0.0.1:{port}/callback"));

    let names: Vec<String> = url.query_pairs().map(|(k, _)| k.into_owned()).collect();
    assert_eq!(names.len(), 7, "no extra parameters: {names:?}");
    assert!(!url.as_str().contains(pending.pkce.verifier()));
}

#[tokio::test]
async fn the_listener_is_bound_to_loopback_only() {
    let pending = PendingSignIn::start(&config()).await.unwrap();
    let addr = pending.listener.local_addr().unwrap();
    assert!(addr.ip().is_loopback());
    assert!(addr.is_ipv4());
}

#[tokio::test]
async fn every_sign_in_gets_fresh_secrets() {
    let a = PendingSignIn::start(&config()).await.unwrap();
    let b = PendingSignIn::start(&config()).await.unwrap();
    assert_ne!(a.state.as_str(), b.state.as_str());
    assert_ne!(a.pkce.verifier(), b.pkce.verifier());
}

#[test]
fn the_default_limits_are_the_locked_design() {
    assert_eq!(
        ListenerLimits::default(),
        ListenerLimits {
            max_request_bytes: 8 * 1024,
            per_connection: Duration::from_secs(5),
            window: Duration::from_secs(600),
        }
    );
}

// ---- the happy paths ------------------------------------------------------------

#[tokio::test]
async fn a_valid_callback_signs_in_with_a_static_page() {
    let h = start(fast()).await;
    let answer = h.get(&h.callback("good_code")).await;
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(answer.contains("You can close this tab"));
    let outcome = h.outcome().await;
    match outcome {
        Ok(SignInOutcome::Authorized(a)) => {
            assert_eq!(a.code.as_str(), "good_code");
            assert!(a.redirect_uri.starts_with("http://127.0.0.1:"));
            assert_eq!(a.verifier.len(), 43);
            let printed = format!("{a:?}");
            assert!(!printed.contains("good_code"));
            assert!(!printed.contains(a.verifier.as_str()));
        }
        other => panic!("expected Authorized, got {other:?}"),
    }
}

#[tokio::test]
async fn an_error_callback_with_the_right_state_cancels() {
    let h = start(fast()).await;
    let target = format!(
        "/callback?error=access_denied&state={}&iss={ISS_ENC}",
        h.state
    );
    let answer = h.get(&target).await;
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(answer.contains("cancelled"));
    assert!(matches!(h.outcome().await, Ok(SignInOutcome::Cancelled)));
}

#[tokio::test]
async fn the_window_expires_without_a_callback() {
    let limits = ListenerLimits {
        window: Duration::from_millis(300),
        ..fast()
    };
    let h = start(limits).await;
    assert!(matches!(
        h.outcome().await,
        Err(AccountError::SignInTimedOut)
    ));
}

// ---- adversarial: nothing but a valid callback ends the flow ----------------------

#[tokio::test]
async fn wrong_state_is_refused_and_the_flow_keeps_waiting() {
    let h = start(fast()).await;
    let answer = h
        .get(&format!("/callback?code=evil&state=wrong&iss={ISS_ENC}"))
        .await;
    assert!(answer.starts_with("HTTP/1.1 400 "), "{answer}");
    h.still_waiting(100).await;
    h.get(&h.callback("good")).await;
    assert_eq!(code_of(h.outcome().await), "good");
}

#[tokio::test]
async fn wrong_iss_is_refused_and_the_flow_keeps_waiting() {
    let h = start(fast()).await;
    let target = format!(
        "/callback?code=evil&state={}&iss=https%3A%2F%2Fevil.example",
        h.state
    );
    assert!(h.get(&target).await.starts_with("HTTP/1.1 400 "));
    let no_iss = format!("/callback?code=evil&state={}", h.state);
    assert!(h.get(&no_iss).await.starts_with("HTTP/1.1 400 "));
    h.still_waiting(100).await;
    h.get(&h.callback("good")).await;
    assert_eq!(code_of(h.outcome().await), "good");
}

#[tokio::test]
async fn an_error_callback_with_the_wrong_state_cannot_cancel_sign_in() {
    let h = start(fast()).await;
    for target in [
        format!("/callback?error=access_denied&state=wrong&iss={ISS_ENC}"),
        "/callback?error=access_denied".to_string(),
    ] {
        assert!(h.get(&target).await.starts_with("HTTP/1.1 400 "));
    }
    h.still_waiting(100).await;
    h.get(&h.callback("good")).await;
    assert_eq!(code_of(h.outcome().await), "good");
}

#[tokio::test]
async fn a_replayed_callback_after_sign_in_gets_nothing() {
    let h = start(fast()).await;
    let valid = h.callback("first");
    let port = h.port;
    h.get(&valid).await;
    assert_eq!(code_of(h.outcome().await), "first");
    let req = format!("GET {valid} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let replay = raw(port, req.as_bytes()).await;
    assert!(
        !replay.contains("200"),
        "a replay must not be answered: {replay}"
    );
}

#[tokio::test]
async fn two_racing_valid_callbacks_sign_in_once() {
    let h = start(fast()).await;
    let (first, second) = (h.callback("aaa"), h.callback("bbb"));
    let (a, b) = tokio::join!(h.get(&first), h.get(&second));
    let successes = [&a, &b]
        .iter()
        .filter(|r| r.starts_with("HTTP/1.1 200 "))
        .count();
    assert_eq!(successes, 1, "exactly one success page:\n{a}\n---\n{b}");
    let winner = if a.starts_with("HTTP/1.1 200 ") {
        "aaa"
    } else {
        "bbb"
    };
    assert_eq!(code_of(h.outcome().await), winner);
}

#[tokio::test]
async fn a_second_request_pipelined_on_one_connection_is_never_read() {
    let h = start(fast()).await;
    let pipelined = format!(
        "GET /favicon.ico HTTP/1.1\r\nHost: x\r\n\r\nGET {} HTTP/1.1\r\nHost: x\r\n\r\n",
        h.callback("smuggled")
    );
    let answer = raw(h.port, pipelined.as_bytes()).await;
    assert!(!answer.contains("200 "), "{answer}");
    h.still_waiting(100).await;
    h.get(&h.callback("good")).await;
    assert_eq!(code_of(h.outcome().await), "good");
}

#[tokio::test]
async fn an_oversized_request_is_refused_and_the_flow_keeps_waiting() {
    let h = start(fast()).await;
    let mut big = format!("GET {} HTTP/1.1\r\nX-Pad: ", h.callback("big")).into_bytes();
    big.resize(9 * 1024, b'a');
    big.extend_from_slice(b"\r\n\r\n");
    let answer = raw(h.port, &big).await;
    assert!(!answer.contains("200 "), "{answer}");
    h.still_waiting(100).await;
    h.get(&h.callback("good")).await;
    assert_eq!(code_of(h.outcome().await), "good");
}

#[tokio::test]
async fn a_silent_connection_neither_blocks_sign_in_nor_lives_forever() {
    let h = start(fast()).await;
    let mut squatter = TcpStream::connect(("127.0.0.1", h.port)).await.unwrap();
    squatter.write_all(b"GET /callback?").await.unwrap();

    h.get(&h.callback("good")).await;
    assert_eq!(code_of(h.outcome().await), "good");

    let mut buf = Vec::new();
    let closed = timeout(Duration::from_secs(2), squatter.read_to_end(&mut buf)).await;
    assert!(closed.is_ok(), "the silent connection was never closed");
}

#[tokio::test]
async fn a_squatters_noise_is_answered_and_ignored() {
    let h = start(fast()).await;
    let port = h.port;
    let noise: Vec<Vec<u8>> = vec![
        vec![0xff, 0x00, 0x13, 0x37, b'\r', b'\n', b'\r', b'\n'],
        b"\x16\x03\x01\x02\x00\x01\x00\x01\xfc\x03\x03".to_vec(),
        format!("POST {} HTTP/1.1\r\nHost: x\r\n\r\n", h.callback("posted")).into_bytes(),
        b"GET / HTTP/1.1\r\n\r\n".to_vec(),
        b"GET /favicon.ico HTTP/1.1\r\n\r\n".to_vec(),
        b"CONNECT evil.example:443 HTTP/1.1\r\n\r\n".to_vec(),
        Vec::new(),
    ];
    for bytes in &noise {
        let answer = raw(port, bytes).await;
        assert!(!answer.contains("200 "), "{answer}");
    }
    h.still_waiting(100).await;
    h.get(&h.callback("good")).await;
    assert_eq!(code_of(h.outcome().await), "good");
}

#[tokio::test]
async fn nothing_from_the_request_is_reflected() {
    let h = start(fast()).await;
    let target = format!(
        "/callback?code=%3Cscript%3Ealert(1)%3C%2Fscript%3E&state=%3Cb%3Ex&iss={ISS_ENC}&note=reflectme"
    );
    let answer = h.get(&target).await.to_ascii_lowercase();
    assert!(answer.starts_with("http/1.1 400 "));
    assert!(!answer.contains("<script"));
    assert!(!answer.contains("alert"));
    assert!(!answer.contains("reflectme"));
    assert!(!answer.contains("<b>"));
}
