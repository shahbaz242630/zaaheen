//! Tests for the last checkpoint before a link reaches the operating system.
//!
//! Nothing here opens a browser: `ExternalLink::open` is the only function
//! that does, and it is not called. What is tested is which links can be
//! *built* at all, because a link that cannot be built cannot be opened.

use super::*;

fn txn() -> TransactionId {
    TransactionId::parse("txn_01h8xce4qhqk1w0dm2rc4v0dxa").expect("fixture is a valid id")
}

// ------------------------------------------------------------- the pay page

#[test]
fn the_pay_link_keeps_the_trailing_slash_and_carries_the_transaction() {
    let link = ExternalLink::pay(&txn()).expect("a valid transaction makes a valid link");
    assert_eq!(
        link.as_str(),
        "https://zaaheen.com/pay/?_ptxn=txn_01h8xce4qhqk1w0dm2rc4v0dxa",
        "SIGNIN-DESIGN.md 8.31 locks the trailing slash: the site redirects \
         without it and Paddle's parameter does not survive the redirect"
    );
}

#[test]
fn the_pay_link_is_on_our_own_site() {
    let link = ExternalLink::pay(&txn()).expect("valid");
    let url = Url::parse(link.as_str()).expect("a built link parses");
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("zaaheen.com"));
    assert_eq!(url.path(), "/pay/");
}

// ------------------------------------------------------------- the sign-in

#[test]
fn a_sign_in_link_must_be_https() {
    for bad in [
        "http://accounts.example.test/authorize",
        "file:///C:/Windows/System32/calc.exe",
        "ftp://accounts.example.test/",
    ] {
        let url = Url::parse(bad).expect("fixture parses");
        assert!(
            ExternalLink::sign_in(&url).is_err(),
            "{bad} should not be openable"
        );
    }
}

/// `https://accounts.example.test@evil.test/` reads to a browser as a visit
/// to evil.test, because the part before the `@` is a username.
#[test]
fn a_link_carrying_credentials_is_refused() {
    for bad in [
        "https://accounts.example.test@evil.test/authorize",
        "https://user:pw@accounts.example.test/authorize",
    ] {
        let url = Url::parse(bad).expect("fixture parses");
        assert!(
            ExternalLink::sign_in(&url).is_err(),
            "{bad} should not be openable"
        );
    }
}

#[test]
fn an_ordinary_https_sign_in_link_is_allowed() {
    let url = Url::parse("https://accounts.example.test/oauth/authorize?state=abc")
        .expect("fixture parses");
    let link = ExternalLink::sign_in(&url).expect("an https authorize URL is allowed");
    assert_eq!(link.as_str(), url.as_str());
}

// -------------------------------------------------------------- the portal

#[test]
fn a_validated_portal_link_is_allowed() {
    let portal = PortalUrl::parse("https://customer-portal.paddle.com/cpl_123")
        .expect("fixture is an allowed portal link");
    let link = ExternalLink::portal(&portal).expect("a validated portal link is openable");
    assert_eq!(link.as_str(), "https://customer-portal.paddle.com/cpl_123");
}

// ------------------------------------------------------- what Debug reveals

/// The pay link's query carries the transaction id, which identifies a
/// purchase in progress. It must not land in a log line by accident
/// (BRD §11.7.2).
#[test]
fn debug_never_prints_the_query() {
    let link = ExternalLink::pay(&txn()).expect("valid");
    let shown = format!("{link:?}");
    assert!(
        !shown.contains("txn_01h8xce4qhqk1w0dm2rc4v0dxa"),
        "the transaction id reached a debug line: {shown}"
    );
    assert!(
        !shown.contains("_ptxn"),
        "the query reached a debug line: {shown}"
    );
    assert!(
        shown.contains("zaaheen.com/pay/"),
        "debug should still say where it points: {shown}"
    );
}

// ------------------------------------------------ the gate is not bypassable

/// Every constructor must end at the same check. If one of them ever stops
/// calling `checked`, this is the test that notices: each hostile input is
/// fed through every public constructor that can accept it.
#[test]
fn every_constructor_refuses_a_link_that_is_not_https_without_credentials() {
    let hostile = [
        "http://zaaheen.com/pay/",
        "https://zaaheen.com@evil.test/pay/",
        "javascript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
    ];
    for raw in hostile {
        if let Ok(url) = Url::parse(raw) {
            assert!(
                ExternalLink::sign_in(&url).is_err(),
                "sign_in accepted {raw}"
            );
        }
    }
}
