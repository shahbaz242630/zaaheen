//! The subscription gate, `EntitledService` (ADR-104 + ADR-SEC-022;
//! `SIGNIN-DESIGN.md` §8.26 §6.1, "§6.1" below).
//!
//! Driven over an in-memory stream with raw JSON-RPC lines, which is exactly
//! what an AI app writes. Raw lines, not an rmcp client, because the gate must
//! be tested against every request kind a client can send, and a client has no
//! method for some of them (tasks, custom requests).
//!
//! The gated server is the real `StdioServer` over the recording
//! `MockAdapter`, wrapped in a `Recorder` that notes every request and
//! notification reaching it. So "never reached the vault" is checked twice:
//! nothing reached the server, and the adapter recorded no call and no audit
//! row.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use common::{make_mock_server_with_adapter, MockAdapter};
use rmcp::model::{ClientNotification, ClientRequest, ServerInfo, ServerResult};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::{ErrorData as McpError, RoleServer, Service, ServiceExt};
use serde_json::{json, Value};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use tokio::sync::{Notify, Semaphore};
use tracing::Instrument;
use vault_mcp::{EntitledService, EntitlementCheck, InFlight, LockReason, StdioServer, Verdict};

/// No wait in these tests may exceed this (BRD §7: no test over 5 s).
const LIMIT: Duration = Duration::from_secs(4);

const ALL_REASONS: [LockReason; 5] = [
    LockReason::SignedOut,
    LockReason::CannotConfirm,
    LockReason::TrialEnded,
    LockReason::SubscriptionEnded,
    LockReason::Unlocking,
];

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

/// Answers what it is told, counts how often it was asked, and can be held
/// until the test releases it.
struct ScriptedCheck {
    verdict: Mutex<Verdict>,
    asked: AtomicUsize,
    hold: Option<Arc<Notify>>,
}

impl ScriptedCheck {
    fn answering(verdict: Verdict) -> Arc<Self> {
        Arc::new(Self {
            verdict: Mutex::new(verdict),
            asked: AtomicUsize::new(0),
            hold: None,
        })
    }

    fn held(verdict: Verdict, hold: Arc<Notify>) -> Arc<Self> {
        Arc::new(Self {
            verdict: Mutex::new(verdict),
            asked: AtomicUsize::new(0),
            hold: Some(hold),
        })
    }

    fn set(&self, verdict: Verdict) {
        *self.verdict.lock().unwrap() = verdict;
    }

    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EntitlementCheck for ScriptedCheck {
    async fn check(&self) -> Verdict {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if let Some(hold) = &self.hold {
            hold.notified().await;
        }
        *self.verdict.lock().unwrap()
    }
}

/// What the `Recorder` does with a tool call before passing it on.
#[derive(Clone)]
enum OnToolCall {
    PassOn,
    /// Wait for a permit. Permits accumulate, so a release that lands before
    /// the call starts waiting is never missed.
    WaitFor(Arc<Semaphore>),
    Crash,
}

/// Stands in front of the vault's real server and records what reaches it.
struct Recorder {
    inner: StdioServer,
    requests: Arc<Mutex<Vec<&'static str>>>,
    notifications: Arc<Mutex<Vec<String>>>,
    on_tool_call: OnToolCall,
}

/// The `ClientRequest` variant a request was decoded as. A request whose
/// params do not decode becomes `CustomRequest`, so the entitled tests below
/// also prove each test request reaches the arm it is meant to.
fn kind(request: &ClientRequest) -> &'static str {
    match request {
        ClientRequest::PingRequest(_) => "ping",
        ClientRequest::InitializeRequest(_) => "initialize",
        ClientRequest::CompleteRequest(_) => "completion/complete",
        ClientRequest::SetLevelRequest(_) => "logging/setLevel",
        ClientRequest::GetPromptRequest(_) => "prompts/get",
        ClientRequest::ListPromptsRequest(_) => "prompts/list",
        ClientRequest::ListResourcesRequest(_) => "resources/list",
        ClientRequest::ListResourceTemplatesRequest(_) => "resources/templates/list",
        ClientRequest::ReadResourceRequest(_) => "resources/read",
        ClientRequest::SubscribeRequest(_) => "resources/subscribe",
        ClientRequest::UnsubscribeRequest(_) => "resources/unsubscribe",
        ClientRequest::CallToolRequest(_) => "tools/call",
        ClientRequest::ListToolsRequest(_) => "tools/list",
        ClientRequest::GetTaskRequest(_) => "tasks/get",
        ClientRequest::ListTasksRequest(_) => "tasks/list",
        ClientRequest::GetTaskPayloadRequest(_) => "tasks/result",
        ClientRequest::CancelTaskRequest(_) => "tasks/cancel",
        ClientRequest::CustomRequest(_) => "custom",
    }
}

