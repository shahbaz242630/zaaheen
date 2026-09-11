//! `RelayServer` (ADR-102): the relay answers the MCP handshake and the tool
//! list itself, forwards every tool call to the keeper, and resends a call
//! whose connection died mid-flight ONLY when repeating it is harmless.
//!
//! Driven end to end over an in-memory stream with a real rmcp client, so
//! these exercise the same routing an AI app does.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResult, Content};
use rmcp::service::RunningService;
use rmcp::{ErrorData as McpError, RoleClient, ServiceExt};
use vault_mcp::{
    RelayServer, Upstream, UpstreamError, MSG_OUTCOME_UNKNOWN, MSG_TIMED_OUT, MSG_TIMED_OUT_SAVE,
};

/// Plays back a fixed script of results and records every tool it was asked
/// to forward.
#[derive(Default)]
struct ScriptedUpstream {
    script: Mutex<VecDeque<Result<CallToolResult, UpstreamError>>>,
    forwarded: Mutex<Vec<String>>,
}

impl ScriptedUpstream {
    fn with(script: Vec<Result<CallToolResult, UpstreamError>>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            forwarded: Mutex::new(Vec::new()),
        })
    }

    fn forwarded(&self) -> Vec<String> {
        self.forwarded.lock().unwrap().clone()
    }
}

#[async_trait]
impl Upstream for ScriptedUpstream {
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
    ) -> Result<CallToolResult, UpstreamError> {
        self.forwarded.lock().unwrap().push(params.name.to_string());
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(UpstreamError::NotSent("script exhausted")))
    }
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
        let result = client.peer().call_tool(call(tool)).await;
        assert!(result.is_ok(), "{tool} should succeed on the resend");
        assert_eq!(
            upstream.forwarded(),
            [tool, tool],
            "{tool}: exactly one resend"
        );
    }
}

/// A save whose reply was lost may already have committed. Resending it
/// would store the memory twice, so it is never resent and the agent is told
/// to check first.
#[tokio::test]
async fn saves_are_never_resent_after_a_lost_connection() {
    for tool in ["memory_write", "memory_update"] {
        let upstream = ScriptedUpstream::with(vec![Err(UpstreamError::Lost), ok()]);
        let client = connect(upstream.clone()).await;
        let err = client
            .peer()
            .call_tool(call(tool))
            .await
            .expect_err("a lost save must surface, not be retried");
        assert!(
            err.to_string().contains("check with memory_read"),
            "{tool}: the agent must be told to check before saving again; got {err}"
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
    let err = client
        .peer()
        .call_tool(call("memory_read"))
        .await
        .expect_err("timeout surfaces");
    assert!(err.to_string().contains(MSG_TIMED_OUT), "got {err}");
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
        let err = client
            .peer()
            .call_tool(call(tool))
            .await
            .expect_err("timeout surfaces");
        assert!(
            err.to_string().contains(MSG_TIMED_OUT_SAVE),
            "{tool}: got {err}"
        );
        assert!(
            !err.to_string().contains("try again"),
            "{tool}: must not invite a blind retry; got {err}"
        );
        assert_eq!(upstream.forwarded(), [tool], "{tool}: never resent");
    }
}

/// "Could not reach the keeper" carries its reason to the agent unchanged.
#[tokio::test]
async fn a_call_that_never_reached_the_keeper_reports_why() {
    let upstream = ScriptedUpstream::with(vec![Err(UpstreamError::NotSent(
        "the vault is starting; try again in a moment",
    ))]);
    let client = connect(upstream.clone()).await;
    let err = client
        .peer()
        .call_tool(call("memory_write"))
        .await
        .expect_err("surfaces");
    assert!(err.to_string().contains("vault is starting"), "got {err}");
    assert_eq!(upstream.forwarded(), ["memory_write"]);
}

/// The keeper's own errors (boundary denials, invalid params) pass through
/// untouched — the relay adds no interpretation.
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
