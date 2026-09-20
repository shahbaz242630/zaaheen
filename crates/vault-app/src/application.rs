//! `Application` — composition root for V0.1. Owns the full dependency
//! graph and exposes the wired [`VaultAdapter`] for the MCP server to
//! dispatch through.
//!
//! ## T0.1.10 Phase 1 scope (this commit)
//!
//! Phase 1 lands [`Application::new`] — the **minimal construction
//! surface** that instantiates every concrete dep the V0.1 stack needs
//! and wires them into a [`VaultAdapter`]. No lifecycle, no MCP server
//! bind, no cascading-retry-worker spawn — those land in Phase 2.
//!
//! Per session-open Decision 2 (HANDOFF.md), T0.1.10 is consume-existing-
//! contracts work — every type used here was locked in T0.1.5–T0.1.9.
//! Phase 1's job is purely to confirm the composed dep graph runs
//! end-to-end against real LanceDB / SQLCipher / ort backends and to
//! exercise the four pre-declared stop-and-escalate triggers (Decision
//! 3) via `tests/integration_smoke.rs`.
//!
//! ## Wiring contract
//!
//! - **`StorageBackend`** owns its own internal `MetadataStore` +
//!   `LanceVectorStore` + `DuckDbGraphStore` per [`StorageBackend::open`].
//! - **`SemanticRetriever`** receives a third `Arc<MetadataStore>` handle
//!   (separate connection to the same SQLCipher file) plus the shared
//!   `Arc<dyn VectorStore>` extracted from `StorageBackend::vector_store`.
//!   Sharing the vector-store `Arc` (not opening a second LanceDB handle)
//!   is the correct pattern — LanceDB does not officially support
//!   concurrent handles to the same dataset directory, and the `Arc`
//!   already provides the necessary sharing.
//! - **`VaultAdapter`** receives a fourth `MetadataStore` handle for its
//!   `append_tool_invoke_audit` path per the existing adapter contract
//!   (sibling docstring at `adapter.rs`).
//!
//! Three separate `MetadataStore` handles to the same SQLCipher file are
//! deliberate — each is its own connection. SQLCipher with WAL mode
//! supports this; the audit-chain BLAKE3 hash links remain consistent
//! across interleaved writes from multiple connections (verified by
//! `trigger_b_audit_chain_consistent_across_composition` in
//! `tests/integration_smoke.rs`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use uuid::Uuid;
use vault_consolidator::{
    migrate_plaintext_reports, write_report_atomic, ConsolidationReport, Consolidator,
    ConsolidatorConfig,
};
use vault_core::{Boundary, VaultError, VaultResult};
use vault_embedding::{
    BgeSmallProvider, EmbeddingProvider, LazyQwen3Reranker, RerankProvider, EMBEDDING_DIM,
};
use vault_llm::{LlmProvider, Phi4MiniConfig, Phi4MiniProvider};
use vault_mcp::{Adapter, StdioServer};
use vault_retrieval::{
    FilesystemReportLoader, GraphRetriever, HybridRetriever, KeywordIndex, KeywordRetriever,
    RerankedRetriever, Retriever, SemanticRetriever, StructuredReadPipeline,
};
use vault_storage::{MemoryFilter, MetadataStore, RetryWorker, StepResult, StorageBackend};
use zeroize::Zeroizing;

use crate::consolidator_lock::ConsolidatorLock;
use crate::process_exit::{LiveProcessExit, ProcessExit};
use crate::signal_source::{LiveSignalSource, SignalSource};
use crate::{AppConfig, VaultAdapter};

/// Hard upper bound on a single consolidation run per the locked-next-arc
/// Step 4 operational-safety contract (2026-05-26): 30 minutes. Past this,
/// the run is cancelled and [`VaultError::ConsolidatorTimeout`] returned.
/// Per-merge transactions already committed remain committed (ADR-046
/// atomic supersession); uncommitted work rolls back via storage primitives'
/// transaction wrappers; atomic REPORT artifact writes (`.tmp + rename` at
/// Commit 4) preserve the previous artifact intact under cancellation.
pub(crate) const CONSOLIDATOR_HARD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Wall-clock budget for the startup cascade drain (ADR-SEC-013).
///
/// Five seconds is chosen against what it is competing with, not against how
/// long a drain "should" take: the desktop app already spends far longer than
/// this loading its ranking model, and the founder's standing decision is that
/// the window opens immediately rather than blocking on background work. A
/// typical backlog is one or two entries and finishes in milliseconds; a
/// pathological one is capped here and simply continues on the background
/// worker, exactly as it did before this drain existed.
pub(crate) const STARTUP_DRAIN_BUDGET: Duration = Duration::from_secs(5);

/// What a startup drain actually managed to do. Reported honestly so the log
/// line cannot imply more than happened.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StartupDrain {
    /// Entries the drain took off the queue and tried to run.
    pub attempted: usize,
    /// Of those, how many completed their vector-store half.
    pub succeeded: usize,
    /// `true` when the queue reported Idle before the budget expired, i.e.
    /// nothing was left behind. `false` means the budget capped the drain and
    /// the background worker still has work — reported rather than inferred so
    /// the log line cannot overclaim.
    pub fully_drained: bool,
}

/// Wrap an inner consolidation future in a hard timeout. If the inner
/// future completes before `timeout_dur`, returns its [`VaultResult`]
/// verbatim. If the timeout fires first, drops the inner future and
/// returns [`VaultError::ConsolidatorTimeout`] with the elapsed-budget
/// seconds.
///
/// Factored out from [`Application::run_consolidation_with_safety`] so
/// the timeout semantics are independently testable with sub-second
/// budgets (the production const is 30 min — untestable end-to-end).
async fn timeout_or_consolidator_timeout<F, T>(timeout_dur: Duration, inner: F) -> VaultResult<T>
where
    F: std::future::Future<Output = VaultResult<T>>,
{
    match tokio::time::timeout(timeout_dur, inner).await {
        Ok(inner_result) => inner_result,
        Err(_elapsed) => Err(VaultError::ConsolidatorTimeout(timeout_dur.as_secs())),
    }
}

/// Environment variable that overrides the per-run consolidation hard timeout.
pub(crate) const CONSOLIDATOR_TIMEOUT_ENV: &str = "VAULT_CONSOLIDATOR_TIMEOUT_SECS";

/// Parse a [`CONSOLIDATOR_TIMEOUT_ENV`] override value into a [`Duration`].
///
/// Returns `None` (→ caller uses the default [`CONSOLIDATOR_HARD_TIMEOUT`]) for
/// a missing, empty, or unparseable value. The sentinel `0` means **no limit**
/// — a one-time full-sweep backfill on a large cold vault enriches every fact
/// (O(facts)) and legitimately runs longer than the nightly default; this is
/// how a scale-validation run is allowed to finish rather than being killed
/// mid-job. Pure + side-effect-free so it is unit-testable without mutating
/// process-global environment state.
fn parse_timeout_override(raw: Option<&str>) -> Option<Duration> {
    let secs = raw?.trim().parse::<u64>().ok()?;
    // `0` is the "no limit" sentinel: an effectively-unbounded budget so a
    // backfill run completes naturally. We map it to the max representable
    // duration rather than skipping the timeout wrapper, keeping one code path.
    if secs == 0 {
        Some(Duration::MAX)
    } else {
        Some(Duration::from_secs(secs))
    }
}

/// Resolve the per-run consolidation hard timeout: the [`CONSOLIDATOR_TIMEOUT_ENV`]
/// override if present and valid, otherwise the [`CONSOLIDATOR_HARD_TIMEOUT`]
/// default. Normal (nightly / unset) behaviour is unchanged.
fn resolve_consolidator_timeout() -> Duration {
    let raw = std::env::var(CONSOLIDATOR_TIMEOUT_ENV).ok();
    match parse_timeout_override(raw.as_deref()) {
        Some(dur) => {
            tracing::info!(
                target: "vault_app::consolidator",
                timeout_secs = dur.as_secs(),
                overridden = true,
                "consolidation hard timeout overridden via {CONSOLIDATOR_TIMEOUT_ENV}"
            );
            dur
        }
        None => CONSOLIDATOR_HARD_TIMEOUT,
    }
}

/// Composition root. Phase 1 wires the dep graph; Phase 1b adds the
/// minimum lifecycle (retry-worker spawn) needed for write→search
/// round-trips through the cascading orchestrator. Phase 2 adds full
/// lifecycle (shutdown handling, MCP server bind, signal handlers).
pub struct Application {
    adapter: Arc<VaultAdapter>,
    /// Held for [`Self::spawn_retry_worker`] to clone into the spawned [`RetryWorker`].
    /// `StorageBackend` is `#[derive(Clone)]` with `Arc<Inner>` semantics
    /// (per `cascading.rs:149`), so this clone is cheap and shares state
    /// with the [`VaultAdapter`]'s clone — both see the same retry_queue.
    storage: StorageBackend,
    /// Consolidator wired when [`AppConfig::phi4_model_path`] is `Some` at
    /// construction. Cloned out by [`Self::run_consolidation_with_safety`]
    /// for each invocation. `None` at integration-test time (no Phi-4
    /// GGUF on disk); in that case the safety wrapper returns
    /// [`VaultError::Config`] — graceful degradation per the locked-next-arc
    /// Thread 3 enterprise practice (fail-open on quality-degrading
    /// dependencies, signal it loudly, do not block startup).
    ///
    /// Added at T0.3.x Batch A (2026-05-26) per the architectural lock
    /// (Phi-4-mini stays at consolidation, Qwen-7B exits the read path).
    consolidator: Option<Arc<Consolidator>>,
    /// Vault root directory — derived from `AppConfig::metadata_path.parent()`
    /// at construction. Used by [`Self::run_consolidation_with_safety`] to
    /// place the cross-process [`ConsolidatorLock`] file. Captured here so
    /// the safety wrapper doesn't require `AppConfig` to be threaded
    /// through the lifecycle.
    vault_root: PathBuf,
    /// At-rest key (K3), captured for the same reason as `vault_root`: the
    /// consolidation safety wrapper needs it to SEAL each per-boundary
    /// REPORT (ADR-SEC-007) and must not require `AppConfig` to be threaded
    /// through the lifecycle.
    ///
    /// `Zeroizing` so it is wiped on drop (BRD §11.5.3). `Application` has
    /// no `Debug` derive, so there is no redaction impl to maintain here —
    /// if one is ever added, this field MUST be redacted.
    at_rest_key: Zeroizing<[u8; 32]>,
    /// Concrete handle to the lazy reranker (ADR-070, 2026-06-05), kept so the
    /// serve path can warm the ~1.2 GB model in the background AFTER the MCP
    /// handshake binds. `None` when no reranker model is configured (the cosine
    /// fallback needs no warm-up). Shares its `OnceCell` with the
    /// `dyn RerankProvider` clone held inside the read pipeline.
    reranker_warmup: Option<Arc<LazyQwen3Reranker>>,
}