impl Service<RoleServer> for Recorder {
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, McpError> {
        self.requests.lock().unwrap().push(kind(&request));
        if matches!(request, ClientRequest::CallToolRequest(_)) {
            match &self.on_tool_call {
                OnToolCall::PassOn => {}
                OnToolCall::WaitFor(release) => {
                    let _permit = release.acquire().await;
                }
                OnToolCall::Crash => panic!("the server crashed mid-call (test)"),
            }
        }
        self.inner.handle_request(request, context).await
    }

    async fn handle_notification(
        &self,
        notification: ClientNotification,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), McpError> {
        let method = serde_json::to_value(&notification)
            .ok()
            .and_then(|v| v["method"].as_str().map(str::to_string))
            .unwrap_or_default();
        self.notifications.lock().unwrap().push(method);
        self.inner.handle_notification(notification, context).await
    }

    fn get_info(&self) -> ServerInfo {
        self.inner.get_info()
    }
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

/// A client that writes raw JSON-RPC lines, as an AI app does.
struct Wire {
    write: WriteHalf<DuplexStream>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    next_id: u64,
    hello: Value,
}

impl Wire {
    async fn write_line(&mut self, message: Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.write
            .write_all(line.as_bytes())
            .await
            .expect("write a request line");
    }

    async fn send(&mut self, method: &str, params: Option<Value>) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(params) = params {
            message["params"] = params;
        }
        self.write_line(message).await;
        id
    }

    async fn notify(&mut self, method: &str, params: Option<Value>) {
        let mut message = json!({ "jsonrpc": "2.0", "method": method });
        if let Some(params) = params {
            message["params"] = params;
        }
        self.write_line(message).await;
    }

    /// The answer to request `id` (anything else on the stream is skipped).
    async fn reply(&mut self, id: u64) -> Value {
        loop {
            let line = tokio::time::timeout(LIMIT, self.lines.next_line())
                .await
                .expect("the server answered in time")
                .expect("the stream reads")
                .expect("the stream is still open");
            let message: Value = serde_json::from_str(&line).expect("the server writes JSON");
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    async fn request(&mut self, method: &str, params: Option<Value>) -> Value {
        let id = self.send(method, params).await;
        self.reply(id).await
    }

    async fn call_tool(&mut self, tool: &str, arguments: Value) -> Value {
        self.request(
            "tools/call",
            Some(json!({ "name": tool, "arguments": arguments })),
        )
        .await
    }
}

/// Serve `service` on one end of an in-memory stream and complete the MCP
/// handshake from the other.
///
/// The server runs inside the caller's span, so a `traced_test` sees the
/// server's log lines as its own (tracing-test keeps only lines carrying the
/// test's span).
async fn open<S: Service<RoleServer>>(service: S) -> Wire {
    let (client, server) = tokio::io::duplex(256 * 1024);
    tokio::spawn(
        async move {
            if let Ok(running) = service.serve(server).await {
                let _ = running.waiting().await;
            }
        }
        .instrument(tracing::Span::current()),
    );
    let (read, write) = tokio::io::split(client);
    let mut wire = Wire {
        write,
        lines: BufReader::new(read).lines(),
        next_id: 0,
        hello: Value::Null,
    };
    let hello = wire
        .request(
            "initialize",
            Some(json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "gate-test", "version": "0" }
            })),
        )
        .await;
    assert!(
        hello.get("result").is_some(),
        "the handshake must succeed: {hello}"
    );
    wire.hello = hello;
    wire.notify("notifications/initialized", None).await;
    wire
}

