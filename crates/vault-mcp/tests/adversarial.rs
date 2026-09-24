//! Adversarial wire-layer tests (T0.1.9 Phase 3 Step 1).
//!
//! Pins vault-mcp's defense surface against malformed / oversized /
//! malicious inputs that pass the JSON-RPC parse layer but should be
//! rejected by the validation discipline encoded in `handle_write`
//! (`Memory::try_new`'s content cap + `Boundary::new`'s charset/length
//! regex) and the dispatch wrappers' error mapping per ADR-024.
//!
//! ## Scope (Phase 3 Step 1)
//!
//! Six tests covering vault-mcp's OWN defense surface:
//!
//! 1. Oversized memory content (> [`MAX_MEMORY_CONTENT_BYTES`]) at write
//! 2. Boundary name > [`MAX_BOUNDARY_LEN`] at write (length-only)
//! 3. Boundary name with unicode chars at write (char-class, distinct
//!    from length)
//! 4. Boundary name with ASCII control char at write (char-class,
//!    distinct from unicode — granular failure-mode separation)
//! 5. Unicode + RTL marks in content pass byte-identical to adapter
//!    (no-parser + no-normalization invariant at the write site)
//! 6. Unicode + combining marks in query_text pass byte-identical to
//!    adapter (no-parser + no-normalization invariant at the search site)
//!
//! ## What's deliberately NOT here
//!
//! Adversarial coverage of `query_text` validation (oversized,
//! whitespace-only, ASCII control chars) and `max_results` bounds —
//! these defenses live inside `vault_retrieval::SemanticRetriever::retrieve()`
//! at `crates/vault-retrieval/src/strategies/semantic.rs:118-133`, NOT
//! in vault-mcp. With any vault-mcp test fixture (SuccessAdapter /
//! MockAdapter / DimMismatchAdapter), those validations never fire —
//! the adapter bypasses SemanticRetriever. vault-retrieval's own
//! adversarial suite already pins these cases at `semantic.rs:482`
//! (`query_text_validation_rejects_invalid_inputs`, T0.1.8 Phase 2 work).
//! Each crate owns its own adversarial coverage.
//!
//! Adversarial integration coverage that traces vault-mcp dispatch →
//! vault-retrieval validation end-to-end against a real composed
//! system lands at T0.1.10 alongside the integration-risk spike,
//! NOT here at Phase 3.
//!
//! Wire-layer malformed JSON / framing-edge tests (oversized framing
//! headers, premature EOF mid-message) are deferred to V0.2 — rmcp's
//! framing parser is the responsible party at V0.1, and testing it
//! tests rmcp internals rather than vault-mcp defenses. Revisit when
//! vault-mcp goes beyond the founder's local dev-loop.
//!
//! ## Complementary suites
//!
//! - `tests/trust_boundary.rs` (Phase 1 schema + Phase 2 Step 8 positive
//!   assertions) covers the trust-boundary contract per ADR-025 — auth
//!   slice from server construction is the SOLE auth-gate input.
//! - `tests/error_mapping.rs` (Phase 2 Steps 3-5) covers the ADR-024
//!   wire-shape mapping for `VaultError::DimensionMismatch`. This file
//!   covers the same wire shape from a different angle: the
//!   `VaultError::InvalidInput` arm of `vault_error_to_mcp` (server.rs:669)
//!   for the four pre-adapter validation rejection paths.

mod common;

use common::{make_mock_server_with_adapter, make_success_server_with_adapter};
use rmcp::handler::server::wrapper::Parameters;
use vault_core::{MAX_BOUNDARY_LEN, MAX_MEMORY_CONTENT_BYTES};
use vault_mcp::{SearchToolParams, WriteToolParams};

// =============================================================================
// 1. Oversized memory content rejected at the vault-mcp defense surface
// =============================================================================

/// Content `MAX_MEMORY_CONTENT_BYTES + 1` bytes exceeds the BRD §11.7.1
/// cap (`vault_core::MAX_MEMORY_CONTENT_BYTES`, currently 100 KiB).
/// Validation fires inside `Memory::try_new()` (called from
/// `StdioServer::handle_write` at server.rs:243 BEFORE adapter
/// dispatch), returns `VaultError::InvalidInput`, which
/// `vault_error_to_mcp` (server.rs:669) maps to JSON-RPC `-32602`
/// "invalid params" per ADR-024 line 765.
///
/// **No-info-leak invariant** (matches the Step 3 dimension-mismatch
/// pattern at `tests/error_mapping.rs:57`): error.message is the static
/// "invalid params" string and `error.data.is_none()`. Future helpful
/// changes that populate `error.data` with size details would slip
/// through assertion (a)/(b) and surface here.
#[tokio::test]
async fn oversized_content_rejected_with_invalid_params() {
    let (server, _adapter) = make_success_server_with_adapter(vec!["work"]);
    let too_big = "x".repeat(MAX_MEMORY_CONTENT_BYTES + 1);
    let params = WriteToolParams {
        content: too_big,
        boundary: "work".into(),
        memory_type: None,
        source_agent: None,
        confidence: None,
        as_of: None,
    };
    let err = server
        .tool_write(Parameters(params))
        .await
        .expect_err("oversized content must be rejected at handle_write before adapter dispatch");

    assert_eq!(
        err.code.0, -32602,
        "VaultError::InvalidInput must map to JSON-RPC InvalidParams (-32602) per ADR-024"
    );
    assert_eq!(err.message, "invalid params", "static no-leak message");
    assert!(
        err.data.is_none(),
        "error.data must be None — no leak via the data channel"
    );
}

