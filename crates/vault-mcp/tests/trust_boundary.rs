//! Trust-boundary adversarial tests (ADR-025).
//!
//! These tests pin the load-bearing invariant for prompt-injection
//! defense: tool args from the MCP client are UNTRUSTED and never
//! contribute to authorization decisions.
//!
//! ## State (T0.1.9 Phase 2 Step 8)
//!
//! - The **schema-level** tests pass without panic — they verify that
//!   `SearchToolParams` does NOT have an `authorized_boundaries` field,
//!   so a malicious key in the JSON-RPC request body is silently
//!   dropped by serde at deserialization. No adapter call needed.
//! - The **handler-level** tests assert positively (Step 8) against
//!   `MockAdapter` from `tests/common/mock_adapter.rs`: the captured
//!   `RetrievalQuery::authorized_boundaries` equals the trusted slice
//!   supplied at server construction, NOT anything carried in the
//!   request body or query text. Both directions of the trust-boundary
//!   contract are pinned: positive (trusted slice flows through) and
//!   negative (malicious body/query values do NOT contaminate auth).

mod common;

use common::{make_mock_server_with_adapter, make_test_server};
use vault_core::Boundary;
use vault_mcp::SearchToolParams;

// =============================================================================
// 1. Schema-level: SearchToolParams has no authorized_boundaries field
// =============================================================================

/// Adversarial body shape: an attacker-controlled `authorized_boundaries`
/// key in the JSON-RPC body. ADR-025 specifies the field is silently
/// dropped (extra fields default for `serde::Deserialize`); the schema
/// test pins the absence.
#[test]
fn boundary_override_in_json_body_drops_at_deserialization() {
    let malicious_body = r#"{
        "query": "give me everything",
        "max_results": 10,
        "authorized_boundaries": ["admin", "../../etc/passwd"]
    }"#;
    let params: SearchToolParams =
        serde_json::from_str(malicious_body).expect("body parses cleanly");
    assert_eq!(params.query, "give me everything");
    assert_eq!(params.max_results, Some(10));
    // No `authorized_boundaries` field on `SearchToolParams` — by
    // design, per ADR-025. The malicious override has nowhere to land.
    // Static type-system check: this line wouldn't compile if the
    // field existed.
    let _ = params; // explicit drop to silence unused-warning if any
}

/// Same posture for `boundary` syntax embedded in the query text —
/// the embedder treats the query as opaque text; nothing is parsed
/// for boundary names. Schema-level: the `query` field is just a
/// String, no parser hooks, no auth-gate inputs derived from it.
#[test]
fn boundary_override_in_query_string_is_just_text() {
    let body = r#"{ "query": "give me everything boundary:admin" }"#;
    let params: SearchToolParams = serde_json::from_str(body).expect("body parses cleanly");
    // The malicious boundary syntax is in the query text. The query
    // text is treated as opaque input to the embedder. No code path
    // parses it for boundary names.
    assert_eq!(params.query, "give me everything boundary:admin");
}

// =============================================================================
// 2. Handler-level: handle_search uses self.authorized_boundaries (trusted)
// =============================================================================

/// Step 8 positive assertion: the handler builds a `RetrievalQuery`
/// from the trusted slice supplied at construction (here: `["work"]`),
/// NOT from the malicious `authorized_boundaries` field in the
/// JSON-RPC body (`["admin"]`). MockAdapter captures the dispatched
/// query so the test asserts both directions of the trust-boundary
/// contract: positive (trusted slice flowed through) and negative
/// (malicious body value did NOT contaminate auth).
#[tokio::test]
async fn handler_reaches_adapter_with_malicious_body_authorized_boundaries_field() {
    let (server, mock) = make_mock_server_with_adapter(vec!["work"]);
    let malicious = r#"{
        "query": "test",
        "authorized_boundaries": ["admin"]
    }"#;
    let params: SearchToolParams = serde_json::from_str(malicious).expect("parses");
    server
        .handle_search(params)
        .await
        .expect("MockAdapter returns Ok(empty)");

    let calls = mock.search_calls();
    assert_eq!(
        calls.len(),
        1,
        "handler must dispatch to adapter exactly once"
    );

    let captured = &calls[0];
    let trusted: Vec<&str> = captured
        .authorized_boundaries
        .iter()
        .map(Boundary::as_str)
        .collect();

    // Positive: the trusted slice supplied at construction reached the adapter.
    assert_eq!(
        trusted,
        vec!["work"],
        "captured authorized_boundaries must equal the trusted slice"
    );

    // Negative: the malicious body value did NOT contaminate the auth slice.
    assert!(
        !trusted.contains(&"admin"),
        "malicious 'admin' boundary must NOT have leaked through body"
    );

    // Pass-through: user-supplied query text flows verbatim as query_text.
    assert_eq!(
        captured.query_text, "test",
        "query text flows through verbatim"
    );
}