/// A gated vault, and everything a test inspects about it.
struct Rig {
    wire: Wire,
    check: Arc<ScriptedCheck>,
    adapter: Arc<MockAdapter>,
    in_flight: InFlight,
    requests: Arc<Mutex<Vec<&'static str>>>,
    notifications: Arc<Mutex<Vec<String>>>,
}

impl Rig {
    async fn new(verdict: Verdict) -> Self {
        Self::with(ScriptedCheck::answering(verdict), OnToolCall::PassOn).await
    }

    async fn with(check: Arc<ScriptedCheck>, on_tool_call: OnToolCall) -> Self {
        let (server, adapter) = make_mock_server_with_adapter(vec!["work"]);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let recorder = Recorder {
            inner: server,
            requests: requests.clone(),
            notifications: notifications.clone(),
            on_tool_call,
        };
        let in_flight = InFlight::new();
        let gated = EntitledService::new(recorder, check.clone(), in_flight.clone());
        let wire = open(gated).await;
        Self {
            wire,
            check,
            adapter,
            in_flight,
            requests,
            notifications,
        }
    }

    /// Requests that reached the server, the handshake left out.
    fn reached(&self) -> Vec<&'static str> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .copied()
            .filter(|k| *k != "initialize")
            .collect()
    }

    fn notified(&self) -> Vec<String> {
        self.notifications.lock().unwrap().clone()
    }

    /// Nothing reached the vault: no adapter call of any kind, no audit row.
    fn assert_vault_untouched(&self) {
        assert!(self.adapter.search_calls().is_empty(), "a search ran");
        assert!(self.adapter.read_calls().is_empty(), "a read ran");
        assert!(self.adapter.write_calls().is_empty(), "a save ran");
        assert!(self.adapter.update_calls().is_empty(), "an update ran");
        assert!(self.adapter.delete_calls().is_empty(), "a delete ran");
        assert!(
            self.adapter.recorded_audits().is_empty(),
            "a refused call must not write to the vault's audit log (§6.6)"
        );
    }
}

/// Polls `condition` until it holds, failing after [`LIMIT`].
async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + LIMIT;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The text of a tool result flagged `isError` (ADR-103 D2).
fn tool_error_text(reply: &Value) -> String {
    let result = reply.get("result").unwrap_or_else(|| {
        panic!("a locked tool call must be a tool result, not a protocol error: {reply}")
    });
    assert_eq!(
        result["isError"],
        json!(true),
        "a locked tool call must be flagged isError: {reply}"
    );
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal must carry text: {reply}"))
        .to_string()
}

fn search_args() -> Value {
    json!({ "query": "what do I like to eat" })
}

/// Every request kind the gate must refuse while locked, with params that
/// decode as that kind, and the kind each must reach when entitled.
fn other_requests() -> Vec<(&'static str, Option<Value>, &'static str)> {
    let uri = json!({ "uri": "file:///notes.txt" });
    let task = json!({ "taskId": "t1" });
    vec![
        (
            "completion/complete",
            Some(json!({
                "ref": { "type": "ref/prompt", "name": "p" },
                "argument": { "name": "a", "value": "b" }
            })),
            "completion/complete",
        ),
        (
            "logging/setLevel",
            Some(json!({ "level": "info" })),
            "logging/setLevel",
        ),
        ("prompts/get", Some(json!({ "name": "p" })), "prompts/get"),
        ("resources/read", Some(uri.clone()), "resources/read"),
        (
            "resources/subscribe",
            Some(uri.clone()),
            "resources/subscribe",
        ),
        ("resources/unsubscribe", Some(uri), "resources/unsubscribe"),
        ("tasks/get", Some(task.clone()), "tasks/get"),
        ("tasks/list", None, "tasks/list"),
        ("tasks/result", Some(task.clone()), "tasks/result"),
        ("tasks/cancel", Some(task), "tasks/cancel"),
        ("zaaheen/anything", Some(json!({ "x": 1 })), "custom"),
    ]
}

// ---------------------------------------------------------------------------
// The fixed wording (§8.26 §6 "Messages")
// ---------------------------------------------------------------------------

