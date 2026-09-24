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
use tokio_util::sync::CancellationToken;
use vault_app::entitlement::ModeCheck;
use vault_app::keeper::discovery::{self, Discovery};
use vault_app::keeper::handshake::{HandshakeKeys, WIRE};
use vault_app::keeper::relay::{
    KeeperPool, KeeperStarter, MasterKeySource, RelaySettings, MSG_BUSY, MSG_KEY_CHANGED,
};
use vault_app::keeper::runtime::{self, KeeperExit, KeeperMode, KeeperSettings, Subscription};
use vault_app::keeper::{exclusive, intent};
use vault_app::{ConsolidatorLock, VAULT_LOCKFILE_NAME};
use vault_core::{Boundary, MemoryId, NewMemory, VaultResult};
use vault_mcp::{
    Adapter, EntitlementCheck, InFlight, LockReason, NoVaultAdapter, StdioServer,
    ToolInvokeDetails, Upstream, UpstreamError, Verdict,
};
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
    /// Searches that ran to the END. `searches` is recorded on arrival, so it
    /// counts a call that was cut off half way through just the same — which
    /// made `a_call_in_flight_finishes_before_the_mode_changes` pass with the
    /// in-flight wait deleted. Anything asserting that a call *completed* must
    /// use this.
    completed: AtomicUsize,
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

    fn completed(&self) -> usize {
        self.completed.load(Ordering::SeqCst)
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
        self.completed.fetch_add(1, Ordering::SeqCst);
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
                None,
                HandshakeKeys::derive(&MASTER_KEY),
                settings,
                None,
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
        drain: Duration::from_secs(1),
    }
}

