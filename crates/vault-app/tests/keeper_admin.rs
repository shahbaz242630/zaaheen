//! D4 (ADR-108, ADR-SEC-033): the desktop app's admin connection to a real
//! keeper serve loop, on a real named pipe (unix socket off Windows).
//!
//! These use a LOCKED keeper over a real encrypted database: it needs no
//! models, so every test stays well under the 5 s budget, and lock mode is
//! where the admin surface's rules are sharpest — the lock screen's two open
//! tools work, every gated one is refused without touching anything, and a
//! computer with no vault yet gets no vault created.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use vault_app::admin::{AdminGate, AdminHost, LockedHost};
use vault_app::entitlement::ModeCheck;
use vault_app::keeper::clients;
use vault_app::keeper::discovery;
use vault_app::keeper::handshake::{admin_handshake, relay_handshake, HandshakeKeys};
use vault_app::keeper::runtime::{self, AdminSide, KeeperExit, KeeperSettings, Subscription};
use vault_app::keeper::transport;
use vault_core::{Boundary, MemoryType, NewMemory, VaultResult};
use vault_mcp::{EntitlementCheck, InFlight, LockReason, NoVaultAdapter, Verdict};
use vault_storage::{MetadataStore, SqlCipherKey};

const MASTER_KEY: [u8; 32] = [0x42; 32];
const STEP: Duration = Duration::from_secs(2);

/// A check whose answer the test can change, counting how often a keeper
/// asked it through each door.
struct ScriptedCheck {
    verdict: Mutex<Verdict>,
    checks: AtomicUsize,
    peeks: AtomicUsize,
}

impl ScriptedCheck {
    fn answering(verdict: Verdict) -> Arc<Self> {
        Arc::new(Self {
            verdict: Mutex::new(verdict),
            checks: AtomicUsize::new(0),
            peeks: AtomicUsize::new(0),
        })
    }

    fn set(&self, verdict: Verdict) {
        *self.verdict.lock().unwrap() = verdict;
    }
}

#[async_trait]
impl EntitlementCheck for ScriptedCheck {
    async fn check(&self) -> Verdict {
        self.checks.fetch_add(1, Ordering::SeqCst);
        *self.verdict.lock().unwrap()
    }
}

#[async_trait]
impl ModeCheck for ScriptedCheck {
    async fn peek(&self) -> Verdict {
        self.peeks.fetch_add(1, Ordering::SeqCst);
        *self.verdict.lock().unwrap()
    }

    async fn refresh_and_peek(&self) -> Verdict {
        *self.verdict.lock().unwrap()
    }
}

fn db_key() -> SqlCipherKey {
    SqlCipherKey::new("d4-admin-test-key")
}

fn settings(root: &std::path::Path) -> KeeperSettings {
    KeeperSettings {
        vault_root: root.to_path_buf(),
        version: "test".into(),
        idle_exit: Duration::from_secs(30),
        idle_check: Duration::from_millis(100),
        frame1_deadline: Duration::from_millis(500),
        handshake_deadline: STEP,
        max_pending_handshakes: 8,
        drain: Duration::from_secs(1),
    }
}

/// A locked keeper with the desktop's admin side, exactly as `vault-cli`'s
/// `serve_locked` builds one.
async fn start_locked_keeper(
    root: &std::path::Path,
    check: Arc<ScriptedCheck>,
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<VaultResult<KeeperExit>>,
) {
    start_locked_keeper_keyed(root, check, MASTER_KEY).await
}

async fn start_locked_keeper_keyed(
    root: &std::path::Path,
    check: Arc<ScriptedCheck>,
    master_key: [u8; 32],
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<VaultResult<KeeperExit>>,
) {
    start_with(root, check, master_key, false).await
}