// =============================================================================
// 2. Boundary length cap rejected at the vault-mcp defense surface
// =============================================================================

/// Boundary name longer than `MAX_BOUNDARY_LEN` (64) bytes rejected
/// by `Boundary::new` at `boundary.rs:78-82`. Distinct from Tests 3/4
/// (char-class) — this test uses ASCII alpha (which would pass the
/// charset check) and only exercises the length check. Granular
/// failure-mode separation: a future regex change that loosens the
/// length cap but tightens the charset (or vice versa) surfaces at
/// exactly the right test.
#[tokio::test]
async fn oversized_boundary_name_rejected_with_invalid_params() {
    let (server, _adapter) = make_success_server_with_adapter(vec!["work"]);
    let too_long = "a".repeat(MAX_BOUNDARY_LEN + 1);
    let params = WriteToolParams {
        content: "valid content".into(),
        boundary: too_long,
        memory_type: None,
        source_agent: None,
        confidence: None,
        as_of: None,
    };
    let err = server
        .tool_write(Parameters(params))
        .await
        .expect_err("oversized boundary must be rejected at handle_write before adapter dispatch");

    assert_eq!(err.code.0, -32602);
    assert_eq!(err.message, "invalid params");
    assert!(err.data.is_none());
}

// =============================================================================
// 3. Boundary char-class — unicode rejected
// =============================================================================

/// Boundary name with unicode chars (`café`, where `é` is U+00E9 →
/// 2 bytes in UTF-8) rejected by `Boundary::validate`'s ASCII
/// alphanumeric check at `boundary.rs:92-99`. The constraint exists
/// per ADR-005 amendment + ADR-015 — boundary names are interpolated
/// into LanceDB `only_if` SQL filters that have no parameter binding,
/// so the type system is the only line of defence against
/// quote-breakout / SQL-metacharacter injection.
///
/// Distinct from Test 4 (control char) — both fail the same regex but
/// for distinct reasons. Granular failure-mode separation catches a
/// future regex change that loosens unicode but still rejects control
/// chars (e.g. allowing IDN-style unicode names while keeping NUL
/// rejection).
#[tokio::test]
async fn boundary_with_unicode_rejected_with_invalid_params() {
    let (server, _adapter) = make_success_server_with_adapter(vec!["work"]);
    let params = WriteToolParams {
        content: "valid".into(),
        boundary: "café".into(),
        memory_type: None,
        source_agent: None,
        confidence: None,
        as_of: None,
    };
    let err = server
        .tool_write(Parameters(params))
        .await
        .expect_err("unicode boundary must be rejected by ASCII charset check");

    assert_eq!(err.code.0, -32602);
    assert_eq!(err.message, "invalid params");
    assert!(err.data.is_none());
}

// =============================================================================
// 4. Boundary char-class — ASCII control char rejected
// =============================================================================

/// Boundary name with embedded NUL byte rejected by `Boundary::validate`'s
/// ASCII alphanumeric check. Distinct from Test 3 (unicode) — control
/// chars and unicode both fail the regex but for distinct reasons.
/// Granular failure-mode separation: a future change that allows
/// extended ASCII or unicode while keeping control-char rejection
/// (or vice versa) surfaces at the right test.
#[tokio::test]
async fn boundary_with_control_char_rejected_with_invalid_params() {
    let (server, _adapter) = make_success_server_with_adapter(vec!["work"]);
    let params = WriteToolParams {
        content: "valid".into(),
        boundary: "work\0attack".into(),
        memory_type: None,
        source_agent: None,
        confidence: None,
        as_of: None,
    };
    let err = server
        .tool_write(Parameters(params))
        .await
        .expect_err("control-char boundary must be rejected by ASCII charset check");

    assert_eq!(err.code.0, -32602);
    assert_eq!(err.message, "invalid params");
    assert!(err.data.is_none());
}

// =============================================================================
// 5. Byte-passthrough invariant (write path) — no parser, no normalization
// =============================================================================

