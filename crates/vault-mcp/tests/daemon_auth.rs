//! Security tests for the multi-agent daemon auth gate (ADR-SEC-001, Step 6).
//!
//! Drives the real [`vault_mcp::DaemonServer`] over rmcp streamable-HTTP on
//! loopback (the production transport) and asserts the capability-token gate:
//!
//! 1. **Scoping (D3/D4):** a request bearing a VALID token reaches the adapter
//!    scoped to exactly that agent's authorized boundaries.
//! 2. **Auth-bypass denied (BRD §11.4.4, SP-4):** a tool call with NO token, or
//!    an unknown/forged token, is rejected and NEVER reaches the adapter — the
//!    same generic failure for both (no info leak).
//!
//! These exercise the daemon-specific seam (header → token → boundary scope).
//! The per-tool boundary enforcement itself (e.g. write to an unauthorized
//! boundary → AccessDenied) is already pinned by `tests/trust_boundary.rs`
//! against `StdioServer`, which `DaemonServer` dispatches through unchanged.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{
    StreamableHttpClientTransport, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::ServiceExt;
use tokio::net::TcpListener;
use vault_core::{Boundary, MemoryId, NewMemory, VaultResult};
use vault_mcp::{
    AccountNotice, Adapter, DaemonServer, EntitlementCheck, Gate, InFlight, LockReason,
    ToolInvokeDetails, Verdict,
};
use vault_retrieval::{
    HealthInfo, HealthStatus, ReadQuery, RetrievalQuery, RetrievedMemory, StructuredReadResponse,
};

/// The single agent token this fixture recognises. The daemon hashes the
/// presented bearer token and looks it up, so the test mints the hash the same
/// way (`hash_capability_token`).
const VALID_TOKEN: &str = "tok-work-agent";

/// Adapter that recognises ONE token (scoped to `work`) and records the
/// `authorized_boundaries` of every search it receives — so the test can prove
/// the daemon scoped the request to the token's boundaries (and that denied
/// requests never reach `search` at all).
#[derive(Default)]
struct AuthMockAdapter {
    search_boundaries: Mutex<Vec<Vec<Boundary>>>,
}

impl AuthMockAdapter {
    fn recorded_searches(&self) -> Vec<Vec<Boundary>> {
        self.search_boundaries.lock().expect("poisoned").clone()
    }
}

#[async_trait]
impl Adapter for AuthMockAdapter {
    async fn search(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        self.search_boundaries
            .lock()
            .expect("poisoned")
            .push(query.authorized_boundaries.clone());
        Ok(Vec::new())
    }

    async fn read(&self, _query: ReadQuery) -> VaultResult<StructuredReadResponse> {
        Ok(StructuredReadResponse {
            boundary: None,
            query: String::new(),
            relevant_facts: Vec::new(),
            abstain: true,
            top_relevance: 0.0,
            health: HealthInfo {
                status: HealthStatus::Ok,
                warnings: Vec::new(),
            },
        })
    }

    async fn write(&self, _new_memory: NewMemory) -> VaultResult<MemoryId> {
        Ok(MemoryId::new())
    }

    async fn update(&self, _id: MemoryId, _new_memory: NewMemory) -> VaultResult<()> {
        Ok(())
    }

    async fn delete(&self, _id: MemoryId) -> VaultResult<()> {
        Ok(())
    }

    async fn lookup_boundary(&self, _id: MemoryId) -> VaultResult<Option<Boundary>> {
        Ok(None)
    }

    async fn append_tool_invoke_audit(&self, _details: ToolInvokeDetails) -> VaultResult<()> {
        Ok(())
    }

    async fn resolve_token_boundaries(
        &self,
        token_hash: &str,
    ) -> VaultResult<Option<(String, Vec<Boundary>)>> {
        if token_hash == vault_storage::hash_capability_token(VALID_TOKEN) {
            Ok(Some((
                "work-agent".to_string(),
                vec![Boundary::new("work").expect("valid boundary")],
            )))
        } else {
            Ok(None)
        }
    }
}