/// What the ranking model is currently doing, for a UI that has to say
/// something truthful about it (ADR-090).
///
/// Exists because [`LazyQwen3Reranker::is_loaded`] alone cannot tell a user
/// which of three very different situations they are in: nothing to wait for,
/// a pending download, or a load in progress. Reporting "not ready" for all
/// three would either invent a wait that will never end or hide one that is
/// genuinely happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RerankerState {
    /// No reranker configured — the vault ranks with its retriever order and
    /// there is nothing to wait for. NOT an error, and NOT a wait.
    NotConfigured,
    /// Configured, but a downloaded component is absent — the first-run fetch
    /// has not landed (or failed). Reads degrade per ADR-089.
    Unavailable,
    /// Files are present and the model is loading (or has not been asked for
    /// yet). This is the only state that represents a genuine, finite wait.
    Preparing,
    /// Loaded and serving. Reads are at full ranking quality.
    Ready,
}

impl RerankerState {
    /// Stable wire string for the IPC boundary.
    ///
    /// Pinned by a test and by the frontend contract guard: the frontend
    /// branches on these exact strings, and a silent rename here would leave
    /// the UI stuck on a wait that never resolves. Deliberately says nothing
    /// about the model, runtime or vendor (ADR-086 white-label).
    pub fn as_wire_str(self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::Unavailable => "unavailable",
            Self::Preparing => "preparing",
            Self::Ready => "ready",
        }
    }
}