/// `full_gate`: the admin side gates as an entitled keeper does (a disk
/// peek), with no subscription ticking, so the keeper stays put for the test.
async fn start_with(
    root: &std::path::Path,
    check: Arc<ScriptedCheck>,
    master_key: [u8; 32],
    full_gate: bool,
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<VaultResult<KeeperExit>>,
) {
    let subscription = Subscription::lock(Arc::clone(&check), InFlight::new());
    let gate = if full_gate {
        AdminGate::Full(Arc::clone(&check) as Arc<dyn ModeCheck>)
    } else {
        AdminGate::Locked {
            check: subscription.check(),
            flip: subscription.flip().clone(),
        }
    };
    let admin = AdminSide {
        host: Arc::new(AdminHost::Locked(LockedHost::new(
            root.join("vault.db"),
            db_key(),
        ))),
        gate: Arc::new(gate),
    };
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(runtime::serve(
        Arc::new(NoVaultAdapter),
        Some(admin),
        HandshakeKeys::derive(&master_key),
        settings(root),
        (!full_gate).then_some(subscription),
        async {
            let _ = stop_rx.await;
        },
    ));
    let deadline = tokio::time::Instant::now() + STEP;
    while discovery::read(root).ok().flatten().is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "keeper never published"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (stop_tx, handle)
}

/// The desktop's side of the pipe: the admin handshake, then an MCP client.
async fn admin_client(root: &std::path::Path) -> RunningService<RoleClient, ()> {
    let record = discovery::read(root).unwrap().unwrap();
    let identity = record.keeper_identity(root).unwrap();
    let mut stream = transport::connect(&identity.endpoint).await.unwrap();
    admin_handshake(
        &mut stream,
        &HandshakeKeys::derive(&MASTER_KEY),
        &identity,
        [1u8; 32],
        STEP,
    )
    .await
    .unwrap();
    ().serve(stream).await.unwrap()
}

async fn call(client: &RunningService<RoleClient, ()>, tool: &str, args: Value) -> CallToolResult {
    let mut params = CallToolRequestParams::new(tool.to_string());
    params.arguments = args.as_object().cloned();
    client.peer().call_tool(params).await.unwrap()
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

/// A real encrypted database with one memory, as a vault on disk would have.
async fn make_vault(root: &std::path::Path) {
    let store = MetadataStore::open(root.join("vault.db"), db_key())
        .await
        .unwrap();
    let memory = vault_core::Memory::try_new(NewMemory {
        content: "I prefer tea".into(),
        memory_type: MemoryType::Semantic,
        boundary: Boundary::new("default").unwrap(),
        source_agent: Some("test".into()),
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: json!({}),
    })
    .unwrap();
    store.create_memory(&memory).await.unwrap();
}

/// The lock screen's numbers and "Download my memories" work on a locked
/// computer: export is always available (BRD §1.6 amendment 1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_locked_keeper_still_serves_the_settings_and_the_export() {
    let tmp = tempfile::TempDir::new().unwrap();
    make_vault(tmp.path()).await;
    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::TrialEnded));
    let (stop, keeper) = start_locked_keeper(tmp.path(), check).await;
    let client = admin_client(tmp.path()).await;

    let settings = call(&client, "admin_settings_info", json!({})).await;
    assert_ne!(settings.is_error, Some(true), "{}", text(&settings));
    let settings: Value = serde_json::from_str(&text(&settings)).unwrap();
    assert_eq!(settings["memory_count"], 1);
    assert_eq!(settings["audit_chain_verified"], true);

    let page = call(&client, "admin_export_page", json!({ "limit": 50 })).await;
    let page: Vec<vault_core::Memory> = serde_json::from_str(&text(&page)).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].content, "I prefer tea");
    assert!(page[0].embedding.is_none(), "exports carry no embeddings");

    drop(client);
    let _ = stop.send(());
    assert_eq!(keeper.await.unwrap().unwrap(), KeeperExit::Shutdown);
}

