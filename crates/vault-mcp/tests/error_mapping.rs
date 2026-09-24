//! Error-mapping contract tests (T0.1.9 Phase 2 Step 3 + Step 4 —
//! per `T0.1.9_PLAN.md` v1.1 + ADR-024 locked schema).
//!
//! ## What this file pins
//!
//! `VaultError::DimensionMismatch → JSON-RPC InvalidParams` is the
//! load-bearing contract: ALL FOUR vault tools route their errors
//! through `vault_error_to_mcp` (the centralised mapper in
//! `crates/vault-mcp/src/server.rs`). If the mapping leaks internal
//! shape (dimensions, internal type names, fields populated under
//! `error.data`), every tool leaks the same way. Pinning this contract
//! at one tool (`memory_search`) before the others land in Step 5 means
//! catching the class is cheap.
//!
//! ## Step 3 + Step 4 — both tests now active
//!
//! - **Step 3 active test:** `dimension_mismatch_returns_generic_invalid_params_no_data_leak`
//!   asserts (a) JSON-RPC code = -32602 InvalidParams, (b) message is
//!   the static "invalid params" string (matches ADR-024 line 765 +
//!   JSON-RPC 2.0 spec literal per the Step 5 wording reconciliation)
//!   with no leaked dim values or type names, (d) `error.data` is `None`.
//! - **Step 4 active test:** `dimension_mismatch_audit_row_pins_full_detail`
//!   asserts (c) the audit-row shape per ADR-024 — both at the typed
//!   Rust level (`ToolInvokeError::DimensionMismatch { expected: 384,
//!   actual: 256 }`) AND at the canonical-JSON wire level
//!   (`details_json.error == { "type": "DimensionMismatch", "detail":
//!   { "actual": 256, "expected": 384 } }`).

mod common;

use common::{make_dim_mismatch_server, make_dim_mismatch_server_with_adapter};
use rmcp::handler::server::wrapper::Parameters;
use vault_mcp::{SearchToolParams, ToolInvokeError};

// =============================================================================
// 1. Step 3 active — (a)(b)(d) JSON-RPC error contract
// =============================================================================