impl Application {
    /// Construct the full V0.1 dependency graph and wire it into a
    /// [`VaultAdapter`].
    ///
    /// # Configuration
    ///
    /// Takes the [`AppConfig`] composition-root configuration by
    /// reference. See [`AppConfig`]'s module docs for the migration-
    /// anchor history (T0.1.10 Phase 2b migrated the seven Phase 1
    /// inline parameters to AppConfig fields with verbatim names per
    /// rename-prohibition discipline).
    ///
    /// # Errors
    ///
    /// Surfaces the first failure in:
    /// 1. `StorageBackend::open` — SQLCipher / LanceDB / DuckDB open.
    /// 2. Second `MetadataStore::open` — adapter audit handle.
    /// 3. Third `MetadataStore::open` — retriever read handle.
    /// 4. `BgeSmallProvider::open` — model/tokenizer SHA verification +
    ///    ort dynamic load + ONNX session.
    ///
    /// All four failure modes propagate as [`VaultError`] variants
    /// the caller (Phase 2 `Application::start_with_mcp`) can pattern-
    /// match for startup-fatal vs degraded reporting.
    ///
    /// [`VaultError`]: vault_core::VaultError
    #[tracing::instrument(skip_all, fields(
        metadata_path = %config.metadata_path.display(),
        vector_dir = %config.vector_dir.display(),
        graph_path = %config.graph_path.display(),
    ))]
    pub async fn new(config: &AppConfig) -> VaultResult<Self> {
        // 1. StorageBackend — owns its own MetadataStore + LanceDB + DuckDB.
        //    SqlCipherKey clone is cheap (clones inner String); cloning
        //    inside the body is the canonical pattern for by-reference
        //    config (per AppConfig module docs).
        //
        //    T0.2.0 Phase 2 (2026-05-11): flipped from plaintext
        //    `StorageBackend::open` to sealed `open_with_at_rest_key`
        //    per ADR-040 amendment ("at_rest_key flows from keychain
        //    through AppConfig to migration consumer"). LanceDB is now
        //    AEAD-sealed at-rest via SealedFileStoreProvider; SQLCipher
        //    metadata + DuckDB graph remain unchanged at Phase 2.
        //    Plaintext `StorageBackend::open` is retained for the V0.1
        //    → V0.2 migration source path (see vault_storage::migration);
        //    Phase 3 deletes both plaintext constructors.
        let storage = StorageBackend::open_with_at_rest_key(
            &config.metadata_path,
            &config.vector_dir,
            &config.graph_path,
            config.key.clone(),
            EMBEDDING_DIM,
            &config.at_rest_key,
        )
        .await?;

        // 1b. Derive vault_root from metadata_path's parent. Moved up
        //     from former-step-12 at Commit 6 (locked-next-arc, 2026-05-26)
        //     because the new StructuredReadPipeline (step 9) needs it to
        //     wire FilesystemReportLoader; the Consolidator lockfile
        //     (step 11) also consumes it. By this line StorageBackend::open
        //     above has already used metadata_path, so its parent is
        //     guaranteed to exist on disk — the parent() check guards
        //     against the edge case where metadata_path has no parent
        //     component (e.g., a bare filename without a directory).
        let vault_root = config
            .metadata_path
            .parent()
            .ok_or_else(|| {
                VaultError::Config(
                    "AppConfig.metadata_path must have a parent directory for \
                     consolidator lockfile placement + structured read pipeline \
                     REPORT-loader root"
                        .into(),
                )
            })?
            .to_path_buf();

        // 2. Second MetadataStore handle for VaultAdapter's audit appends.
        let adapter_metadata =
            MetadataStore::open(&config.metadata_path, config.key.clone()).await?;

        // 3. Third MetadataStore handle, Arc-shared for SemanticRetriever.
        let retriever_metadata =
            Arc::new(MetadataStore::open(&config.metadata_path, config.key.clone()).await?);

        // 4. BgeSmallProvider — sync open (verifies SHA-256 model+tokenizer
        //    integrity, idempotent ort init, loads ONNX session +
        //    tokenizer). Sync at startup is acceptable per the existing
        //    vault-embedding test pattern; CPU-heavy work after this
        //    point goes through `EmbeddingProvider::embed` which itself
        //    handles `spawn_blocking` correctly.
        let provider = BgeSmallProvider::open(
            &config.model_path,
            &config.tokenizer_path,
            &config.ort_lib_path,
        )?;
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(provider);

        // 5. SemanticRetriever — shares storage's vector store Arc.
        //
        //    DO NOT open a second `LanceVectorStore::open_with_at_rest_key(vector_dir, …)`
        //    handle here. LanceDB does not officially support concurrent
        //    dataset handles to the same data directory; the `Arc<dyn
        //    VectorStore>` already in `StorageBackend` is the correct
        //    sharing primitive. Future refactors that "helpfully" open a
        //    second handle will surface as fragmentation / write-races
        //    under load — see the integration spike at
        //    `tests/integration_smoke.rs` trigger (b)/(c).
        let vector_store = storage.vector_store().clone();
        let semantic =
            SemanticRetriever::new(retriever_metadata.clone(), embedder.clone(), vector_store);
        let semantic: Arc<dyn Retriever> = Arc::new(semantic);

        // 6. KeywordIndex (T0.2.7 Phase 1) — in-RAM BM25 over all
        //    memory content. Bulk-loaded from the encrypted SQLite
        //    metadata store at startup; subsequent writes/updates/
        //    deletes maintain the index incrementally (vault-app's
        //    write path is wired in a follow-on phase — Phase 1 left
        //    a documented gap that lands when the read-path validation
        //    proves the architecture).
        //
        //    Per [[run-cargo-gates-in-background]] memory: the bulk-
        //    load completes in ~1 sec at 10K memories, ~10 sec at 100K
        //    — fine for V0.2 beta scale. Future on-disk sealed-sidecar
        //    persistence is deferred until startup-rebuild cost
        //    matters in practice.
        let keyword_index = Arc::new(KeywordIndex::new()?);
        let all_memories = retriever_metadata
            .list_memories(MemoryFilter::default(), None)
            .await?;
        keyword_index.bulk_insert(&all_memories).await?;
        drop(all_memories);
        // Clone (not move): the graph retrieval channel below also needs this
        // metadata-store handle to hydrate connected memories (ADR-SEC-002 Part 2).
        let keyword = KeywordRetriever::new(keyword_index.clone(), retriever_metadata.clone());
        let keyword: Arc<dyn Retriever> = Arc::new(keyword);

        // 7. HybridRetriever — fuses semantic + keyword via Reciprocal
        //    Rank Fusion (k=60, top_n_each=200) per T0.2.7 Phase 2. `keyword`
        //    is moved in here (its last use) — the `memory_search` retriever
        //    below is now the raw hybrid, so there is no separate keyword-gate
        //    handle to retain.
        let hybrid: Arc<dyn Retriever> = Arc::new(HybridRetriever::new(semantic.clone(), keyword));

        // 7b. The cross-encoder reranker — built ONCE here, shared by BOTH the
        //    `memory_search` retriever (step 8, ADR-071) and the `memory_read`
        //    pipeline (step 9). ADR-070 (2026-06-05): wrapped in
        //    `LazyQwen3Reranker` so its ~1.2 GB model load is deferred OFF the
        //    MCP `initialize` handshake (eager loading cost ~40 s before the
        //    server could answer — timed out Kimi, sat close to Claude Desktop's
        //    60 s window). Construction does NO disk I/O; the model loads on
        //    first use, warmed in the background by `start_with_mcp`'s
        //    `spawn_warmup`. `relevance_floor()` returns its constant without a
        //    load, so nothing on the handshake path can trigger it. One
        //    allocation, two views: the concrete `LazyQwen3Reranker` handle is
        //    kept on `reranker_warmup` for the serve-path warm-up; the
        //    `dyn RerankProvider` clone(s) drive search + read. All share the
        //    same `OnceCell`.
        let mut reranker_warmup: Option<Arc<LazyQwen3Reranker>> = None;
        let reranker_opt: Option<Arc<dyn RerankProvider>> = match (
            &config.rerank_model_path,
            &config.rerank_tokenizer_path,
        ) {
            (Some(rerank_model), Some(rerank_tokenizer)) => {
                let lazy = Arc::new(LazyQwen3Reranker::new(
                    rerank_model,
                    rerank_tokenizer,
                    &config.ort_lib_path,
                ));
                let reranker: Arc<dyn RerankProvider> = lazy.clone();
                reranker_warmup = Some(lazy);
                tracing::info!(
                    target: "vault_app::startup",
                    "reranker configured: Qwen3-Reranker-0.6B (ADR-057 amendment; lazy-loaded per ADR-070; shared by search + read)"
                );
                Some(reranker)
            }
            _ => {
                tracing::info!(
                    target: "vault_app::startup",
                    "no reranker model configured — search stays raw hybrid, read falls back to the cosine gate (graceful degradation)"
                );
                None
            }
        };

        // 8. `memory_search` retriever. PRODUCTION (ADR-071, 2026-06-05): the raw
        //    hybrid wrapped in a `RerankedRetriever` — base hybrid → ADR-069
        //    recall-union (semantic top-N) → cross-encoder rerank → drop
        //    below-no-signal-floor junk → top-K. This brings search up to the
        //    same relevance quality `memory_read` has (BRD §5.5: "Reranked. No
        //    single-strategy weakness."). Cross-agent dogfood (2026-06-05) showed
        //    the raw hybrid ranked the correct "instrument" answer #4/10 behind
        //    keyword-overlap distractors; the reranker promotes it to #1 and the
        //    floor lets search honestly return "nothing found" (empty) on a
        //    no-signal query instead of dumping the K nearest irrelevant facts.
        //    FALLBACK: with no reranker model configured, search stays the raw
        //    hybrid (recall-first, ADR-067) — mirrors the read cosine fallback.
        let retriever: Arc<dyn Retriever> = match &reranker_opt {
            Some(reranker) => {
                // ADR-SEC-002 Part 2 Amendment 1 — the knowledge-graph recall
                // channel is PARKED as tech-debt #9 (2026-06-29): the hard
                // 40-distractor dogfood showed no win (graph-ON == graph-OFF,
                // byte-identical), so it ships OFF by default. The tested mechanism
                // (recall-channels-first pool assembly + the 2-hop traverse) is
                // preserved behind an opt-in switch: `VAULT_ENABLE_GRAPH_CHANNEL=1`
                // (or `true`) wires it ON at runtime with NO recompile — the lever
                // the graph read-path dogfood A/B uses to isolate the channel's
                // contribution when we revisit it on real beta/agent demand.
                let graph_enabled = std::env::var("VAULT_ENABLE_GRAPH_CHANNEL")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                let graph_channel: Option<Arc<dyn Retriever>> = if graph_enabled {
                    tracing::info!(
                        target: "vault_app::startup",
                        "memory_search: reranked retriever (hybrid ∪ semantic ∪ graph → rerank → floor, ADR-071 + ADR-SEC-002 Part 2) — graph channel ENABLED via VAULT_ENABLE_GRAPH_CHANNEL"
                    );
                    // ADR-SEC-002 Part 2: knowledge-graph recall channel. Resolves
                    // query-named entities → traverses → surfaces the connected
                    // memories, unioned into the rerank pool (additive, reorder-only).
                    Some(Arc::new(GraphRetriever::new(
                        storage.graph_store().clone(),
                        retriever_metadata.clone(),
                    )) as Arc<dyn Retriever>)
                } else {
                    tracing::info!(
                        target: "vault_app::startup",
                        "memory_search: reranked retriever (hybrid ∪ semantic → rerank → floor, ADR-071) — graph channel PARKED OFF (tech-debt #9; set VAULT_ENABLE_GRAPH_CHANNEL=1 to revisit)"
                    );
                    None
                };
                Arc::new(
                    RerankedRetriever::new(
                        hybrid.clone(),
                        Some(semantic.clone()),
                        reranker.clone(),
                    )
                    .with_graph(graph_channel),
                )
            }
            None => {
                tracing::info!(
                    target: "vault_app::startup",
                    "memory_search: raw hybrid (recall-first, ADR-067 — no reranker configured)"
                );
                hybrid.clone()
            }
        };

        // 9. StructuredReadPipeline — deterministic filter+pack for the
        //    `memory_read` MCP tool per ADR-052 + ADR-054 (Commit 6 of
        //    the locked-next-arc, 2026-05-26). Replaces the V0.2-era
        //    Qwen-7B single-call synthesis pipeline (ADR-048 + ADR-049,
        //    formally retired by ADR-052) with code that:
        //
        //    - loads the per-boundary REPORT artifact via
        //      [`FilesystemReportLoader`] from
        //      `<vault_root>/reports/<boundary>.report.sealed`
        //      (SEALED since ADR-SEC-007 — the loader holds the at-rest
        //      key and unseals in memory),
        //    - enriches each retrieved candidate with its
        //      consolidator-discovered topic label, and
        //    - emits the six ADR-054 Contract 2 health-warnings
        //      (REPORT_MISSING, REPORT_STALE_*, TOPIC_NAMES_UNAVAILABLE,
        //      CLOCK_SKEW_DETECTED). DELTA_LOG_UNAVAILABLE was retired by
        //      ADR-054 Amendment 2 (Commit 7) when Plan Iteration 3
        //      Contract 4 was falsified by the shipped Commit 6 shape.
        //
        //    No LLM in this stage. The pipeline is always constructed
        //    (no Option) — no model loading, no fallible setup. The
        //    `AppConfig.qwen_model_path` field is now dead (kept with
        //    #[allow(dead_code)] until Commit 8 removes it).
        // ADR-SEC-007 one-shot migration. Every vault that ran a
        // consolidation before this build has
        // `reports/<boundary>.report.json` sitting on disk in the CLEAR,
        // containing verbatim memory text. Shipping the sealed writer alone
        // would never touch those files — the new writer uses a different
        // filename, so the plaintext would simply persist forever.
        //
        // Runs before the loader is built, on every startup, and is
        // idempotent (a vault with no legacy REPORTs is an all-zero no-op).
        // Deliberately NOT gated on a version marker: a marker that got out
        // of step would silently skip the sweep, and the sweep is cheap.
        //
        // A failure here is logged, not fatal — it must never prevent the
        // app from starting — but `failed_to_remove > 0` is an ERROR
        // because it means plaintext memory text is still readable.
        match migrate_plaintext_reports(&vault_root, &config.at_rest_key) {
            Ok(m) if m.found > 0 => tracing::warn!(
                target: "vault_app::report_migration",
                found = m.found,
                sealed = m.sealed,
                discarded = m.discarded,
                failed_to_remove = m.failed_to_remove,
                "ADR-SEC-007: legacy PLAINTEXT REPORT artifacts found and removed"
            ),
            Ok(_) => tracing::debug!(
                target: "vault_app::report_migration",
                "no legacy plaintext REPORT artifacts present"
            ),
            Err(e) => tracing::error!(
                target: "vault_app::report_migration",
                error = %e,
                "ADR-SEC-007 plaintext REPORT sweep failed; plaintext may remain on disk"
            ),
        }

        // ADR-SEC-013 — drain cascades left over from a previous session.
        //
        // `write_memory` / `delete_memory` commit metadata + an audit row
        // transactionally and ENQUEUE the vector-store half as a cascade; a
        // background `RetryWorker` performs it. The background worker is
        // fire-and-forget, and in the desktop app its `JoinHandle` is dropped
        // (tech-debt, HANDOFF), so anything still queued when the window closes
        // is deferred to the next launch. Nothing was making that next launch
        // actually happen.
        //
        // Observed live 2026-08-26: a memory deleted in the desktop app left
        // its row gone from SQLCipher but its VECTOR still in Lance 30 minutes
        // later, and closing the app did not clear it. Retrieval is
        // orphan-safe, so the deleted memory was never returned — but the
        // embedding of deleted content lingered on disk, which for a product
        // whose promise is "delete means delete" is the wrong kind of
        // almost-right.
        //
        // Deliberately BOUNDED and non-fatal: a large or permanently-failing
        // backlog must never stop the app from starting. Whatever does not
        // drain here stays queued for the background worker exactly as before,
        // so this can only improve on the previous behaviour.
        let drained = Self::drain_startup_cascades(&storage, STARTUP_DRAIN_BUDGET).await;
        if drained.attempted > 0 {
            tracing::info!(
                target: "vault_app::startup_drain",
                attempted = drained.attempted,
                succeeded = drained.succeeded,
                fully_drained = drained.fully_drained,
                "drained cascades queued by a previous session"
            );
        }

        let report_loader = Arc::new(FilesystemReportLoader::new(
            vault_root.clone(),
            &config.at_rest_key,
        ));
        // Relevance gate. Production (ADR-057 amendment, 2026-05-29): the
        // cross-encoder reranker (Qwen3-Reranker-0.6B) is the relevance gate —
        // it separates topically-adjacent-but-wrong facts that the cosine floor
        // could not. When both rerank paths are configured, open the reranker
        // and wire `with_reranker`; otherwise fall back to the cosine
        // `with_relevance_gate(semantic)` so a deployment without the ~1.2 GB
        // model still abstains on no-signal queries (graceful degradation).
        // Bug-2 fix (2026-05-31): the read pipeline uses the RAW `hybrid`. Read
        // abstention is owned by the reranker floor (production) or the cosine
        // floor (fallback) below — both judge meaning, so a no-keyword-overlap-
        // but-relevant read is not short-circuited by a lexical gate. As of
        // ADR-067 (2026-06-04) `memory_search` (step 8) is also wired to the raw
        // `hybrid`, so neither read nor search runs a hard BM25 gate.
        let base_pipeline = StructuredReadPipeline::new(hybrid, report_loader);
        // Reuse the shared `reranker_opt` built at step 7b (ADR-070 lazy load).
        // The reranker SUPERSEDES the cosine relevance GATE in production; the
        // ADR-069 recall-union still hands the pipeline the semantic channel to
        // widen the rerank pool with strong pure-semantic matches the hybrid's
        // RRF fusion starves on a populated vault (scale finding 2026-06-04: a
        // subject-less fact ranked pure-BGE #6/100 but hybrid > 20/100, so the
        // reranker never saw it). Fallback (no model): the cosine floor owns
        // abstention so a no-signal read still abstains (graceful degradation).
        let read_pipeline = match &reranker_opt {
            Some(reranker) => base_pipeline
                .with_reranker(reranker.clone())
                .with_relevance_gate(semantic.clone()),
            None => base_pipeline.with_relevance_gate(semantic),
        };
        tracing::info!(
            target: "vault_app::startup",
            vault_root = %vault_root.display(),
            "structured read pipeline wired (deterministic filter+pack, no LLM)"
        );

        // 10. VaultAdapter — composes the trait deps + optional read
        //    pipeline into the MCP Adapter surface. Clone the
        //    StorageBackend so Application retains a handle for
        //    `spawn_retry_worker()` to construct the worker against. The
        //    `#[derive(Clone)]` on StorageBackend is `Arc<Inner>`-
        //    shallow per cascading.rs:149 — both clones share the
        //    same retry_queue so writes via the adapter are drained
        //    by the worker constructed from Application's clone.
        let adapter_storage = storage.clone();
        let adapter = VaultAdapter::new(
            retriever,
            read_pipeline,
            embedder.clone(),
            adapter_storage,
            adapter_metadata,
            // Same Arc the retriever's keyword channel holds — inline
            // upsert/delete here keeps a fresh write searchable in the
            // same process (read-after-write fix, 2026-05-28).
            keyword_index.clone(),
        );

        // 11. Optional Consolidator (T0.3.x Batch A, 2026-05-26).
        //
        //    When `phi4_model_path` is `Some`, load Phi-4-mini-instruct
        //    at startup so the nightly consolidation workload doesn't
        //    pay model-load cost per run, then construct a
        //    `vault_consolidator::Consolidator` with the shared storage
        //    + embedder + a default `ConsolidatorConfig` (BRD §5.6
        //    defaults: 3 AM, 180-day decay, 365-day archive, 1000
        //    memories/run; similarity 0.84 per ADR-097, diverging from
        //    the BRD's 0.92 on measured evidence).
        //
        //    When `None`, the consolidator is unwired and
        //    `run_consolidation_with_safety` surfaces `VaultError::Config`
        //    — graceful degradation per the locked-next-arc Thread 3
        //    enterprise practice. Write + read paths remain fully
        //    functional; only nightly consolidation is unavailable.
        //
        //    Per the architectural lock (2026-05-26): Phi-4-mini stays
        //    at consolidation (cheap, offline, real quality contribution
        //    on the binary merge-classifier role); Qwen-7B exits the
        //    read path entirely. Read still uses Qwen via the existing
        //    `ReadPipeline` wiring above at step 9; Commit 6 of the
        //    locked-next-arc removes that and replaces it with a
        //    deterministic structured-fact pipeline.
        let consolidator = match &config.phi4_model_path {
            Some(path) => {
                tracing::info!(
                    target: "vault_app::startup",
                    phi4_model_path = %path.display(),
                    "loading Phi-4-mini for consolidator"
                );
                let model_dir = path
                    .parent()
                    .ok_or_else(|| {
                        VaultError::Config(
                            "AppConfig.phi4_model_path must have a parent directory".into(),
                        )
                    })?
                    .to_path_buf();
                let model_filename = path
                    .file_name()
                    .ok_or_else(|| {
                        VaultError::Config(
                            "AppConfig.phi4_model_path must have a filename component".into(),
                        )
                    })?
                    .to_string_lossy()
                    .into_owned();
                let mut phi4_config = Phi4MiniConfig::v0_2_default(model_dir);
                phi4_config.model_filename = model_filename;
                let phi4_provider = Phi4MiniProvider::new(phi4_config).await.map_err(|e| {
                    VaultError::Llm(format!("Phi-4-mini load failed at startup: {e}"))
                })?;
                let llm: Arc<dyn LlmProvider> = Arc::new(phi4_provider);
                let cons = Consolidator::new(
                    Arc::new(storage.clone()),
                    llm,
                    embedder,
                    ConsolidatorConfig::default(),
                );
                Some(Arc::new(cons))
            }
            None => {
                tracing::info!(
                    target: "vault_app::startup",
                    "phi4_model_path is None; consolidator not wired (graceful degradation \
                     per locked-next-arc Thread 3 — write/read remain functional, \
                     `vault-cli consolidate run` returns VaultError::Config)"
                );
                None
            }
        };

        // vault_root was derived at step 1b (moved up at Commit 6 so
        // step 9's StructuredReadPipeline could use it). The Consolidator
        // lockfile in `run_consolidation_with_safety` continues to consume
        // the same value via `self.vault_root`.

        Ok(Self {
            adapter: Arc::new(adapter),
            storage,
            consolidator,
            vault_root,
            at_rest_key: config.at_rest_key.clone(),
            reranker_warmup,
        })
    }

    /// Borrow the wired adapter. Phase 2 `Application::spawn_retry_worker` clones
    /// this `Arc` into the `StdioServer`'s constructor; integration
    /// tests in `tests/integration_smoke.rs` use it for direct dispatch.
    pub fn adapter(&self) -> &Arc<VaultAdapter> {
        &self.adapter
    }

    /// Borrow the vault root directory (the parent of the SQLCipher file).
    /// Surfaced so the desktop UI's Settings tab can show the user where
    /// their data actually lives — a local-first product should never make
    /// that a mystery.
    pub fn vault_root(&self) -> &Path {
        &self.vault_root
    }

    /// Kick off the ranking model's background load, returning whether there
    /// was anything to warm.
    ///
    /// # Why this is public rather than folded into [`Self::spawn_retry_worker`]
    ///
    /// [`Self::start_with_mcp`] already warms the model (step 3b). The desktop
    /// app cannot call it — `start_with_mcp` blocks on an MCP handshake that
    /// never arrives in a GUI (ADR-034) — so it calls
    /// [`Self::spawn_retry_worker`], which spawns only the retry worker. The
    /// result was that the desktop app silently never warmed the model and paid
    /// the full ~24 s load on the user's first search (measured live
    /// 2026-07-22). That method was then named `start()` and labelled
    /// "test-focused", which is a large part of why the gap went unnoticed
    /// (ADR-095).
    ///
    /// Folding the warm-up into [`Self::spawn_retry_worker`] would have fixed
    /// that by making every test that configures a reranker load a 1.2 GB model
    /// as a side effect of starting. So the warm-up stays **explicit at the call
    /// site**, matching the discipline that keeps the two entry points separate
    /// and named rather than flag-switched.
    ///
    /// Safe to call when the first-run download has not landed: the load fails
    /// fast with [`VaultError::ModelUnavailable`], logs, and leaves the cell
    /// cold so a later call retries (ADR-089). That is exactly why the Tauri
    /// layer calls this again after acquisition completes.
    ///
    /// Returns `false` when no reranker is configured — nothing was spawned.
    pub fn spawn_reranker_warmup(&self) -> bool {
        match &self.reranker_warmup {
            Some(reranker) => {
                reranker.spawn_warmup();
                true
            }
            None => false,
        }
    }

    /// Report what the ranking model is currently doing (ADR-090).
    ///
    /// Cheap: at worst two `stat` calls. Never triggers a load — asking
    /// whether the model is ready must not itself start the work being asked
    /// about.
    pub fn reranker_state(&self) -> RerankerState {
        match &self.reranker_warmup {
            None => RerankerState::NotConfigured,
            Some(reranker) => {
                if reranker.is_loaded() {
                    RerankerState::Ready
                } else if reranker.files_present() {
                    RerankerState::Preparing
                } else {
                    RerankerState::Unavailable
                }
            }
        }
    }

    /// Spawn the cascading retry worker on its own; return the
    /// [`tokio::sync::watch::Sender<bool>`] that signals shutdown when dropped
    /// or when `send(true)` is called.
    ///
    /// # This is a PRODUCTION entry point (renamed 2026-07-25, ADR-095)
    ///
    /// It was called `start()` and documented as a **"test-focused entry
    /// point"** for most of the project's life. That label was wrong, and
    /// believing it cost us four separate defects: a GUI cannot call
    /// [`Self::start_with_mcp`] (that blocks on an MCP handshake which never
    /// arrives — ADR-034), so the desktop app has ALWAYS started its lifecycle
    /// here, as does `vault-cli daemon`. Three of the four are catalogued in
    /// ADR-090; the fourth was the shutdown-drain gap (ADR-095), where the
    /// question "does the desktop drain its queue on close?" was hard to even
    /// ask while the method claimed to be for tests.
    ///
    /// The honest split is by COMPOSITION, not by test-vs-production:
    /// - this method — the worker alone, for hosts that own their own
    ///   event loop (Tauri, the HTTP daemon) and for tests;
    /// - [`Self::start_with_mcp`] — the same worker plus stdio-MCP bind,
    ///   signal handlers, and the await-aware
    ///   [`ApplicationHandle::shutdown`].
    ///
    /// # Why calling one of them is mandatory
    ///
    /// `StorageBackend::write_memory` writes SQLite + `retry_queue` only; the
    /// vector store is updated asynchronously by the worker draining
    /// `retry_queue` → `vector.upsert`. With no worker running, writes never
    /// reach the vector store and `SemanticRetriever` returns empty (the Phase
    /// 1 spike surfaced this; triggers (b) and (d) failed deterministically
    /// until this method existed).
    ///
    /// # Shutdown caveat (ADR-095)
    ///
    /// The spawned task's `JoinHandle` is dropped, so callers can signal the
    /// worker but cannot AWAIT its drain. `start_with_mcp` keeps the handle and
    /// awaits it in `ApplicationHandle::shutdown`; callers of this method
    /// (Tauri, daemon) therefore leave any still-queued cascades to the next
    /// launch. That is safe — `retry_queue` is durable and the worker resumes
    /// on the next start — but it is a deferral, not a flush.
    pub fn spawn_retry_worker(&self) -> tokio::sync::watch::Sender<bool> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let worker = RetryWorker::new(self.storage.clone());
        tokio::spawn(worker.run(rx));
        tx
    }

    /// Work the cascade queue until it is empty or `budget` expires
    /// (ADR-SEC-013).
    ///
    /// Called during [`Self::new`] so it cannot be forgotten by a caller — the
    /// desktop app and the CLI both get it for free. That placement is
    /// deliberate: the equivalent "each host remembers to do it" arrangement is
    /// how the desktop app ended up with no reranker warm-up for months
    /// (ADR-090), and how a queued delete could sit unprocessed across a whole
    /// app session.
    ///
    /// Never returns an error. A drain is opportunistic cleanup of work that
    /// is already durably queued; failing to complete it must not prevent the
    /// vault from opening, and anything left behind is retried by the
    /// background worker on its normal backoff schedule.
    async fn drain_startup_cascades(storage: &StorageBackend, budget: Duration) -> StartupDrain {
        let mut out = StartupDrain::default();
        let deadline = Instant::now() + budget;
        let mut worker = RetryWorker::new(storage.clone());

        while Instant::now() < deadline {
            match worker.step().await {
                Ok(StepResult::Idle) => {
                    out.fully_drained = true;
                    break;
                }
                Ok(StepResult::SucceededEntry { .. }) => {
                    out.attempted += 1;
                    out.succeeded += 1;
                }
                // Rescheduled / DeadLettered are both "handled": the entry was
                // taken off the due-list and its next state recorded. Counting
                // them as attempted-but-not-succeeded keeps the log honest
                // instead of looping on an entry that will not progress.
                Ok(_) => {
                    out.attempted += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        target: "vault_app::startup_drain",
                        error = %e,
                        "startup cascade drain stopped early; queued work stays \
                         for the background worker"
                    );
                    break;
                }
            }
        }

        out
    }

    /// **Production lifecycle entry point.** Spawn the cascading retry
    /// worker, bind the MCP `StdioServer` against `self.adapter`, and
    /// register signal handlers (Ctrl-C → graceful shutdown; second
    /// Ctrl-C → forced exit per locked semantics). Returns an
    /// [`ApplicationHandle`] that owns the spawned task `JoinHandle`s and
    /// exposes [`ApplicationHandle::shutdown`] for await-aware cleanup.
    ///
    /// # Path α discipline (T0.1.10 Phase 2)
    ///
    /// This method is **separate from** [`Self::spawn_retry_worker`], which
    /// spawns the worker alone for hosts that own their own event loop (the
    /// Tauri app, the HTTP daemon) and for tests. The two diverge at the API
    /// surface — explicitly named, no bool flag — so caller intent is
    /// clear from the call site. See HANDOFF.md Phase 2 plan paragraph
    /// for the Path α reasoning.
    ///
    /// Unlike [`Self::spawn_retry_worker`], this one KEEPS the worker's
    /// `JoinHandle`, which is what lets [`ApplicationHandle::shutdown`] await
    /// the shutdown drain rather than merely signalling it (ADR-095).
    ///
    /// # Errors
    ///
    /// - [`VaultError::McpBindFailed`] — `rmcp::ServiceExt::serve` failed
    ///   to bind the stdio transport (rare in practice; possible if
    ///   another process holds stdin or rmcp's transport layer hits an
    ///   I/O error during initial setup).
    /// - [`VaultError::WorkerSpawnFailed`] is reserved as a future-proof
    ///   variant for fallible worker startup paths (e.g., when worker
    ///   construction grows config-validation or initial-state inspection
    ///   that can fail). Phase 2's `RetryWorker::new` + `tokio::spawn`
    ///   are both infallible, so this variant is **technically dead code
    ///   at Phase 2 landing** — kept defined per session-open pre-flag
    ///   #5 awaiting user (a)/(b) decision on whether to retain as
    ///   future-proof or remove until a concrete consumer surfaces.
    #[tracing::instrument(skip_all, fields(boundary_count = authorized_boundaries.len()))]
    pub async fn start_with_mcp(
        &self,
        authorized_boundaries: Vec<Boundary>,
        consolidation_run_at: Option<chrono::NaiveTime>,
        gate: Option<vault_mcp::Gate>,
    ) -> VaultResult<ApplicationHandle> {
        use rmcp::ServiceExt;

        // 1. Spawn cascading retry worker — same as Self::spawn_retry_worker().
        let (shutdown_signal, rx) = tokio::sync::watch::channel(false);
        let worker = RetryWorker::new(self.storage.clone());
        let worker_handle = tokio::spawn(worker.run(rx));

        // 2. Build StdioServer (infallible) against the wired adapter.
        //    `Arc<VaultAdapter>` coerces to `Arc<dyn Adapter>` at the
        //    let-binding via DST coercion since `VaultAdapter: Adapter`.
        let adapter_dyn: Arc<dyn Adapter> = self.adapter.clone();
        let server = StdioServer::new(adapter_dyn, authorized_boundaries);
        // The subscription gate (ADR-104), when this build carries a sign-in.
        // Direct mode serves one AI app straight from this process, so the
        // gate goes around the same server the keeper wraps.
        let server = vault_mcp::maybe_gated(gate.as_ref(), server);

        // 3. Bind stdio transport synchronously — McpBindFailed propagates
        //    here if rmcp's serve() setup errs. Awaiting serve() returns
        //    a `RunningService` (concrete generic type, not named because
        //    inference handles the storage); the waiting() loop then runs
        //    until the transport closes.
        let running = ServiceExt::serve(server, rmcp::transport::stdio())
            .await
            .map_err(|e| VaultError::McpBindFailed(format!("rmcp serve: {e}")))?;
        let server_handle = tokio::spawn(async move {
            // waiting() blocks until the transport closes (stdin EOF or
            // process termination). We discard its Result; benign errors
            // already surface as the server task's exit, and panics
            // become JoinError on the handle.
            let _ = running.waiting().await;
        });

        // 3b. Warm the lazy reranker OFF the handshake path (ADR-070). The
        //     transport is bound — `initialize` is answerable now — so kicking
        //     the ~1.2 GB model load onto a background thread means the first
        //     read does not pay the full load. No-op when no reranker model is
        //     configured (the cosine fallback needs no warm-up).
        if let Some(reranker) = &self.reranker_warmup {
            reranker.spawn_warmup();
        }

        // 4. Spawn signal handler — first Ctrl-C → graceful shutdown
        //    signal; second Ctrl-C → forced exit per locked semantics.
        //    Production wires `LiveProcessExit` (Phase 4a) +
        //    `LiveSignalSource` (Phase 4b). Tests construct
        //    `CapturingProcessExit` + `MockSignalSource` to drive the
        //    handler through both Ctrl-C paths without OS signals.
        let signal_tx = shutdown_signal.clone();
        let exit_impl: Arc<dyn ProcessExit> = Arc::new(LiveProcessExit);
        let signal_impl: Arc<dyn SignalSource> = Arc::new(LiveSignalSource);
        let signal_handle = tokio::spawn(handle_signals(signal_tx, exit_impl, signal_impl));

        // 5. Spawn the nightly consolidator scheduler (T0.2.6) — only when a
        //    consolidator is configured. It sleeps until the configured local
        //    `run_at`, runs the full safe pipeline (lockfile + 30-min timeout +
        //    merge/contradiction/decay + enrichment + REPORT), then repeats.
        //    The sleep is cancellable via the shared shutdown signal so Ctrl-C
        //    exits promptly instead of waiting hours for the next run window.
        let consolidator_handle = self.consolidator.as_ref().map(|consolidator| {
            let consolidator = consolidator.clone();
            let vault_root = self.vault_root.clone();
            // ADR-SEC-007: the scheduled run seals its REPORTs like any
            // other, so the task owns a zeroizing clone of the at-rest key
            // alongside the vault root.
            let at_rest_key = self.at_rest_key.clone();
            // The configured-time default lives in `ConsolidatorConfig`
            // (BRD §5.6 default 03:00); a caller-supplied override
            // (`vault-cli mcp --run-at HH:MM`) takes precedence — primarily
            // an ops/testing affordance to point the run window a minute
            // ahead and watch the scheduler fire end-to-end.
            let run_at = consolidation_run_at.unwrap_or_else(|| consolidator.run_at());
            let cancel = shutdown_signal.subscribe();
            tracing::info!(
                target: "vault_app::consolidator",
                run_at = %run_at,
                overridden = consolidation_run_at.is_some(),
                "nightly consolidation scheduler started"
            );
            tokio::spawn(run_consolidator_schedule(
                consolidator,
                vault_root,
                at_rest_key,
                run_at,
                cancel,
            ))
        });

        Ok(ApplicationHandle {
            shutdown_signal,
            worker_handle,
            server_handle,
            signal_handle,
            consolidator_handle,
        })
    }

    /// Run one consolidation cycle under cross-process lockfile +
    /// [`CONSOLIDATOR_HARD_TIMEOUT`] (30 min). Returns the underlying
    /// [`vault_consolidator::ConsolidationReport`] on success.
    ///
    /// # Operational safety (locked-next-arc Step 4, 2026-05-26)
    ///
    /// - **Cross-process lockfile** at `<vault_root>/.consolidator.lock` —
    ///   refuses with [`VaultError::ConsolidatorBusy`] if held. Released
    ///   on drop (RAII guard via [`ConsolidatorLock`]) including under
    ///   panic unwind, and by the OS if the holder crashes, so a stale lock
    ///   cannot block a later run (ADR-SEC-020, `consolidator_lock` module
    ///   docs).
    /// - **30-min hard timeout** — past this, the run is cancelled and
    ///   [`VaultError::ConsolidatorTimeout`] returned. Per-merge
    ///   transactions already committed remain committed (ADR-046);
    ///   uncommitted work rolls back via storage primitives' tx wrappers.
    /// - **Tracing span** tagged with `run_id = Uuid::new_v4()` propagates
    ///   to every consolidator phase log line for end-to-end correlation.
    ///
    /// # Errors
    ///
    /// - [`VaultError::Config`] — consolidator not wired
    ///   ([`AppConfig::phi4_model_path`] was `None` at construction).
    /// - [`VaultError::ConsolidatorBusy`] — another run holds the lockfile.
    /// - [`VaultError::ConsolidatorTimeout`] — exceeded the 30-min budget.
    /// - Any [`VaultError`] propagated by
    ///   [`vault_consolidator::Consolidator::run_consolidation`].
    #[tracing::instrument(skip_all)]
    pub async fn run_consolidation_with_safety(&self) -> VaultResult<ConsolidationReport> {
        let consolidator = self
            .consolidator
            .as_ref()
            .ok_or_else(|| {
                VaultError::Config(
                    "consolidator not configured (AppConfig.phi4_model_path was None at \
                     Application::new); set phi4_model_path to enable nightly consolidation"
                        .into(),
                )
            })?
            .clone();
        run_consolidation_under_safety(consolidator, &self.vault_root, &self.at_rest_key).await
    }
}

