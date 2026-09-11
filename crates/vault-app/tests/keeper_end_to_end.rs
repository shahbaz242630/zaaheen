//! ADR-102 end to end: a real keeper serve loop on a real named pipe (unix
//! socket off Windows), reached by a real relay pool — the path every AI app's
//! tool call takes. Mock adapter, so no models and no vault files: each test
//! stays well under the 5 s budget.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rmcp::model::CallToolRequestParams;
use rmcp::ServerHandler;
use tokio::sync::oneshot;
use vault_app::keeper::discovery::{self, Discovery};
use vault_app::keeper::handshake::{HandshakeKeys, WIRE};
use vault_app::keeper::relay::{
    KeeperPool, KeeperStarter, MasterKeySource, RelaySettings, MSG_BUSY, MSG_KEY_CHANGED,
};
use vault_app::keeper::runtime::{self, KeeperExit, KeeperSettings};
use vault_app::keeper::{exclusive, intent};
use vault_app::{ConsolidatorLock, VAULT_LOCKFILE_NAME};
use vault_core::{Boundary, MemoryId, NewMemory, VaultResult};
use vault_mcp::{Adapter, NoVaultAdapter, StdioServer, ToolInvokeDetails, Upstream, UpstreamError};
use vault_retrieval::{
    HealthInfo, HealthStatus, ReadQuery, RetrievalQuery, RetrievedMemory, StructuredReadResponse,
};
use zeroize::Zeroizing;

const MASTER_KEY: [u8; 32] = [0x42; 32];

/// Records the boundaries every search arrived with, so a test can prove the
/// keeper scoped each connection to exactly what its handshake proved.
/// `search_delay` makes it a slow keeper, like one whose model is paged out.
#[derive(Default)]
struct RecordingAdapter {
    searches: Mutex<Vec<Vec<Boundary>>>,
    search_delay: Duration,
}

impl RecordingAdapter {
    fn slow(delay: Duration) -> Self {
        Self {
            search_delay: delay,
            ..Self::default()
        }
    }

    fn searches(&self) -> Vec<Vec<Boundary>> {
        self.searches.lock().unwrap().clone()
    }
}

