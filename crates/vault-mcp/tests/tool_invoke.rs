//! Tool-invocation contract tests (T0.1.9 Phase 2 Step 5).
//!
//! ## What this file pins
//!
//! End-to-end success-path + error-path coverage for all four tools
//! (`memory_search` / `memory_write` / `memory_update` / `memory_delete`)
//! through the audit + tracing wiring landed in Step 4 (`tool_search`)
//! and Step 5 (`tool_write` / `tool_update` / `tool_delete`).
//!
//! Each per-tool success test bundles three concerns into one body
//! per Shahbaz's Step 5 refinement (cleaner than three tests with
//! overlapping setup):
//!
//! 1. **Wire-shape pin** — the `CallToolResult` content matches the
//!    tool's locked success-response JSON shape.
//! 2. **Audit-row pin** — exactly one `ToolInvokeDetails` was recorded
//!    with the correct `tool` field, sane `duration_ms`, expected
//!    `result_count`, and `error: None`.
//! 3. **Q1 absent-not-null serialisation invariant** — for write /
//!    update / delete, the canonical-JSON serialisation of the
//!    recorded `ToolInvokeDetails` must OMIT (not null-serialise) the
//!    search-only keys (`max_results` / `score_threshold` /
//!    `include_archived` / `query_length`). Closes the matching audit
//!    chain hash determinism property.
//!
//! Per-tool error tests pin the error-mapping contract:
//!
//! - `tool_write` + `tool_update` AccessDenied via unauthorized
//!   boundary — fires at the handler layer BEFORE the adapter is
//!   reached, so a `SuccessAdapter` is sufficient. Asserts wire code
//!   `-32001` + message `"access denied"` + audit row records
//!   `error.type = "AccessDenied"` with the rejected boundary in
//!   `error.detail.boundary_attempted`.
//! - `tool_delete` missing-id is **idempotent success** (ADR-056,
//!   2026-05-28). `DimMismatchAdapter::lookup_boundary` returns `None`,
//!   so `handle_delete` short-circuits to `Ok(())` before dispatch.
//!   Asserts the success wire shape + a no-error audit row. Founder
//!   dogfood surfaced the prior `NotFound` behaviour contradicting the
//!   tool's documented idempotency contract.

mod common;

use common::{make_dim_mismatch_server_with_adapter, make_success_server_with_adapter};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use vault_mcp::{
    DeleteToolParams, SearchToolParams, ToolInvokeError, UpdateToolParams, WriteToolParams,
};

// =============================================================================
// Helpers
// =============================================================================

/// Extract the single JSON content block from a `CallToolResult` for
/// shape-assertion. Every vault tool uses `success_json_result(...)`
/// which produces exactly one content block.
fn extract_success_json(result: CallToolResult) -> serde_json::Value {
    assert_eq!(
        result.content.len(),
        1,
        "vault tools always produce exactly one content block; got {}",
        result.content.len()
    );
    let item = &result.content[0];
    let raw_obj: serde_json::Value =
        serde_json::to_value(item).expect("content block round-trips via Value");
    // rmcp wraps the user value in `{"type": "text", "text": "<json string>"}`
    // for `ContentBlock::json` — Step 4's success path uses the same wrapper.
    // Either shape works here: parse the inner `text` if present, else
    // assume the raw value is the user payload.
    if raw_obj.get("type").map(|t| t.as_str()) == Some(Some("text")) {
        let inner_text = raw_obj["text"]
            .as_str()
            .expect("rmcp text content block has string text");
        serde_json::from_str(inner_text).expect("inner text parses as JSON")
    } else {
        raw_obj
    }
}

