//! Checkout-client tests, with the adversarial list this step owns: every
//! value here ends up opening something on the user's computer, so the
//! interesting cases are all "what if the account service is lying".

use super::*;
use crate::test_support::{json_response, FakeService};

fn access() -> AccessToken {
    AccessToken::for_test("at_ACCESS_1234")
}

const GOOD_TXN: &str = "txn_01h8xce4qhqk1w0dm2rc4v0dxa";

async fn answer_to(status: u16, body: &str) -> AccountResult<CheckoutAnswer> {
    let fake = FakeService::fixed(Some(json_response(status, body))).await;
    CheckoutClient::for_loopback_test(fake.port)
        .start(&access(), Plan::Monthly)
        .await
}

// ------------------------------------------------------------ the request

#[tokio::test]
async fn start_sends_the_designed_request() {
    let body = format!(r#"{{"kind":"checkout","txn":"{GOOD_TXN}"}}"#);
    let fake = FakeService::fixed(Some(json_response(200, &body))).await;
    let answer = CheckoutClient::for_loopback_test(fake.port)
        .start(&access(), Plan::Annual)
        .await
        .expect("a well-formed checkout is accepted");

    assert_eq!(
        answer,
        CheckoutAnswer::Checkout(TransactionId::parse(GOOD_TXN).expect("fixture is valid"))
    );
    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].path, "/v1/checkout");
    assert!(
        seen[0].body.contains(r#""plan":"annual""#),
        "the chosen plan must reach the Worker verbatim: {}",
        seen[0].body
    );
}

#[tokio::test]
async fn an_already_subscribed_person_is_sent_to_the_portal() {
    let answer = answer_to(
        200,
        r#"{"kind":"portal","url":"https://customer-portal.paddle.com/cpl_123"}"#,
    )
    .await
    .expect("a portal answer on a paddle host is accepted");
    match answer {
        CheckoutAnswer::Portal(url) => {
            assert_eq!(url.as_str(), "https://customer-portal.paddle.com/cpl_123");
        }
        other => panic!("expected a portal answer, got {other:?}"),
    }
}

// ------------------------------------------------- the transaction id check

#[test]
fn a_well_formed_transaction_id_is_accepted() {
    assert_eq!(
        TransactionId::parse(GOOD_TXN).expect("valid").as_str(),
        GOOD_TXN
    );
}

/// §8.26 §5 pins `^txn_[a-z0-9]{26}$`. Everything here would otherwise be
/// interpolated into a URL the app then opens.
#[test]
fn a_transaction_id_that_is_not_the_locked_shape_is_refused() {
    let hostile = [
        ("", "empty"),
        ("txn_", "prefix only"),
        ("01h8xce4qhqk1w0dm2rc4v0dxa", "no prefix"),
        ("TXN_01h8xce4qhqk1w0dm2rc4v0dxa", "upper-case prefix"),
        ("txn_01h8xce4qhqk1w0dm2rc4v0dx", "one char short"),
        ("txn_01h8xce4qhqk1w0dm2rc4v0dxaa", "one char long"),
        ("txn_01H8XCE4QHQK1W0DM2RC4V0DXA", "upper-case body"),
        ("txn_01h8xce4qhqk1w0dm2rc4v0dx-", "hyphen in body"),
        ("txn_01h8xce4qhqk1w0dm2rc4v0dx.", "dot in body"),
        ("txn_01h8xce4qhqk1w0dm2rc4v0dx/", "slash in body"),
        ("txn_01h8xce4qhqk1w0dm2rc4v0d a", "space in body"),
        ("txn_01h8xce4qhqk1w0dm2rc4v0d\na", "newline in body"),
        ("txn_01h8xce4qhqk1w0dm2rc4v0d&x=", "query injection"),
        ("txn_01h8xce4qhqk1w0dm2rc4v0d#x", "fragment injection"),
        (" txn_01h8xce4qhqk1w0dm2rc4v0dxa", "leading space"),
        ("txn_01h8xce4qhqk1w0dm2rc4v0dxa ", "trailing space"),
    ];
    for (raw, why) in hostile {
        assert!(
            TransactionId::parse(raw).is_err(),
            "{why}: {raw:?} was accepted as a transaction id"
        );
    }
}

/// A 26-**byte** id that is not 26 **characters** is refused.
///
/// **What this test does and does not prove.** It pins the behaviour, which
/// is worth pinning. It does *not* prove that the `chars().count()` half of
/// the length check is what refuses it: planting a bug that removed that half
/// (4b05) left this test green, because the charset check below catches the
/// same input first — every byte of a multi-byte character is >= 0x80, so it
/// is never an ASCII lowercase letter or digit.
///
/// Working that through: the charset check already guarantees every byte is
/// ASCII, and for ASCII the byte length and the character count are equal. So
/// `chars().count()` is **redundant today** and cannot be shown to matter by
/// any input, because the input that would show it cannot exist.
///
/// It is kept deliberately, as defence in depth (BRD §11.2 SP-3): it is the
/// check that would still bound the length if the charset check were ever
/// loosened — to allow `-`, say. Recorded here rather than left looking like
/// coverage, which is the mistake the 4a review caught twice.
#[test]
fn a_multi_byte_transaction_id_is_refused() {
    // 20 ASCII + 3 two-byte characters = 26 bytes, 23 characters.
    let sneaky = format!("txn_{}{}", "a".repeat(20), "\u{00e9}".repeat(3));
    assert_eq!(sneaky.strip_prefix("txn_").expect("prefix").len(), 26);
    assert!(
        TransactionId::parse(&sneaky).is_err(),
        "a 26-byte, 23-character id was accepted"
    );
}