/// "Download my memories" page by page: every memory exactly once, even when
/// several were saved in the same instant, and a bad cursor is refused
/// (ADR-108 D2; the storage cursor is also pinned in vault-storage).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_export_pages_cover_every_memory_exactly_once() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = MetadataStore::open(tmp.path().join("vault.db"), db_key())
        .await
        .unwrap();
    let same_moment = chrono::Utc::now();
    let mut saved = Vec::new();
    for i in 0..5 {
        let mut m = vault_core::Memory::try_new(NewMemory {
            content: format!("fact {i}"),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new("personal").unwrap(),
            source_agent: None,
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: json!({}),
        })
        .unwrap();
        m.created_at = same_moment;
        store.create_memory(&m).await.unwrap();
        saved.push(m.id.to_string());
    }
    drop(store);

    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::TrialEnded));
    let (stop, keeper) = start_locked_keeper(tmp.path(), check).await;
    let client = admin_client(tmp.path()).await;

    let mut seen: Vec<String> = Vec::new();
    let mut args = json!({ "limit": 2 });
    loop {
        let page = call(&client, "admin_export_page", args.clone()).await;
        let page: Vec<vault_core::Memory> = serde_json::from_str(&text(&page)).unwrap();
        let Some(last) = page.last() else { break };
        args = json!({
            "limit": 2,
            "after_created_at": last.created_at.to_rfc3339(),
            "after_id": last.id.to_string(),
        });
        seen.extend(page.iter().map(|m| m.id.to_string()));
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "no memory twice");
    saved.sort();
    assert_eq!(unique, saved, "every memory once");

    let bad = call(
        &client,
        "admin_export_page",
        json!({ "limit": 2, "after_id": "not-an-id" }),
    )
    .await;
    assert_eq!(text(&bad), "invalid_input");

    drop(client);
    let _ = stop.send(());
    let _ = keeper.await;
}

/// Every gated tool is refused on a locked keeper, with the lock's own code,
/// and never reaches the vault.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_locked_keeper_refuses_every_gated_tool_with_the_lock_code() {
    let tmp = tempfile::TempDir::new().unwrap();
    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::TrialEnded));
    let (stop, keeper) = start_locked_keeper(tmp.path(), Arc::clone(&check)).await;
    let client = admin_client(tmp.path()).await;

    // EVERY tool the table marks gated, so a tool added without the gate
    // fails here (security review S2), plus the gated audit event.
    for (tool, args) in gated_calls() {
        let result = call(&client, tool, args).await;
        assert_eq!(result.is_error, Some(true), "{tool}");
        assert_eq!(text(&result), "locked_trial_ended", "{tool}");
    }
    assert!(
        !tmp.path().join("vault.db").exists(),
        "nothing was opened or created"
    );
    // The admin side reads the keeper's own view from disk: it never asks the
    // check the way a call does (review B-M3: no refresh, no recorded use).
    assert_eq!(check.checks.load(Ordering::SeqCst), 0);

    drop(client);
    let _ = stop.send(());
    let _ = keeper.await;
}

/// Arguments that parse for each gated admin tool. A new gated tool with no
/// entry here fails the test until it is added — and so is gate-checked.
fn args_for(tool: &str) -> Value {
    match tool {
        "admin_memory_add" => {
            json!({ "content": "x", "memory_type": "semantic", "boundary": "default" })
        }
        "admin_memory_search" => json!({ "query": "tea", "limit": 5 }),
        "admin_memory_update" => json!({
            "id": vault_core::MemoryId::new().to_string(),
            "content": "x", "memory_type": "semantic", "boundary": "default"
        }),
        "admin_memory_delete" => json!({ "id": vault_core::MemoryId::new().to_string() }),
        "admin_memory_list_recent" => json!({ "limit": 5 }),
        "admin_boundary_create" => json!({ "name": "work" }),
        "admin_agent_revoke" => json!({ "agent_name": "cursor" }),
        "admin_boundary_list"
        | "admin_agent_list"
        | "admin_engine_status"
        | "admin_engine_fetch"
        | "admin_engine_warm" => json!({}),
        other => panic!("{other} is gated but has no test arguments: add them here"),
    }
}