/// Mount a `DaemonServer` on a loopback ephemeral port (the production transport).
async fn spawn_daemon(daemon: DaemonServer) -> std::net::SocketAddr {
    let service = StreamableHttpService::new(
        move || Ok::<_, std::io::Error>(daemon.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        loop {
            let (stream, _peer) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };
            let io = TokioIo::new(stream);
            let hyper_service = TowerToHyperService::new(service.clone());
            tokio::spawn(async move {
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(io, hyper_service)
                    .await;
            });
        }
    });
    addr
}

fn search_call(query: &str) -> CallToolRequestParams {
    let mut params = CallToolRequestParams::new("memory_search");
    params.arguments = serde_json::json!({ "query": query }).as_object().cloned();
    params
}

/// A VALID token's request reaches the adapter scoped to exactly that agent's
/// authorized boundaries (`work`) — proving per-request token → boundary scoping.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_token_scopes_request_to_its_boundaries() {
    let adapter = Arc::new(AuthMockAdapter::default());
    let daemon = DaemonServer::new(adapter.clone() as Arc<dyn Adapter>, None);
    let addr = spawn_daemon(daemon).await;
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    let client = ()
        .serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url).auth_header(VALID_TOKEN),
        ))
        .await
        .expect("valid-token agent initializes");

    let result = client
        .peer()
        .call_tool(search_call("anything"))
        .await
        .expect("authenticated search dispatches");
    assert_ne!(
        result.is_error,
        Some(true),
        "an authenticated search must not error"
    );
    drop(client);

    let recorded = adapter.recorded_searches();
    assert_eq!(
        recorded.len(),
        1,
        "exactly one search should have reached the adapter"
    );
    assert_eq!(
        recorded[0],
        vec![Boundary::new("work").expect("valid boundary")],
        "the request MUST be scoped to the token's authorized boundaries (work), \
         not anything the request body could claim"
    );
}

/// A tool call with NO `Authorization` header is denied and NEVER reaches the
/// adapter (SP-4 fail-secure).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_token_is_denied_before_the_adapter() {
    let adapter = Arc::new(AuthMockAdapter::default());
    let daemon = DaemonServer::new(adapter.clone() as Arc<dyn Adapter>, None);
    let addr = spawn_daemon(daemon).await;
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    // No auth_header → no Authorization header on the wire.
    let client = ()
        .serve(StreamableHttpClientTransport::from_uri(url))
        .await
        .expect("no-token client still completes the (un-gated) initialize");

    let result = client.peer().call_tool(search_call("anything")).await;
    assert!(
        result.is_err(),
        "a tool call with no capability token MUST be rejected"
    );
    drop(client);

    assert!(
        adapter.recorded_searches().is_empty(),
        "a denied request must NEVER reach the adapter (no boundary scope to leak)"
    );
}

/// An unknown / forged token is denied with the same generic failure as a
/// missing one (no info leak distinguishing "no token" from "bad token").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forged_token_is_denied_before_the_adapter() {
    let adapter = Arc::new(AuthMockAdapter::default());
    let daemon = DaemonServer::new(adapter.clone() as Arc<dyn Adapter>, None);
    let addr = spawn_daemon(daemon).await;
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    let client = ()
        .serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url).auth_header("not-a-real-token"),
        ))
        .await
        .expect("forged-token client completes the (un-gated) initialize");

    let result = client.peer().call_tool(search_call("anything")).await;
    assert!(
        result.is_err(),
        "a tool call with an unknown/forged token MUST be rejected"
    );
    drop(client);

    assert!(
        adapter.recorded_searches().is_empty(),
        "a forged-token request must NEVER reach the adapter"
    );
}

// ---------------------------------------------------------------------------
// The subscription gate (ADR-104; SIGNIN-DESIGN.md 8.26 6.1)
// ---------------------------------------------------------------------------

/// A check that answers the same verdict every time and counts the asking.
struct FixedCheck {
    verdict: Verdict,
    asked: Arc<AtomicUsize>,
}

#[async_trait]
impl EntitlementCheck for FixedCheck {
    async fn check(&self) -> Verdict {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.verdict
    }
}

/// Entitled, with a notice for the agent (§8.40).
struct NoticeCheck(AccountNotice);

#[async_trait]
impl EntitlementCheck for NoticeCheck {
    async fn check(&self) -> Verdict {
        Verdict::Entitled
    }

    async fn check_with_notice(&self) -> (Verdict, Option<AccountNotice>) {
        (Verdict::Entitled, Some(self.0))
    }
}