/// Assert the canonical-JSON serialisation of a `ToolInvokeDetails`
/// for write / update / delete OMITS the search-only keys (Q1
/// absent-not-null invariant). Centralised here because Step 5
/// asserts it in three places (write / update / delete success).
fn assert_search_only_keys_absent(details: &vault_mcp::ToolInvokeDetails) {
    let raw = details
        .to_canonical_json()
        .expect("canonical JSON serialisation must succeed");
    for key in [
        "max_results",
        "score_threshold",
        "include_archived",
        "query_length",
    ] {
        assert!(
            !raw.contains(&format!("\"{key}\"")),
            "Q1: search-only key `{key}` must be ABSENT (not null) on \
             write/update/delete; got {raw}"
        );
    }
    assert!(
        !raw.contains("null"),
        "no `null` sentinels may appear in canonical JSON; got {raw}"
    );
}

// =============================================================================
// Success integration tests (4) — per Step 5 plan
// =============================================================================

#[tokio::test]
async fn tool_search_success_records_audit_and_returns_results() {
    let (server, adapter) = make_success_server_with_adapter(vec!["work"]);
    let result = server
        .tool_search(Parameters(SearchToolParams {
            query: "anything".to_string(),
            max_results: Some(5),
            score_threshold: None,
            include_archived: None,
        }))
        .await
        .expect("SuccessAdapter::search returns one hit; tool_search must succeed");

    // (1) Wire-shape pin — ADR-071 Option B: a JSON object wrapping the
    //     reordered `results` array with the additive recall-safe hint
    //     (`top_relevance` + `weak_match`).
    let body = extract_success_json(result);
    let arr = body
        .get("results")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| {
            panic!("memory_search success body must have a `results` array; got {body}")
        });
    assert_eq!(arr.len(), 1, "SuccessAdapter returns exactly one hit");
    assert!(
        arr[0].get("memory").is_some(),
        "RetrievedMemory must serialise with `memory` field; got {body}"
    );
    assert!(
        arr[0].get("score").is_some(),
        "RetrievedMemory must serialise with `score` field; got {body}"
    );
    assert!(
        body.get("top_relevance").is_some(),
        "response must carry a `top_relevance` hint; got {body}"
    );
    assert!(
        body.get("weak_match").is_some(),
        "response must carry a `weak_match` hint; got {body}"
    );

    // (2) Audit-row pin — one row, correct shape.
    let audits = adapter.recorded_audits();
    assert_eq!(audits.len(), 1, "exactly one audit row per tool invocation");
    let details = &audits[0];
    assert_eq!(details.tool, "memory_search");
    assert_eq!(details.result_count, 1);
    assert_eq!(details.boundary_count, 1);
    assert!(details.duration_ms < 60_000, "duration_ms must be sane");
    assert!(details.error.is_none(), "success path: error absent");
    assert_eq!(details.max_results, Some(5));
    assert_eq!(details.query_length, Some(8));
    // (3) For search the search-only keys are PRESENT; the absent-
    // invariant test runs on write / update / delete only.
}

#[tokio::test]
async fn tool_write_success_records_audit_returns_id_and_omits_search_only_keys() {
    let (server, adapter) = make_success_server_with_adapter(vec!["work"]);
    let result = server
        .tool_write(Parameters(WriteToolParams {
            content: "remember the milk".to_string(),
            boundary: "work".to_string(),
            memory_type: None,
            source_agent: None,
            confidence: None,
            as_of: None,
        }))
        .await
        .expect("SuccessAdapter::write returns Ok; tool_write must succeed");

    // (1) Wire-shape pin — { "id": "<uuid>" }.
    let body = extract_success_json(result);
    let id_str = body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("memory_write success body must have `id` string; got {body}"));
    uuid::Uuid::parse_str(id_str).expect("id must parse as UUID");

    // (2) Audit-row pin.
    let audits = adapter.recorded_audits();
    assert_eq!(audits.len(), 1);
    let details = &audits[0];
    assert_eq!(details.tool, "memory_write");
    assert_eq!(details.result_count, 1);
    assert_eq!(details.boundary_count, 1);
    assert!(details.duration_ms < 60_000);
    assert!(details.error.is_none());
    // typed-Rust check that search-only fields are None.
    assert_eq!(details.max_results, None);
    assert_eq!(details.score_threshold, None);
    assert_eq!(details.include_archived, None);
    assert_eq!(details.query_length, None);

    // (3) Q1 absent-not-null serialisation invariant.
    assert_search_only_keys_absent(details);
}