/// Every gated call: each gated tool in the table, and the gated audit event.
fn gated_calls() -> Vec<(&'static str, Value)> {
    let mut calls: Vec<(&'static str, Value)> = vault_app::admin::ADMIN_TOOLS
        .iter()
        .filter(|t| t.access == vault_app::admin::Access::Gated)
        .map(|t| (t.name, args_for(t.name)))
        .collect();
    assert!(calls.len() >= 12, "the table lists the gated tools");
    calls.push((
        "admin_audit_event",
        json!({ "event": "set_maintenance_schedule", "duration_ms": 1, "result_count": 1, "failed": false }),
    ));
    calls
}

/// An entitled keeper whose own view (read from disk) says locked refuses
/// every gated call too, before it reaches the vault (ADR-108 D3: the full
/// gate). Served here over a locked host, so a call that slipped past the
/// gate would answer "unlocking", not the lock's own code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_gate_that_reads_locked_refuses_every_gated_tool() {
    let tmp = tempfile::TempDir::new().unwrap();
    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::SubscriptionEnded));
    let (stop, keeper) = start_with(tmp.path(), Arc::clone(&check), MASTER_KEY, true).await;
    let client = admin_client(tmp.path()).await;
    for (tool, args) in gated_calls() {
        let result = call(&client, tool, args).await;
        assert_eq!(text(&result), "locked_subscription_ended", "{tool}");
    }
    assert!(
        check.peeks.load(Ordering::SeqCst) >= 12,
        "the full gate peeks"
    );
    assert_eq!(check.checks.load(Ordering::SeqCst), 0, "and never asks");
    drop(client);
    let _ = stop.send(());
    let _ = keeper.await;
}

/// A computer with no vault yet: the lock screen's numbers are zero and the
/// export is empty, and NOTHING is created (ADR-108's amendment to ADR-104).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_locked_keeper_with_no_vault_creates_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::SignedOut));
    let (stop, keeper) = start_locked_keeper(tmp.path(), check).await;
    let client = admin_client(tmp.path()).await;

    let settings = call(&client, "admin_settings_info", json!({})).await;
    let settings: Value = serde_json::from_str(&text(&settings)).unwrap();
    assert_eq!(settings["memory_count"], 0);
    let page = call(&client, "admin_export_page", json!({ "limit": 50 })).await;
    assert_eq!(text(&page), "[]");
    let audit = call(
        &client,
        "admin_audit_event",
        json!({ "event": "export_memories", "duration_ms": 3, "result_count": 0, "failed": false }),
    )
    .await;
    assert_ne!(audit.is_error, Some(true), "{}", text(&audit));

    assert!(
        !tmp.path().join("vault.db").exists(),
        "no database may be created"
    );

    drop(client);
    let _ = stop.send(());
    let _ = keeper.await;
}

/// The desktop is entitled but the keeper is still in lock mode (a sign-in
/// just finished): the gated call answers "unlocking", touches nothing, and
/// the keeper leaves so a full one can start (review A R2-3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gated_call_to_a_lock_mode_keeper_that_is_now_entitled_unlocks_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::SignedOut));
    let (_stop, keeper) = start_locked_keeper(tmp.path(), Arc::clone(&check)).await;
    let client = admin_client(tmp.path()).await;

    check.set(Verdict::Entitled);
    let result = call(&client, "admin_memory_list_recent", json!({ "limit": 5 })).await;
    assert_eq!(text(&result), "locked_unlocking");
    assert!(!tmp.path().join("vault.db").exists());

    let exit = tokio::time::timeout(STEP, keeper)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(exit, KeeperExit::ModeChanged);
}

/// The desktop is not an AI app: its session keeps the keeper alive but is
/// never listed in the Agents tab.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_admin_session_keeps_the_keeper_and_is_not_listed_as_an_app() {
    let tmp = tempfile::TempDir::new().unwrap();
    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::SignedOut));
    let (stop, keeper) = start_locked_keeper(tmp.path(), check).await;
    let client = admin_client(tmp.path()).await;
    let _ = call(&client, "admin_settings_info", json!({})).await;

    assert!(
        clients::read_live(tmp.path()).is_empty(),
        "the desktop is not an app"
    );
    assert!(!keeper.is_finished());

    drop(client);
    let _ = stop.send(());
    let _ = keeper.await;
}