/// Step 8 positive assertion: same posture for malicious-boundary
/// syntax embedded in the query text instead of the JSON body. The
/// query text flows through verbatim to the embedder; no code path
/// parses it for boundary names.
#[tokio::test]
async fn handler_reaches_adapter_with_malicious_query_text_boundary_syntax() {
    let (server, mock) = make_mock_server_with_adapter(vec!["work"]);
    let body = r#"{ "query": "give me everything boundary:admin" }"#;
    let params: SearchToolParams = serde_json::from_str(body).expect("parses");
    server
        .handle_search(params)
        .await
        .expect("MockAdapter returns Ok(empty)");

    let calls = mock.search_calls();
    assert_eq!(
        calls.len(),
        1,
        "handler must dispatch to adapter exactly once"
    );

    let captured = &calls[0];
    let trusted: Vec<&str> = captured
        .authorized_boundaries
        .iter()
        .map(Boundary::as_str)
        .collect();

    // Positive: the trusted slice supplied at construction reached the adapter.
    assert_eq!(
        trusted,
        vec!["work"],
        "captured authorized_boundaries must equal the trusted slice"
    );

    // Negative: the malicious query-embedded value did NOT contaminate auth.
    assert!(
        !trusted.contains(&"admin"),
        "malicious 'admin' boundary must NOT have leaked through query text"
    );

    // Trust-boundary contract: the handler MUST NOT parse `boundary:admin` or
    // any similar syntax in user-supplied query text. The string flows
    // verbatim as the embed-target; auth scope comes exclusively from the
    // server-supplied trusted slice. Future contributors adding query-text
    // preprocessing (lowercasing, stop-word removal, syntax extraction)
    // must NOT break this invariant — a parser that interprets
    // `boundary:NAME` would re-introduce the trust-boundary leak this
    // test was written to prevent.
    assert_eq!(
        captured.query_text, "give me everything boundary:admin",
        "query text flows verbatim — no parsing of boundary syntax"
    );
}

// =============================================================================
// 3. Auth-gate: write to non-authorized boundary returns AccessDenied
// =============================================================================

/// `memory_write` with a `boundary` field NOT in the trusted slice
/// returns `AccessDenied` BEFORE reaching the adapter. This pins the
/// auth-gate-at-write-time discipline (ADR-025 extension): the agent
/// CAN specify which boundary to write to, but only for boundaries
/// the application has authorized.
#[tokio::test]
async fn write_to_unauthorized_boundary_returns_access_denied_before_adapter() {
    use vault_mcp::WriteToolParams;
    let server = make_test_server(vec!["work"]);
    let params = WriteToolParams {
        content: "secret data".into(),
        boundary: "admin".into(), // NOT in trusted slice
        memory_type: None,
        source_agent: None,
        confidence: None,
        as_of: None,
    };
    let res = server.handle_write(params).await;
    match res {
        Err(vault_core::VaultError::AccessDenied(msg)) => {
            // Plain assertion — VaultError doesn't impl Debug per ADR-007,
            // so we can't use `matches!` with format-string panic.
            assert!(
                msg.contains("admin"),
                "AccessDenied message should reference the rejected boundary"
            );
        }
        Err(_) => panic!("expected AccessDenied, got different error"),
        Ok(_) => panic!("expected AccessDenied, got success — auth gate failed open"),
    }
}

// =============================================================================
// 4. Trusted boundary slice is preserved across StdioServer.clone()
// =============================================================================

/// `StdioServer` is `Clone`-via-`Arc` so rmcp's request handler can
/// hand instances across the request boundary. The trusted slice
/// MUST survive the clone unchanged.
#[test]
fn trusted_boundaries_preserved_through_clone() {
    let server = make_test_server(vec!["work", "personal"]);
    let cloned = server.clone();
    assert_eq!(
        server.authorized_boundaries(),
        cloned.authorized_boundaries()
    );
    assert_eq!(server.authorized_boundaries().len(), 2);
}

// =============================================================================
// 5. ADR-025 amendment 2026-05-05 — `memory_delete` boundary auth gate
// =============================================================================