/// Content with multi-byte unicode (emoji, RTL Arabic, combining marks,
/// explicit RTL/LTR mark) flows BYTE-IDENTICAL from the agent-supplied
/// JSON to the adapter's `NewMemory.content`. The byte-equality
/// assertion (NOT just `String` equality) pins TWO load-bearing
/// invariants at the assertion site:
///
/// 1. **No parser:** the handler MUST NOT interpret content as anything
///    other than opaque text. Same trust-boundary discipline as
///    `tests/trust_boundary.rs::handler_reaches_adapter_with_malicious_query_text_boundary_syntax`
///    (Step 8, no-parser invariant for query_text), applied to the
///    write path.
/// 2. **No normalization:** the handler MUST NOT apply unicode
///    normalization (NFC/NFD shifting), case folding, whitespace
///    stripping, or any other byte-changing transform. A change that
///    preserves `String` equality but changes the byte representation
///    (e.g. NFC normalization collapsing `e + U+0301` → `U+00E9`) would
///    pass a `==` assertion but break byte equality.
///
/// Future contributors adding "unicode normalization for search quality"
/// or "content sanitization" must update this test deliberately —
/// silent transforms break the wire-to-storage byte-fidelity contract.
#[tokio::test]
async fn content_with_unicode_passes_byte_identical_to_adapter() {
    let (server, mock) = make_mock_server_with_adapter(vec!["work"]);
    // Mix: ASCII + 4-byte emoji + RTL Arabic + combining acute
    // (e + U+0301, distinct bytes from precomposed é U+00E9) +
    // explicit RTL mark (U+200F). Each is a normalization-attractive
    // target.
    let content = "hello 🦀 مرحبا e\u{0301}\u{200F}";
    let original_bytes = content.as_bytes().to_vec();

    let params = WriteToolParams {
        content: content.to_string(),
        boundary: "work".into(),
        memory_type: None,
        source_agent: None,
        confidence: None,
        as_of: None,
    };
    server
        .tool_write(Parameters(params))
        .await
        .expect("MockAdapter returns Ok");

    let calls = mock.write_calls();
    assert_eq!(
        calls.len(),
        1,
        "handler must dispatch to adapter exactly once"
    );

    // Byte-identity (NOT just `String` equality which would miss
    // normalization shifts that produce different bytes but render
    // identically).
    assert_eq!(
        calls[0].content.as_bytes(),
        original_bytes.as_slice(),
        "content must reach adapter byte-identical (no parser, no normalization)"
    );
}

// =============================================================================
// 6. Byte-passthrough invariant (search path) — no parser, no normalization
// =============================================================================

/// Query text with multi-byte unicode (emoji, mixed scripts, combining
/// marks, conjunct consonant) flows byte-identical from the agent-
/// supplied JSON to MockAdapter's `RetrievalQuery.query_text`. Same
/// no-parser + no-normalization invariant as Test 5 but at the
/// search-path call site.
///
/// This reinforces Step 8's no-parser invariant (which pinned "the
/// handler MUST NOT parse `boundary:NAME` syntax") at a different
/// angle: Step 8 was about syntactic parsing of identifiers; this
/// test is about byte-level normalization of the entire query
/// representation. Both are forms of the same discipline — the
/// handler treats user-supplied text as opaque bytes from MCP wire
/// to retrieval call.
///
/// Combining marks specifically: `e + U+0301 (combining acute)`
/// renders as `é` but is byte-distinct from precomposed `é (U+00E9)`.
/// NFC normalization would collapse them; this test pins that
/// vault-mcp does NOT normalize.
#[tokio::test]
async fn query_text_with_unicode_passes_byte_identical_to_adapter() {
    let (server, mock) = make_mock_server_with_adapter(vec!["work"]);
    // Mix: ASCII + emoji + Spanish ñ (precomposed U+00F1) + combining
    // mark (e + U+0301) + RTL mark + Devanagari conjunct (क्ष =
    // U+0915 + U+094D + U+0937).
    let query = "search 🔍 mañana e\u{0301}\u{200F} \u{0915}\u{094D}\u{0937}";
    let original_bytes = query.as_bytes().to_vec();

    let params = SearchToolParams {
        query: query.to_string(),
        max_results: None,
        score_threshold: None,
        include_archived: None,
    };
    server
        .tool_search(
            Parameters(params),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("MockAdapter returns Ok(empty)");

    let calls = mock.search_calls();
    assert_eq!(
        calls.len(),
        1,
        "handler must dispatch to adapter exactly once"
    );

    // Byte-identity pins no normalization shifts. A future change
    // to NFC normalization (collapsing `e + U+0301` → `U+00E9`)
    // would break this assertion at the byte level even though
    // `String` equality would still hold visually.
    assert_eq!(
        calls[0].query_text.as_bytes(),
        original_bytes.as_slice(),
        "query_text must reach adapter byte-identical (no parser, no normalization)"
    );
}
