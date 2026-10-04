//! `/v1/cancel` client tests (SIGNIN-DESIGN §8.48, ADR-SEC-042). The portal
//! answer opens something on the user's computer, so it gets the same "what
//! if the account service is lying" list as Manage subscription.

use super::*;
use crate::test_support::{json_response, FakeService};

fn access() -> AccessToken {
    AccessToken::for_test("at_ACCESS_1234")
}

const CANCEL_LINK: &str =
    "https://customer-portal.paddle.com/cpl_01abc?action=cancel_subscription&token=pga_x";

async fn answer_to(status: u16, body: &str) -> AccountResult<CancelAnswer> {
    let fake = FakeService::fixed(Some(json_response(status, body))).await;
    CheckoutClient::for_loopback_test(fake.port)
        .cancel(&access())
        .await
}

#[tokio::test]
async fn cancel_sends_the_designed_request() {
    let body = format!(r#"{{"kind":"portal","url":"{CANCEL_LINK}"}}"#);
    let fake = FakeService::fixed(Some(json_response(200, &body))).await;
    let answer = CheckoutClient::for_loopback_test(fake.port)
        .cancel(&access())
        .await
        .expect("a cancel link on a paddle host is accepted");

    assert_eq!(
        answer,
        CancelAnswer::Portal(PortalUrl::parse(CANCEL_LINK).expect("fixture is valid"))
    );
    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].path, "/v1/cancel");
    assert!(
        !seen[0].body.contains("plan"),
        "a cancel must never name a plan: {}",
        seen[0].body
    );
}

#[tokio::test]
async fn nothing_to_cancel_and_already_ending_open_nothing() {
    assert_eq!(
        answer_to(200, r#"{"kind":"none"}"#).await.expect("none"),
        CancelAnswer::NothingToCancel
    );
    assert_eq!(
        answer_to(200, r#"{"kind":"ending"}"#)
            .await
            .expect("ending"),
        CancelAnswer::AlreadyEnding
    );
}

#[tokio::test]
async fn a_cancel_link_off_paddle_is_refused() {
    for url in [
        "https://paddle.com.evil.test/cancel",
        "https://notpaddle.com/cancel",
        "https://paddle.com@evil.test/",
        "http://customer-portal.paddle.com/cpl_1",
        "javascript:alert(1)",
        "file:///C:/Windows/System32/calc.exe",
    ] {
        let body = format!(r#"{{"kind":"portal","url":"{url}"}}"#);
        let answer = answer_to(200, &body).await;
        assert!(
            matches!(answer, Err(AccountError::Protocol(_))),
            "{url:?} must not be opened: {answer:?}"
        );
    }
}

#[tokio::test]
async fn an_answer_of_no_known_shape_is_refused() {
    for body in [
        r#"{"kind":"portal"}"#,
        r#"{"kind":"checkout","txn":"txn_01h8xce4qhqk1w0dm2rc4v0dxa"}"#,
        r#"{"kind":"cancelled"}"#,
        r#"{"kind":"none","url":"https://customer-portal.paddle.com/cpl_1"}"#,
        r#"{"kind":"ending","until":1}"#,
        r#"{"url":"https://customer-portal.paddle.com/cpl_1"}"#,
        "not json at all",
        "",
    ] {
        let answer = answer_to(200, body).await;
        assert!(
            matches!(answer, Err(AccountError::Protocol(_))),
            "{body:?} should not parse as a cancel answer: {answer:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_token_is_not_retryable_and_a_busy_worker_is() {
    assert!(matches!(
        answer_to(401, r#"{"error":"token_refused"}"#).await,
        Err(AccountError::Protocol(_))
    ));
    for status in [429, 500, 503] {
        let answer = answer_to(status, r#"{"error":"unavailable"}"#).await;
        assert!(
            matches!(answer, Err(AccountError::Network(_))),
            "status {status} should read as retryable: {answer:?}"
        );
    }
}