fn relay_settings(root: std::path::PathBuf, boundaries: &[&str]) -> RelaySettings {
    RelaySettings {
        vault: vault_app::keeper::relay::RelayVault::Fixed(root),
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
        // No sign-in unless a test asks for one, so every test that predates
        // this arc behaves exactly as it did.
        account_dir: None,
        marker_recheck: Duration::from_millis(50),
        purpose: vault_app::keeper::relay::Purpose::Serve,
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
    start_keeper_in_mode(root, adapter, idle_exit, None).await
}

/// The same, behind the subscription gate (ADR-104). A lock-mode keeper is
/// this with `NoVaultAdapter` in place of the recording one — see
/// [`start_lock_mode_keeper`].
async fn start_keeper_in_mode(
    root: &std::path::Path,
    adapter: Arc<RecordingAdapter>,
    idle_exit: Duration,
    subscription: Option<Subscription>,
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<VaultResult<KeeperExit>>,
) {
    let adapter: Arc<dyn Adapter> = adapter;
    start_keeper_serving(root, adapter, idle_exit, subscription).await
}

/// A locked keeper, exactly as `vault-cli` starts one: the same serve loop
/// over [`NoVaultAdapter`], with no `Application` and no models behind it.
async fn start_lock_mode_keeper(
    root: &std::path::Path,
    idle_exit: Duration,
    subscription: Subscription,
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<VaultResult<KeeperExit>>,
) {
    assert_eq!(subscription.mode(), KeeperMode::Lock);
    let adapter: Arc<dyn Adapter> = Arc::new(NoVaultAdapter);
    start_keeper_serving(root, adapter, idle_exit, Some(subscription)).await
}

async fn start_keeper_serving(
    root: &std::path::Path,
    adapter: Arc<dyn Adapter>,
    idle_exit: Duration,
    subscription: Option<Subscription>,
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<VaultResult<KeeperExit>>,
) {
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(runtime::serve(
        adapter,
        None,
        HandshakeKeys::derive(&MASTER_KEY),
        keeper_settings(root.to_path_buf(), idle_exit),
        subscription,
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
        .call_tool(search_call(), soon(), CancellationToken::new())
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
        chat.call_tool(search_call(), soon(), CancellationToken::new()),
        pool.call_tool(search_call(), soon(), CancellationToken::new())
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
    match pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await
    {
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

    pool.call_tool(search_call(), soon(), CancellationToken::new())
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
    pool.call_tool(search_call(), soon(), CancellationToken::new())
        .await
        .expect("served");
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
    pool.call_tool(search_call(), soon(), CancellationToken::new())
        .await
        .expect("served");

    let _ = stop.send(());
    keeper.await.unwrap().unwrap();
    // Let the connection close before the next call.
    tokio::time::sleep(Duration::from_millis(200)).await;

    match pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await
    {
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
    match pool
        .call_tool(search_call(), deadline, CancellationToken::new())
        .await
    {
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
        .call_tool(
            search_call(),
            asked_at + Duration::from_millis(500),
            CancellationToken::new(),
        )
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
    match pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await
    {
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
    pool.call_tool(search_call(), soon(), CancellationToken::new())
        .await
        .expect("served");

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

    match pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await
    {
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
    // Recorded 2026-09-24 from the wire-v4 tool contract (session 59:
    // "one question at a time" in memory_read / memory_search, ADR-107),
    // computed on Windows under rmcp 3.4.1. Wire 3 was
    // 7a941150d4fe95809f1a0847607b607d3ecc79caabfad2e2f645719e1af4584d
    // (session 52, rmcp 2.2.0); wire 2 was
    // 64bafc664287b3e09449bb4c093e38bfa87b287bbb769ce86e14967363a7d836 (CI,
    // Linux and macOS, ADR-SEC-021 D4c); wire 1 was
    // 2e062fb5bc6adb1a62b37ca9c33adb51dbfd2f59249e501d7fa6dfb9871309be.
    const PINNED: &str = "198591bd3b035767929eef3e50d802a83596e1231b0c129b40a281df5973998c";

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
        (4, PINNED),
        "The MCP tool contract changed. Relays and keepers from different \
         builds must not disagree about it silently: bump WIRE in \
         vault_app::keeper::handshake and set PINNED to {actual}."
    );
}

// ---------------------------------------------------------------------------
// The subscription gate, end to end through a relay (ADR-104)
// ---------------------------------------------------------------------------

/// A check that answers the same verdict every time.
struct FixedCheck(Verdict);

#[async_trait]
impl EntitlementCheck for FixedCheck {
    async fn check(&self) -> Verdict {
        self.0
    }
}

#[async_trait]
impl ModeCheck for FixedCheck {
    async fn peek(&self) -> Verdict {
        self.0
    }

    async fn refresh_and_peek(&self) -> Verdict {
        self.0
    }
}

/// A full keeper's subscription that always answers `verdict`, with the shared
/// counter handed back so a test can prove a call stopped being counted.
fn gate_answering(verdict: Verdict) -> (Subscription, InFlight) {
    let in_flight = InFlight::new();
    (
        Subscription::full(Arc::new(FixedCheck(verdict)), in_flight.clone()),
        in_flight,
    )
}

/// What an AI app sees when the trial ends under a keeper that is already
/// running: the fixed words as a readable tool error, and nothing touched in
/// the vault.
///
/// This is the transient window §6.2 creates. A full keeper never *stays*
/// locked — it refreshes and then leaves (`ModeChanged`) — but a call arriving
/// while it is on its way out must still be refused rather than served. The
/// refresh here is deliberately slower than the call, which is what makes the
/// window deterministic instead of a race.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keeper_whose_trial_ends_refuses_the_call_and_never_touches_the_vault() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let check = SwitchableCheck::slow_refresh(Verdict::Entitled, Duration::from_secs(3));
    let in_flight = InFlight::new();
    let (stop, keeper) = start_keeper_in_mode(
        tmp.path(),
        adapter.clone(),
        Duration::from_secs(30),
        Some(Subscription::full(check.clone(), in_flight.clone())),
    )
    .await;

    // The trial ends under the running keeper.
    check.set(Verdict::Locked(LockReason::TrialEnded));

    let pool = pool_for(tmp.path());
    let result = pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await
        .expect("a locked call comes back as a tool result");
    assert_eq!(result.is_error, Some(true));
    assert_eq!(result_text(&result), LockReason::TrialEnded.message());
    assert!(
        adapter.searches().is_empty(),
        "a locked call reached the vault"
    );
    assert_eq!(in_flight.count(), 0, "the call is no longer counted");

    let _ = stop.send(());
    let _ = keeper.await;
}

/// With a live subscription the keeper serves exactly as it did before the
/// gate existed, and the call is no longer counted once it has answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_entitled_keeper_serves_the_call_as_before() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let (subscription, in_flight) = gate_answering(Verdict::Entitled);
    let (stop, keeper) = start_keeper_in_mode(
        tmp.path(),
        adapter.clone(),
        Duration::from_secs(30),
        Some(subscription),
    )
    .await;

    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["work"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    );
    let result = pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await
        .expect("an entitled call is served");
    assert_ne!(result.is_error, Some(true));
    assert_eq!(adapter.searches().len(), 1);
    assert_eq!(in_flight.count(), 0);

    let _ = stop.send(());
    let _ = keeper.await;
}

// ---------------------------------------------------------------------------
// Keeper modes: lock mode and `ModeChanged` (§8.26 §6.2, §8.35)
//
// The keeper here is the real serve loop on a real pipe, so these prove what
// an AI app actually gets. `idle_check` is 100 ms in `keeper_settings`, so a
// tick happens several times a second and no test sleeps for long.
// ---------------------------------------------------------------------------

/// A check whose answer the test changes while the keeper runs, recording
/// every question the keeper asked and which kind it was.
struct SwitchableCheck {
    verdict: Mutex<Verdict>,
    /// What a *call* is answered with, when that must differ from what a tick
    /// reads. Separating the two is what lets a test prove which of the two
    /// paths — the call or the tick — triggered a mode change.
    call_verdict: Mutex<Option<Verdict>>,
    /// What a refresh changes the answer to, if anything.
    on_refresh: Mutex<Option<Verdict>>,
    /// Makes `refresh_and_peek` slow, like a real refresh waiting on a server.
    refresh_delay: Duration,
    peeks: AtomicUsize,
    refreshes: AtomicUsize,
    calls: AtomicUsize,
}

impl SwitchableCheck {
    fn answering(verdict: Verdict) -> Arc<Self> {
        Arc::new(Self {
            verdict: Mutex::new(verdict),
            call_verdict: Mutex::new(None),
            on_refresh: Mutex::new(None),
            refresh_delay: Duration::ZERO,
            peeks: AtomicUsize::new(0),
            refreshes: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        })
    }

    /// Reads as `peek` on a tick, but answers `call` to a request.
    fn splitting(peek: Verdict, call: Verdict) -> Arc<Self> {
        let check = Self::answering(peek);
        *check.call_verdict.lock().unwrap() = Some(call);
        check
    }

    /// Reads as `verdict`, but a refresh finds `after_refresh`.
    fn refreshing_to(verdict: Verdict, after_refresh: Verdict) -> Arc<Self> {
        let check = Self::answering(verdict);
        *check.on_refresh.lock().unwrap() = Some(after_refresh);
        check
    }

    fn slow_refresh(verdict: Verdict, delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            verdict: Mutex::new(verdict),
            call_verdict: Mutex::new(None),
            on_refresh: Mutex::new(None),
            refresh_delay: delay,
            peeks: AtomicUsize::new(0),
            refreshes: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        })
    }

    fn set(&self, verdict: Verdict) {
        *self.verdict.lock().unwrap() = verdict;
    }

    fn now(&self) -> Verdict {
        *self.verdict.lock().unwrap()
    }

    fn peeks(&self) -> usize {
        self.peeks.load(Ordering::SeqCst)
    }

    fn refreshes(&self) -> usize {
        self.refreshes.load(Ordering::SeqCst)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EntitlementCheck for SwitchableCheck {
    async fn check(&self) -> Verdict {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.call_verdict
            .lock()
            .unwrap()
            .unwrap_or_else(|| self.now())
    }
}

#[async_trait]
impl ModeCheck for SwitchableCheck {
    async fn peek(&self) -> Verdict {
        self.peeks.fetch_add(1, Ordering::SeqCst);
        self.now()
    }

    async fn refresh_and_peek(&self) -> Verdict {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        if !self.refresh_delay.is_zero() {
            tokio::time::sleep(self.refresh_delay).await;
        }
        if let Some(after) = *self.on_refresh.lock().unwrap() {
            self.set(after);
        }
        self.now()
    }
}

/// Wait up to `limit` for the keeper to stop, and say how it stopped.
async fn exit_within(
    keeper: tokio::task::JoinHandle<VaultResult<KeeperExit>>,
    limit: Duration,
) -> Option<KeeperExit> {
    match tokio::time::timeout(limit, keeper).await {
        Ok(Ok(Ok(exit))) => Some(exit),
        Ok(other) => panic!("the keeper failed rather than exiting: {other:?}"),
        Err(_) => None,
    }
}

/// A relay pool pointed at `root`, which never asks for a keeper to start.
fn pool_for(root: &std::path::Path) -> Arc<KeeperPool> {
    KeeperPool::new(
        relay_settings(root.to_path_buf(), &["work"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    )
}

/// The text of a tool result, however many blocks it came in.
fn result_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

// ---- a full keeper -------------------------------------------------------

/// The quiet case, and the one that must cost nothing: while the subscription
/// is live the keeper ticks on and never reaches the network.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_keeper_stays_while_the_user_is_entitled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let check = SwitchableCheck::answering(Verdict::Entitled);
    let (stop, keeper) = start_keeper_in_mode(
        tmp.path(),
        adapter.clone(),
        Duration::from_secs(30),
        Some(Subscription::full(check.clone(), InFlight::new())),
    )
    .await;

    // Several ticks (idle_check is 100 ms).
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        check.peeks() >= 2,
        "the keeper must re-evaluate on its ticks, saw {}",
        check.peeks()
    );
    assert_eq!(
        check.refreshes(),
        0,
        "an entitled tick must not reach the network"
    );
    assert!(
        exit_within(keeper, Duration::from_millis(200))
            .await
            .is_none(),
        "an entitled keeper must keep serving"
    );
    let _ = stop.send(());
}

/// The subscription lapses while the keeper is running: it refreshes first
/// (the lease might merely be stale), then leaves, so the 2.86 GB it holds is
/// freed instead of sitting behind relays that stay connected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_keeper_whose_entitlement_lapses_exits_mode_changed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let check = SwitchableCheck::answering(Verdict::Entitled);
    let (_stop, keeper) = start_keeper_in_mode(
        tmp.path(),
        adapter.clone(),
        Duration::from_secs(30),
        Some(Subscription::full(check.clone(), InFlight::new())),
    )
    .await;

    check.set(Verdict::Locked(LockReason::TrialEnded));
    assert_eq!(
        exit_within(keeper, Duration::from_secs(5)).await,
        Some(KeeperExit::ModeChanged),
        "a keeper whose user is locked must change mode"
    );
    assert!(
        check.refreshes() >= 1,
        "§6.2: it refreshes before it decides to unload the models"
    );
}

/// The reason the refresh comes first: a lease that is merely stale must not
/// cost an unload and reload of 2.86 GB of models.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_lease_that_a_refresh_fixes_does_not_change_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    // Reads as "could not confirm", but a refresh finds a live subscription.
    let check = SwitchableCheck::refreshing_to(
        Verdict::Locked(LockReason::CannotConfirm),
        Verdict::Entitled,
    );
    let (stop, keeper) = start_keeper_in_mode(
        tmp.path(),
        adapter.clone(),
        Duration::from_secs(30),
        Some(Subscription::full(check.clone(), InFlight::new())),
    )
    .await;

    assert!(
        exit_within(keeper, Duration::from_secs(2)).await.is_none(),
        "a stale lease the refresh fixed must not change the keeper's mode"
    );
    assert_eq!(check.refreshes(), 1, "and it refreshes once, not per tick");
    let _ = stop.send(());
}

/// §6.2: `ModeChanged` does not require zero connections, so it must not cut
/// a write off mid-way. The call started before the flip finishes normally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_in_flight_finishes_before_the_mode_changes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::slow(Duration::from_millis(700)));
    let check = SwitchableCheck::answering(Verdict::Entitled);
    let (_stop, keeper) = start_keeper_in_mode(
        tmp.path(),
        adapter.clone(),
        Duration::from_secs(30),
        Some(Subscription::full(check.clone(), InFlight::new())),
    )
    .await;

    let pool = pool_for(tmp.path());
    let calling = {
        let pool = pool.clone();
        tokio::spawn(async move {
            pool.call_tool(search_call(), soon(), CancellationToken::new())
                .await
        })
    };
    // The call is in the vault; now the subscription lapses under it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    check.set(Verdict::Locked(LockReason::SubscriptionEnded));

    // THE assertion, and the only one that can pin this: `serve` must not
    // RETURN while a call is in flight.
    //
    // Everything softer passes with the wait deleted. rmcp runs each request in
    // a DETACHED task, so the call still answers after the keeper has gone —
    // asserting that the call completed proves nothing. What makes the wait
    // necessary is that `dispatch_keeper` calls `std::process::exit` the moment
    // serving ends, killing those detached tasks mid-write; no in-process test
    // can survive to observe that. So the ordering IS the invariant.
    //
    // Timeline: the search takes 700 ms, the mode change is decided around
    // 300 ms, so at 450 ms the keeper must still be serving.
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !keeper.is_finished(),
        "the keeper left while a call was still in flight; a save would have \
         been cut off mid-write"
    );

    let answered = calling.await.expect("the calling task");
    let result = answered.expect("a call already in flight must still answer");
    assert_ne!(
        result.is_error,
        Some(true),
        "a call that was already running must not be turned into an error"
    );
    assert_eq!(adapter.searches().len(), 1, "the search really started");
    // The assertion that actually pins the in-flight wait: `searches` is
    // recorded on arrival, so it would still be 1 for a call the keeper cut
    // off. Only `completed` proves the work finished before the keeper left.
    assert_eq!(
        adapter.completed(),
        1,
        "the keeper must let a call in flight FINISH before it changes mode"
    );
    assert_eq!(
        exit_within(keeper, Duration::from_secs(5)).await,
        Some(KeeperExit::ModeChanged),
        "and only then does the keeper leave"
    );
}

/// A slow refresh must not make ticks pile up: the `deciding` latch means one
/// evaluation at a time, however many ticks arrive while it runs.
///
/// **What this does and does not prove.** It catches a missing or broken latch:
/// without one, a 600 ms refresh with a 100 ms tick spawns an evaluation — and
/// a network refresh — every tick. It does **not** prove the narrower rule that
/// the latch stays held once a mode change is decided (the `return` in
/// `spawn_mode_evaluation`). A planted bug removing that `return` was NOT caught
/// here, at 150 ms or 600 ms, with one attempt or eight: the stop message is
/// queued before the latch is released, so the serve loop nearly always breaks
/// before a burst tick can start a second evaluation. That `return` is
/// therefore defensive and reasoned, not test-proven, and it is recorded that
/// way in `SIGNIN-DESIGN.md` §8.35 rather than claimed as covered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ticks_never_pile_up_behind_a_slow_evaluation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    // Locked, so every tick wants to refresh, and each refresh outlasts the
    // 100 ms tick six times over.
    let check = SwitchableCheck::slow_refresh(
        Verdict::Locked(LockReason::CannotConfirm),
        Duration::from_millis(600),
    );
    let (_stop, keeper) = start_keeper_in_mode(
        tmp.path(),
        adapter.clone(),
        Duration::from_secs(30),
        Some(Subscription::full(check.clone(), InFlight::new())),
    )
    .await;

    assert_eq!(
        exit_within(keeper, Duration::from_secs(5)).await,
        Some(KeeperExit::ModeChanged)
    );
    assert_eq!(
        check.refreshes(),
        1,
        "six ticks passed during one refresh; only one evaluation may run"
    );
}

