//! The loopback listener's HTTP, kept deliberately tiny: read one bounded
//! request head, judge it, answer with a fixed page.
//!
//! Only what a browser sends on a redirect is understood: an origin-form
//! `GET /callback?…` request line. Nothing from the request is ever written
//! back (every page is a compile-time constant), and every page forbids
//! scripts, caching and referrers.

use std::fmt;

use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncReadExt};
use zeroize::Zeroizing;

/// Longest authorization code accepted. Clerk's are a few dozen characters;
/// the whole request is capped at 8 KB anyway.
pub(super) const MAX_CODE_LEN: usize = 2048;

/// What the listener decided about one request.
pub(super) enum Verdict {
    /// A valid callback carrying a code.
    Code(Zeroizing<String>),
    /// A valid callback carrying `error=`.
    Cancelled,
    /// `/callback`, but not a valid callback. The flow continues.
    Invalid,
    /// Any other path. The flow continues.
    NotFound,
    /// `/callback` with a method other than `GET`. The flow continues.
    MethodNotAllowed,
}

impl fmt::Debug for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Verdict::Code(_) => f.write_str("Code(<redacted>)"),
            Verdict::Cancelled => f.write_str("Cancelled"),
            Verdict::Invalid => f.write_str("Invalid"),
            Verdict::NotFound => f.write_str("NotFound"),
            Verdict::MethodNotAllowed => f.write_str("MethodNotAllowed"),
        }
    }
}

/// Judge one complete request head (everything up to and including the
/// blank line). `expected_state` is compared in constant time; `expected_iss`
/// by exact string equality (RFC 9207).
pub(super) fn evaluate(head: &[u8], expected_state: &str, expected_iss: &str) -> Verdict {
    let Ok(text) = std::str::from_utf8(head) else {
        return Verdict::Invalid;
    };
    let request_line = text.split("\r\n").next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Verdict::Invalid;
    };
    if method.is_empty() || target.is_empty() || !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Verdict::Invalid;
    }

    // Origin form only, compared byte for byte: no decoding or normalising of
    // the path, so `/callback%3F…`, `/x/../callback` and absolute forms miss.
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    };
    if path != "/callback" {
        return Verdict::NotFound;
    }
    if method != "GET" {
        return Verdict::MethodNotAllowed;
    }
    let Some(query) = query else {
        return Verdict::Invalid;
    };

    let mut code: Option<Zeroizing<String>> = None;
    let mut state: Option<String> = None;
    let mut iss: Option<String> = None;
    let mut error: Option<String> = None;
    for (name, value) in url::form_urlencoded::parse(query.as_bytes()) {
        // Each parameter we read may appear once; a second copy is an
        // ambiguity an attacker could exploit, so the whole request fails.
        let duplicated = match name.as_ref() {
            "code" => code.replace(Zeroizing::new(value.into_owned())).is_some(),
            "state" => state.replace(value.into_owned()).is_some(),
            "iss" => iss.replace(value.into_owned()).is_some(),
            "error" => error.replace(value.into_owned()).is_some(),
            _ => false,
        };
        if duplicated {
            return Verdict::Invalid;
        }
    }

    let state_matches = state
        .as_deref()
        .is_some_and(|s| bool::from(s.as_bytes().ct_eq(expected_state.as_bytes())));
    if !state_matches || iss.as_deref() != Some(expected_iss) {
        return Verdict::Invalid;
    }
    match (code, error) {
        (Some(code), None) if is_valid_code(&code) => Verdict::Code(code),
        (None, Some(_)) => Verdict::Cancelled,
        _ => Verdict::Invalid,
    }
}

/// 1 to [`MAX_CODE_LEN`] visible ASCII characters.
fn is_valid_code(code: &str) -> bool {
    !code.is_empty() && code.len() <= MAX_CODE_LEN && code.bytes().all(|b| b.is_ascii_graphic())
}

/// The fixed pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    SignedIn,
    Cancelled,
    Invalid,
    NotFound,
    MethodNotAllowed,
    TooLarge,
}