/// A relay (a SERVE connection) never gets the admin tools: its tool list is
/// the AI apps' five, and calling an admin tool through it is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_connection_never_reaches_the_admin_tools() {
    let tmp = tempfile::TempDir::new().unwrap();
    make_vault(tmp.path()).await;
    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::TrialEnded));
    let (stop, keeper) = start_locked_keeper(tmp.path(), check).await;

    let record = discovery::read(tmp.path()).unwrap().unwrap();
    let identity = record.keeper_identity(tmp.path()).unwrap();
    let mut stream = transport::connect(&identity.endpoint).await.unwrap();
    relay_handshake(
        &mut stream,
        &HandshakeKeys::derive(&MASTER_KEY),
        &identity,
        &[Boundary::new("personal").unwrap()],
        [1u8; 32],
        STEP,
    )
    .await
    .unwrap();
    let relay = ().serve(stream).await.unwrap();
    let tools = relay.peer().list_all_tools().await.unwrap();
    assert!(
        tools.iter().all(|t| !t.name.starts_with("admin_")),
        "a relay sees no admin tool"
    );
    let mut params = CallToolRequestParams::new("admin_export_page".to_string());
    params.arguments = json!({ "limit": 5 }).as_object().cloned();
    let refused = relay.peer().call_tool(params).await;
    let exported = match refused {
        Ok(result) => text(&result).contains("I prefer tea"),
        Err(_) => false,
    };
    assert!(!exported, "the export is never reachable over a relay");

    drop(relay);
    let _ = stop.send(());
    let _ = keeper.await;
}

/// A keeper handing over lets a running call finish, turns new work away,
/// and still leaves within the drain (ADR-SEC-033 D3). Here the "running
/// call" is an admin export page on a real database.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keeper_asked_to_hand_over_leaves_within_the_drain() {
    let tmp = tempfile::TempDir::new().unwrap();
    make_vault(tmp.path()).await;
    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::TrialEnded));
    let (_stop, keeper) = start_locked_keeper(tmp.path(), check).await;
    let client = admin_client(tmp.path()).await;
    let _ = call(&client, "admin_settings_info", json!({})).await;

    let record = discovery::read(tmp.path()).unwrap().unwrap();
    let identity = record.keeper_identity(tmp.path()).unwrap();
    let mut stream = transport::connect(&identity.endpoint).await.unwrap();
    vault_app::keeper::handshake::request_handover(
        &mut stream,
        &HandshakeKeys::derive(&MASTER_KEY),
        &identity,
        [2u8; 32],
        STEP,
    )
    .await
    .unwrap();

    let started = std::time::Instant::now();
    let exit = tokio::time::timeout(Duration::from_secs(4), keeper)
        .await
        .expect("the keeper leaves within its drain")
        .unwrap()
        .unwrap();
    assert_eq!(exit, KeeperExit::HandedOver);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        discovery::read(tmp.path()).unwrap().is_none(),
        "discovery removed first"
    );
}

/// The server serves exactly the tools the table lists (which the desktop's
/// guard test checks against its own lists), no more and no fewer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_admin_server_serves_exactly_the_tools_in_the_table() {
    let tmp = tempfile::TempDir::new().unwrap();
    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::SignedOut));
    let (stop, keeper) = start_locked_keeper(tmp.path(), check).await;
    let client = admin_client(tmp.path()).await;

    let mut served: Vec<String> = client
        .peer()
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    served.sort();
    let mut listed: Vec<String> = vault_app::admin::ADMIN_TOOLS
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    listed.sort();
    assert_eq!(served, listed);

    drop(client);
    let _ = stop.send(());
    let _ = keeper.await;
}