/// A build with no account settings has no subscription at all, and every
/// tick must stay exactly as inert as it was before this arc.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keeper_with_no_subscription_never_changes_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let (stop, keeper) =
        start_keeper_in_mode(tmp.path(), adapter.clone(), Duration::from_secs(30), None).await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        exit_within(keeper, Duration::from_millis(200))
            .await
            .is_none(),
        "a build with no sign-in must serve as it always did"
    );
    let _ = stop.send(());
}

/// Every exit removes the discovery file first, so no relay dials a keeper
/// that has gone. A new exit reason must not miss that cleanup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mode_change_removes_the_discovery_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    // Entitled at first, so the keeper publishes and settles; then it lapses.
    let check = SwitchableCheck::answering(Verdict::Entitled);
    let (_stop, keeper) = start_keeper_in_mode(
        tmp.path(),
        adapter.clone(),
        Duration::from_secs(30),
        Some(Subscription::full(check.clone(), InFlight::new())),
    )
    .await;
    assert!(
        discovery::read(tmp.path()).unwrap().is_some(),
        "the keeper published before it changed mode"
    );

    check.set(Verdict::Locked(LockReason::TrialEnded));
    assert_eq!(
        exit_within(keeper, Duration::from_secs(5)).await,
        Some(KeeperExit::ModeChanged)
    );
    assert!(
        discovery::read(tmp.path()).unwrap().is_none(),
        "a keeper that changed mode left its discovery file behind"
    );
}