/// Core of [`Application::run_consolidation_with_safety`], factored out so the
/// nightly scheduler task can run the identical safe pipeline.
///
/// The scheduler task owns an `Arc<Consolidator>` plus the vault root (both
/// cheaply cloned from the `Application` at startup) rather than the whole
/// `Application`, so this free function takes exactly those two. Guarantees are
/// identical to the method: cross-process lockfile (RAII guard), 30-minute hard
/// timeout, `run_consolidation` → `enrich_facts` → `generate_reports` under one
/// cancellation budget, then best-effort per-boundary REPORT persistence.
#[tracing::instrument(skip_all)]
async fn run_consolidation_under_safety(
    consolidator: Arc<Consolidator>,
    vault_root: &std::path::Path,
    at_rest_key: &[u8; 32],
) -> VaultResult<ConsolidationReport> {
    let run_id = Uuid::new_v4();
    tracing::info!(
        target: "vault_app::consolidator",
        run_id = %run_id,
        "consolidation run starting under safety wrapper"
    );

    // Acquire the cross-process lockfile. The guard is held for the
    // entire run; dropped on function exit (success / error / panic
    // unwind) which removes the lockfile.
    let _lock = ConsolidatorLock::try_acquire(vault_root)?;

    // ADR-082: capture the run's START time and read the incremental watermark
    // while holding the lock. The run seeds Phase 1 / Phase 2b on facts created
    // since the watermark; it is advanced to `run_started_at` only after the
    // FULL pipeline below succeeds. A watermark read failure fails open to a
    // full sweep (a slow full run beats a missed merge/contradiction).
    let run_started_at = chrono::Utc::now();
    let since = match consolidator.consolidation_watermark().await {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(
                target: "vault_app::consolidator",
                run_id = %run_id,
                error = %e,
                "could not read consolidation watermark; falling back to a full sweep"
            );
            None
        }
    };

    // Wrap the consolidator's run_consolidation + per-boundary REPORT
    // generation in one hard timeout — both phases call the LLM and
    // re-embed, so both belong under the same cancellation budget. We
    // .clone() the Arc<Consolidator> so the future is 'static-friendly
    // (no borrow on self threaded through tokio::timeout's internal
    // future polling) AND so the original handle survives for the
    // post-success watermark advance below. enrich_facts + generate_reports
    // run AFTER run_consolidation so they reflect the post-merge /
    // post-invalidate active set (ADR-058 / ADR-074).
    let consolidator_run = consolidator.clone();
    let inner = async move {
        let report = consolidator_run.run_consolidation(since).await?;
        // ADR-074: document-side alias enrichment of the post-merge active
        // set (Gap-2 vocabulary-gap fix). Per-fact failures are counted
        // inside enrich_facts and never abort the run; only the initial
        // enumeration can error here.
        let enrichment = consolidator_run.enrich_facts().await?;
        tracing::info!(
            target: "vault_app::consolidator",
            run_id = %run_id,
            enriched = enrichment.facts_enriched,
            skipped = enrichment.facts_skipped,
            failed = enrichment.facts_failed,
            "alias enrichment pass complete"
        );
        let reports = consolidator_run.generate_reports(run_id).await?;
        Ok::<_, VaultError>((report, reports))
    };
    let (report, reports) =
        timeout_or_consolidator_timeout(resolve_consolidator_timeout(), inner).await?;

    // Persist each per-boundary REPORT atomically to the vault root.
    // The filesystem write lives in this app layer (it owns
    // `vault_root`); the consolidator stays filesystem-agnostic. A
    // single REPORT write failure is logged-and-continued rather than
    // aborting the whole run — mirrors the contradiction-invalidate
    // philosophy (a transient failure is retried next cycle, and a
    // missing REPORT surfaces as REPORT_MISSING at read time, which is
    // the correct degraded signal). The merge work already committed to
    // storage above is durable regardless.
    for report_artifact in &reports {
        match write_report_atomic(report_artifact, vault_root, at_rest_key) {
            Ok(path) => tracing::info!(
                target: "vault_app::consolidator",
                run_id = %run_id,
                boundary = %report_artifact.boundary.as_str(),
                topics = report_artifact.facts_by_topic.len(),
                path = %path.display(),
                "per-boundary REPORT written"
            ),
            Err(e) => tracing::warn!(
                target: "vault_app::consolidator",
                run_id = %run_id,
                boundary = %report_artifact.boundary.as_str(),
                error = %e,
                "REPORT write failed; REPORT_MISSING will surface at read until the next run succeeds"
            ),
        }
    }

    // ADR-082: the full pipeline succeeded and REPORTs are persisted — advance
    // the incremental watermark to this run's START time. A run that timed out
    // or errored above returned early via `?` and never reaches here, so its
    // watermark stays put and the next run retries the same backlog. A failed
    // advance is logged, not fatal (the run's work is already durable).
    if let Err(e) = consolidator
        .advance_consolidation_watermark(run_started_at)
        .await
    {
        tracing::warn!(
            target: "vault_app::consolidator",
            run_id = %run_id,
            error = %e,
            "consolidation succeeded but advancing the watermark failed; \
             the next run may re-process this window"
        );
    }

    // Report the WHOLE operation's duration, not just `run_consolidation`'s.
    //
    // `ConsolidationReport.duration` is set inside `run_consolidation`, which
    // stops its clock before `enrich_facts` and `generate_reports` have run --
    // and those two are where the Phi-4 calls live, so they are usually most of
    // the wall time. On the founder's first live run (2026-08-27) the recorded
    // summary said "in 0s" for a run that took ~20 seconds: the merge phase
    // genuinely finished in under a second with 6 memories, and the other 18
    // seconds were report generation.
    //
    // The number is read by a human in the Maintenance tab, where it can only
    // mean "how long did maintenance take". At 6 memories the discrepancy is
    // cosmetic; at a thousand it would hide a slow run, which is exactly when
    // someone would look. So the wrapper -- which owns the whole operation --
    // overwrites it with the whole operation's elapsed time.
    // `now - start`, in that order, so the delta is positive; `to_std` rejects
    // a negative one, and a clock that moved backwards falls back to the inner
    // duration rather than panicking.
    let mut report = report;
    report.duration = chrono::Utc::now()
        .signed_duration_since(run_started_at)
        .to_std()
        .unwrap_or(report.duration);

    tracing::info!(
        target: "vault_app::consolidator",
        run_id = %run_id,
        memories_processed = report.memories_processed,
        memories_merged = report.memories_merged,
        contradictions_resolved = report.contradictions_resolved,
        contradictions_auto_resolved = report.contradictions_auto_resolved,
        reports_written = reports.len(),
        duration_secs = report.duration.as_secs(),
        "consolidation run completed under safety wrapper"
    );

    Ok(report)
}

