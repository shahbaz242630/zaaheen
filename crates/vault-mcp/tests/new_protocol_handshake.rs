//! The 2026-07-28 MCP spec's handshake (ADR-SEC-032).
//!
//! On 2026-09-24 Antigravity, freshly updated, connected to `zaaheen mcp
//! serve` and got "MCP transport bind failed: expect initialized request, but
//! received: ... CustomRequest { method: \"server/discover\", params:
//! Some(Object {}) }". It sent `initialize`, then `server/discover` (SEP-2575,
//! which servers MUST answer) with no `notifications/initialized` in between,
//! and rmcp 2.2 closed the connection. These tests replay that sequence, line
//! for line, against the relay every AI app starts and against the keeper's
//! own server, and keep the old handshake working for every other app.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use rmcp::{RoleServer, Service, ServiceExt};
use serde_json::{json, Value};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use vault_mcp::{NoVaultAdapter, RelayServer, StdioServer, Upstream, UpstreamError};

const LIMIT: Duration = Duration::from_secs(5);

/// An upstream that answers every call, so the relay has somewhere to send.
struct AnswersEverything;

#[async_trait]
impl Upstream for AnswersEverything {
    async fn call_tool(
        &self,
        _params: CallToolRequestParams,
        _deadline: Instant,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<CallToolResult, UpstreamError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("ok")]))
    }
}

/// Raw JSON-RPC lines, as an AI app writes them.
struct Wire {
    write: WriteHalf<DuplexStream>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
}

impl Wire {
    async fn open<S: Service<RoleServer>>(service: S) -> Self {
        let (client, server) = tokio::io::duplex(256 * 1024);
        tokio::spawn(async move {
            if let Ok(running) = service.serve(server).await {
                let _ = running.waiting().await;
            }
        });
        let (read, write) = tokio::io::split(client);
        Self {
            write,
            lines: BufReader::new(read).lines(),
        }
    }

    async fn send(&mut self, message: Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.write
            .write_all(line.as_bytes())
            .await
            .expect("write a line");
    }

    /// The answer to `id`; anything else on the stream is skipped. A closed
    /// stream (what rmcp 2.2 did here) fails the test with that reason.
    async fn reply(&mut self, id: u64) -> Value {
        loop {
            let line = tokio::time::timeout(LIMIT, self.lines.next_line())
                .await
                .expect("the server answered in time")
                .expect("the stream reads")
                .expect("the server kept the connection open");
            let message: Value = serde_json::from_str(&line).expect("the server writes JSON");
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        self.reply(id).await
    }
}

fn initialize(version: &str) -> Value {
    json!({
        "protocolVersion": version,
        "capabilities": {},
        "clientInfo": { "name": "antigravity", "version": "2.1" }
    })
}

/// `server/discover`'s params as the 2026-07-28 spec sends them: the
/// request's own `_meta` carries the protocol version and the capabilities.
/// (Antigravity's error line showed `params: Some(Object {})`, but rmcp moves
/// `_meta` out of the params before printing them, so whether it sent these
/// is for the live test to show; both cases are tested here.)
fn discover_params() -> Value {
    json!({ "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    }})
}

/// Antigravity's sequence: `initialize` (id 0), then `server/discover` (id 1)
/// straight away, never `notifications/initialized`; then the session is used
/// as normal.
async fn antigravity_sequence(wire: &mut Wire, version: &str) {
    let hello = wire.request(0, "initialize", initialize(version)).await;
    assert!(hello.get("result").is_some(), "initialize: {hello}");

    let discover = wire.request(1, "server/discover", discover_params()).await;
    assert!(
        discover.get("error").is_none(),
        "server/discover: {discover}"
    );
    assert!(
        discover["result"]["supportedVersions"].is_array(),
        "server/discover lists the versions: {discover}"
    );

    let tools = wire.request(2, "tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"memory_read"), "{tools}");
}

#[tokio::test]
async fn the_relay_answers_antigravitys_discover_after_initialize() {
    for version in ["2025-11-25", "2026-07-28"] {
        let mut wire = Wire::open(RelayServer::new(Arc::new(AnswersEverything))).await;
        antigravity_sequence(&mut wire, version).await;
        let call = wire
            .request(
                3,
                "tools/call",
                json!({ "name": "memory_read", "arguments": { "query": "q" } }),
            )
            .await;
        assert_eq!(
            call["result"]["content"][0]["text"],
            json!("ok"),
            "{version}: the call reaches the keeper: {call}"
        );
    }
}

#[tokio::test]
async fn the_keepers_server_answers_antigravitys_discover_after_initialize() {
    for version in ["2025-11-25", "2026-07-28"] {
        let mut wire = Wire::open(StdioServer::new(Arc::new(NoVaultAdapter), Vec::new())).await;
        antigravity_sequence(&mut wire, version).await;
    }
}

/// A `server/discover` WITHOUT the spec's `_meta` (if that is what Antigravity
/// sent) gets an error answer, and the connection stays open and usable. What
/// failed live was rmcp 2.2 closing the connection on it.
#[tokio::test]
async fn a_bare_discover_is_answered_and_the_session_carries_on() {
    bare_discover_then_carry_on(Wire::open(RelayServer::new(Arc::new(AnswersEverything))).await)
        .await;
    bare_discover_then_carry_on(
        Wire::open(StdioServer::new(Arc::new(NoVaultAdapter), Vec::new())).await,
    )
    .await;
}

async fn bare_discover_then_carry_on(mut wire: Wire) {
    let hello = wire
        .request(0, "initialize", initialize("2025-11-25"))
        .await;
    assert!(hello.get("result").is_some(), "{hello}");
    let discover = wire.request(1, "server/discover", json!({})).await;
    assert!(
        discover.get("result").is_some() || discover.get("error").is_some(),
        "an answer, not a closed connection: {discover}"
    );
    let tools = wire.request(2, "tools/list", json!({})).await;
    assert!(
        tools["result"]["tools"].is_array(),
        "still serving: {tools}"
    );
}

/// Every app that still speaks the old handshake (Claude, Cursor, ChatGPT on
/// 2026-09-24) keeps working exactly as before.
#[tokio::test]
async fn the_old_handshake_still_works() {
    let mut wire = Wire::open(RelayServer::new(Arc::new(AnswersEverything))).await;
    let hello = wire
        .request(0, "initialize", initialize("2025-06-18"))
        .await;
    assert_eq!(
        hello["result"]["serverInfo"]["name"],
        json!("zaaheen"),
        "{hello}"
    );
    wire.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    let call = wire
        .request(
            1,
            "tools/call",
            json!({ "name": "memory_read", "arguments": { "query": "q" } }),
        )
        .await;
    assert_eq!(call["result"]["content"][0]["text"], json!("ok"), "{call}");
}