// ---- lock mode ----------------------------------------------------------

/// What an AI app gets from a locked computer: the fixed words, over the same
/// tool surface, with no vault open behind the keeper at all. `NoVaultAdapter`
/// is the proof — it fails every vault call, so a served call could not have
/// looked like this.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_mode_keeper_answers_the_locked_message_through_a_relay() {
    let tmp = tempfile::TempDir::new().unwrap();
    let check = SwitchableCheck::answering(Verdict::Locked(LockReason::TrialEnded));
    let in_flight = InFlight::new();
    let (stop, keeper) = start_lock_mode_keeper(
        tmp.path(),
        Duration::from_secs(30),
        Subscription::lock(check.clone(), in_flight.clone()),
    )
    .await;

    let pool = pool_for(tmp.path());
    let result = pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await
        .expect("a locked call comes back as a readable tool result");
    assert_eq!(result.is_error, Some(true));
    assert_eq!(result_text(&result), LockReason::TrialEnded.message());
    assert_eq!(in_flight.count(), 0, "the call is no longer counted");
    assert!(check.calls() >= 1, "the real check decided it");

    let _ = stop.send(());
    let _ = keeper.await;
}

/// §6.2 asks for an identical tool list in lock mode. It is identical by
/// construction — the contract does not depend on the adapter behind it — and
/// this pins that, so nobody can make the locked surface drift.
#[test]
fn the_tool_contract_does_not_depend_on_the_adapter_behind_it() {
    fn contract<A: vault_mcp::Adapter + 'static>(adapter: A) -> String {
        let server = StdioServer::new(Arc::new(adapter), Vec::new());
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
        hasher.finalize().to_hex().to_string()
    }
    assert_eq!(
        contract(NoVaultAdapter),
        contract(RecordingAdapter::default()),
        "lock mode must present the same tools as a full keeper"
    );
}