/// Nightly consolidator scheduler loop (T0.2.6, production path).
///
/// Sleeps until the next local `run_at`, runs the full safe pipeline via
/// [`run_consolidation_under_safety`], then repeats. Mirrors the retry worker's
/// shutdown-aware loop: the `select!` lets `cancel.changed()` interrupt the
/// sleep so shutdown is prompt instead of blocking for the next run window. A
/// failed or timed-out run is logged and the loop just waits for the next
/// `run_at` — one bad night never tears the scheduler down, and the
/// consolidator's idempotent passes self-heal on the following cycle.
async fn run_consolidator_schedule(
    consolidator: Arc<Consolidator>,
    vault_root: PathBuf,
    at_rest_key: Zeroizing<[u8; 32]>,
    run_at: chrono::NaiveTime,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        if *cancel.borrow() {
            break;
        }
        let wait =
            vault_consolidator::scheduler::duration_until_next_run(chrono::Local::now(), run_at);
        tracing::info!(
            target: "vault_app::consolidator",
            run_at = %run_at,
            wait_secs = wait.as_secs(),
            "next nightly consolidation scheduled"
        );
        tokio::select! {
            _ = tokio::time::sleep(wait) => {
                match run_consolidation_under_safety(
                    consolidator.clone(),
                    &vault_root,
                    &at_rest_key,
                ).await {
                    Ok(report) => tracing::info!(
                        target: "vault_app::consolidator",
                        memories_processed = report.memories_processed,
                        memories_merged = report.memories_merged,
                        "nightly consolidation complete"
                    ),
                    Err(e) => tracing::error!(
                        target: "vault_app::consolidator",
                        error = %e,
                        "nightly consolidation failed; retrying at next run_at"
                    ),
                }
            }
            _ = cancel.changed() => break,
        }
    }
}