#[async_trait]
impl Adapter for RecordingAdapter {
    async fn search(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        self.searches
            .lock()
            .unwrap()
            .push(query.authorized_boundaries.clone());
        if !self.search_delay.is_zero() {
            tokio::time::sleep(self.search_delay).await;
        }
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
}

struct FixedKey([u8; 32]);

impl MasterKeySource for FixedKey {
    fn read(&self) -> Option<Zeroizing<[u8; 32]>> {
        Some(Zeroizing::new(self.0))
    }
}

/// A starter that does nothing: the tests start the keeper themselves.
struct NoStart;

impl KeeperStarter for NoStart {
    fn request_start(&self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Starts nothing, counts every request.
#[derive(Default)]
struct CountingStarter(AtomicUsize);

impl KeeperStarter for CountingStarter {
    fn request_start(&self) -> std::io::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Starts a real keeper on the FIRST request, like Task Scheduler would, and
/// counts every request.
struct SpawningStarter {
    requests: AtomicUsize,
    runtime: tokio::runtime::Handle,
    adapter: Arc<RecordingAdapter>,
    root: std::path::PathBuf,
}

impl KeeperStarter for SpawningStarter {
    fn request_start(&self) -> std::io::Result<()> {
        if self.requests.fetch_add(1, Ordering::SeqCst) == 0 {
            let adapter: Arc<dyn Adapter> = self.adapter.clone();
            let settings = keeper_settings(self.root.clone(), Duration::from_secs(30));
            self.runtime.spawn(runtime::serve(
                adapter,
                HandshakeKeys::derive(&MASTER_KEY),
                settings,
                std::future::pending::<()>(),
            ));
        }
        Ok(())
    }
}

fn keeper_settings(root: std::path::PathBuf, idle_exit: Duration) -> KeeperSettings {
    KeeperSettings {
        vault_root: root,
        version: "test".into(),
        idle_exit,
        idle_check: Duration::from_millis(100),
        frame1_deadline: Duration::from_millis(500),
        handshake_deadline: Duration::from_secs(2),
        max_pending_handshakes: 8,
    }
}

fn relay_settings(root: std::path::PathBuf, boundaries: &[&str]) -> RelaySettings {
    RelaySettings {
        vault_root: root,
        boundaries: boundaries
            .iter()
            .map(|b| Boundary::new(*b).unwrap())
            .collect(),
        resolve_budget: Duration::from_secs(3),
        start_every: Duration::from_millis(200),
        poll_every: Duration::from_millis(50),
        failed_cooldown: Duration::from_secs(60),
        step_deadline: Duration::from_secs(2),
        idle_drop: Duration::from_secs(60),
    }
}

/// A deadline no test here should reach: every call is well under a second.
fn soon() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn search_call() -> CallToolRequestParams {
    let mut params = CallToolRequestParams::new("memory_search");
    params.arguments = serde_json::json!({ "query": "anything" })
        .as_object()
        .cloned();
    params
}

/// Start a keeper that runs until `stop` fires; wait until it is listening.
async fn start_keeper(
    root: &std::path::Path,
    adapter: Arc<RecordingAdapter>,
    idle_exit: Duration,
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<VaultResult<KeeperExit>>,
) {
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let adapter: Arc<dyn Adapter> = adapter;
    let handle = tokio::spawn(runtime::serve(
        adapter,
        HandshakeKeys::derive(&MASTER_KEY),
        keeper_settings(root.to_path_buf(), idle_exit),
        async {
            let _ = stop_rx.await;
        },
    ));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while discovery::read(root).ok().flatten().is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "keeper never published"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (stop_tx, handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_reaches_the_keeper_scoped_to_the_boundaries_it_proved() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let (stop, keeper) = start_keeper(tmp.path(), adapter.clone(), Duration::from_secs(30)).await;

    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["work"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    );
    let result = pool
        .call_tool(search_call(), soon())
        .await
        .expect("forwarded");
    assert_ne!(result.is_error, Some(true));
    assert_eq!(
        adapter.searches(),
        vec![vec![Boundary::new("work").unwrap()]],
        "the keeper must scope the call to exactly the boundaries the relay proved"
    );

    drop(pool);
    let _ = stop.send(());
    assert_eq!(keeper.await.unwrap().unwrap(), KeeperExit::Shutdown);
    assert!(
        discovery::read(tmp.path()).unwrap().is_none(),
        "a stopping keeper removes its discovery file"
    );
}

/// Claude Desktop runs its chat and its shared pool at once; both must be
/// served, each with its own scope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_relays_are_served_at_the_same_time() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let (stop, keeper) = start_keeper(tmp.path(), adapter.clone(), Duration::from_secs(30)).await;

    let chat = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["personal"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    );
    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["work"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    );
    let (a, b) = tokio::join!(
        chat.call_tool(search_call(), soon()),
        pool.call_tool(search_call(), soon())
    );
    assert!(a.is_ok() && b.is_ok());
    let mut seen = adapter.searches();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            vec![Boundary::new("personal").unwrap()],
            vec![Boundary::new("work").unwrap()]
        ]
    );

    let _ = stop.send(());
    keeper.await.unwrap().unwrap();
}

/// A relay that cannot read this vault's key never reaches the vault.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_with_the_wrong_key_never_reaches_the_vault() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let (stop, keeper) = start_keeper(tmp.path(), adapter.clone(), Duration::from_secs(30)).await;

    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["personal"]),
        Arc::new(NoStart),
        Arc::new(FixedKey([0x99; 32])),
    );
    match pool.call_tool(search_call(), soon()).await {
        Err(UpstreamError::NotSent(reason)) => assert_eq!(reason, MSG_KEY_CHANGED),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(adapter.searches().is_empty(), "nothing may reach the vault");

    let _ = stop.send(());
    keeper.await.unwrap().unwrap();
}

/// No keeper yet: the relay asks for one (as it would through Task
/// Scheduler), waits for it to appear, and then serves the call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_starts_a_keeper_when_there_is_none() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let starter = Arc::new(SpawningStarter {
        requests: AtomicUsize::new(0),
        runtime: tokio::runtime::Handle::current(),
        adapter: adapter.clone(),
        root: tmp.path().to_path_buf(),
    });
    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["personal"]),
        starter.clone(),
        Arc::new(FixedKey(MASTER_KEY)),
    );

    pool.call_tool(search_call(), soon())
        .await
        .expect("served once the keeper is up");
    assert!(starter.requests.load(Ordering::SeqCst) >= 1);
    assert_eq!(adapter.searches().len(), 1);
}

/// With nobody connected, the keeper leaves on its own — which is what frees
/// the vault for nightly maintenance once the AI apps go quiet.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unused_keeper_exits_and_cleans_up() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let (_stop, keeper) = start_keeper(tmp.path(), adapter, Duration::from_millis(400)).await;

    let exit = tokio::time::timeout(Duration::from_secs(3), keeper)
        .await
        .expect("an idle keeper must exit")
        .unwrap()
        .unwrap();
    assert_eq!(exit, KeeperExit::Idle);
    assert!(discovery::read(tmp.path()).unwrap().is_none());
}