/// The user pays, then asks their agent something. §6.2: the call answers
/// "unlocking" and triggers the exit, so the next call reaches a full keeper.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_mode_call_that_finds_the_user_entitled_unlocks_the_keeper() {
    let tmp = tempfile::TempDir::new().unwrap();
    // A tick still reads "locked", so nothing but the CALL can end this
    // keeper — which is what this test is about (§6.2's call path, not its
    // tick). The real check would see both change together.
    let check =
        SwitchableCheck::splitting(Verdict::Locked(LockReason::TrialEnded), Verdict::Entitled);
    let (_stop, keeper) = start_lock_mode_keeper(
        tmp.path(),
        Duration::from_secs(30),
        Subscription::lock(check.clone(), InFlight::new()),
    )
    .await;

    let pool = pool_for(tmp.path());
    let result = pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await
        .expect("the call is answered, not dropped");
    assert_eq!(result.is_error, Some(true));
    assert_eq!(result_text(&result), LockReason::Unlocking.message());
    assert_eq!(
        exit_within(keeper, Duration::from_secs(5)).await,
        Some(KeeperExit::ModeChanged),
        "an entitled call in lock mode must end the locked keeper"
    );
}

/// With no call at all — nobody is using their agent — the tick alone must
/// notice that the subscription came back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_mode_keeper_exits_when_entitlement_returns_with_no_call_at_all() {
    let tmp = tempfile::TempDir::new().unwrap();
    let check = SwitchableCheck::answering(Verdict::Locked(LockReason::TrialEnded));
    let (_stop, keeper) = start_lock_mode_keeper(
        tmp.path(),
        Duration::from_secs(30),
        Subscription::lock(check.clone(), InFlight::new()),
    )
    .await;

    check.set(Verdict::Entitled);
    assert_eq!(
        exit_within(keeper, Duration::from_secs(5)).await,
        Some(KeeperExit::ModeChanged)
    );
    assert_eq!(
        check.calls(),
        0,
        "no call was made, so the tick found it by reading"
    );
}

