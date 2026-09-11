//! `RelayServer` (ADR-102): the relay answers the MCP handshake and the tool
//! list itself, forwards every tool call to the keeper, and resends a call
//! whose connection died mid-flight ONLY when repeating it is harmless.
//!
//! ADR-103: one deadline per incoming call, shared by the resend; and the
//! relay's own failures reach the agent as tool results with `isError`, not
//! as protocol errors a client reduces to "Tool execution failed".
//!
//! Driven end to end over an in-memory stream with a real rmcp client, so
//! these exercise the same routing an AI app does.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResult, Content};
use rmcp::service::RunningService;
use rmcp::{ErrorData as McpError, RoleClient, ServiceExt};
use vault_mcp::{
    RelayServer, Upstream, UpstreamError, MSG_OUTCOME_UNKNOWN, MSG_TIMED_OUT, MSG_TIMED_OUT_SAVE,
    RELAY_CALL_BUDGET,
};

/// Plays back a fixed script of results and records every tool it was asked
/// to forward, with the deadline each attempt was given.
#[derive(Default)]
struct ScriptedUpstream {
    script: Mutex<VecDeque<Result<CallToolResult, UpstreamError>>>,
    forwarded: Mutex<Vec<String>>,
    deadlines: Mutex<Vec<Instant>>,
}

impl ScriptedUpstream {
    fn with(script: Vec<Result<CallToolResult, UpstreamError>>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            forwarded: Mutex::new(Vec::new()),
            deadlines: Mutex::new(Vec::new()),
        })
    }

    fn forwarded(&self) -> Vec<String> {
        self.forwarded.lock().unwrap().clone()
    }

    fn deadlines(&self) -> Vec<Instant> {
        self.deadlines.lock().unwrap().clone()
    }
}

#[async_trait]
impl Upstream for ScriptedUpstream {
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        deadline: Instant,
    ) -> Result<CallToolResult, UpstreamError> {
        self.forwarded.lock().unwrap().push(params.name.to_string());
        self.deadlines.lock().unwrap().push(deadline);
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(UpstreamError::NotSent("script exhausted")))
    }
}

/// The text of a tool result the agent must be able to READ as a failure:
/// `isError: true` with our message in its content (ADR-103 D2). A protocol
/// error instead would reach Claude Desktop's model as "Tool execution
/// failed", with the message dropped — observed live 2026-09-11.
fn tool_error_text(result: &CallToolResult) -> String {
    assert_eq!(
        result.is_error,
        Some(true),
        "a relay failure must be a tool result flagged isError; got {result:?}"
    );
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join(" ")
}

async fn connect(upstream: Arc<ScriptedUpstream>) -> RunningService<RoleClient, ()> {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = RelayServer::new(upstream);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    ().serve(client_io)
        .await
        .expect("relay completes the handshake")
}

fn call(tool: &str) -> CallToolRequestParams {
    let mut params = CallToolRequestParams::new(tool.to_string());
    params.arguments = serde_json::json!({ "query": "anything" })
        .as_object()
        .cloned();
    params
}

fn ok() -> Result<CallToolResult, UpstreamError> {
    Ok(CallToolResult::success(vec![Content::text("ok")]))
}

/// The handshake and `tools/list` must never wait on the keeper: Claude
/// Desktop's startup probe gives a server well under a second.
#[tokio::test]
async fn the_handshake_and_tool_list_never_touch_the_keeper() {
    let upstream = ScriptedUpstream::with(vec![]);
    let client = connect(upstream.clone()).await;

    let info = client.peer_info().expect("server info after initialize");
    assert_eq!(info.server_info.name, "zaaheen");

    let tools = client.peer().list_all_tools().await.expect("tools/list");
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "memory_delete",
            "memory_read",
            "memory_search",
            "memory_update",
            "memory_write"
        ],
        "the relay advertises exactly the keeper's tool contract"
    );
    assert!(
        upstream.forwarded().is_empty(),
        "nothing may be forwarded before a tool is actually called"
    );
}

#[tokio::test]
async fn a_tool_call_is_forwarded_and_its_result_returned() {
    let upstream = ScriptedUpstream::with(vec![ok()]);
    let client = connect(upstream.clone()).await;
    let result = client
        .peer()
        .call_tool(call("memory_search"))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    assert_eq!(upstream.forwarded(), ["memory_search"]);
}

/// Reads, searches and deletes are resent once after the connection died
/// mid-call: repeating them changes nothing.
#[tokio::test]
async fn repeat_safe_tools_are_resent_once_after_a_lost_connection() {
    for tool in ["memory_read", "memory_search", "memory_delete"] {
        let upstream = ScriptedUpstream::with(vec![Err(UpstreamError::Lost), ok()]);
        let client = connect(upstream.clone()).await;
        let result = client
            .peer()
            .call_tool(call(tool))
            .await
            .expect("the resend's result is returned");
        assert_ne!(
            result.is_error,
            Some(true),
            "{tool} should succeed on the resend"
        );
        assert_eq!(
            upstream.forwarded(),
            [tool, tool],
            "{tool}: exactly one resend"
        );
    }
}

/// ADR-103 D1: the resend does NOT get a fresh budget. Before, a read whose
/// connection died late could run 15 s + 35 s twice and outlive the client
/// (review round 3, K7). Both attempts carry the one deadline fixed when the
/// call arrived.
#[tokio::test]
async fn the_resend_shares_the_first_attempts_deadline() {
    let upstream = ScriptedUpstream::with(vec![Err(UpstreamError::Lost), ok()]);
    let client = connect(upstream.clone()).await;
    client
        .peer()
        .call_tool(call("memory_read"))
        .await
        .expect("served on the resend");
    let deadlines = upstream.deadlines();
    assert_eq!(deadlines.len(), 2, "one attempt plus one resend");
    assert_eq!(
        deadlines[0], deadlines[1],
        "the resend must share the first attempt's deadline, not start a new budget"
    );
}