/// Handle returned by [`Application::start_with_mcp`]. Owns the task
/// `JoinHandle`s for the retry worker, MCP server, and signal handler;
/// provides await-aware [`Self::shutdown`] for graceful production
/// cleanup.
///
/// # Lifecycle
///
/// - **Drop without `shutdown`**: the `shutdown_signal` `Sender` drops,
///   the worker exits cleanly via `cancel.changed()` Err arm, but the
///   server + signal tasks keep running until process exit. Acceptable
///   for tests / abnormal exit paths but NOT for production graceful
///   shutdown.
/// - **`shutdown().await`**: signals the worker, awaits its drain,
///   aborts the server + signal tasks (in-flight MCP requests dropped),
///   awaits all task `JoinHandle`s. Returns when all tasks have exited.
///
/// # Why `shutdown` consumes self by value
///
/// Terminal lifecycle methods consume by value to enforce single-call
/// semantics at compile time. Calling `shutdown` twice on the same
/// handle would attempt to re-await already-consumed `JoinHandle`s
/// (which panics). Consuming `self` prevents this entirely; the
/// borrow-checker rejects double-shutdown at compile time.
pub struct ApplicationHandle {
    shutdown_signal: tokio::sync::watch::Sender<bool>,
    worker_handle: tokio::task::JoinHandle<()>,
    server_handle: tokio::task::JoinHandle<()>,
    signal_handle: tokio::task::JoinHandle<()>,
    /// Nightly consolidator scheduler task (T0.2.6). `None` when no
    /// consolidator is configured (`AppConfig.phi4_model_path` was unset), in
    /// which case the vault still serves reads/writes — it just never
    /// self-consolidates.
    consolidator_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ApplicationHandle {
    /// Test-only constructor. Allows lifecycle tests to assert
    /// `shutdown` semantics without constructing a full `Application`
    /// (which requires SqlCipher + LanceDB + DuckDB + ORT). Caller
    /// passes pre-built `JoinHandle`s; production callers go through
    /// [`Application::start_with_mcp`].
    ///
    /// Phase 4b T0.1.11 — added per multi-agent code-review CRITICAL
    /// finding "vault-app/src/application.rs has zero tests."
    #[cfg(test)]
    pub(crate) fn for_test(
        shutdown_signal: tokio::sync::watch::Sender<bool>,
        worker_handle: tokio::task::JoinHandle<()>,
        server_handle: tokio::task::JoinHandle<()>,
        signal_handle: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            shutdown_signal,
            worker_handle,
            server_handle,
            signal_handle,
            // Lifecycle tests exercise worker/server/signal shutdown only; the
            // scheduler task is covered by its own pure-timing tests + the
            // app-layer scheduler test, so for_test omits it.
            consolidator_handle: None,
        }
    }

    /// Borrow the shutdown-signal `Sender`. Useful when an external
    /// supervisor wants to signal cancellation without consuming the
    /// handle (e.g., a parent task that's also tracking other lifecycle
    /// resources).
    pub fn shutdown_signal(&self) -> &tokio::sync::watch::Sender<bool> {
        &self.shutdown_signal
    }

    /// Block until one of the spawned tasks naturally exits, then perform
    /// graceful shutdown. The typical "main loop" entry-point for a CLI
    /// subcommand that runs the vault as a long-lived MCP stdio server
    /// (`vault-cli mcp serve`).
    ///
    /// Selects across:
    /// - **`server_handle`** — completes on stdio EOF (the MCP client,
    ///   typically Claude Desktop, disconnected) or on rmcp-internal task
    ///   panic.
    /// - **`signal_handle`** — completes when the SIGINT handler's future
    ///   resolves (the signal source closed, OR the second-Ctrl-C path
    ///   already called `process_exit` and we never reach here).
    ///
    /// The retry worker is intentionally *not* selected on — under normal
    /// operation it polls indefinitely until [`Self::shutdown_signal`]
    /// flips, which this method does after the select completes. A worker
    /// task exiting on its own is anomalous (panic), surfaced via
    /// [`Self::shutdown`]'s join-error logging.
    ///
    /// Consumes `self` by value to enforce single-call semantics at compile
    /// time (same rationale as [`Self::shutdown`]).
    ///
    /// # Errors
    ///
    /// Propagates [`Self::shutdown`]'s error surface. Currently
    /// [`Self::shutdown`] always returns `Ok(())`, so this is reserved for
    /// future shutdown-fallibility surfacing.
    pub async fn wait(mut self) -> VaultResult<()> {
        tokio::select! {
            _ = &mut self.server_handle => {
                // stdio EOF — client disconnected, or rmcp server task
                // returned. Graceful shutdown of remaining tasks below.
            }
            _ = &mut self.signal_handle => {
                // Signal handler resolved — typically the signal stream
                // broke (rare) or the second-Ctrl-C path called
                // `process_exit` and we never observed the resolution.
            }
        }
        self.shutdown().await
    }