/// A locked keeper whose user is still locked stays put: it is cheap, and
/// restarting it for nothing would only make the next call wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_mode_keeper_stays_while_the_user_is_still_locked() {
    let tmp = tempfile::TempDir::new().unwrap();
    let check = SwitchableCheck::answering(Verdict::Locked(LockReason::SubscriptionEnded));
    let (stop, keeper) = start_lock_mode_keeper(
        tmp.path(),
        Duration::from_secs(30),
        Subscription::lock(check.clone(), InFlight::new()),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        exit_within(keeper, Duration::from_millis(200))
            .await
            .is_none(),
        "a still-locked keeper must not restart itself"
    );
    assert!(check.peeks() >= 2, "it was re-evaluating all along");
    let _ = stop.send(());
}

// ---------------------------------------------------------------------------
// The relay's sign-in short-circuit (§8.26 §6.3, §8.35)
//
// Nobody signed in on this computer means no keeper can do anything useful:
// the relay says so itself rather than asking Task Scheduler to start one that
// could only say the same thing. `CountingStarter` is the proof — a
// short-circuit that still asked for a keeper would show up here.
// ---------------------------------------------------------------------------

/// A local app data folder with an account folder inside it, and optionally a
/// marker saying somebody is signed in.
fn account_home(root: &std::path::Path, signed_in: bool) -> std::path::PathBuf {
    let home = root.join("localappdata");
    std::fs::create_dir_all(&home).unwrap();
    if signed_in {
        write_marker(&home);
    } else {
        // The folder exists but holds no marker.
        vault_app::account::open_account_dir(&home).unwrap();
    }
    vault_app::account::account_dir_path(&home)
}