#[tokio::test]
async fn tool_update_success_records_audit_returns_id_and_omits_search_only_keys() {
    let (server, adapter) = make_success_server_with_adapter(vec!["work"]);
    let target_id = "01910000-0000-7000-8000-000000000001"; // valid UUIDv7 shape.
    let result = server
        .tool_update(Parameters(UpdateToolParams {
            id: target_id.to_string(),
            content: "updated content".to_string(),
            boundary: "work".to_string(),
            memory_type: None,
            source_agent: None,
            confidence: None,
        }))
        .await
        .expect("SuccessAdapter::update returns Ok; tool_update must succeed");

    // (1) Wire-shape pin — { "updated": "<uuid>" }.
    let body = extract_success_json(result);
    assert_eq!(
        body["updated"].as_str(),
        Some(target_id),
        "memory_update success body must echo the input id; got {body}"
    );

    // (2) Audit-row pin.
    let audits = adapter.recorded_audits();
    assert_eq!(audits.len(), 1);
    let details = &audits[0];
    assert_eq!(details.tool, "memory_update");
    assert_eq!(details.result_count, 1);
    assert_eq!(details.boundary_count, 1);
    assert!(details.error.is_none());

    // (3) Q1 absent-not-null serialisation invariant.
    assert_search_only_keys_absent(details);
}

#[tokio::test]
async fn tool_delete_success_records_audit_and_omits_search_only_keys() {
    let (server, adapter) = make_success_server_with_adapter(vec!["work"]);
    let target_id = "01910000-0000-7000-8000-000000000002";
    let result = server
        .tool_delete(Parameters(DeleteToolParams {
            id: target_id.to_string(),
        }))
        .await
        .expect("SuccessAdapter::delete returns Ok; tool_delete must succeed");

    // (1) Wire-shape pin — { "deleted": "<uuid>" }.
    let body = extract_success_json(result);
    assert_eq!(
        body["deleted"].as_str(),
        Some(target_id),
        "memory_delete success body must echo the input id; got {body}"
    );

    // (2) Audit-row pin.
    let audits = adapter.recorded_audits();
    assert_eq!(audits.len(), 1);
    let details = &audits[0];
    assert_eq!(details.tool, "memory_delete");
    assert_eq!(details.result_count, 1);
    assert_eq!(details.boundary_count, 1);
    assert!(details.error.is_none());

    // (3) Q1 absent-not-null serialisation invariant.
    assert_search_only_keys_absent(details);
}

// =============================================================================
// Error integration tests (3) — per Step 5 plan
// =============================================================================

/// `tool_write` with an unauthorized boundary — handler-layer
/// `AccessDenied` fires before adapter is reached. SuccessAdapter is
/// sufficient because adapter.write is never called.
///
/// Pins:
/// - JSON-RPC code `-32001`
/// - Message `"access denied"` (lowercase, ADR-024 line 764)
/// - `error.data` is None (no info leak)
/// - Audit row records `error.type = "AccessDenied"` with
///   `error.detail.boundary_attempted = "admin"` (the rejected
///   boundary)
#[tokio::test]
async fn tool_write_access_denied_pins_wire_code_and_audit_shape() {
    let (server, adapter) = make_success_server_with_adapter(vec!["work"]);
    let result = server
        .tool_write(Parameters(WriteToolParams {
            // Agent supplies "admin" — not in the trusted slice.
            content: "anything".to_string(),
            boundary: "admin".to_string(),
            memory_type: None,
            source_agent: None,
            confidence: None,
            as_of: None,
        }))
        .await;
    let err =
        result.expect_err("unauthorized boundary must surface as Err; SuccessAdapter never called");

    assert_eq!(
        err.code.0, -32001,
        "AccessDenied must map to ADR-024's -32001 code"
    );
    assert_eq!(
        err.message, "access denied",
        "ADR-024 line 764: access-denied wire message"
    );
    assert!(
        err.data.is_none(),
        "error.data must be None — no info leak through the data channel"
    );

    let audits = adapter.recorded_audits();
    assert_eq!(audits.len(), 1, "one audit row even on error path");
    let details = &audits[0];
    assert_eq!(details.tool, "memory_write");
    assert_eq!(details.result_count, 0, "error path: no result");
    assert_eq!(details.boundary_count, 1);
    match &details.error {
        Some(ToolInvokeError::AccessDenied { boundary_attempted }) => {
            assert!(
                boundary_attempted.contains("admin"),
                "AccessDenied detail must carry the rejected boundary name; got {boundary_attempted}"
            );
        }
        other => panic!("expected AccessDenied, got {other:?}"),
    }
}