/// The daemon asks the check itself rather than being wrapped, so it must
/// carry a served call's notice to `memory_read` the way the gate does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_passes_an_account_notice_to_memory_read() {
    let adapter = Arc::new(AuthMockAdapter::default());
    let gate = Gate::new(
        Arc::new(NoticeCheck(AccountNotice::PaymentFailed)),
        InFlight::new(),
    );
    let daemon = DaemonServer::new(adapter as Arc<dyn Adapter>, Some(gate));
    let addr = spawn_daemon(daemon).await;
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    let client = ()
        .serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url).auth_header(VALID_TOKEN),
        ))
        .await
        .expect("the agent initializes");
    let mut read = CallToolRequestParams::new("memory_read");
    read.arguments = serde_json::json!({ "query": "where do I live" })
        .as_object()
        .cloned();
    let result = client
        .peer()
        .call_tool(read)
        .await
        .expect("an entitled read dispatches");
    drop(client);

    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect();
    let answer: serde_json::Value = serde_json::from_str(&text).expect("the read answers JSON");
    assert_eq!(
        answer["health"]["warnings"][0]["code"],
        serde_json::json!("SUBSCRIPTION_PAYMENT_FAILED"),
        "{answer}"
    );
}

fn gate_answering(verdict: Verdict) -> (Gate, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    let check = Arc::new(FixedCheck {
        verdict,
        asked: asked.clone(),
    });
    (Gate::new(check, InFlight::new()), asked)
}

/// A locked vault answers the agent in words it can read, and the call never
/// reaches the adapter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_locked_daemon_refuses_an_authenticated_call() {
    let adapter = Arc::new(AuthMockAdapter::default());
    let (gate, asked) = gate_answering(Verdict::Locked(LockReason::TrialEnded));
    let daemon = DaemonServer::new(adapter.clone() as Arc<dyn Adapter>, Some(gate));
    let addr = spawn_daemon(daemon).await;
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    let client = ()
        .serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url).auth_header(VALID_TOKEN),
        ))
        .await
        .expect("the agent initializes");

    let result = client
        .peer()
        .call_tool(search_call("anything"))
        .await
        .expect("a locked call is a tool result, not a protocol error");
    assert_eq!(result.is_error, Some(true));
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect();
    assert_eq!(text, LockReason::TrialEnded.message());
    drop(client);

    assert_eq!(asked.load(Ordering::SeqCst), 1, "the check is asked once");
    assert!(
        adapter.recorded_searches().is_empty(),
        "a locked call must never reach the adapter"
    );
}

/// An entitled vault serves exactly as before the gate existed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_entitled_daemon_serves_the_call() {
    let adapter = Arc::new(AuthMockAdapter::default());
    let (gate, asked) = gate_answering(Verdict::Entitled);
    let daemon = DaemonServer::new(adapter.clone() as Arc<dyn Adapter>, Some(gate));
    let addr = spawn_daemon(daemon).await;
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    let client = ()
        .serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url).auth_header(VALID_TOKEN),
        ))
        .await
        .expect("the agent initializes");

    let result = client
        .peer()
        .call_tool(search_call("anything"))
        .await
        .expect("an entitled search dispatches");
    assert_ne!(result.is_error, Some(true));
    drop(client);

    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.recorded_searches().len(), 1);
}

/// BRD 11.4.4: authentication comes first. An unknown token is refused
/// without the subscription ever being consulted, so it learns nothing about
/// the account.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_token_is_refused_before_the_subscription_is_consulted() {
    let adapter = Arc::new(AuthMockAdapter::default());
    let (gate, asked) = gate_answering(Verdict::Entitled);
    let daemon = DaemonServer::new(adapter.clone() as Arc<dyn Adapter>, Some(gate));
    let addr = spawn_daemon(daemon).await;
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    let client = ()
        .serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url).auth_header("not-a-real-token"),
        ))
        .await
        .expect("the client completes the un-gated initialize");

    let result = client.peer().call_tool(search_call("anything")).await;
    assert!(result.is_err(), "an unknown token must be rejected");
    drop(client);

    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "the check must not be asked for a request that never authenticated"
    );
    assert!(adapter.recorded_searches().is_empty());
}