/// The desktop and the keeper ship together, but an old window left open
/// through an update meets a new keeper: the admin tool contract is pinned to
/// the wire, like the AI apps' one, so a change without a WIRE bump fails
/// here (ADR-SEC-033 D1).
#[test]
fn the_admin_tool_contract_is_pinned_to_the_wire_version() {
    // Recorded 2026-09-24 on Windows under rmcp 3.4.1 (session 60, wire 4,
    // the first admin contract).
    const PINNED_ADMIN: &str = "f4d3f7faeca531a07e2e6dfbc93c7cf6d24fa7ead4fdb5e58ae16285edd2b6cf";

    let server = vault_app::admin::AdminServer::new(
        Arc::new(AdminHost::Locked(LockedHost::new(
            std::path::PathBuf::from("vault.db"),
            db_key(),
        ))),
        Arc::new(AdminGate::Open),
        vault_mcp::ReadDesk::new(),
    );
    let mut hasher = blake3::Hasher::new();
    let mut names: Vec<&str> = vault_app::admin::ADMIN_TOOLS
        .iter()
        .map(|t| t.name)
        .collect();
    names.sort_unstable();
    for name in names {
        let tool = rmcp::ServerHandler::get_tool(&server, name)
            .unwrap_or_else(|| panic!("{name} missing"));
        hasher.update(&serde_json::to_vec(&tool).unwrap());
    }
    let actual = hasher.finalize().to_hex().to_string();
    assert_eq!(
        (vault_app::keeper::handshake::WIRE, actual.as_str()),
        (4, PINNED_ADMIN),
        "The admin tool contract changed: bump WIRE in \
         vault_app::keeper::handshake and set PINNED_ADMIN to {actual}."
    );
}

/// The take-over waits of erasure and the move outlast the shipped drain,
/// with room for the process to exit (ADR-SEC-033 D3).
#[test]
fn the_take_over_waits_outlast_the_drain() {
    let exit_room = Duration::from_secs(5);
    assert!(
        runtime::DRAIN + exit_room <= Duration::from_secs(20),
        "erasure waits 20 s"
    );
    assert!(
        runtime::DRAIN + exit_room <= Duration::from_secs(30),
        "the move waits 30 s"
    );
}

// ---------------------------------------------------------------------------
// The desktop's link: the relays' pool with the admin purpose (ADR-108 D5)
// ---------------------------------------------------------------------------

use vault_app::keeper::relay::{
    CallError, KeeperPool, KeeperStarter, MasterKeySource, Purpose, RelaySettings, ResolveError,
};
use zeroize::Zeroizing;

/// A key source whose key the test can replace, as "Delete everything"
/// followed by a fresh start does.
struct SwappableKey(Mutex<[u8; 32]>);

impl MasterKeySource for SwappableKey {
    fn read(&self) -> Option<Zeroizing<[u8; 32]>> {
        Some(Zeroizing::new(*self.0.lock().unwrap()))
    }
}

#[derive(Default)]
struct CountingStarter(AtomicUsize);

impl KeeperStarter for CountingStarter {
    fn request_start(&self) -> std::io::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn desktop_settings(root: &std::path::Path, budget: Duration) -> RelaySettings {
    let mut s = RelaySettings::production(root.to_path_buf(), Vec::new());
    s.purpose = Purpose::Admin;
    s.resolve_budget = budget;
    s.start_every = Duration::from_millis(100);
    s.poll_every = Duration::from_millis(50);
    s.step_deadline = STEP;
    s
}

fn in_a_while() -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_secs(4)
}

fn tool(name: &str, args: Value) -> CallToolRequestParams {
    let mut params = CallToolRequestParams::new(name.to_string());
    params.arguments = args.as_object().cloned();
    params
}