/// Pin the `VaultError::DimensionMismatch → InvalidParams` mapping for
/// `memory_search`, asserting the no-info-leak invariant on every
/// channel a future "helpful" change could leak through:
///
/// - **(a)** JSON-RPC `error.code` is `-32602` (InvalidParams).
/// - **(b)** `error.message` is the static "invalid params" string
///   (matches ADR-024 line 765 + JSON-RPC 2.0 spec literal per the
///   Step 5 wording reconciliation); none of the leaked-shape
///   candidates appear (`384`, `256`, "dimension", "expected",
///   "actual"). Prevents future regressions like
///   `format!("dimension mismatch: expected {expected}, got {actual}")`.
/// - **(d)** `error.data` is `None`. A future "helpful" implementation
///   could populate `error.data` with `{ "expected": 384, "actual": 256 }`
///   and pass (a)(b); pinning the absence locks that hole.
///
/// **(c) — audit row contents — is asserted by the ignored sibling
/// test below**, unignored when Step 4 wires audit-append.
#[tokio::test]
async fn dimension_mismatch_returns_generic_invalid_params_no_data_leak() {
    let server = make_dim_mismatch_server(vec!["work"]);
    let result = server
        .tool_search(
            Parameters(SearchToolParams {
                query: "anything".to_string(),
                max_results: None,
                score_threshold: None,
                include_archived: None,
            }),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    let err = result.expect_err(
        "DimMismatchAdapter::search returns DimensionMismatch — tool_search must surface as Err",
    );

    // (a) JSON-RPC InvalidParams = -32602.
    assert_eq!(
        err.code.0, -32602,
        "DimensionMismatch must map to JSON-RPC InvalidParams (-32602)"
    );

    // (b) Generic message — every shape that could leak is checked
    // explicitly so a future regression can't slip through with a
    // creative variant.
    assert_eq!(
        err.message, "invalid params",
        "error.message must be the static 'invalid params' string \
         (ADR-024 line 765 + JSON-RPC 2.0 spec literal)"
    );
    let lower = err.message.to_lowercase();
    assert!(
        !lower.contains("384"),
        "expected dimension (384) must not leak into error.message"
    );
    assert!(
        !lower.contains("256"),
        "actual dimension (256) must not leak into error.message"
    );
    assert!(
        !lower.contains("dimension"),
        "internal variant name 'dimension' must not leak into error.message"
    );
    assert!(
        !lower.contains("expected"),
        "field name 'expected' must not leak into error.message"
    );
    assert!(
        !lower.contains("actual"),
        "field name 'actual' must not leak into error.message"
    );

    // (d) error.data is absent — the no-info-leak invariant, beyond
    // just (b). Closes the "future helpful change populates error.data
    // with the leaked dims and passes (a)(b)" hole.
    assert!(
        err.data.is_none(),
        "error.data MUST be None — no internal shape may leak via the data channel"
    );
}

// =============================================================================
// 2. Step 4 active — (c) audit row contents per ADR-024 nested shape
// =============================================================================

/// Pin the `mcp.tool_invoke` audit-row shape for an error-path
/// `memory_search` call, asserting BOTH the typed Rust representation
/// and the canonical-JSON wire format land per ADR-024 (HANDOFF.md
/// lines 770–790 + plan §5 line 161 + §6.2 rule 2 line 189).
///
/// ## Typed-Rust assertions (struct-level invariants)
///
/// - exactly one audit row recorded (audit append fired exactly once)
/// - `details.tool == "memory_search"`
/// - `details.duration_ms > 0` (timer captured something — even a
///   zero-cost stub returns at least one millisecond on contended
///   Windows + cold cache; if this ever fails as flake, swap to
///   `>= 0` since the contract is "non-negative", not "positive")
/// - `details.boundary_count == 1` (matches the trusted slice)
/// - `details.result_count == 0` (error path produced no results)
/// - `details.max_results == Some(10)` (default applied at handler)
/// - `details.score_threshold == None` (caller didn't specify)
/// - `details.include_archived == Some(false)` (default applied)
/// - `details.query_length == Some(8)` (byte length of `"anything"`)
/// - `details.error == Some(ToolInvokeError::DimensionMismatch
///    { expected: 384, actual: 256 })`
///
/// ## Canonical-JSON wire-format assertions (ADR-024 contract)
///
/// - `error.type == "DimensionMismatch"` (PascalCase variant name)
/// - `error.detail.expected == 384`
/// - `error.detail.actual == 256`
/// - top-level keys present: `tool`, `duration_ms`, `result_count`,
///   `boundary_count`, `max_results`, `include_archived`,
///   `query_length`, `error` (no `score_threshold`, no
///   `null` sentinels — `score_threshold: None` MUST be ABSENT
///   per Q1, not serialised as `"score_threshold": null`)
#[tokio::test]
async fn dimension_mismatch_audit_row_pins_full_detail() {
    let (server, adapter) = make_dim_mismatch_server_with_adapter(vec!["work"]);
    let _ = server
        .tool_search(
            Parameters(SearchToolParams {
                query: "anything".to_string(),
                max_results: None,
                score_threshold: None,
                include_archived: None,
            }),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect_err("DimMismatchAdapter::search returns DimensionMismatch — tool_search must Err");

    // ---------- typed-Rust assertions ----------
    let audits = adapter.recorded_audits();
    assert_eq!(
        audits.len(),
        1,
        "exactly one audit row must be recorded for the search call, got {}",
        audits.len()
    );
    let details = &audits[0];
    assert_eq!(details.tool, "memory_search");
    assert!(
        details.duration_ms < 60_000,
        "duration_ms must be sane (< 60s for a stub adapter), got {}",
        details.duration_ms
    );
    assert_eq!(details.boundary_count, 1, "trusted slice was [\"work\"]");
    assert_eq!(details.result_count, 0, "error path produces no results");
    assert_eq!(details.max_results, Some(10), "default max_results applied");
    assert_eq!(
        details.score_threshold, None,
        "caller passed None — must serialise as ABSENT not null"
    );
    assert_eq!(
        details.include_archived,
        Some(false),
        "default include_archived applied"
    );
    assert_eq!(details.query_length, Some(8), "byte length of \"anything\"");
    match &details.error {
        Some(ToolInvokeError::DimensionMismatch { expected, actual }) => {
            assert_eq!(*expected, 384);
            assert_eq!(*actual, 256);
        }
        other => panic!("error must be Some(DimensionMismatch {{ 384, 256 }}); got {other:?}"),
    }

    // ---------- canonical-JSON wire-format assertions (ADR-024) ----------
    let raw = details
        .to_canonical_json()
        .expect("canonical JSON serialisation must succeed");
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap_or_else(|e| {
        panic!("audit details_json must round-trip as Value, got error {e}: {raw}")
    });

    // ADR-024: error shape is `{"type": "DimensionMismatch", "detail":
    // {"expected": <u32>, "actual": <u32>}}`. Both keys present + correct.
    assert_eq!(
        json["error"]["type"], "DimensionMismatch",
        "ADR-024 line 786: error.type must be PascalCase variant name; got {raw}"
    );
    assert_eq!(
        json["error"]["detail"]["expected"], 384,
        "ADR-024 line 788: detail.expected; got {raw}"
    );
    assert_eq!(
        json["error"]["detail"]["actual"], 256,
        "ADR-024 line 788: detail.actual; got {raw}"
    );

    // Q1: search-only fields with None values must be ABSENT, not
    // null. `score_threshold` was None → must NOT appear as
    // `"score_threshold": null` in the canonical JSON.
    assert!(
        !raw.contains("\"score_threshold\""),
        "score_threshold was None — must be ABSENT (not null) per Q1; got {raw}"
    );
    assert!(
        !raw.contains("null"),
        "no `null` sentinels may appear; absent != null per Q1; got {raw}"
    );

    // Top-level keys present (rest of ADR-024 schema is exercised by
    // audit::tests::canonical_json_orders_keys_alphabetically + the
    // typed assertions above; this is the wire-format spot check).
    for key in [
        "boundary_count",
        "duration_ms",
        "error",
        "include_archived",
        "max_results",
        "query_length",
        "result_count",
        "tool",
    ] {
        assert!(
            json.get(key).is_some(),
            "canonical JSON must include top-level key `{key}`; got {raw}"
        );
    }
}