#[test]
fn each_lock_reason_says_exactly_the_locked_words() {
    assert_eq!(
        LockReason::SignedOut.message(),
        "Zaaheen needs you to sign in. Open the Zaaheen app on this computer to sign in."
    );
    assert_eq!(
        LockReason::CannotConfirm.message(),
        "Zaaheen couldn't confirm your subscription. Check your internet connection and try again."
    );
    assert_eq!(
        LockReason::TrialEnded.message(),
        "Your Zaaheen free trial has ended. Open the Zaaheen app and choose Subscribe to keep \
         using your memories. Nothing has been deleted."
    );
    assert_eq!(
        LockReason::SubscriptionEnded.message(),
        "Your Zaaheen subscription has ended. Open the Zaaheen app and choose Subscribe to keep \
         using your memories. Nothing has been deleted."
    );
    assert_eq!(
        LockReason::Unlocking.message(),
        "Zaaheen is unlocking. Try again in a moment."
    );
}

// ---------------------------------------------------------------------------
// What always passes
// ---------------------------------------------------------------------------

/// ADR-102 K8: an AI app's startup probe gives a server well under a second,
/// so the handshake, `ping` and the tool list must never wait on a refresh.
#[tokio::test]
async fn the_handshake_ping_and_tool_list_never_ask_the_check() {
    let mut rig = Rig::new(Verdict::Locked(LockReason::TrialEnded)).await;
    let pong = rig.wire.request("ping", None).await;
    assert!(pong.get("result").is_some(), "ping must answer: {pong}");
    let tools = rig.wire.request("tools/list", None).await;
    assert!(
        tools.get("result").is_some(),
        "tools/list must answer: {tools}"
    );
    assert_eq!(
        rig.check.asked(),
        0,
        "initialize, ping and tools/list must not consult the check"
    );
}