/// ADR-103 D1: the deadline is `RELAY_CALL_BUDGET` from when the call
/// arrived — the whole call, however it is spent (finding the keeper, the
/// call itself, a resend).
#[tokio::test]
async fn the_deadline_is_the_call_budget_from_arrival() {
    let upstream = ScriptedUpstream::with(vec![ok()]);
    let client = connect(upstream.clone()).await;
    let before = Instant::now();
    client
        .peer()
        .call_tool(call("memory_search"))
        .await
        .expect("served");
    let after = Instant::now();
    let deadline = upstream.deadlines()[0];
    assert!(
        deadline >= before + RELAY_CALL_BUDGET && deadline <= after + RELAY_CALL_BUDGET,
        "the deadline must be the call budget measured from arrival"
    );
}

/// The budget must let a slow keeper answer before a client gives up — and
/// never outlive the client. The floor is the live evidence (2026-09-11): an
/// already-running keeper answered a correct `memory_read` at 36.852 s and the
/// old 35 s cut-off threw the answer away.
#[test]
fn the_call_budget_fits_inside_the_clients_timeout_and_above_the_observed_answer() {
    // MCP TypeScript SDK and Codex default: 60 s. Keep 5 s for the reply.
    assert!(RELAY_CALL_BUDGET <= Duration::from_secs(55));
    assert!(RELAY_CALL_BUDGET > Duration::from_millis(36_852));
}

/// A save whose reply was lost may already have committed. Resending it
/// would store the memory twice, so it is never resent and the agent is told
/// to check first.
#[tokio::test]
async fn saves_are_never_resent_after_a_lost_connection() {
    for tool in ["memory_write", "memory_update"] {
        let upstream = ScriptedUpstream::with(vec![Err(UpstreamError::Lost), ok()]);
        let client = connect(upstream.clone()).await;
        let result = client
            .peer()
            .call_tool(call(tool))
            .await
            .expect("a relay failure is a tool result the agent can read");
        let text = tool_error_text(&result);
        assert!(
            text.contains("check with memory_read"),
            "{tool}: the agent must be told to check before saving again; got {text}"
        );
        assert_eq!(upstream.forwarded(), [tool], "{tool}: never resent");
    }
    assert!(MSG_OUTCOME_UNKNOWN.contains("memory_read"));
}

/// A slow keeper is not a dead one: a timeout is never resent, for any tool.
#[tokio::test]
async fn a_timeout_is_never_resent() {
    let upstream = ScriptedUpstream::with(vec![Err(UpstreamError::TimedOut), ok()]);
    let client = connect(upstream.clone()).await;
    let result = client
        .peer()
        .call_tool(call("memory_read"))
        .await
        .expect("a relay failure is a tool result the agent can read");
    let text = tool_error_text(&result);
    assert!(text.contains(MSG_TIMED_OUT), "got {text}");
    assert_eq!(upstream.forwarded(), ["memory_read"]);
}

/// The keeper keeps working after the relay stops waiting, so a timed-out
/// save may still land. "Try again" would store it twice; the agent is told
/// to check first — the same rule as a save whose connection was lost.
#[tokio::test]
async fn a_timed_out_save_says_check_before_saving_again() {
    for tool in ["memory_write", "memory_update"] {
        let upstream = ScriptedUpstream::with(vec![Err(UpstreamError::TimedOut), ok()]);
        let client = connect(upstream.clone()).await;
        let result = client
            .peer()
            .call_tool(call(tool))
            .await
            .expect("a relay failure is a tool result the agent can read");
        let text = tool_error_text(&result);
        assert!(text.contains(MSG_TIMED_OUT_SAVE), "{tool}: got {text}");
        assert!(
            !text.contains("try again"),
            "{tool}: must not invite a blind retry; got {text}"
        );
        assert_eq!(upstream.forwarded(), [tool], "{tool}: never resent");
    }
}

/// "Could not reach the keeper" carries its reason to the agent unchanged —
/// starting, busy (maintenance), update needed, key changed: each is advice
/// the agent must be able to read and pass on.
#[tokio::test]
async fn a_call_that_never_reached_the_keeper_reports_why() {
    let upstream = ScriptedUpstream::with(vec![Err(UpstreamError::NotSent(
        "the vault is starting; try again in a moment",
    ))]);
    let client = connect(upstream.clone()).await;
    let result = client
        .peer()
        .call_tool(call("memory_write"))
        .await
        .expect("a relay failure is a tool result the agent can read");
    let text = tool_error_text(&result);
    assert!(text.contains("vault is starting"), "got {text}");
    assert_eq!(upstream.forwarded(), ["memory_write"]);
}

/// The keeper's own errors (boundary denials, invalid params) pass through
/// untouched — the relay adds no interpretation, so relay mode and direct
/// mode fail identically for them (ADR-103 D2 changes only the relay's OWN
/// failures).
#[tokio::test]
async fn keeper_errors_pass_through_unchanged() {
    let keeper_error = McpError::invalid_params("invalid params", None);
    let upstream = ScriptedUpstream::with(vec![Err(UpstreamError::Keeper(keeper_error))]);
    let client = connect(upstream.clone()).await;
    let err = client
        .peer()
        .call_tool(call("memory_write"))
        .await
        .expect_err("surfaces");
    assert!(err.to_string().contains("invalid params"), "got {err}");
    assert_eq!(upstream.forwarded(), ["memory_write"]);
}