fn write_marker(home: &std::path::Path) {
    let dir = vault_app::account::open_account_dir(home).unwrap();
    let lock = dir
        .lock(Duration::from_secs(2))
        .unwrap()
        .expect("the refresh lock is free");
    dir.write_marker(&lock, "user_2abc").unwrap();
}

fn relay_settings_for_account(
    root: std::path::PathBuf,
    account_dir: Option<std::path::PathBuf>,
    marker_recheck: Duration,
) -> RelaySettings {
    RelaySettings {
        account_dir,
        marker_recheck,
        ..relay_settings(root, &["work"])
    }
}

/// The whole point of §6.3: no marker, no keeper start. On a signed-out
/// computer every AI app that is open would otherwise ask Task Scheduler for a
/// keeper on every call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signed_out_computer_answers_the_sign_in_message_and_starts_no_keeper() {
    let tmp = tempfile::TempDir::new().unwrap();
    let account_dir = account_home(tmp.path(), false);
    let starter = Arc::new(CountingStarter::default());
    let pool = KeeperPool::new(
        relay_settings_for_account(
            tmp.path().to_path_buf(),
            Some(account_dir),
            Duration::from_millis(50),
        ),
        starter.clone(),
        Arc::new(FixedKey(MASTER_KEY)),
    );

    // At this layer a call that never left the process is `NotSent`; it is
    // `RelayServer` that turns it into the `isError` tool result the agent
    // reads (ADR-103 D2), which `vault-mcp`'s own tests cover.
    match pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await
    {
        Err(UpstreamError::NotSent(reason)) => {
            assert_eq!(reason, LockReason::SignedOut.message());
        }
        other => panic!("a signed-out computer must answer the sign-in message, got {other:?}"),
    }
    assert_eq!(
        starter.0.load(Ordering::SeqCst),
        0,
        "a signed-out computer must not ask for a keeper"
    );
}

/// The reason §6.3 says "checked twice, 200 ms apart": sign-in writes the
/// marker, and a relay that looked once at the wrong moment would tell a user
/// who has just signed in to sign in again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_marker_that_appears_between_the_two_checks_is_not_a_signed_out_computer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("localappdata");
    std::fs::create_dir_all(&home).unwrap();
    vault_app::account::open_account_dir(&home).unwrap();
    let account_dir = vault_app::account::account_dir_path(&home);

    let writing = {
        let home = home.clone();
        tokio::task::spawn_blocking(move || {
            std::thread::sleep(Duration::from_millis(150));
            write_marker(&home);
        })
    };

    let starter = Arc::new(CountingStarter::default());
    let pool = KeeperPool::new(
        relay_settings_for_account(
            tmp.path().to_path_buf(),
            Some(account_dir),
            Duration::from_millis(400),
        ),
        starter.clone(),
        Arc::new(FixedKey(MASTER_KEY)),
    );

    // No keeper exists, so this ends in "starting" — the point is that it got
    // as far as asking for one instead of short-circuiting.
    let _ = pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await;
    writing.await.unwrap();
    assert!(
        starter.0.load(Ordering::SeqCst) >= 1,
        "a marker written between the two checks means somebody IS signed in"
    );
}

/// Signed in but no lease yet (the first fetch failed, or it is a brand new
/// account): that is a normal keeper start, which retries the fetch. Only the
/// marker decides here — the relay never reads the lease.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_marker_with_no_lease_starts_a_keeper_as_usual() {
    let tmp = tempfile::TempDir::new().unwrap();
    let account_dir = account_home(tmp.path(), true);
    let starter = Arc::new(CountingStarter::default());
    let pool = KeeperPool::new(
        relay_settings_for_account(
            tmp.path().to_path_buf(),
            Some(account_dir),
            Duration::from_millis(50),
        ),
        starter.clone(),
        Arc::new(FixedKey(MASTER_KEY)),
    );

    let _ = pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await;
    assert!(
        starter.0.load(Ordering::SeqCst) >= 1,
        "a signed-in computer asks for a keeper as it always did"
    );
}

/// Every build before this arc: no account settings, so no folder to read and
/// no short-circuit, ever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_build_with_no_sign_in_never_short_circuits() {
    let tmp = tempfile::TempDir::new().unwrap();
    let starter = Arc::new(CountingStarter::default());
    let pool = KeeperPool::new(
        relay_settings_for_account(tmp.path().to_path_buf(), None, Duration::from_millis(50)),
        starter.clone(),
        Arc::new(FixedKey(MASTER_KEY)),
    );

    let _ = pool
        .call_tool(search_call(), soon(), CancellationToken::new())
        .await;
    assert!(
        starter.0.load(Ordering::SeqCst) >= 1,
        "a build with no sign-in must behave exactly as it did before"
    );
}