/// A locked vault looks the same to the AI app: same name, same five tools
/// with the same descriptions, so nothing changes in the app's tool list.
#[tokio::test]
async fn a_locked_vault_introduces_itself_and_lists_its_tools_unchanged() {
    let mut rig = Rig::new(Verdict::Locked(LockReason::SubscriptionEnded)).await;
    let (plain_server, _adapter) = make_mock_server_with_adapter(vec!["work"]);
    let mut plain = open(plain_server).await;

    assert_eq!(
        rig.wire.hello["result"]["serverInfo"]["name"],
        json!("zaaheen")
    );
    assert_eq!(
        rig.wire.hello["result"]["serverInfo"],
        plain.hello["result"]["serverInfo"]
    );
    let locked_tools = rig.wire.request("tools/list", None).await;
    let plain_tools = plain.request("tools/list", None).await;
    assert_eq!(locked_tools["result"], plain_tools["result"]);
    assert_eq!(
        locked_tools["result"]["tools"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        5
    );
}

// ---------------------------------------------------------------------------
// Tool calls
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_entitled_tool_call_reaches_the_vault() {
    let mut rig = Rig::new(Verdict::Entitled).await;
    let reply = rig.wire.call_tool("memory_search", search_args()).await;
    assert!(
        reply.get("result").is_some(),
        "the search must answer: {reply}"
    );
    assert_ne!(reply["result"]["isError"], json!(true), "{reply}");
    assert_eq!(rig.adapter.search_calls().len(), 1);
    assert_eq!(rig.check.asked(), 1);
}

/// The agent must READ the reason to tell the user (ADR-103 D2), so each
/// reason arrives as a tool result flagged `isError` with its exact words.
#[tokio::test]
async fn a_locked_tool_call_is_a_tool_error_the_agent_can_read() {
    for reason in ALL_REASONS {
        let mut rig = Rig::new(Verdict::Locked(reason)).await;
        let reply = rig.wire.call_tool("memory_search", search_args()).await;
        assert_eq!(
            tool_error_text(&reply),
            reason.message(),
            "wrong words for {reason:?}"
        );
        assert!(
            !reason.message().is_empty(),
            "{reason:?} must say something"
        );
    }
}

#[tokio::test]
async fn a_locked_tool_call_never_reaches_the_vault_or_its_audit_log() {
    let mut rig = Rig::new(Verdict::Locked(LockReason::TrialEnded)).await;
    let id = "00000000-0000-0000-0000-000000000001";
    let calls = [
        ("memory_search", search_args()),
        ("memory_read", json!({ "query": "where do I live" })),
        (
            "memory_write",
            json!({ "content": "I live in Dubai", "boundary": "work" }),
        ),
        (
            "memory_update",
            json!({ "id": id, "content": "I live in Abu Dhabi", "boundary": "work" }),
        ),
        ("memory_delete", json!({ "id": id })),
    ];
    for (tool, arguments) in calls {
        let reply = rig.wire.call_tool(tool, arguments).await;
        assert_eq!(
            tool_error_text(&reply),
            LockReason::TrialEnded.message(),
            "{tool}"
        );
    }
    assert!(
        !rig.reached().contains(&"tools/call"),
        "a locked tool call reached the server: {:?}",
        rig.reached()
    );
    rig.assert_vault_untouched();
}

/// Refresh-then-decide runs per call (§8.26 §4): nothing is cached, so a
/// change in either direction applies to the very next call.
#[tokio::test]
async fn every_call_asks_the_check_so_a_change_applies_at_once() {
    let mut rig = Rig::new(Verdict::Entitled).await;
    let first = rig.wire.call_tool("memory_search", search_args()).await;
    assert_ne!(first["result"]["isError"], json!(true), "{first}");

    rig.check
        .set(Verdict::Locked(LockReason::SubscriptionEnded));
    let second = rig.wire.call_tool("memory_search", search_args()).await;
    assert_eq!(
        tool_error_text(&second),
        LockReason::SubscriptionEnded.message()
    );

    rig.check.set(Verdict::Entitled);
    let third = rig.wire.call_tool("memory_search", search_args()).await;
    assert_ne!(third["result"]["isError"], json!(true), "{third}");

    assert_eq!(rig.check.asked(), 3);
    assert_eq!(rig.adapter.search_calls().len(), 2);
}

/// A task-style caller expects a task, not a tool result. Our tools forbid
/// tasks, so the server refuses such a call with invalid-params; while locked
/// the gate gives that same answer itself, without passing the call on.
#[tokio::test]
async fn a_locked_task_style_call_gets_the_servers_own_answer_without_reaching_it() {
    let params = json!({ "name": "memory_search", "arguments": search_args(), "task": {} });

    let mut open_rig = Rig::new(Verdict::Entitled).await;
    let servers_answer = open_rig
        .wire
        .request("tools/call", Some(params.clone()))
        .await;
    assert!(
        servers_answer.get("error").is_some(),
        "our server refuses task-style calls: {servers_answer}"
    );
    assert_eq!(servers_answer["error"]["code"], json!(-32602));

    let mut locked = Rig::new(Verdict::Locked(LockReason::TrialEnded)).await;
    let gates_answer = locked.wire.request("tools/call", Some(params)).await;
    assert_eq!(gates_answer["error"], servers_answer["error"]);
    assert!(
        !locked.reached().contains(&"tools/call"),
        "a locked task-style call reached the server"
    );
    locked.assert_vault_untouched();
}

// ---------------------------------------------------------------------------
// Every other request
// ---------------------------------------------------------------------------

/// The prompt and resource lists are empty, so they are answered either way,
/// and without asking the check: asking could only add a refresh's wait
/// (`SIGNIN-DESIGN.md` §8.32).
#[tokio::test]
async fn prompt_and_resource_lists_are_answered_even_while_locked() {
    let mut rig = Rig::new(Verdict::Locked(LockReason::TrialEnded)).await;
    for (method, field) in [
        ("prompts/list", "prompts"),
        ("resources/list", "resources"),
        ("resources/templates/list", "resourceTemplates"),
    ] {
        let reply = rig.wire.request(method, None).await;
        assert_eq!(reply["result"][field], json!([]), "{method}: {reply}");
    }
    assert_eq!(
        rig.reached(),
        ["prompts/list", "resources/list", "resources/templates/list"]
    );
    assert_eq!(rig.check.asked(), 0, "the lists must not wait on a refresh");
    rig.assert_vault_untouched();
}

/// Deny by default: every request kind that is not named as safe is refused
/// while locked, with the fixed words, and never reaches the server.
#[tokio::test]
async fn every_other_request_is_refused_while_locked_and_never_reaches_the_server() {
    let mut rig = Rig::new(Verdict::Locked(LockReason::TrialEnded)).await;
    for (method, params, _) in other_requests() {
        let reply = rig.wire.request(method, params).await;
        let error = reply
            .get("error")
            .unwrap_or_else(|| panic!("{method} must be refused while locked: {reply}"));
        assert_eq!(
            error["message"],
            json!(LockReason::TrialEnded.message()),
            "{method}: {reply}"
        );
    }
    assert!(
        rig.reached().is_empty(),
        "refused requests reached the server: {:?}",
        rig.reached()
    );
    assert_eq!(rig.check.asked(), other_requests().len());
    rig.assert_vault_untouched();
}

/// The same requests, entitled, are passed on, each as the kind it is.
#[tokio::test]
async fn every_other_request_is_passed_on_while_entitled() {
    let mut rig = Rig::new(Verdict::Entitled).await;
    let expected: Vec<&str> = other_requests().iter().map(|r| r.2).collect();
    for (method, params, _) in other_requests() {
        let _ = rig.wire.request(method, params).await;
    }
    assert_eq!(rig.reached(), expected);
}

/// Notifications carry no user data and ask for nothing back.
#[tokio::test]
async fn notifications_pass_through_even_while_locked() {
    let mut rig = Rig::new(Verdict::Locked(LockReason::SignedOut)).await;
    rig.wire
        .notify("notifications/roots/list_changed", None)
        .await;
    eventually("the notification reaches the server", || {
        rig.notified()
            .contains(&"notifications/roots/list_changed".to_string())
    })
    .await;
    assert_eq!(rig.check.asked(), 0);
}

// ---------------------------------------------------------------------------
// Calls in flight (§6.2: the keeper lets a call finish before a mode change)
// ---------------------------------------------------------------------------

/// Counted from arrival, before the check has decided: otherwise the keeper
/// could see zero and change mode between the check saying yes and the call
/// starting.
#[tokio::test]
async fn a_call_is_counted_while_the_check_is_still_deciding() {
    let release = Arc::new(Notify::new());
    let check = ScriptedCheck::held(Verdict::Entitled, release.clone());
    let mut rig = Rig::with(check, OnToolCall::PassOn).await;

    let id = rig
        .wire
        .send(
            "tools/call",
            Some(json!({ "name": "memory_search", "arguments": search_args() })),
        )
        .await;
    eventually("the check is asked", || rig.check.asked() == 1).await;
    assert_eq!(rig.in_flight.count(), 1);

    release.notify_one();
    let reply = rig.wire.reply(id).await;
    assert_ne!(reply["result"]["isError"], json!(true), "{reply}");
    eventually("the count returns to zero", || rig.in_flight.count() == 0).await;
}

#[tokio::test]
async fn a_call_the_vault_is_working_on_is_counted_until_it_answers() {
    let release = Arc::new(Semaphore::new(0));
    let check = ScriptedCheck::answering(Verdict::Entitled);
    let mut rig = Rig::with(check, OnToolCall::WaitFor(release.clone())).await;

    let id = rig
        .wire
        .send(
            "tools/call",
            Some(json!({ "name": "memory_search", "arguments": search_args() })),
        )
        .await;
    eventually("the call reaches the server", || {
        rig.reached().contains(&"tools/call")
    })
    .await;
    assert_eq!(rig.in_flight.count(), 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rig.in_flight.wait_idle())
            .await
            .is_err(),
        "wait_idle must wait while a call is in flight"
    );

    release.add_permits(1);
    let _ = rig.wire.reply(id).await;
    tokio::time::timeout(LIMIT, rig.in_flight.wait_idle())
        .await
        .expect("wait_idle resolves once the call has answered");
    assert_eq!(rig.in_flight.count(), 0);
}