/// `tool_update` with an unauthorized boundary. Same handler-layer
/// path as `tool_write`. Asserts the error contract carries through
/// the parse_memory_id_traced step (id IS valid; only the boundary is
/// unauthorized — the parse path is exercised by the tool_update
/// success test above and the access-denied path independently).
#[tokio::test]
async fn tool_update_access_denied_pins_wire_code_and_audit_shape() {
    let (server, adapter) = make_success_server_with_adapter(vec!["work"]);
    let valid_id = "01910000-0000-7000-8000-000000000003";
    let result = server
        .tool_update(Parameters(UpdateToolParams {
            id: valid_id.to_string(),
            content: "anything".to_string(),
            boundary: "admin".to_string(),
            memory_type: None,
            source_agent: None,
            confidence: None,
        }))
        .await;
    let err = result.expect_err("unauthorized boundary must surface as Err");

    assert_eq!(err.code.0, -32001);
    assert_eq!(err.message, "access denied");
    assert!(err.data.is_none());

    let audits = adapter.recorded_audits();
    assert_eq!(audits.len(), 1);
    let details = &audits[0];
    assert_eq!(details.tool, "memory_update");
    assert_eq!(details.result_count, 0);
    match &details.error {
        Some(ToolInvokeError::AccessDenied { boundary_attempted }) => {
            assert!(boundary_attempted.contains("admin"));
        }
        other => panic!("expected AccessDenied, got {other:?}"),
    }
}

/// ADR-056 (2026-05-28): deleting an id that does not exist is
/// **idempotent success**, not `NotFound`. `lookup_boundary` returns
/// `None` for a missing memory, and `handle_delete` short-circuits to
/// `Ok(())` before any auth-gate or dispatch. Founder dogfood (Claude
/// Desktop) surfaced the prior `NotFound` behaviour contradicting the
/// tool description's documented "idempotent on missing ids" contract.
///
/// `DimMismatchAdapter::lookup_boundary` returns `Ok(None)`, so this
/// exercises the missing-memory path. The adapter's `delete()` (which
/// would return `NotFound`) is never reached — the short-circuit fires
/// first — so the call succeeds.
#[tokio::test]
async fn tool_delete_missing_id_is_idempotent_success() {
    let (server, adapter) = make_dim_mismatch_server_with_adapter(vec!["work"]);
    let missing_id = "01910000-0000-7000-8000-000000000004";
    let result = server
        .tool_delete(Parameters(DeleteToolParams {
            id: missing_id.to_string(),
        }))
        .await
        .expect("deleting a missing id must be idempotent success per ADR-056");

    // Success wire shape — { "deleted": "<uuid>" } echo, even though
    // nothing existed to delete.
    let body = extract_success_json(result);
    assert_eq!(
        body["deleted"].as_str(),
        Some(missing_id),
        "idempotent delete still echoes the id; got {body}"
    );

    // Exactly one audit row, recorded as a clean success (no error).
    let audits = adapter.recorded_audits();
    assert_eq!(audits.len(), 1, "one audit row even on idempotent no-op");
    let details = &audits[0];
    assert_eq!(details.tool, "memory_delete");
    assert!(
        details.error.is_none(),
        "idempotent delete records no error"
    );
}