/// "Delete everything" from a locked computer. Somebody whose trial has just
/// ended is exactly the person most likely to ask for their memories back and
/// then delete them — and a lock-mode keeper holds `.vault.lock` just as a
/// full one does, so the handover has to work from lock mode too (§6.2 asks
/// for an "identical ... erasure intent and handover path", and this is what
/// proves it rather than arguing it from shared code).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn erasure_takes_the_vault_from_a_lock_mode_keeper() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let check = SwitchableCheck::answering(Verdict::Locked(LockReason::TrialEnded));

    // As `zaaheen keeper` does: the process holds the vault lock while it
    // serves, and its exit is what releases it.
    let keeper_lock = ConsolidatorLock::try_acquire_named(&root, VAULT_LOCKFILE_NAME).unwrap();
    let (_stop, serving) = start_lock_mode_keeper(
        &root,
        Duration::from_secs(30),
        Subscription::lock(check, InFlight::new()),
    )
    .await;
    let keeper = tokio::spawn(async move {
        let exit = serving.await;
        drop(keeper_lock);
        exit
    });

    let held = exclusive::take_exclusive(
        &root,
        &FixedKey(MASTER_KEY),
        Duration::from_secs(3),
        Duration::from_secs(1),
    )
    .await
    .expect("a locked keeper must hand the vault over too");
    assert_eq!(
        keeper.await.unwrap().unwrap().unwrap(),
        KeeperExit::HandedOver,
        "the handover path is the same one a full keeper takes"
    );
    assert!(discovery::read(&root).unwrap().is_none());
    assert!(
        intent::is_held(&root),
        "no new keeper may start while the erasure runs"
    );
    drop(held);
}

// ── Session 59: the Agents tab's list, and cancelled calls (ADR-107) ─────────

/// The relay introduces itself under its AI app's own name, the keeper lists
/// that name for the desktop, and the entry goes when the app does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_keeper_lists_the_app_a_relay_names_until_it_leaves() {
    use vault_app::keeper::clients;
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::default());
    let (stop, keeper) = start_keeper(tmp.path(), adapter, Duration::from_secs(30)).await;

    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["work"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    );
    pool.app_connected(&rmcp::model::Implementation::new("cursor-vscode", "1"));
    pool.call_tool(search_call(), soon(), CancellationToken::new())
        .await
        .expect("served");
    let names: Vec<String> = clients::read_live(tmp.path())
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, ["cursor-vscode"]);

    drop(pool);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !clients::read_live(tmp.path()).is_empty() {
        assert!(
            Instant::now() < deadline,
            "the app left but is still listed"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let _ = stop.send(());
    assert_eq!(keeper.await.unwrap().unwrap(), KeeperExit::Shutdown);
    assert!(
        !clients::clients_path(tmp.path()).exists(),
        "a stopping keeper removes the list"
    );
}

/// A call the relay gives up on is dropped by the keeper, not finished for
/// nobody: the next caller gets the desk at once instead of queueing behind
/// it (2026-09-24: a new chat's single question timed out behind a burst the
/// relay had already abandoned).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_the_relay_gives_up_on_is_dropped_by_the_keeper() {
    let tmp = tempfile::TempDir::new().unwrap();
    let adapter = Arc::new(RecordingAdapter::slow(Duration::from_secs(3)));
    let (stop, keeper) = start_keeper(tmp.path(), adapter.clone(), Duration::from_secs(30)).await;
    let pool = KeeperPool::new(
        relay_settings(tmp.path().to_path_buf(), &["work"]),
        Arc::new(NoStart),
        Arc::new(FixedKey(MASTER_KEY)),
    );

    let abandoned = pool
        .call_tool(
            search_call(),
            Instant::now() + Duration::from_millis(400),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(abandoned, Err(UpstreamError::TimedOut)),
        "{abandoned:?}"
    );

    let asked = Instant::now();
    pool.call_tool(search_call(), soon(), CancellationToken::new())
        .await
        .expect("served");
    assert!(
        asked.elapsed() < Duration::from_millis(4500),
        "the next call waited behind the abandoned one ({:?})",
        asked.elapsed()
    );
    assert_eq!(
        adapter.completed(),
        1,
        "only the wanted call ran to the end"
    );

    drop(pool);
    let _ = stop.send(());
    let _ = keeper.await;
}