/// A connected relay keeps the keeper alive; once it lets go, the keeper
/// idles out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connected_relay_keeps_the_keeper_until_it_disconnects() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let (_stop, keeper) = start_keeper(tmp.path(), adapter, Duration::from_millis(400)).await;

    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["personal"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    );
    pool.call_tool(search_call(), soon()).await.expect("served");
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(
        !keeper.is_finished(),
        "the keeper must not idle out under a live connection"
    );

    drop(pool);
    let exit = tokio::time::timeout(Duration::from_secs(3), keeper)
        .await
        .expect("the keeper must idle out after the relay leaves")
        .unwrap()
        .unwrap();
    assert_eq!(exit, KeeperExit::Idle);
}

/// Once the keeper is gone, a call is reported as not sent — never as a
/// success, and never as "outcome unknown" for a call that never left.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_after_the_keeper_stops_is_reported_not_sent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let (stop, keeper) = start_keeper(tmp.path(), adapter.clone(), Duration::from_secs(30)).await;

    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["personal"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    );
    pool.call_tool(search_call(), soon()).await.expect("served");

    let _ = stop.send(());
    keeper.await.unwrap().unwrap();
    // Let the connection close before the next call.
    tokio::time::sleep(Duration::from_millis(200)).await;

    match pool.call_tool(search_call(), soon()).await {
        Err(UpstreamError::NotSent(_)) | Err(UpstreamError::Lost) => {}
        other => panic!("expected a failure once the keeper is gone, got {other:?}"),
    }
    assert_eq!(
        adapter.searches().len(),
        1,
        "only the first call reached it"
    );
}

/// ADR-103 D1, found live 2026-09-11: a keeper that answers after the call's
/// deadline is cut off AT the deadline — reported `TimedOut` (outcome
/// unknown), never left hanging past it. Before, the cut-off was a fixed 35 s
/// regardless of how much of the client's 60 s remained.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keeper_slower_than_the_deadline_times_out_at_the_deadline() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::slow(Duration::from_secs(3)));
    let (stop, keeper) = start_keeper(tmp.path(), adapter.clone(), Duration::from_secs(30)).await;
    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["personal"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    );

    let asked_at = Instant::now();
    let deadline = asked_at + Duration::from_millis(1200);
    match pool.call_tool(search_call(), deadline).await {
        Err(UpstreamError::TimedOut) => {}
        other => panic!("a keeper slower than the deadline must time out, got {other:?}"),
    }
    let waited = asked_at.elapsed();
    assert!(
        waited >= Duration::from_millis(1100) && waited < Duration::from_millis(2500),
        "the relay must give up at the deadline, not before and not long after; waited {waited:?}"
    );
    assert_eq!(adapter.searches().len(), 1, "the call did reach the keeper");

    let _ = stop.send(());
    keeper.await.unwrap().unwrap();
}

/// ADR-103 D1: finding (or starting) a keeper spends the SAME deadline — it
/// can never run on for its own full budget past the point the client has
/// given up. The call was never sent, so it is `NotSent`, not `TimedOut`: a
/// save the keeper never saw must not be reported as "may still complete".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finding_a_keeper_never_outlives_the_deadline() {
    let tmp = tempfile::TempDir::new().unwrap();
    // No keeper, and a starter that never starts one: resolution can only end
    // by running out of time. Its own budget (relay_settings: 3 s) is longer
    // than the call's deadline.
    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["personal"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    );

    let asked_at = Instant::now();
    match pool
        .call_tool(search_call(), asked_at + Duration::from_millis(500))
        .await
    {
        Err(UpstreamError::NotSent(_)) => {}
        other => panic!("a call that never found a keeper is not sent, got {other:?}"),
    }
    assert!(
        asked_at.elapsed() < Duration::from_millis(1500),
        "resolution must stop at the call's deadline, not its own 3 s budget; took {:?}",
        asked_at.elapsed()
    );
}

/// During a maintenance run no keeper can start. The relay must say "busy"
/// at once — not spend its whole wait asking Windows for keepers that would
/// only find the vault taken and exit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_answers_busy_at_once_during_maintenance() {
    let tmp = tempfile::TempDir::new().unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    discovery::write(tmp.path(), &Discovery::maintenance(1, WIRE, "test", &now)).unwrap();
    let starter = Arc::new(CountingStarter::default());
    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["personal"]),
        starter.clone(),
        Arc::new(FixedKey(MASTER_KEY)),
    );

    let asked_at = std::time::Instant::now();
    match pool.call_tool(search_call(), soon()).await {
        Err(UpstreamError::NotSent(reason)) => assert_eq!(reason, MSG_BUSY),
        other => panic!("expected busy, got {other:?}"),
    }
    assert!(
        asked_at.elapsed() < Duration::from_secs(1),
        "busy must be reported at once"
    );
    assert_eq!(
        starter.0.load(Ordering::SeqCst),
        0,
        "no keeper start requests"
    );
}