// =============================================================================
// 3. ADR-027 pinning test — pre-dispatch parse failure: tracing-only, no audit
// =============================================================================

/// Pin the ADR-027 contract: pre-dispatch validation failures emit a
/// `tracing::warn!(target: "vault_mcp::request_validation", ...)` event
/// and DO NOT append to the audit chain. The wire response stays the
/// ADR-024 mapping for `VaultError::InvalidInput` (-32602
/// `"invalid params"`); only the audit-append is skipped.
///
/// The test exercises `memory_delete` with a malformed UUID because
/// `tool_delete` is the simplest of the three tools that go through
/// `parse_memory_id_traced` (id-only request body). The contract is
/// equivalent for `tool_update`'s id-parse path.
///
/// Asserts:
/// 1. Tracing event captured at `vault_mcp::request_validation` (not
///    `vault_mcp::tool_invoke`) — different target keeps ops-tooling
///    filtering clean per ADR-027 reasoning (iv).
/// 2. Event carries the tool name `memory_delete` so operators can
///    correlate parse-rejections to which tool was probed.
/// 3. Recording adapter's `recorded_audits().len()` stays at 0 — the
///    audit chain is reserved for handler-dispatched vault operations
///    per Q7(a). A pre-dispatch parse failure never reaches the
///    handler; nothing is appended.
/// 4. Wire response is `-32602 "invalid params"` per ADR-024 mapping
///    (preserved across parse and handler-side `InvalidInput` paths).
#[tokio::test]
#[tracing_test::traced_test]
async fn parse_failure_emits_tracing_does_not_append_audit() {
    let (server, adapter) = make_success_server_with_adapter(vec!["work"]);

    let result = server
        .tool_delete(Parameters(DeleteToolParams {
            id: "not-a-uuid".to_string(),
        }))
        .await;
    let err = result.expect_err("malformed id must surface as McpError");

    // (4) Wire response unchanged from ADR-024 mapping for the
    // InvalidInput / parse-failure path.
    assert_eq!(
        err.code.0, -32602,
        "ADR-024 line 765: malformed id maps to -32602 invalid_params"
    );
    assert_eq!(
        err.message, "invalid params",
        "ADR-024 line 765 + Step 5 wording reconciliation: \
         spec-literal 'invalid params'"
    );
    assert!(err.data.is_none(), "no info-leak via the data channel");

    // (1) Tracing event captured at `vault_mcp::request_validation`
    // — distinct from `vault_mcp::tool_invoke` per ADR-027 (iv).
    assert!(
        tracing_test::internal::logs_with_scope_contain(
            "vault_mcp",
            "vault_mcp::request_validation"
        ),
        "ADR-027: parse failure must emit at `vault_mcp::request_validation` target"
    );
    assert!(
        tracing_test::internal::logs_with_scope_contain(
            "vault_mcp",
            "malformed id in tool request"
        ),
        "ADR-027: warn event message string must be present"
    );

    // (2) Tool-name field present so operators can correlate
    // parse-rejections to the probed tool.
    assert!(
        tracing_test::internal::logs_with_scope_contain("vault_mcp", "memory_delete"),
        "ADR-027: tool field must be `memory_delete` for delete-path parse failures"
    );

    // (3) **Load-bearing assertion** — the audit chain is reserved
    // for vault dispatches per Q7(a) handler-mediated audit. A
    // pre-dispatch parse failure never reaches the handler; the
    // audit count stays at the baseline 0.
    assert_eq!(
        adapter.recorded_audits().len(),
        0,
        "ADR-027: pre-dispatch parse failure MUST NOT append to the \
         audit chain. Audit chain is reserved for handler-dispatched \
         vault operations (Q7 a). If this assertion fails, the \
         architectural decision in ADR-027 has silently changed."
    );
}