/// The desktop reaches its admin tools through the same pool the relays use.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_desktop_link_reaches_the_admin_tools() {
    let tmp = tempfile::TempDir::new().unwrap();
    make_vault(tmp.path()).await;
    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::TrialEnded));
    let (stop, keeper) = start_locked_keeper(tmp.path(), check).await;
    let link = KeeperPool::new(
        desktop_settings(tmp.path(), STEP),
        Arc::new(CountingStarter::default()),
        Arc::new(SwappableKey(Mutex::new(MASTER_KEY))),
    );

    let answer = link
        .call(
            tool("admin_settings_info", json!({})),
            in_a_while(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("served");
    let settings: Value = serde_json::from_str(&text(&answer)).unwrap();
    assert_eq!(settings["memory_count"], 1);
    assert!(
        clients::read_live(tmp.path()).is_empty(),
        "not listed as an app"
    );

    drop(link);
    let _ = stop.send(());
    let _ = keeper.await;
}

/// After "Delete everything" (or meeting a newer keeper) the link is
/// poisoned: it neither reaches nor starts a keeper (review B-S7).
#[tokio::test]
async fn a_poisoned_link_never_starts_or_reaches_a_keeper() {
    let tmp = tempfile::TempDir::new().unwrap();
    let starter = Arc::new(CountingStarter::default());
    let link = KeeperPool::new(
        desktop_settings(tmp.path(), Duration::from_millis(300)),
        starter.clone(),
        Arc::new(SwappableKey(Mutex::new(MASTER_KEY))),
    );
    link.poison().await;
    let refused = link
        .call(
            tool("admin_settings_info", json!({})),
            in_a_while(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        refused,
        Err(CallError::NotSent(ResolveError::Poisoned))
    ));
    assert_eq!(starter.0.load(Ordering::SeqCst), 0);

    // A failed erasure clears it again.
    link.clear_poison();
    assert!(!link.is_poisoned());
}

/// The key is read afresh on every connect and never cached: after the vault
/// is erased and a new key made, the same link connects to the new keeper
/// without "key changed" (review A-M4, A R2-4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_link_reads_the_key_afresh_on_every_connect() {
    let tmp = tempfile::TempDir::new().unwrap();
    let key = Arc::new(SwappableKey(Mutex::new(MASTER_KEY)));
    let link = KeeperPool::new(
        desktop_settings(tmp.path(), STEP),
        Arc::new(CountingStarter::default()),
        key.clone(),
    );

    let check = ScriptedCheck::answering(Verdict::Locked(LockReason::TrialEnded));
    let (stop, keeper) = start_locked_keeper(tmp.path(), Arc::clone(&check)).await;
    link.call(
        tool("admin_settings_info", json!({})),
        in_a_while(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("served under the first key");
    let _ = stop.send(());
    let _ = keeper.await;

    let new_key = [0x24; 32];
    *key.0.lock().unwrap() = new_key;
    let (stop, keeper) = start_locked_keeper_keyed(tmp.path(), check, new_key).await;
    link.disconnect().await;
    link.call(
        tool("admin_settings_info", json!({})),
        in_a_while(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("served under the new key, with no 'key changed'");

    drop(link);
    let _ = stop.send(());
    let _ = keeper.await;
}

/// A recent Failed record: a relay reports it at once and starts nothing;
/// the desktop tries ONE fresh start first, and reports the failure only
/// when that does not bring a keeper (ADR-108 D5).
#[tokio::test]
async fn the_desktop_gives_a_failed_start_one_fresh_try_and_a_relay_does_not() {
    let tmp = tempfile::TempDir::new().unwrap();
    let at = chrono::Utc::now().to_rfc3339();
    discovery::write(
        tmp.path(),
        &discovery::Discovery::failed(4242, vault_app::keeper::handshake::WIRE, "test", &at),
    )
    .unwrap();

    let relay_starter = Arc::new(CountingStarter::default());
    let mut relay_settings = desktop_settings(tmp.path(), Duration::from_millis(400));
    relay_settings.purpose = Purpose::Serve;
    relay_settings.boundaries = vec![Boundary::new("personal").unwrap()];
    let relay = KeeperPool::new(
        relay_settings,
        relay_starter.clone(),
        Arc::new(SwappableKey(Mutex::new(MASTER_KEY))),
    );
    let answer = relay
        .call(
            tool("memory_read", json!({ "query": "x" })),
            in_a_while(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        answer,
        Err(CallError::NotSent(ResolveError::Failed { pid: 4242 }))
    ));
    assert_eq!(relay_starter.0.load(Ordering::SeqCst), 0);

    let desktop_starter = Arc::new(CountingStarter::default());
    let desktop = KeeperPool::new(
        desktop_settings(tmp.path(), Duration::from_millis(400)),
        desktop_starter.clone(),
        Arc::new(SwappableKey(Mutex::new(MASTER_KEY))),
    );
    let answer = desktop
        .call(
            tool("admin_settings_info", json!({})),
            in_a_while(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        answer,
        Err(CallError::NotSent(ResolveError::Failed { pid: 4242 }))
    ));
    assert!(
        desktop_starter.0.load(Ordering::SeqCst) >= 1,
        "the desktop asked for a fresh start first"
    );
}
