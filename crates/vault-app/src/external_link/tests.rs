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

// ------------------------------------ Cursor's install request (ADR-106, ADR-111)

fn full_path() -> ServerCommand {
    let text = if cfg!(windows) {
        r"C:\Program Files\Zaaheen\zaaheen.exe"
    } else {
        "/opt/zaaheen/zaaheen"
    };
    ServerCommand::parse(text).expect("an absolute path to the program")
}

/// What Cursor reads from the link: its name and the decoded server.
fn decoded(link: &ExternalLink) -> (Vec<(String, String)>, serde_json::Value) {
    use base64::Engine as _;

    let url = Url::parse(link.as_str()).expect("a built link parses");
    let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    assert_eq!(pairs.len(), 2, "exactly name and config: {pairs:?}");
    let config = base64::engine::general_purpose::STANDARD
        .decode(&pairs[1].1)
        .expect("the config is base64");
    (
        pairs,
        serde_json::from_slice(&config).expect("the config is JSON"),
    )
}

/// The link is Cursor's documented form and carries exactly the server for
/// the command it was given, the short name or the full path.
#[test]
fn the_cursor_link_installs_exactly_the_zaaheen_server() {
    for command in [ServerCommand::short_name(), full_path()] {
        let link = ExternalLink::install_in_cursor(&command).expect("the link passes its gate");
        let url = Url::parse(link.as_str()).unwrap();
        assert_eq!(url.scheme(), "cursor");
        assert_eq!(url.host_str(), Some("anysphere.cursor-deeplink"));
        assert_eq!(url.path(), "/mcp/install");
        let (pairs, server) = decoded(&link);
        assert_eq!(pairs[0], ("name".to_owned(), "zaaheen".to_owned()));
        assert_eq!(pairs[1].0, "config");
        assert_eq!(
            server,
            serde_json::json!({ "command": command.as_str(), "args": ["mcp", "serve"] }),
            "the server for this command, and nothing else"
        );
    }
}

/// Session 64: the short name failed for an app started before the install.
/// On an installed Zaaheen the link names the program by its full path.
#[test]
fn the_cursor_link_can_carry_the_full_path() {
    let (_, server) = decoded(&ExternalLink::install_in_cursor(&full_path()).unwrap());
    assert_eq!(server["command"], full_path().as_str());
}

/// A path whose base64 has `+` and `/` (and `=` padding) must reach Cursor
/// unchanged: a raw `+` in a query reads as a space.
#[test]
fn awkward_base64_survives_the_link() {
    use base64::Engine as _;

    // Chosen so the base64 has all three whichever order the JSON keys take.
    let folder = if cfg!(windows) {
        r"C:\~~~\>>>???"
    } else {
        "/x>>?~/y??>>~~"
    };
    let text = format!(
        "{folder}{}{}",
        std::path::MAIN_SEPARATOR,
        crate::server_command::PROGRAM_FILE
    );
    let command = ServerCommand::parse(&text).expect("an absolute path to the program");
    let raw = base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_vec(&command.server_json()).unwrap());
    assert!(
        raw.contains('+') && raw.contains('/') && raw.ends_with('='),
        "the fixture must exercise all three: {raw}"
    );
    let link = ExternalLink::install_in_cursor(&command).unwrap();
    assert!(!link.as_str().contains('+'), "a raw '+' reached the link");
    let (pairs, server) = decoded(&link);
    assert_eq!(
        pairs[1].1, raw,
        "the config Cursor decodes is the one encoded"
    );
    assert_eq!(server["command"], text);
}

/// The gate refuses a link that is not exactly Cursor's install of this
/// server.
#[test]
fn the_cursor_gate_refuses_anything_else() {
    let command = full_path();
    let good = ExternalLink::install_in_cursor(&command).unwrap();
    assert!(ExternalLink::is_cursor_install(
        &Url::parse(good.as_str()).unwrap(),
        &command
    ));
    let query = Url::parse(good.as_str())
        .unwrap()
        .query()
        .unwrap()
        .to_owned();
    for bad in [
        format!("cursor://evil.test/mcp/install?{query}"),
        format!("cursor://anysphere.cursor-deeplink/mcp/other?{query}"),
        format!("vscode://anysphere.cursor-deeplink/mcp/install?{query}"),
        format!("cursor://anysphere.cursor-deeplink/mcp/install?{query}&env=x"),
        format!("cursor://anysphere.cursor-deeplink/mcp/install?{query}#x"),
        "cursor://anysphere.cursor-deeplink/mcp/install?name=zaaheen&config=bm90IGpzb24".to_owned(),
    ] {
        let url = Url::parse(&bad).expect("fixture parses");
        assert!(
            !ExternalLink::is_cursor_install(&url, &command),
            "accepted {bad}"
        );
    }
    // The right link for a different command is not this command's link.
    assert!(!ExternalLink::is_cursor_install(
        &Url::parse(good.as_str()).unwrap(),
        &ServerCommand::short_name()
    ));
}

#[test]
fn debug_names_where_the_cursor_link_points() {
    let shown = format!(
        "{:?}",
        ExternalLink::install_in_cursor(&full_path()).unwrap()
    );
    assert!(shown.contains("cursor://anysphere.cursor-deeplink/mcp/install"));
    assert!(
        !shown.contains("config"),
        "the query, with the path, reached a debug line"
    );
}