/// A call whose handler dies (a panic unwinds and drops it) must not leave
/// the count stuck above zero, or the keeper would never change mode.
#[tokio::test]
async fn a_call_that_crashes_is_no_longer_counted() {
    let check = ScriptedCheck::answering(Verdict::Entitled);
    let mut rig = Rig::with(check, OnToolCall::Crash).await;
    let _ = rig
        .wire
        .send(
            "tools/call",
            Some(json!({ "name": "memory_search", "arguments": search_args() })),
        )
        .await;
    eventually("the call reaches the server", || {
        rig.reached().contains(&"tools/call")
    })
    .await;
    eventually("the crashed call is no longer counted", || {
        rig.in_flight.count() == 0
    })
    .await;
}

#[tokio::test]
async fn every_connection_counts_into_the_same_counter() {
    let release = Arc::new(Semaphore::new(0));
    let in_flight = InFlight::new();
    let mut wires = Vec::new();
    for _ in 0..2 {
        let (server, _adapter) = make_mock_server_with_adapter(vec!["work"]);
        let recorder = Recorder {
            inner: server,
            requests: Arc::new(Mutex::new(Vec::new())),
            notifications: Arc::new(Mutex::new(Vec::new())),
            on_tool_call: OnToolCall::WaitFor(release.clone()),
        };
        let check = ScriptedCheck::answering(Verdict::Entitled);
        wires.push(open(EntitledService::new(recorder, check, in_flight.clone())).await);
    }
    for wire in &mut wires {
        let _ = wire
            .send(
                "tools/call",
                Some(json!({ "name": "memory_search", "arguments": search_args() })),
            )
            .await;
    }
    eventually("both calls are counted", || in_flight.count() == 2).await;
    release.add_permits(2);
    tokio::time::timeout(LIMIT, in_flight.wait_idle())
        .await
        .expect("both calls finish");
}