// -------------------------------------------------------- the portal check

#[test]
fn a_portal_link_on_paddle_or_a_subdomain_is_accepted() {
    for good in [
        "https://paddle.com/portal",
        "https://customer-portal.paddle.com/cpl_123",
        "https://sandbox-customer-portal.paddle.com/cpl_123",
        "https://PADDLE.COM/portal",
    ] {
        assert!(
            PortalUrl::parse(good).is_ok(),
            "{good} should be an allowed portal link"
        );
    }
}

/// §8.30: *"The portal link must be `https` on `paddle.com` or one of its
/// subdomains."* Each entry below is a way to look like that without being
/// it — and each would otherwise be handed to the operating system to open.
#[test]
fn a_portal_link_anywhere_else_is_refused() {
    let hostile = [
        ("http://paddle.com/portal", "plain http"),
        ("https://paddle.com.evil.test/portal", "suffix look-alike"),
        ("https://notpaddle.com/portal", "prefix look-alike"),
        ("https://evil.test/paddle.com", "host in the path"),
        ("https://evil.test/?x=paddle.com", "host in the query"),
        ("https://evil.test#paddle.com", "host in the fragment"),
        ("https://paddle.com@evil.test/", "credentials as authority"),
        ("https://user:pw@paddle.com/", "credentials on a real host"),
        ("file:///C:/Windows/System32/calc.exe", "local file"),
        ("javascript:alert(1)", "script scheme"),
        ("data:text/html,<script>alert(1)</script>", "data scheme"),
        ("ftp://paddle.com/", "wrong scheme"),
        ("", "empty"),
        ("not a url at all", "not a url"),
        ("//paddle.com/portal", "scheme-relative"),
    ];
    for (raw, why) in hostile {
        assert!(
            PortalUrl::parse(raw).is_err(),
            "{why}: {raw:?} was accepted as a portal link"
        );
    }
}

// ------------------------------------------------------ what the Worker says

#[tokio::test]
async fn a_checkout_answer_carrying_a_bad_transaction_is_refused() {
    let answer = answer_to(200, r#"{"kind":"checkout","txn":"txn_not-valid"}"#).await;
    assert!(
        matches!(answer, Err(AccountError::Protocol(_))),
        "a malformed txn must not become a URL: {answer:?}"
    );
}

#[tokio::test]
async fn a_portal_answer_pointing_off_paddle_is_refused() {
    let answer = answer_to(200, r#"{"kind":"portal","url":"https://evil.test/cpl"}"#).await;
    assert!(
        matches!(answer, Err(AccountError::Protocol(_))),
        "a hostile portal link must not reach the OS: {answer:?}"
    );
}

#[tokio::test]
async fn an_answer_of_neither_shape_is_refused() {
    for body in [
        r#"{"kind":"something_else","txn":"txn_01h8xce4qhqk1w0dm2rc4v0dxa"}"#,
        r#"{"txn":"txn_01h8xce4qhqk1w0dm2rc4v0dxa"}"#,
        r#"{"kind":"checkout"}"#,
        r#"{"kind":"portal"}"#,
        "not json at all",
        "",
    ] {
        let answer = answer_to(200, body).await;
        assert!(
            matches!(answer, Err(AccountError::Protocol(_))),
            "{body:?} should not parse as an answer: {answer:?}"
        );
    }
}

/// An answer that is both shapes at once is refused rather than
/// half-understood: `deny_unknown_fields` is what makes that true.
#[tokio::test]
async fn an_answer_that_is_both_shapes_is_refused() {
    let body = format!(r#"{{"kind":"checkout","txn":"{GOOD_TXN}","url":"https://evil.test/"}}"#);
    let answer = answer_to(200, &body).await;
    assert!(
        matches!(answer, Err(AccountError::Protocol(_))),
        "an answer carrying both a txn and a url must be refused: {answer:?}"
    );
}

#[tokio::test]
async fn a_refused_token_is_not_retryable() {
    let answer = answer_to(401, r#"{"error":"token_refused"}"#).await;
    assert!(
        matches!(answer, Err(AccountError::Protocol(_))),
        "401 is a protocol failure, not a transient one: {answer:?}"
    );
}

/// 429 and 5xx mean "try again", which the caller distinguishes by the error
/// kind -- a subscribe button must not tell somebody their card failed
/// because the Worker was briefly busy.
#[tokio::test]
async fn a_busy_or_broken_worker_is_retryable() {
    for status in [429, 500, 502, 503] {
        let answer = answer_to(status, r#"{"error":"upstream"}"#).await;
        assert!(
            matches!(answer, Err(AccountError::Network(_))),
            "status {status} should read as retryable: {answer:?}"
        );
    }
}

#[test]
fn the_client_refuses_an_origin_that_is_not_https() {
    for bad in [
        "http://api.example.test",
        "https://api.example.test/v1",
        "",
        "api.example.test",
    ] {
        assert!(
            CheckoutClient::new(bad).is_err(),
            "{bad:?} should not be accepted as an API origin"
        );
    }
}