/// The full HTTP response for `page`: status line, security headers,
/// `Connection: close`, and a static body.
pub(super) fn response(page: Page) -> Vec<u8> {
    let (status, title, message) = match page {
        Page::SignedIn => (
            "200 OK",
            "Signed in",
            "You're signed in to Zaaheen. You can close this tab and go back to the app.",
        ),
        Page::Cancelled => (
            "200 OK",
            "Sign-in cancelled",
            "Sign-in was cancelled. You can close this tab and try again from the Zaaheen app.",
        ),
        Page::Invalid => (
            "400 Bad Request",
            "Not valid",
            "This sign-in link isn't valid. Close this tab and start again from the Zaaheen app.",
        ),
        Page::NotFound => ("404 Not Found", "Not found", "Not found."),
        Page::MethodNotAllowed => ("405 Method Not Allowed", "Not allowed", "Not allowed."),
        Page::TooLarge => (
            "431 Request Header Fields Too Large",
            "Too large",
            "Request too large.",
        ),
    };
    let allow = if page == Page::MethodNotAllowed {
        "Allow: GET\r\n"
    } else {
        ""
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head>\
         <body><p>{message}</p></body></html>"
    );
    format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Content-Security-Policy: default-src 'none'\r\n\
         Cache-Control: no-store\r\n\
         Referrer-Policy: no-referrer\r\n\
         X-Content-Type-Options: nosniff\r\n\
         {allow}\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    )
    .into_bytes()
}

/// Result of reading one request head.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum HeadRead {
    /// The head, up to and including the first `\r\n\r\n`. Anything after it
    /// (a pipelined second request) is dropped.
    Complete(Vec<u8>),
    /// More than the limit arrived without the head ending.
    TooLarge,
    /// The peer closed before the head ended.
    Incomplete,
}