#[tokio::test]
async fn wait_idle_returns_at_once_when_nothing_is_in_flight() {
    tokio::time::timeout(Duration::from_millis(200), InFlight::new().wait_idle())
        .await
        .expect("nothing in flight: wait_idle returns at once");
}

// ---------------------------------------------------------------------------
// Logging and the source
// ---------------------------------------------------------------------------

/// §6.6: a refused call goes to the application log, never to the vault, and
/// the log line carries the reason, never what the agent sent.
#[tokio::test]
#[tracing_test::traced_test]
async fn a_refused_call_is_logged_with_its_reason_and_without_its_arguments() {
    let mut rig = Rig::new(Verdict::Locked(LockReason::TrialEnded)).await;
    let secret = "my private diary entry about the hospital";
    let reply = rig
        .wire
        .call_tool(
            "memory_write",
            json!({ "content": secret, "boundary": "work" }),
        )
        .await;
    let _ = tool_error_text(&reply);

    logs_assert(|lines: &[&str]| {
        let gate: Vec<&&str> = lines
            .iter()
            .filter(|l| l.contains("vault_mcp::gate"))
            .collect();
        if gate.is_empty() {
            return Err("no vault_mcp::gate line for the refused call".into());
        }
        if !gate.iter().any(|l| l.contains("TrialEnded")) {
            return Err(format!("the gate line must name the reason: {gate:?}"));
        }
        if gate.iter().any(|l| l.contains(secret)) {
            return Err("the gate logged what the agent sent".into());
        }
        Ok(())
    });
    rig.assert_vault_untouched();
}

/// §6.1: "a future rmcp variant fails to compile". That holds only while the
/// gate's `match` names every variant and has no catch-all arm.
#[test]
fn the_gate_names_every_request_kind_and_has_no_catch_all() {
    let source = include_str!("../src/gate.rs");
    assert!(
        !source.contains("_ =>"),
        "a catch-all arm would let a new request kind through unchecked"
    );
    for variant in [
        "PingRequest",
        "InitializeRequest",
        "CompleteRequest",
        "SetLevelRequest",
        "GetPromptRequest",
        "ListPromptsRequest",
        "ListResourcesRequest",
        "ListResourceTemplatesRequest",
        "ReadResourceRequest",
        "SubscribeRequest",
        "UnsubscribeRequest",
        "CallToolRequest",
        "ListToolsRequest",
        "GetTaskRequest",
        "ListTasksRequest",
        "GetTaskPayloadRequest",
        "CancelTaskRequest",
        "CustomRequest",
    ] {
        assert!(
            source.contains(&format!("ClientRequest::{variant}")),
            "the gate's match must name ClientRequest::{variant}"
        );
    }
}