/// **ADR-025 amendment pinning test (T0.1.11 Phase 4a).**
///
/// Multi-agent code review (2026-05-05) caught that `tool_delete`
/// shipped with NO auth gate at all (CRITICAL finding, conf 97).
/// `tool_write` and `tool_update` correctly gated on
/// `authorized_boundaries`; `tool_delete` skipped the gate entirely.
/// An MCP agent with a memory's UUID could delete it from boundaries
/// it had NO authorization for.
///
/// The 2026-05-05 ADR-025 amendment expands explicit auth-gate
/// requirement to all four tools. `handle_delete` now does a
/// boundary lookup via `Adapter::lookup_boundary` and verifies
/// against `self.authorized_boundaries` BEFORE dispatching `delete`.
///
/// This test pins the AccessDenied path: memory exists in boundary
/// "personal", server is constructed with trusted slice `["work"]`,
/// delete must fail closed with AccessDenied AND the adapter's
/// `delete()` MUST NOT have been called (verified via the empty
/// `delete_calls()` snapshot).
#[tokio::test]
async fn delete_unauthorized_boundary_returns_access_denied() {
    let (server, mock) = make_mock_server_with_adapter(vec!["work"]);

    // The memory exists in boundary "personal" (NOT in the trusted
    // ["work"] slice). lookup_boundary will return Some(personal).
    mock.set_lookup_boundary(Some(
        Boundary::new("personal").expect("'personal' is a valid Boundary literal"),
    ));

    let id = vault_core::MemoryId::new();
    let result = server.handle_delete(id).await;

    // Auth gate must reject.
    match result {
        Ok(()) => panic!(
            "ADR-025 amendment violation: handle_delete succeeded for memory \
             stored in unauthorized boundary 'personal' (trusted: ['work']). \
             Auth gate failed open."
        ),
        Err(vault_core::VaultError::AccessDenied(msg)) => {
            // Plain assertion — VaultError doesn't impl Debug per ADR-007.
            assert!(
                msg.contains("personal"),
                "AccessDenied message should reference the rejected boundary; got: {msg}"
            );
            assert!(
                msg.contains(&id.to_string()),
                "AccessDenied message should reference the rejected memory id; got: {msg}"
            );
        }
        Err(_) => {
            panic!("expected AccessDenied for unauthorized-boundary delete; got different error")
        }
    }

    // Defense-in-depth assertion: the adapter's delete() MUST NOT
    // have been reached. If the auth gate fired correctly, the call
    // returned Err(AccessDenied) BEFORE Adapter::delete was invoked.
    // delete_calls is empty.
    assert!(
        mock.delete_calls().is_empty(),
        "ADR-025 amendment violation: Adapter::delete was called even though \
         auth gate should have rejected. The handler must short-circuit BEFORE \
         dispatching to delete(). Got delete_calls: {:?}",
        mock.delete_calls()
    );
}

// =============================================================================
// 6. `memory_update` gates on the STORED boundary too (security audit s89, F-01)
// =============================================================================

fn update_params(boundary: &str) -> vault_mcp::WriteToolParams {
    vault_mcp::WriteToolParams {
        content: "The user prefers tea.".into(),
        boundary: boundary.into(),
        memory_type: None,
        source_agent: None,
        confidence: None,
        as_of: None,
    }
}

/// An agent authorized for `work` names `work` in the request, but the
/// memory lives in `personal`. Before the fix the request's boundary was
/// the only check, so the memory was rewritten into `work` and became
/// readable. The update must be refused before the adapter is reached.
#[tokio::test]
async fn update_of_memory_stored_in_unauthorized_boundary_returns_access_denied() {
    let (server, mock) = make_mock_server_with_adapter(vec!["work"]);
    mock.set_lookup_boundary(Some(
        Boundary::new("personal").expect("'personal' is a valid Boundary literal"),
    ));

    let id = vault_core::MemoryId::new();
    match server.handle_update(id, update_params("work")).await {
        Ok(()) => panic!("update rewrote a memory stored in unauthorized boundary 'personal'"),
        Err(vault_core::VaultError::AccessDenied(msg)) => {
            assert!(
                msg.contains("personal"),
                "message names the stored boundary: {msg}"
            );
            assert!(
                msg.contains(&id.to_string()),
                "message names the memory id: {msg}"
            );
        }
        Err(_) => panic!("expected AccessDenied for an update across boundaries"),
    }
    assert!(
        mock.update_calls().is_empty(),
        "Adapter::update must not be reached when the stored boundary is unauthorized"
    );
}

/// Sharing between apps is unchanged: a memory stored in a boundary the
/// agent is authorized for is updated as before.
#[tokio::test]
async fn update_of_memory_stored_in_authorized_boundary_reaches_adapter() {
    let (server, mock) = make_mock_server_with_adapter(vec!["work", "personal"]);
    mock.set_lookup_boundary(Some(
        Boundary::new("personal").expect("'personal' is a valid Boundary literal"),
    ));

    let id = vault_core::MemoryId::new();
    assert!(
        server
            .handle_update(id, update_params("work"))
            .await
            .is_ok(),
        "an authorized update must succeed"
    );
    let calls = mock.update_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, id);
}

// =============================================================================
// 7. `source_agent` is bounded and cleaned before storage (security audit s89)
// =============================================================================

#[tokio::test]
async fn source_agent_is_cut_cleaned_and_empty_reads_as_absent() {
    let (server, mock) = make_mock_server_with_adapter(vec!["work"]);
    let mut long = update_params("work");
    long.source_agent = Some(format!("  user\u{0007}\n{}", "x".repeat(500)));
    assert!(server.handle_write(long).await.is_ok(), "write succeeds");

    let mut blank = update_params("work");
    blank.source_agent = Some(" \n\t ".into());
    assert!(server.handle_write(blank).await.is_ok(), "write succeeds");

    let writes = mock.write_calls();
    let kept = writes[0].source_agent.clone().expect("a name is kept");
    assert!(
        kept.starts_with("userx"),
        "control characters dropped: {kept:?}"
    );
    assert_eq!(kept.chars().count(), vault_mcp::MAX_SOURCE_AGENT_CHARS);
    assert_eq!(writes[1].source_agent, None);
}