/// Read one request head of at most `max` bytes. The caller wraps this in the
/// per-connection timeout.
pub(super) async fn read_head<R: AsyncRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> std::io::Result<HeadRead> {
    const END: &[u8] = b"\r\n\r\n";
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            return Ok(HeadRead::Incomplete);
        }
        // The terminator may straddle two reads, so search from 3 bytes back.
        let from = head.len().saturating_sub(END.len() - 1);
        head.extend_from_slice(&chunk[..n]);
        if let Some(at) = head[from..].windows(END.len()).position(|w| w == END) {
            let end = from + at + END.len();
            if end > max {
                return Ok(HeadRead::TooLarge);
            }
            head.truncate(end);
            return Ok(HeadRead::Complete(head));
        }
        if head.len() >= max {
            return Ok(HeadRead::TooLarge);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = "Zx9_state-value-abcdefghijklmnopqrstuvwxyz0";
    const ISS: &str = "https://issuer.example";
    const ISS_ENC: &str = "https%3A%2F%2Fissuer.example";

    fn head(request_line: &str) -> Vec<u8> {
        format!("{request_line}\r\nHost: 127.0.0.1:5000\r\n\r\n").into_bytes()
    }

    fn judge(request_line: &str) -> Verdict {
        evaluate(&head(request_line), STATE, ISS)
    }

    fn code_of(v: Verdict) -> String {
        match v {
            Verdict::Code(c) => c.to_string(),
            other => panic!("expected a code, got {other:?}"),
        }
    }

    // ---- valid callbacks -------------------------------------------------

    #[test]
    fn a_valid_callback_yields_its_code() {
        let v = judge(&format!(
            "GET /callback?code=abc123&state={STATE}&iss={ISS_ENC} HTTP/1.1"
        ));
        assert_eq!(code_of(v), "abc123");
    }

    #[test]
    fn parameter_order_does_not_matter_and_values_are_percent_decoded() {
        let v = judge(&format!(
            "GET /callback?iss={ISS_ENC}&state={STATE}&code=a%2Bb HTTP/1.1"
        ));
        assert_eq!(code_of(v), "a+b");
    }

    #[test]
    fn http_1_0_is_accepted() {
        let v = judge(&format!(
            "GET /callback?code=c&state={STATE}&iss={ISS_ENC} HTTP/1.0"
        ));
        assert_eq!(code_of(v), "c");
    }

    #[test]
    fn an_error_callback_with_the_right_state_and_iss_is_a_cancel() {
        let v = judge(&format!(
            "GET /callback?error=access_denied&state={STATE}&iss={ISS_ENC} HTTP/1.1"
        ));
        assert!(matches!(v, Verdict::Cancelled), "{v:?}");
    }

    // ---- state -----------------------------------------------------------

    #[test]
    fn wrong_missing_truncated_or_extended_state_is_invalid() {
        let short = &STATE[..STATE.len() - 1];
        for state_part in [
            "state=wrong".to_string(),
            String::new(),
            "state=".to_string(),
            format!("state={short}"),
            format!("state={STATE}x"),
        ] {
            let line = format!("GET /callback?code=c&{state_part}&iss={ISS_ENC} HTTP/1.1");
            assert!(matches!(judge(&line), Verdict::Invalid), "{line}");
        }
    }

    #[test]
    fn an_error_callback_with_the_wrong_state_is_invalid_not_a_cancel() {
        for line in [
            format!("GET /callback?error=access_denied&state=wrong&iss={ISS_ENC} HTTP/1.1"),
            format!("GET /callback?error=access_denied&iss={ISS_ENC} HTTP/1.1"),
            "GET /callback?error=access_denied HTTP/1.1".to_string(),
        ] {
            assert!(matches!(judge(&line), Verdict::Invalid), "{line}");
        }
    }

    // ---- iss -------------------------------------------------------------

    #[test]
    fn wrong_or_missing_iss_is_invalid() {
        for iss_part in [
            "iss=https%3A%2F%2Fevil.example".to_string(),
            "iss=https%3A%2F%2Fissuer.example%2F".to_string(),
            "iss=http%3A%2F%2Fissuer.example".to_string(),
            "iss=".to_string(),
            String::new(),
        ] {
            let line = format!("GET /callback?code=c&state={STATE}&{iss_part} HTTP/1.1");
            assert!(matches!(judge(&line), Verdict::Invalid), "{line}");
        }
    }

    // ---- ambiguity -------------------------------------------------------

    #[test]
    fn a_duplicated_parameter_is_invalid() {
        for line in [
            format!("GET /callback?code=c&state={STATE}&state=x&iss={ISS_ENC} HTTP/1.1"),
            format!("GET /callback?code=c&state=x&state={STATE}&iss={ISS_ENC} HTTP/1.1"),
            format!("GET /callback?code=a&code=b&state={STATE}&iss={ISS_ENC} HTTP/1.1"),
            format!("GET /callback?code=c&state={STATE}&iss={ISS_ENC}&iss={ISS_ENC} HTTP/1.1"),
            format!("GET /callback?error=a&error=b&state={STATE}&iss={ISS_ENC} HTTP/1.1"),
        ] {
            assert!(matches!(judge(&line), Verdict::Invalid), "{line}");
        }
    }

    #[test]
    fn code_and_error_together_are_invalid() {
        let line = format!(
            "GET /callback?code=c&error=access_denied&state={STATE}&iss={ISS_ENC} HTTP/1.1"
        );
        assert!(matches!(judge(&line), Verdict::Invalid));
    }

    // ---- the code itself -------------------------------------------------

    #[test]
    fn empty_oversized_or_control_character_codes_are_invalid() {
        let long = "a".repeat(MAX_CODE_LEN + 1);
        for code in [
            "",
            "a%00b",
            "a%0Ab",
            "a%20b",
            "a%7Fb",
            "caf%C3%A9",
            long.as_str(),
        ] {
            let line = format!("GET /callback?code={code}&state={STATE}&iss={ISS_ENC} HTTP/1.1");
            assert!(matches!(judge(&line), Verdict::Invalid), "code {code:?}");
        }
        let max = "a".repeat(MAX_CODE_LEN);
        let line = format!("GET /callback?code={max}&state={STATE}&iss={ISS_ENC} HTTP/1.1");
        assert_eq!(code_of(judge(&line)).len(), MAX_CODE_LEN);
    }

    #[test]
    fn no_code_and_no_error_is_invalid() {
        let line = format!("GET /callback?state={STATE}&iss={ISS_ENC} HTTP/1.1");
        assert!(matches!(judge(&line), Verdict::Invalid));
        assert!(matches!(judge("GET /callback HTTP/1.1"), Verdict::Invalid));
    }

    // ---- paths and methods -------------------------------------------------

    #[test]
    fn every_other_path_is_not_found() {
        let q = format!("code=c&state={STATE}&iss={ISS_ENC}");
        for target in [
            "/favicon.ico".to_string(),
            "/".to_string(),
            format!("/callback/?{q}"),
            format!("/Callback?{q}"),
            format!("/callbackx?{q}"),
            format!("/callback%3F{q}"),
            format!("/x/../callback?{q}"),
            format!("http://127.0.0.1:5000/callback?{q}"),
            "*".to_string(),
        ] {
            let line = format!("GET {target} HTTP/1.1");
            assert!(matches!(judge(&line), Verdict::NotFound), "{line}");
        }
    }

    #[test]
    fn a_non_get_callback_is_method_not_allowed_even_when_valid() {
        let q = format!("code=c&state={STATE}&iss={ISS_ENC}");
        for method in ["POST", "HEAD", "PUT", "OPTIONS", "get"] {
            let line = format!("{method} /callback?{q} HTTP/1.1");
            assert!(matches!(judge(&line), Verdict::MethodNotAllowed), "{line}");
        }
    }

    #[test]
    fn malformed_request_lines_and_garbage_are_invalid() {
        let q = format!("code=c&state={STATE}&iss={ISS_ENC}");
        for line in [
            format!("GET /callback?{q}"),
            format!("GET /callback?{q} HTTP/2"),
            format!("GET  /callback?{q} HTTP/1.1"),
            format!("GET /callback?{q} HTTP/1.1 extra"),
            String::new(),
        ] {
            assert!(matches!(judge(&line), Verdict::Invalid), "{line:?}");
        }
        let garbage: Vec<u8> = vec![0xff, 0xfe, 0x00, b'\r', b'\n', b'\r', b'\n'];
        assert!(matches!(evaluate(&garbage, STATE, ISS), Verdict::Invalid));
    }

    // ---- pages -------------------------------------------------------------

    const ALL_PAGES: [Page; 6] = [
        Page::SignedIn,
        Page::Cancelled,
        Page::Invalid,
        Page::NotFound,
        Page::MethodNotAllowed,
        Page::TooLarge,
    ];

    #[test]
    fn every_page_has_the_right_status_line() {
        let expected = [
            (Page::SignedIn, "HTTP/1.1 200 "),
            (Page::Cancelled, "HTTP/1.1 200 "),
            (Page::Invalid, "HTTP/1.1 400 "),
            (Page::NotFound, "HTTP/1.1 404 "),
            (Page::MethodNotAllowed, "HTTP/1.1 405 "),
            (Page::TooLarge, "HTTP/1.1 431 "),
        ];
        for (page, status) in expected {
            let text = String::from_utf8(response(page)).unwrap();
            assert!(text.starts_with(status), "{page:?}: {text}");
        }
    }

    #[test]
    fn every_page_is_locked_down_and_scriptless() {
        for page in ALL_PAGES {
            let text = String::from_utf8(response(page)).unwrap();
            let lower = text.to_ascii_lowercase();
            let (headers, body) = lower.split_once("\r\n\r\n").unwrap();
            assert!(headers.contains("\r\ncontent-security-policy: default-src 'none'"));
            assert!(headers.contains("\r\ncache-control: no-store"));
            assert!(headers.contains("\r\nreferrer-policy: no-referrer"));
            assert!(headers.contains("\r\nx-content-type-options: nosniff"));
            assert!(headers.contains("\r\nconnection: close"));
            assert!(headers.contains("\r\ncontent-type: text/html; charset=utf-8"));
            assert!(headers.contains(&format!("\r\ncontent-length: {}", body.len())));
            assert!(!body.contains("<script"), "{page:?}");
            assert!(!body.contains("http"), "{page:?} must carry no links");
        }
    }

    #[test]
    fn pages_speak_plain_english_and_name_no_stack() {
        let signed_in = String::from_utf8(response(Page::SignedIn)).unwrap();
        assert!(signed_in.contains("You can close this tab"));
        let cancelled = String::from_utf8(response(Page::Cancelled)).unwrap();
        assert!(cancelled.contains("cancelled"));
        for page in ALL_PAGES {
            let lower = String::from_utf8(response(page))
                .unwrap()
                .to_ascii_lowercase();
            for vendor in ["clerk", "oauth", "pkce", "rust", "tokio", "paddle"] {
                assert!(!lower.contains(vendor), "{page:?} names {vendor}");
            }
        }
    }

    // ---- reading the head --------------------------------------------------

    #[tokio::test]
    async fn read_head_stops_at_the_first_blank_line() {
        let mut input: &[u8] = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\nGET /b HTTP/1.1\r\n\r\n";
        let got = read_head(&mut input, 8192).await.unwrap();
        assert_eq!(
            got,
            HeadRead::Complete(b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n".to_vec())
        );
    }

    #[tokio::test]
    async fn read_head_accepts_a_head_of_exactly_the_limit() {
        let mut head = b"GET /a HTTP/1.1\r\nX: ".to_vec();
        head.resize(100 - 4, b'a');
        head.extend_from_slice(b"\r\n\r\n");
        assert_eq!(head.len(), 100);
        let mut input: &[u8] = &head;
        assert_eq!(
            read_head(&mut input, 100).await.unwrap(),
            HeadRead::Complete(head.clone())
        );
    }

    #[tokio::test]
    async fn read_head_refuses_one_byte_over_the_limit() {
        let mut head = b"GET /a HTTP/1.1\r\nX: ".to_vec();
        head.resize(101 - 4, b'a');
        head.extend_from_slice(b"\r\n\r\n");
        let mut input: &[u8] = &head;
        assert_eq!(
            read_head(&mut input, 100).await.unwrap(),
            HeadRead::TooLarge
        );
    }

    #[tokio::test]
    async fn read_head_reports_a_peer_that_hangs_up_early() {
        let mut input: &[u8] = b"GET /callback?code=abc HTTP/1.1\r\nHost";
        assert_eq!(
            read_head(&mut input, 8192).await.unwrap(),
            HeadRead::Incomplete
        );
        let mut empty: &[u8] = b"";
        assert_eq!(
            read_head(&mut empty, 8192).await.unwrap(),
            HeadRead::Incomplete
        );
    }
}