    /// Graceful shutdown. Signals the worker to drain, aborts the server
    /// and signal tasks, awaits all `JoinHandle`s. Consumes `self` (see
    /// type-level docstring for why).
    ///
    /// # V0.1 known limitation
    ///
    /// MCP server shutdown aborts the running task rather than closing
    /// the stdio transport gracefully — in-flight tool calls are
    /// dropped. Closing stdin from inside the process is not directly
    /// supported by rmcp's stdio transport; a future-proof graceful-MCP
    /// shutdown would require either a transport-level close API or a
    /// supervisor pattern that closes stdio externally. Acceptable for
    /// V0.1 internal alpha (single-user, single-agent); revisit at V0.2
    /// multi-agent task if concrete consumer surfaces.
    pub async fn shutdown(self) -> VaultResult<()> {
        // 1. Signal the retry worker to wind down. It stops polling for new
        //    work and drains whatever is already queued (bounded by
        //    `retry_worker::DEFAULT_DRAIN_TIMEOUT`) before exiting, so a
        //    cascade enqueued moments before shutdown is written now rather
        //    than deferred to the next session. Step 4 awaits that drain.
        let _ = self.shutdown_signal.send(true);

        // 2. Abort the signal handler (it's blocked on Ctrl-C waiting).
        //    Aborting drops the future; the underlying ctrl_c handler
        //    is unregistered when the future is dropped.
        self.signal_handle.abort();

        // 3. Abort the MCP server task (see V0.1 known limitation above).
        self.server_handle.abort();

        // 3b. Stop the nightly consolidator scheduler. It honors the shutdown
        //     signal (its sleep is `select!`'d against `cancel.changed()`), so
        //     `send(true)` above already nudges it; abort covers the rare case
        //     where it's mid-run (a consolidation in flight is dropped — the
        //     idempotent passes self-heal next launch).
        if let Some(handle) = &self.consolidator_handle {
            handle.abort();
        }

        // 4. Await the worker — graceful drain. JoinError = panic;
        //    log but don't return an error (shutdown is best-effort
        //    cleanup; a panicked worker is a correctness bug surfaced
        //    elsewhere via tracing).
        if let Err(e) = self.worker_handle.await {
            if !e.is_cancelled() {
                tracing::error!(error = %e, "retry worker join error during shutdown");
            }
        }

        // 5. Await the aborted handles to confirm cleanup. JoinError on
        //    aborted tasks is expected (cancellation), so swallow.
        //
        //    `is_finished()` guard (2026-05-28, Codex dogfood): when reached
        //    via `wait()`, the `select!` already polled one of these handles
        //    to completion by `&mut` (stdio EOF completes `server_handle`).
        //    Re-awaiting an already-finished `JoinHandle` panics ("JoinHandle
        //    polled after completion"). Skip the await when the task is already
        //    finished; on the direct-`shutdown()` path the freshly-aborted
        //    handles are not yet finished, so they're awaited to confirm
        //    cancellation exactly as before.
        if !self.server_handle.is_finished() {
            let _ = self.server_handle.await;
        }
        if !self.signal_handle.is_finished() {
            let _ = self.signal_handle.await;
        }
        if let Some(handle) = self.consolidator_handle {
            if !handle.is_finished() {
                let _ = handle.await;
            }
        }

        Ok(())
    }
}