/// "Delete everything" while an AI app is connected: the keeper hands the
/// vault over, nothing new can start, and the app's next call does not reach
/// the vault. Before this, the keeper kept serving the "deleted" memories
/// from its open stores until it went idle.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn erasure_takes_the_vault_from_a_serving_keeper() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let adapter = Arc::new(RecordingAdapter::default());

    // The keeper process holds the vault lock for as long as it serves, as
    // `zaaheen keeper` does; its exit is what releases it.
    let keeper_lock = ConsolidatorLock::try_acquire_named(&root, VAULT_LOCKFILE_NAME).unwrap();
    let (_stop, serving) = start_keeper(&root, adapter.clone(), Duration::from_secs(30)).await;
    let keeper = tokio::spawn(async move {
        let exit = serving.await;
        drop(keeper_lock);
        exit
    });

    let pool = KeeperPool::new(
        relay_settings(root.clone(), &["personal"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    );
    pool.call_tool(search_call(), soon()).await.expect("served");

    let held = exclusive::take_exclusive(
        &root,
        &FixedKey(MASTER_KEY),
        Duration::from_secs(3),
        Duration::from_secs(1),
    )
    .await
    .expect("the keeper must hand the vault over");
    let exit = keeper.await.unwrap().unwrap().unwrap();
    assert_eq!(exit, KeeperExit::HandedOver);
    assert!(discovery::read(&root).unwrap().is_none());
    assert!(
        intent::is_held(&root),
        "no new keeper may start while the erasure runs"
    );

    match pool.call_tool(search_call(), soon()).await {
        Err(UpstreamError::NotSent(_)) | Err(UpstreamError::Lost) => {}
        other => panic!("the vault must be out of reach, got {other:?}"),
    }
    assert_eq!(
        adapter.searches().len(),
        1,
        "only the first call reached it"
    );

    drop(held);
    assert!(
        !intent::is_held(&root),
        "keepers may start again afterwards"
    );
}

/// A maintenance run holds the vault and must not be interrupted: erasure
/// reports busy, having changed nothing.
#[tokio::test]
async fn erasure_waits_out_and_reports_a_maintenance_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _maintenance =
        ConsolidatorLock::try_acquire_named(tmp.path(), VAULT_LOCKFILE_NAME).unwrap();
    let result = exclusive::take_exclusive(
        tmp.path(),
        &FixedKey(MASTER_KEY),
        Duration::from_millis(300),
        Duration::from_millis(200),
    )
    .await;
    assert!(matches!(
        result,
        Err(vault_core::VaultError::ConsolidatorBusy(_))
    ));
    assert!(
        !intent::is_held(tmp.path()),
        "a failed attempt must not leave keepers blocked"
    );
}

/// A relay and a keeper from different builds must never disagree about the
/// tool contract silently. Any change to the tools or instructions changes
/// this hash, and the fix is to bump `WIRE` deliberately.
#[test]
fn the_tool_contract_is_pinned_to_the_wire_version() {
    // Recorded 2026-09-10 from the wire-v1 tool contract (session 35).
    const PINNED: &str = "2e062fb5bc6adb1a62b37ca9c33adb51dbfd2f59249e501d7fa6dfb9871309be";

    let server = StdioServer::new(Arc::new(NoVaultAdapter), Vec::new());
    let mut hasher = blake3::Hasher::new();
    hasher.update(
        server
            .get_info()
            .instructions
            .unwrap_or_default()
            .as_bytes(),
    );
    for name in [
        "memory_delete",
        "memory_read",
        "memory_search",
        "memory_update",
        "memory_write",
    ] {
        let tool = server
            .get_tool(name)
            .unwrap_or_else(|| panic!("{name} missing"));
        hasher.update(&serde_json::to_vec(&tool).unwrap());
    }
    let actual = hasher.finalize().to_hex().to_string();
    assert_eq!(
        (WIRE, actual.as_str()),
        (1, PINNED),
        "The MCP tool contract changed. Relays and keepers from different \
         builds must not disagree about it silently: bump WIRE in \
         vault_app::keeper::handshake and set PINNED to {actual}."
    );
}