/// Signal handler task: first Ctrl-C → flip shutdown signal + stderr
/// announce; second Ctrl-C → forced exit per locked semantics
/// (`std::process::exit(130)` + stderr message).
///
/// # Locked semantics (T0.1.10 Phase 2a pre-declaration)
///
/// - Exit code 130 = 128 + SIGINT(2), the SIGINT-conventional shell
///   convention (bash, zsh) for "process killed by Ctrl-C." Tools
///   monitoring exit codes (CI systems, supervisors) can distinguish
///   "graceful shutdown didn't complete in time" from a clean exit (0)
///   or a panic (101).
/// - Stderr messages document why exit happened. NOT logged via
///   `tracing` because the tracing subsystem may itself be torn down by
///   the time the second SIGINT fires; raw `eprintln!` is the
///   most-reliable signal.
///
/// # Cross-platform support
///
/// `tokio::signal::ctrl_c` works on **both Unix and Windows** under the
/// `tokio` `signal` feature, which is enabled via the workspace `tokio`
/// dep's `["full"]` feature set (`Cargo.toml` line 40). Verified
/// 2026-05-04 directly against `docs.rs/tokio/1.52.1/tokio/signal/fn.ctrl_c.html`,
/// which states verbatim: *"While signals are handled very differently
/// between Unix and Windows, both platforms support receiving a signal
/// on 'ctrl-c'. This function provides a portable API for receiving this
/// notification."* No `cfg(unix)` / `cfg(windows)` gating needed.
async fn handle_signals(
    shutdown_signal: tokio::sync::watch::Sender<bool>,
    exit: Arc<dyn ProcessExit>,
    signals: Arc<dyn SignalSource>,
) {
    // First Ctrl-C — graceful shutdown request.
    if signals.next_signal().await.is_err() {
        // Signal stream broken (rare; signal handler couldn't install
        // on this platform). Exit silently — process will rely on
        // explicit `ApplicationHandle::shutdown` for cleanup.
        return;
    }
    eprintln!(
        "[vault-app] graceful shutdown requested (SIGINT received); awaiting in-flight cascade drain. \
         Press Ctrl-C again to force exit."
    );
    let _ = shutdown_signal.send(true);

    // Second Ctrl-C — forced exit because graceful shutdown didn't
    // complete fast enough (or the user is in a hurry).
    if signals.next_signal().await.is_err() {
        return;
    }
    eprintln!(
        "[vault-app] forced exit triggered (second SIGINT received before graceful shutdown completed). \
         Exit code 130 (128 + SIGINT)."
    );
    exit.exit(130);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_exit::CapturingProcessExit;
    use crate::signal_source::MockSignalSource;

    // =========================================================================
    // RerankerState wire contract (ADR-090)
    //
    // The desktop frontend branches on these exact strings to decide whether to
    // show a "getting ready" state at all. A rename here with no matching
    // frontend change leaves the UI waiting forever on a model that is already
    // serving — silent, and only visible by running the app. Pinned here and
    // cross-checked in vault-tauri's frontend_contract.rs.
    // =========================================================================

    #[test]
    fn reranker_state_wire_strings_are_stable() {
        assert_eq!(RerankerState::NotConfigured.as_wire_str(), "not_configured");
        assert_eq!(RerankerState::Unavailable.as_wire_str(), "unavailable");
        assert_eq!(RerankerState::Preparing.as_wire_str(), "preparing");
        assert_eq!(RerankerState::Ready.as_wire_str(), "ready");
    }

    #[test]
    fn reranker_state_wire_strings_are_distinct() {
        // Two states collapsing to one string would make "pending download"
        // and "loading" indistinguishable to the UI — the exact confusion
        // RerankerState exists to remove.
        let all = [
            RerankerState::NotConfigured,
            RerankerState::Unavailable,
            RerankerState::Preparing,
            RerankerState::Ready,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for s in all {
            assert!(
                seen.insert(s.as_wire_str()),
                "duplicate wire string for {s:?}"
            );
        }
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn reranker_state_wire_strings_name_no_stack_component_per_adr_086() {
        for s in [
            RerankerState::NotConfigured,
            RerankerState::Unavailable,
            RerankerState::Preparing,
            RerankerState::Ready,
        ] {
            let w = s.as_wire_str();
            for forbidden in ["qwen", "onnx", "bge", "phi", "gguf", "rerank", "llama"] {
                assert!(
                    !w.contains(forbidden),
                    "ADR-086: wire string must not name the stack ('{forbidden}'); got {w}"
                );
            }
        }
    }

    // =========================================================================
    // Consolidator safety wrapper — timeout helper unit tests (T0.3.x Batch A)
    //
    // These tests pin `timeout_or_consolidator_timeout`'s contract independently
    // of the full `Application::run_consolidation_with_safety` path so we can
    // exercise the timeout behaviour with sub-second budgets (the production
    // const is 30 min — untestable end-to-end). The lockfile contract is pinned
    // in `consolidator_lock::tests`. End-to-end wiring is exercised at Batch A
    // Commit 2 (vault-cli consolidate run subcommand) where a real Application
    // is constructed against a tempdir backend.
    // =========================================================================

    #[tokio::test]
    async fn timeout_or_returns_inner_value_when_inner_completes_before_budget() {
        let fast_inner = async { Ok::<u32, VaultError>(42) };
        let result = timeout_or_consolidator_timeout(Duration::from_secs(60), fast_inner).await;
        assert_eq!(
            result.unwrap(),
            42,
            "inner future completing within budget MUST return its value verbatim"
        );
    }

    #[tokio::test]
    async fn timeout_or_returns_consolidator_timeout_when_inner_exceeds_budget() {
        let slow_inner = async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok::<(), VaultError>(())
        };
        let result = timeout_or_consolidator_timeout(Duration::from_millis(50), slow_inner).await;
        match result {
            Err(VaultError::ConsolidatorTimeout(secs)) => {
                // 50ms rounds to 0 seconds under `as_secs()`. The point of the
                // assertion is the variant + that the value is what we passed
                // in, not the exact ms-vs-secs precision.
                assert_eq!(
                    secs, 0,
                    "ConsolidatorTimeout payload MUST be the budget's as_secs() value"
                );
            }
            other => panic!("expected VaultError::ConsolidatorTimeout, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn timeout_or_propagates_inner_error_verbatim_when_inner_errs_before_timeout() {
        let inner = async { Err::<u32, _>(VaultError::Storage("simulated".into())) };
        let result = timeout_or_consolidator_timeout(Duration::from_secs(60), inner).await;
        match result {
            Err(VaultError::Storage(msg)) => assert_eq!(
                msg, "simulated",
                "inner error MUST propagate verbatim when it fires before timeout"
            ),
            other => panic!("expected VaultError::Storage, got: {other:?}"),
        }
    }

    #[test]
    fn parse_timeout_override_none_for_absent_value() {
        assert_eq!(
            parse_timeout_override(None),
            None,
            "an unset override MUST fall back to the default"
        );
    }

    #[test]
    fn parse_timeout_override_none_for_empty_or_garbage() {
        assert_eq!(parse_timeout_override(Some("")), None);
        assert_eq!(parse_timeout_override(Some("   ")), None);
        assert_eq!(parse_timeout_override(Some("not-a-number")), None);
        assert_eq!(parse_timeout_override(Some("-5")), None);
        assert_eq!(parse_timeout_override(Some("3.5")), None);
    }

    #[test]
    fn parse_timeout_override_parses_positive_seconds_and_trims() {
        assert_eq!(
            parse_timeout_override(Some("900")),
            Some(Duration::from_secs(900))
        );
        assert_eq!(
            parse_timeout_override(Some("  7200  ")),
            Some(Duration::from_secs(7200)),
            "surrounding whitespace MUST be tolerated"
        );
    }

    #[test]
    fn parse_timeout_override_zero_is_no_limit_sentinel() {
        assert_eq!(
            parse_timeout_override(Some("0")),
            Some(Duration::MAX),
            "the `0` sentinel MUST map to an effectively-unbounded budget so a \
             one-time full-sweep backfill completes instead of being killed"
        );
    }

    // =========================================================================
    // Lifecycle test 1 (v2 test 10) — `start_with_mcp` McpBindFailed path
    //
    // PHASE 4B SCOPE DEFERRAL per Shahbaz approval at v2-greenlit-step-expansion
    // review (2026-05-05): rmcp's `ServiceExt::serve` transport-error mock would
    // require either (a) implementing a mock transport against rmcp's Layer/
    // Service trait surface (research-spike scope, not implementation), or
    // (b) closing stdin to force serve failure (test-environment-fragile across
    // CI runners), or (c) refactoring `start_with_mcp` to take a transport-
    // builder closure (contract-establishing scope, out of bounds for
    // consume-existing-contracts depth). Per `feedback_forward_compat_concrete_vs_hypothetical.md`,
    // V0.2 alpha-distribution task IS the named concrete consumer where
    // transport-hardening scope is touched; deferral preserves intent without
    // paying the implementation cost now.
    // =========================================================================

    /// **Phase 4b ignored placeholder.** Pin McpBindFailed wiring at
    /// V0.2 alpha-distribution task time when transport-mock infra
    /// is appropriately scoped. See module-level deferral note above.
    #[tokio::test]
    #[ignore = "Phase 4b deferred — needs rmcp transport mock; lands at V0.2 alpha-distribution task"]
    async fn start_with_mcp_returns_mcp_bind_failed_when_serve_errs() {
        unimplemented!(
            "Phase 4b ignored placeholder — V0.2 alpha-distribution task lands the rmcp \
             transport mock. Per ADR-024 + ADR-026 cross-link: McpBindFailed surfaces \
             from rmcp::ServiceExt::serve setup errors; testing requires a swappable \
             transport at start_with_mcp boundary."
        );
    }

    // =========================================================================
    // Lifecycle test 2 (v2 test 11) — `ApplicationHandle::shutdown` drain
    // =========================================================================

    /// Verifies `ApplicationHandle::shutdown` cleanly awaits all three
    /// task handles + sends the shutdown signal. Uses test-only
    /// `for_test` constructor so the test doesn't need a full
    /// Application (SqlCipher + LanceDB + DuckDB + ORT — heavy).
    ///
    /// Mock handles are `tokio::spawn(async { ... })` futures that
    /// observe the shutdown signal and exit cleanly when received,
    /// mirroring the production worker / server / signal-handler
    /// behaviour at the JoinHandle level.
    #[tokio::test]
    async fn application_handle_shutdown_drains_worker() {
        let (shutdown_signal, mut rx) = tokio::sync::watch::channel(false);

        // Mock worker: spawned task that waits for shutdown signal,
        // then exits. Mirrors production RetryWorker::run shape.
        let mut rx_worker = rx.clone();
        let worker_handle = tokio::spawn(async move {
            // Wait for the first true signal.
            while !*rx_worker.borrow_and_update() {
                if rx_worker.changed().await.is_err() {
                    break;
                }
            }
        });

        // Mock server + signal handlers: trivial spawned tasks. In
        // production these are aborted by `shutdown` rather than
        // awaiting cleanly; we use simple pending tasks here so
        // `shutdown`'s abort+await sequence has something to abort.
        let server_handle = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        let signal_handle = tokio::spawn(async {
            std::future::pending::<()>().await;
        });

        let handle = ApplicationHandle::for_test(
            shutdown_signal,
            worker_handle,
            server_handle,
            signal_handle,
        );

        // Snapshot the shutdown_signal state pre-shutdown.
        rx.mark_unchanged();
        let pre_state = *rx.borrow();
        assert!(
            !pre_state,
            "Pre-shutdown the channel must be `false`; got {pre_state}"
        );

        // Bound the test wait — if shutdown hangs, fail the test
        // rather than hanging the test runner.
        let shutdown_result =
            tokio::time::timeout(std::time::Duration::from_secs(5), handle.shutdown()).await;

        assert!(
            shutdown_result.is_ok(),
            "ApplicationHandle::shutdown MUST complete within 5s for the \
             happy-path mock-worker scenario; timed out (potential drain \
             regression — worker_handle.await may have hung)."
        );
        assert!(
            shutdown_result.unwrap().is_ok(),
            "ApplicationHandle::shutdown's inner Result MUST be Ok for the \
             clean-exit mock-worker path."
        );

        // Verify the shutdown signal was sent.
        let post_state = *rx.borrow();
        assert!(
            post_state,
            "ApplicationHandle::shutdown MUST have sent `true` over \
             shutdown_signal so the worker observed the drain request; \
             post-shutdown channel state is `false` (regression)."
        );
    }

    /// Regression (Codex dogfood 2026-05-28): `wait()`'s `select!` drives
    /// `server_handle` to completion on stdio EOF; the subsequent
    /// `shutdown()` MUST NOT re-await that already-completed handle — doing so
    /// panics with "JoinHandle polled after completion". Pins the
    /// `is_finished()` guard in `shutdown()`. Pre-fix this test panics.
    #[tokio::test]
    async fn wait_does_not_panic_when_server_handle_completes_first() {
        // `_rx` kept alive so `shutdown_signal.send` has a live receiver.
        let (shutdown_signal, _rx) = tokio::sync::watch::channel(false);

        // worker exits immediately (drain trivially complete).
        let worker_handle = tokio::spawn(async {});
        // server_handle completes immediately == stdio EOF: wait()'s select!
        // drives it to completion via `&mut`.
        let server_handle = tokio::spawn(async {});
        // signal_handle stays pending (the SIGINT path never fires here).
        let signal_handle = tokio::spawn(async {
            std::future::pending::<()>().await;
        });

        let handle = ApplicationHandle::for_test(
            shutdown_signal,
            worker_handle,
            server_handle,
            signal_handle,
        );

        // wait() → select! fires on the completed server_handle → shutdown().
        // MUST return Ok within the bound, never panic on a re-awaited handle.
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle.wait()).await;
        assert!(
            result.is_ok(),
            "wait() MUST complete (not hang) after server_handle EOF"
        );
        assert!(
            result.unwrap().is_ok(),
            "wait() MUST return Ok after graceful shutdown, not panic re-awaiting \
             the already-completed server_handle"
        );
    }

    // =========================================================================
    // Lifecycle test 3 (v2 test 12) — `handle_signals` double-Ctrl-C path
    // =========================================================================

    /// Pin both Ctrl-C paths in one consolidated test (per v2 test
    /// consolidation per Shahbaz greenlight): first Ctrl-C → shutdown
    /// signal sent; second Ctrl-C → ProcessExit::exit(130) called.
    ///
    /// Uses MockSignalSource + CapturingProcessExit to drive the
    /// handler without OS signals. CapturingProcessExit panics inside
    /// the spawned task on the second Ctrl-C; JoinHandle::await
    /// returns Err(JoinError::panic) which is the expected shape.
    #[tokio::test]
    async fn handle_signals_first_ctrl_c_signals_shutdown_then_second_ctrl_c_force_exits_with_130()
    {
        let (shutdown_signal, mut rx) = tokio::sync::watch::channel(false);
        let exit = CapturingProcessExit::new();
        let captured_handle = exit.captured_handle();

        // Pre-load the queue with two Ok(()) events — first triggers
        // shutdown signal; second triggers force exit.
        let signals: Arc<dyn SignalSource> =
            Arc::new(MockSignalSource::with_queue(vec![Ok(()), Ok(())]));
        let exit_arc: Arc<dyn ProcessExit> = Arc::new(exit);

        // Spawn handle_signals as the production callsite would. The
        // task panics when CapturingProcessExit::exit fires (second
        // Ctrl-C path); the panic is the expected shape.
        let handle = tokio::spawn(handle_signals(shutdown_signal, exit_arc, signals));

        // Wait for the task to complete (panic). Bound the wait so a
        // hung handle_signals fails the test rather than hanging.
        let join_result = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("handle_signals MUST complete within 5s; timeout indicates hang");

        // The spawned task panicked (expected — CapturingProcessExit::exit
        // panics by design to convert the divergence into a JoinError).
        assert!(
            join_result.is_err(),
            "handle_signals task MUST have panicked from CapturingProcessExit::exit \
             on the second Ctrl-C path; instead got Ok(()) — force-exit-130 \
             regression."
        );
        let join_err = join_result.unwrap_err();
        assert!(
            join_err.is_panic(),
            "JoinError MUST be a panic (CapturingProcessExit::exit panics by design); \
             got: {join_err:?}"
        );

        // First Ctrl-C: shutdown_signal received `true`.
        rx.mark_unchanged();
        let signal_state = *rx.borrow();
        assert!(
            signal_state,
            "ADR-locked first-Ctrl-C path: handle_signals MUST send `true` over \
             shutdown_signal after the first signal event; got `false` (regression)."
        );

        // Second Ctrl-C: ProcessExit::exit(130) was called.
        let captured = *captured_handle.lock().expect("CapturingProcessExit mutex");
        assert_eq!(
            captured,
            Some(130),
            "ADR-locked second-Ctrl-C-130 path (T0.1.10 Phase 2a): handle_signals MUST \
             call ProcessExit::exit(130) after the second signal event; \
             CapturingProcessExit captured {captured:?} instead. Per the operational \
             contract, exit code 130 (128 + SIGINT) distinguishes user-requested forced \
             exit from general failure (1) — wrapper scripts and CI rely on this code."
        );
    }
}
