//! Structured read-time pipeline — Commit 6 of the locked-next-arc
//! (architectural lock 2026-05-26: LLM out of the read path).
//!
//! Replaces the V0.2-era [`crate::read_pipeline::ReadPipeline`] (Qwen-7B
//! single-call synthesis, mean 86s on Vulkan iGPU) with a deterministic
//! filter + pack stage. Read latency target: ~500ms total (retrieval cost
//! dominates).
//!
//! ## Three-player model
//!
//! - The **calling agent** (Claude / GPT / Codex / Kimi via MCP) composes
//!   the user-facing answer in its own voice. The vault never speaks to
//!   the user directly.
//! - **Phi-4-mini** stays in `vault-consolidator` for nightly merge
//!   classification + topic naming. Its REPORT artifact is what this
//!   pipeline enriches retrieved candidates from.
//! - **No LLM in this module.** The pipeline is pure code: retrieve →
//!   filter → enrich-with-REPORT-topics → emit structured facts +
//!   health signals.
//!
//! ## Two-stage flow
//!
//! 1. **Stage 1 — Retrieval.** Hand the query to the existing
//!    [`crate::Retriever`] (production: BGE-small dense, Tantivy BM25,
//!    RRF fusion, abstain gate). Returns top-N
//!    [`crate::RetrievedMemory`]s already filtered by
//!    `authorized_boundaries`.
//!
//! 2. **Stage 2 — Structured-fact assembly.** Load the per-boundary
//!    REPORT artifact (via [`crate::ReportLoader`]). For each
//!    retrieved memory, look up its topic via the REPORT's
//!    `facts_by_topic` (O(1) after one-pass invert). Build the
//!    [`StructuredReadResponse`] with `relevant_facts` + `abstain` +
//!    `health` warnings.
//!
//! ## Output contract (ADR-054)
//!
//! The MCP tool returns a JSON object with these fields:
//!
//! ```text
//! {
//!   "boundary": "personal" | null,        // null for multi-boundary reads
//!   "query": "<echo of trimmed query>",
//!   "relevant_facts": [
//!     { "fact": "...", "topic": "<label>"|null, "memory_id": "<uuid>",
//!       "as_of": "...", "confidence": 0.95, "source_agent": "<agent>"|null }
//!   ],
//!   "abstain": false,
//!   "health": {
//!     "status": "ok"|"degraded"|"critical",
//!     "warnings": [
//!       { "code": "REPORT_STALE_WARN", "severity": "warn",
//!         "detail": "...", "recovery_hint": "..." }
//!     ]
//!   }
//! }
//! ```
//!
//! The warning codes ([`WarningCode`]) are locked by ADR-054 Contract 2;
//! any future addition requires a Contract amendment. The two
//! `SUBSCRIPTION_*` codes (Amendment 3) are added by the MCP layer, never by
//! this pipeline, and never change `status`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;

use vault_core::{Boundary, MemoryId, VaultError, VaultResult};
use vault_embedding::RerankProvider;

use crate::report_io::{LoadedReport, ReportLoader};
use crate::reranked_retriever::relevance_score;
use crate::retriever::{RetrievalOptions, RetrievalQuery, RetrievedMemory, Retriever};
use crate::search_hint::search_hint;

// =============================================================================
// Constants — locked by ADR-054 (Commit 6)
// =============================================================================

/// Default top-N retrieved candidates handed to the filter+pack stage.
/// Matches the V0.2-era `read_pipeline::DEFAULT_MAX_CANDIDATES` for
/// continuity with the t026 8-query gauntlet anchoring.
pub const DEFAULT_MAX_CANDIDATES: usize = 20;

/// Number of top semantic hits the relevance gate averages (ADR-057,
/// 2026-05-28). **Top-1** (K=1): the single best semantic match. We started at
/// top-3 mean (wider gap on the 100-memory fixture: 0.070 vs 0.054), but live
/// dogfood proved top-3 mean DILUTES a single strong match with weak fillers on
/// a sparse vault — it over-abstained on a real query whose answer was present
/// (the "zafflang" case: search found it, the read hid it). top-1 cannot be
/// diluted; for a memory vault, recall > precision (hiding a real memory is the
/// worst failure). See ADR-057 over-abstain amendment.
const RELEVANCE_GATE_TOP_K: usize = 1;

/// Number of top retrieved candidates handed to the cross-encoder reranker.
/// **Rerank the FULL retrieved pool** (= [`DEFAULT_MAX_CANDIDATES`]), not a
/// smaller slice.
///
/// History: ADR-057 amendment (2026-05-29) capped this at **8** as a latency
/// compromise. The §7 live dogfood (2026-06-01) proved that wrong on a
/// populated vault: with 13 facts seeded, a subject-less hobby fact
/// ("Plays the cello…") was retrieved by BGE at a rank BELOW 8 for a
/// loosely-phrased query ("what does the user do for fun?"), so it was
/// truncated away BEFORE the reranker could score it — and the read abstained
/// on a fact the vault held (Bug-2 / ADR-064 recurrence). The cap re-introduced
/// exactly the BGE-ranking weakness ([[bge-small-cannot-separate-relevant]])
/// that the reranker exists to correct. The reranker is the relevance
/// authority; it must see every retrieved candidate, not BGE's top-8. Cost:
/// ~0.39s/candidate CPU → ≤8s worst case at 20 (correctness-before-latency;
/// GPU/int8 is the latency fast-follow).
///
/// Scale finding (ADR-069, 2026-06-04): the rerank pool is no longer just the
/// hybrid's top-N. At 100 facts the hybrid's RRF fusion *starved* a strong
/// pure-semantic match (a subject-less fact sharing no query keywords ranked
/// pure-BGE #6/100 but hybrid >20/100, because every "The user …" fact earned a
/// second RRF term from the incidental "user" keyword overlap). So `read` now
/// unions the semantic channel's top-[`DEFAULT_MAX_CANDIDATES`] onto the hybrid
/// hits before reranking (see [`StructuredReadPipeline::union_semantic_recall`]).
/// The pool is therefore bounded by hybrid([`DEFAULT_MAX_CANDIDATES`]) ∪
/// semantic([`DEFAULT_MAX_CANDIDATES`]) = at most `2 × DEFAULT_MAX_CANDIDATES`
/// unique; this cap sizes the reranker batch to that union so a unioned-in
/// semantic match is never truncated away before the reranker (the relevance
/// authority) scores it.
const RERANK_CANDIDATE_CAP: usize = 2 * DEFAULT_MAX_CANDIDATES;

/// Minimum top-1 BGE cosine for a query to count as having relevant content.
/// Below this the read abstains (no-signal). Calibrated 2026-05-28
/// (`abstain_channel_diagnostic` top-1 column, n=5 no-signal probes): no-signal
/// top-1 ≤ 0.642, the four must-proceed contradictions ≥ 0.696 → 0.66 sits in
/// that gap (slight recall bias toward proceeding). ADR-057. V0.2 closes
/// no-signal abstention ONLY; topical-noise (the Q21 class, top-1 0.717 — above
/// two contradictions) is structurally unseparable by a cosine floor and is
/// deferred to a non-LLM cross-encoder reranker at V1.0+.
const RELEVANCE_COSINE_FLOOR: f32 = 0.66;

/// No-signal floor on the rank-1 reranked relevance score (`[0,1]` sigmoid),
/// for the `abstain` HINT only (ADR-073, 2026-06-08). A top below this marks
/// the read `weak_match`/`abstain` — it catches the lone/few no-signal fact that
/// `search_hint`'s separation test reads as "separated" (a single candidate, or
/// the cat→dog / Lisbon-guard class: one adjacent-but-wrong fact). Combined with
/// `search_hint`'s separation test (which catches flat distractor clusters like
/// the salary trap), the two cover both no-signal shapes. **This floor governs
/// only the hint — it NEVER drops a fact** (reorder-only `apply_reranker` returns
/// every candidate), so a mis-placed floor can never hide a real answer; recall
/// is safe by construction. Calibrated from the 1k live dogfood (2026-06-08):
/// lowest real answer relevance 0.0388 (stay-fit), highest no-signal 0.004
/// (Lisbon-guard) — 0.01 sits in that ~10× gap. See [[project_1k_live_read_false_abstain]].
const READ_NO_SIGNAL_FLOOR: f32 = 0.01;

/// Staleness tier thresholds. Age = `now() - generated_at`.
///
/// - `0 ≤ age < INFO`: status `ok` (no staleness warning).
/// - `INFO ≤ age < WARN` (24h ≤ age < 72h): `REPORT_STALE_INFO`, severity Info.
/// - `WARN ≤ age < CRITICAL` (72h ≤ age < 7d): `REPORT_STALE_WARN`, severity Warn.
/// - `CRITICAL ≤ age` (7d ≤ age): `REPORT_STALE_CRITICAL`, severity Critical.
pub const STALE_INFO_THRESHOLD: Duration = Duration::from_secs(24 * 60 * 60);
/// See [`STALE_INFO_THRESHOLD`].
pub const STALE_WARN_THRESHOLD: Duration = Duration::from_secs(72 * 60 * 60);
/// See [`STALE_INFO_THRESHOLD`].
pub const STALE_CRITICAL_THRESHOLD: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Highest REPORT `schema_version` this read pipeline understands. A
/// REPORT with a higher version is treated as missing — the consumer
/// cannot safely interpret unknown future fields, so the pipeline
/// surfaces `REPORT_MISSING` rather than acting on partial data.
pub const SUPPORTED_REPORT_SCHEMA_VERSION: u32 = 1;

// =============================================================================
// Input
// =============================================================================

/// User-facing read query. Mirrors the V0.2-era `ReadQuery` shape so the
/// MCP `tool_read` handler doesn't need to migrate its construction site.
#[derive(Debug, Clone)]
pub struct ReadQuery {
    /// Raw user / agent question text. Trimmed + validated when [`StructuredReadPipeline::read`] runs.
    pub query_text: String,
    /// Boundaries the caller is authorised to read from. Empty slice
    /// short-circuits to `abstain=true` per BRD §11.4.3 — never an
    /// authorisation error.
    pub authorized_boundaries: Vec<Boundary>,
}

// =============================================================================
// Output — locked by ADR-054 Contract 2
// =============================================================================

/// The structured read-pipeline response the MCP `memory_read` tool
/// surfaces to the calling agent.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StructuredReadResponse {
    /// The single boundary in scope when `authorized_boundaries.len() == 1`.
    /// `None` for multi-boundary reads — facts from multiple boundaries
    /// may be intermixed in `relevant_facts`.
    pub boundary: Option<String>,
    /// Echo of `query.query_text` post-trim. Aids agent-side diagnosis
    /// and audit trails.
    pub query: String,
    /// Ordered facts most relevant to the query (reranker-relevance DESC).
    /// **Populated even when `abstain=true`** (ADR-073) — the closest candidates
    /// are always returned so the agent can judge / be helpful; empty ONLY when
    /// retrieval genuinely returned nothing or zero boundaries were authorised.
    pub relevant_facts: Vec<RelevantFact>,
    /// `true` when the vault holds no *confident* match for the query — read's
    /// recall-safe weak-match signal (ADR-073). `abstain=true` no longer means
    /// "no facts": `relevant_facts` may still carry the closest low-confidence
    /// candidates (see `top_relevance`). A weak agent can trust `abstain` and say
    /// "I don't have that"; a capable agent can inspect the facts and decide
    /// (e.g. "no cat, but you have a dog"). Fabricating an answer the facts don't
    /// support is still a contract violation. Set when retrieval was empty, zero
    /// boundaries were authorised, or the rank-1 relevance is below the no-signal
    /// floor / not separated from the pool.
    pub abstain: bool,
    /// Rank-1 reranked relevance on the `[0,1]` sigmoid scale (`0.0` when no
    /// candidates). Agent-facing transparency into match strength — mirrors
    /// `memory_search`'s `top_relevance` hint (ADR-073). On the no-reranker
    /// cosine fallback this is a BGE cosine rather than a reranker score
    /// (best-effort; the reranked path is the production default).
    pub top_relevance: f32,
    /// Health of the vault state behind this response.
    pub health: HealthInfo,
}

/// One structured fact in the response. Field names match the
/// `vault_consolidator::report::ReportFact` shape so REPORT topics flow
/// through to the agent without translation.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RelevantFact {
    pub fact: String,
    /// Consolidator-assigned topic label. `None` when:
    /// - the memory was written since the last consolidation run, or
    /// - no REPORT exists for the boundary, or
    /// - `topic_names_unavailable` on the loaded REPORT (placeholder labels).
    pub topic: Option<String>,
    /// UUID string. Agents can resolve back to a typed `MemoryId` if
    /// needed for follow-up MCP calls (e.g. `memory_delete`).
    pub memory_id: String,
    /// Fact-time anchor — when this fact became true in the world
    /// (`Memory::valid_from`). NOT when the memory row was added.
    pub as_of: DateTime<Utc>,
    pub confidence: f32,
    pub source_agent: Option<String>,
}

/// Aggregate health of the response, plus per-warning detail.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HealthInfo {
    pub status: HealthStatus,
    /// Empty when `status == HealthStatus::Ok`. Ordered by emission
    /// order (boundary order × per-boundary code emission order).
    pub warnings: Vec<HealthWarning>,
}

/// Aggregate severity of the response. Rule:
/// - Any [`WarningSeverity::Critical`] warning → [`HealthStatus::Critical`].
/// - Else any [`WarningSeverity::Warn`] or `Info` → [`HealthStatus::Degraded`].
/// - Else no warnings → [`HealthStatus::Ok`].
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Ok,
    Degraded,
    Critical,
}

/// One health warning. Surfaces to the calling agent via the
/// `health.warnings` array.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HealthWarning {
    pub code: WarningCode,
    pub severity: WarningSeverity,
    /// Human-readable detail the agent can mention to the user when
    /// relevant. Bounded length; never includes vault contents or
    /// memory IDs.
    pub detail: String,
    /// Action the user can take to clear the warning. Bounded length;
    /// e.g. "Run the consolidator to refresh the REPORT".
    pub recovery_hint: String,
}

/// The warning codes locked by ADR-054 Contract 2 (2026-05-26): six from the
/// pipeline (Amendment 2, 2026-05-27, retired `DELTA_LOG_UNAVAILABLE`), and
/// two account codes the MCP layer adds (Amendment 3, 2026-09-21,
/// `SIGNIN-DESIGN.md` §8.40). Adding another requires a Contract amendment.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WarningCode {
    /// No REPORT artifact exists for the boundary in scope. Most
    /// common cause: nightly consolidator hasn't run yet on a fresh
    /// vault.
    ReportMissing,
    /// REPORT age in the 24-72h band.
    ReportStaleInfo,
    /// REPORT age in the 72h-7d band.
    ReportStaleWarn,
    /// REPORT age ≥ 7d. Vault state has drifted; consolidator hasn't
    /// run in a week.
    ReportStaleCritical,
    /// REPORT's topic labels are placeholder `"topic_<id>"` strings.
    /// Driven by the `topic_names_unavailable: true` flag in the loaded
    /// REPORT (Phi-4-mini was unavailable at consolidation time).
    TopicNamesUnavailable,
    /// REPORT `generated_at` is in the future relative to the read-time
    /// clock. Indicates clock drift; staleness math becomes unreliable.
    ClockSkewDetected,
    /// The user's free trial ends within five days (ADR-054 Contract 2,
    /// amendment 3; `SIGNIN-DESIGN.md` §8.40). **Never emitted by this
    /// pipeline**: the MCP layer adds it from the account, after `status` is
    /// computed, and it never changes `status` — it is about the account, not
    /// the vault's data.
    SubscriptionTrialEnding,
    /// The user's last payment did not go through; the vault is still open.
    /// Same rules as [`Self::SubscriptionTrialEnding`].
    SubscriptionPaymentFailed,
}

/// Three-level severity scale. Drives the [`HealthStatus`] aggregation.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WarningSeverity {
    Info,
    Warn,
    Critical,
}

// =============================================================================
// Clock abstraction — testable time without `tokio::time::pause()` brittleness
// =============================================================================

/// Wall-clock provider. Production uses [`SystemClock`]; tests inject
/// a fixed-point implementation so staleness-tier assertions are
/// deterministic.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Production [`Clock`] impl — delegates to `chrono::Utc::now()`.
#[derive(Debug, Clone, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

// =============================================================================
// Pipeline
// =============================================================================

/// Production read-time pipeline. Pair an `Arc<dyn Retriever>` (V0.2
/// production: `AbstainingRetriever` wrapping the BGE + Tantivy + RRF
/// stack) with an `Arc<dyn ReportLoader>` (V0.2 production:
/// [`crate::FilesystemReportLoader`]) at construction; call
/// [`Self::read`] per agent query.
///
/// Concrete struct (NOT a trait surface) per
/// [[forward-compat-concrete-vs-hypothetical]] — promote to a trait when
/// V0.3 cloud-tier becomes the imminent next task and a second concrete
/// implementation surfaces.
#[derive(Clone)]
pub struct StructuredReadPipeline {
    retriever: Arc<dyn Retriever>,
    /// Optional semantic-only channel. Serves two roles depending on wiring:
    /// - **No reranker (cosine fallback, ADR-057):** the relevance gate — `read`
    ///   abstains if the top-K-mean BGE cosine is below [`RELEVANCE_COSINE_FLOOR`].
    /// - **Reranker wired (production, ADR-069):** the recall-union source —
    ///   [`Self::union_semantic_recall`] widens the rerank pool with this
    ///   channel's top hits so an RRF-starved pure-semantic match still reaches
    ///   the reranker. (The cosine gate itself is bypassed when a reranker owns
    ///   abstention.)
    ///
    /// Wired in production via [`Self::with_relevance_gate`]; `None` (the `new`
    /// default) leaves both behaviours off so unit tests exercising other
    /// contracts are unaffected.
    semantic: Option<Arc<dyn Retriever>>,
    /// Optional cross-encoder reranker (ADR-057 amendment, 2026-05-29). When
    /// `Some`, the top [`RERANK_CANDIDATE_CAP`] retrieved candidates are
    /// re-scored, filtered to those at/above the reranker's relevance floor,
    /// and re-sorted by reranker score — the relevance gate that SUPERSEDES the
    /// cosine `semantic` floor (which couldn't separate topically-adjacent
    /// wrong-attribute facts). When wired, the reranker is the relevance gate
    /// and the cosine `semantic` probe is not consulted. `None` (the `new`
    /// default) leaves the prior behaviour for tests + the no-reranker fallback.
    reranker: Option<Arc<dyn RerankProvider>>,
    report_loader: Arc<dyn ReportLoader>,
    clock: Arc<dyn Clock>,
    max_candidates: usize,
}

impl StructuredReadPipeline {
    /// Construct with default [`DEFAULT_MAX_CANDIDATES`] and a production
    /// [`SystemClock`].
    #[must_use]
    pub fn new(retriever: Arc<dyn Retriever>, report_loader: Arc<dyn ReportLoader>) -> Self {
        Self {
            retriever,
            semantic: None,
            reranker: None,
            report_loader,
            clock: Arc::new(SystemClock),
            max_candidates: DEFAULT_MAX_CANDIDATES,
        }
    }

    /// Enable the cross-encoder reranker relevance gate (ADR-057 amendment).
    /// When wired, [`Self::read`] reranks the top [`RERANK_CANDIDATE_CAP`]
    /// retrieved candidates, keeps those scoring at/above the reranker's
    /// [`RerankProvider::relevance_floor`], and re-sorts by reranker score.
    /// This is the production relevance gate; it supersedes (and bypasses) the
    /// cosine `semantic` floor from [`Self::with_relevance_gate`].
    #[must_use]
    pub fn with_reranker(mut self, reranker: Arc<dyn RerankProvider>) -> Self {
        self.reranker = Some(reranker);
        self
    }

    /// Enable the relevance gate (ADR-057) with a semantic-only probe channel
    /// (production: the `SemanticRetriever` backing the hybrid's dense leg).
    /// When wired, [`Self::read`] abstains on a query whose top-K-mean BGE
    /// cosine is below [`RELEVANCE_COSINE_FLOOR`] — closing the no-signal
    /// ship-gate. Mirrors the [`Self::with_clock`] /
    /// [`Self::with_max_candidates`] builder style.
    #[must_use]
    pub fn with_relevance_gate(mut self, semantic: Arc<dyn Retriever>) -> Self {
        self.semantic = Some(semantic);
        self
    }

    /// Override the wall-clock provider. Tests inject a fixed-point
    /// clock to assert staleness-tier boundaries deterministically.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Override the top-N retrieval cap. Defaults to
    /// [`DEFAULT_MAX_CANDIDATES`]; clamped by the underlying retriever
    /// to `[1, crate::retriever::MAX_RESULTS_CAP]`.
    #[must_use]
    pub fn with_max_candidates(mut self, n: usize) -> Self {
        self.max_candidates = n;
        self
    }

    /// Run the two-stage structured read.
    ///
    /// # Errors
    ///
    /// - [`VaultError::InvalidInput`] — `query_text` is empty after trim.
    /// - Any [`VaultError`] surfaced by the retriever stage.
    /// - [`VaultError::Io`] / [`VaultError::Serde`] from the
    ///   REPORT loader on malformed-JSON failures (file-missing
    ///   becomes the `REPORT_MISSING` warning, not an error).
    #[tracing::instrument(
        skip_all,
        fields(
            query_len = query.query_text.len(),
            boundary_count = query.authorized_boundaries.len()
        )
    )]
    pub async fn read(&self, query: ReadQuery) -> VaultResult<StructuredReadResponse> {
        // Validate query text — empty after trim is the only InvalidInput
        // surface; oversize / control chars get caught by the retriever.
        let trimmed = query.query_text.trim();
        if trimmed.is_empty() {
            return Err(VaultError::InvalidInput(
                "structured read pipeline: query_text is empty after trim".into(),
            ));
        }
        let query_echo = trimmed.to_string();

        // boundary field — Some(name) for single-boundary, None for multi
        // or zero. Zero-boundary case also short-circuits below.
        let response_boundary = if query.authorized_boundaries.len() == 1 {
            Some(query.authorized_boundaries[0].as_str().to_string())
        } else {
            None
        };

        // Zero-boundary short-circuit per BRD §11.4.3 — the auth gate is
        // a short, not an error. No REPORTs to load, no warnings to emit.
        if query.authorized_boundaries.is_empty() {
            return Ok(StructuredReadResponse {
                boundary: response_boundary,
                query: query_echo,
                relevant_facts: Vec::new(),
                abstain: true,
                top_relevance: 0.0,
                health: HealthInfo {
                    status: HealthStatus::Ok,
                    warnings: Vec::new(),
                },
            });
        }

        // Load REPORT per authorised boundary in input order. Each load is
        // independent: a missing/malformed REPORT for one boundary does
        // not poison the others — per ADR-053's "one file per boundary"
        // isolation contract.
        let mut loaded_reports: Vec<(Boundary, Option<LoadedReport>)> =
            Vec::with_capacity(query.authorized_boundaries.len());
        for b in &query.authorized_boundaries {
            let report = self.report_loader.load(b).await?;
            loaded_reports.push((b.clone(), report));
        }

        // Build the warnings vector. Order is boundary-order × per-boundary
        // code emission order: schema-guard → clock-skew → staleness tier
        // → topic-names-unavailable. Deterministic so consecutive responses
        // diff cleanly under identical state.
        let now = self.clock.now();
        let mut warnings: Vec<HealthWarning> = Vec::new();
        for (b, maybe_report) in &loaded_reports {
            match maybe_report {
                None => {
                    warnings.push(report_missing_warning(b));
                }
                Some(report) => {
                    // Unsupported future schema_version → surface as missing.
                    // The consumer can't safely interpret unknown fields.
                    if report.schema_version > SUPPORTED_REPORT_SCHEMA_VERSION {
                        warnings.push(HealthWarning {
                            code: WarningCode::ReportMissing,
                            severity: WarningSeverity::Warn,
                            detail: format!(
                                "REPORT for boundary '{}' has schema_version {} (this binary supports {})",
                                b.as_str(),
                                report.schema_version,
                                SUPPORTED_REPORT_SCHEMA_VERSION
                            ),
                            recovery_hint:
                                "Upgrade the vault binary to a version that understands this REPORT schema."
                                    .into(),
                        });
                        continue;
                    }

                    // Clock-skew dominates staleness (a future-dated REPORT
                    // makes age math meaningless). When skew fires, skip the
                    // staleness tier check for this REPORT.
                    if report.generated_at > now {
                        let skew_secs = (report.generated_at - now).num_seconds();
                        warnings.push(HealthWarning {
                            code: WarningCode::ClockSkewDetected,
                            severity: WarningSeverity::Critical,
                            detail: format!(
                                "REPORT for boundary '{}' generated_at is {skew_secs}s ahead of read-time clock",
                                b.as_str()
                            ),
                            recovery_hint:
                                "Check the system clock on the consolidator host against an authoritative time source (e.g. NTP)."
                                    .into(),
                        });
                    } else {
                        // Staleness tier check.
                        let age_chrono = now - report.generated_at;
                        let age = age_chrono.to_std().unwrap_or(Duration::ZERO);
                        if let Some(w) = staleness_warning(b, age) {
                            warnings.push(w);
                        }
                    }

                    if report.topic_names_unavailable {
                        warnings.push(HealthWarning {
                            code: WarningCode::TopicNamesUnavailable,
                            severity: WarningSeverity::Info,
                            detail: format!(
                                "topic labels for boundary '{}' are placeholder identifiers (Phi-4-mini was unavailable at consolidation)",
                                b.as_str()
                            ),
                            recovery_hint:
                                "Ensure phi4_model_path is configured + the model file is present before the next consolidation run."
                                    .into(),
                        });
                    }
                }
            }
        }

        // Build memory_id → topic_label lookup across all loaded REPORTs.
        // O(1) lookup at the per-fact mapping step below. Cross-boundary
        // collisions are impossible in practice — MemoryId is UUID v7
        // unique, no two boundaries can hold the same id.
        let mut topic_lookup: HashMap<MemoryId, String> = HashMap::new();
        for (_, maybe_report) in &loaded_reports {
            if let Some(report) = maybe_report {
                for (topic_label, facts) in &report.facts_by_topic {
                    for fact in facts {
                        topic_lookup.insert(fact.memory_id, topic_label.clone());
                    }
                }
            }
        }

        // Stage 1 — retrieval, gated on semantic relevance (ADR-057).
        let retrieval_query = RetrievalQuery {
            query_text: query_echo.clone(),
            authorized_boundaries: query.authorized_boundaries.clone(),
            max_results: self.max_candidates,
            options: RetrievalOptions::default(),
        };

        // Relevance gate. Production (ADR-057 amendment, 2026-05-29) uses the
        // CROSS-ENCODER RERANKER: retrieve the top-N, then rerank + filter by
        // the reranker's relevance floor. This supersedes the cosine `semantic`
        // floor, which could not separate topically-adjacent wrong-attribute
        // facts (the Q21 class ADR-057 deferred). The cosine gate (Approach P)
        // remains as the no-reranker fallback + for its existing tests.
        // Precedence: reranker > cosine gate > plain retrieve.
        //
        // `self.retriever` is the RAW hybrid (BGE + Tantivy + RRF), NOT the BM25
        // `AbstainingRetriever` (Bug-2 fix, 2026-05-31). The lexical abstain gate
        // keyword-blocked purely-semantic reads ("what does the user do for fun?"
        // → "plays the cello…", BM25 ~0) before this relevance gate could run;
        // read abstention is now owned here. The app wires the keyword gate to
        // `memory_search` only.
        let candidates = if self.reranker.is_some() {
            let retrieved = self.retriever.retrieve(retrieval_query).await?;
            // Recall-union (ADR-069): widen the rerank pool with the semantic
            // channel's top hits so an RRF-starved pure-semantic match still
            // reaches the reranker (the relevance authority). No-op when no
            // semantic channel is wired.
            let pool = self
                .union_semantic_recall(retrieved, &query_echo, &query.authorized_boundaries)
                .await?;
            self.apply_reranker(&query_echo, pool).await?
        } else if let Some(semantic) = &self.semantic {
            let gate_query = RetrievalQuery {
                query_text: query_echo.clone(),
                authorized_boundaries: query.authorized_boundaries.clone(),
                max_results: RELEVANCE_GATE_TOP_K,
                options: RetrievalOptions::default(),
            };
            let probe = semantic.retrieve(gate_query).await?;
            let relevance = mean_top_k_cosine(&probe, RELEVANCE_GATE_TOP_K);
            if relevance < RELEVANCE_COSINE_FLOOR {
                tracing::info!(
                    target: "vault_retrieval::relevance_gate",
                    relevance_top_k_mean = relevance,
                    floor = RELEVANCE_COSINE_FLOOR,
                    top_k = RELEVANCE_GATE_TOP_K,
                    "abstain: top-K mean semantic cosine below relevance floor"
                );
                Vec::new()
            } else {
                self.retriever.retrieve(retrieval_query).await?
            }
        } else {
            self.retriever.retrieve(retrieval_query).await?
        };

        // Stage 2 — recall-safe abstain HINT + pack (ADR-073).
        //
        // `top_relevance` = the rank-1 score for agent transparency (sigmoid
        // reranker-relevance on the production path; best-effort cosine/RRF on a
        // fallback path).
        //
        // On the RERANKER path (production) `apply_reranker` is reorder-only — it
        // drops nothing, so `candidates` holds every retrieved fact and `abstain`
        // is a HINT computed by combining `search_hint`'s separation test (catches
        // flat distractor clusters, e.g. the salary trap) with the no-signal floor
        // (catches the lone/few no-signal fact `search_hint` reads as "separated",
        // e.g. the cat→dog / Lisbon-guard class). The hint NEVER removes a fact —
        // facts are always packed below — so recall is safe regardless of where
        // the floor sits.
        //
        // On the no-reranker FALLBACK paths (cosine gate / plain) the prior
        // semantics are preserved exactly: `abstain = candidates.is_empty()` (the
        // cosine gate already returns an empty pool when it fires). Per ADR-073 (d)
        // the fallback paths are out of scope for the hint.
        let top_relevance = candidates.first().map(|c| c.score).unwrap_or(0.0);
        let abstain = if self.reranker.is_some() {
            let hint = search_hint(&candidates);
            candidates.is_empty() || hint.weak_match || hint.top_relevance < READ_NO_SIGNAL_FLOOR
        } else {
            candidates.is_empty()
        };
        let relevant_facts: Vec<RelevantFact> = candidates
            .into_iter()
            .map(|c| {
                let topic = topic_lookup.get(&c.memory.id).cloned();
                RelevantFact {
                    fact: c.memory.content,
                    topic,
                    memory_id: c.memory.id.0.to_string(),
                    as_of: c.memory.valid_from,
                    confidence: c.memory.confidence,
                    source_agent: c.memory.source_agent,
                }
            })
            .collect();

        let status = aggregate_status(&warnings);

        Ok(StructuredReadResponse {
            boundary: response_boundary,
            query: query_echo,
            relevant_facts,
            abstain,
            top_relevance,
            health: HealthInfo { status, warnings },
        })
    }

    /// Widen the rerank candidate pool with the semantic channel's top hits
    /// (ADR-069, scale finding 2026-06-04).
    ///
    /// At scale the hybrid's RRF fusion starves a strong PURE-semantic match: a
    /// fact sharing no query keywords (e.g. the subject-less "Plays the cello…")
    /// earns only its semantic-channel RRF term, while every "The user …" fact
    /// earns a second term from the incidental "user" keyword overlap — pushing
    /// the keyword-less fact below the hybrid's returned top-N, out of the
    /// reranker's view (measured: cello at pure-BGE rank 6/100 but dropped past
    /// hybrid rank 20/100). The reranker is the relevance authority; its pool
    /// must not be gated by RRF's keyword bias. We union the semantic channel's
    /// top-[`Self::max_candidates`] (deduped by id) onto the hybrid hits. The
    /// reranker re-sorts the whole pool by relevance and drops below-floor junk,
    /// so unioning in extra candidates only widens recall — it cannot lower
    /// precision. No-op when no semantic channel is wired (e.g. unit tests, or a
    /// no-reranker deployment where the cosine gate owns abstention instead).
    async fn union_semantic_recall(
        &self,
        mut hits: Vec<RetrievedMemory>,
        query: &str,
        boundaries: &[Boundary],
    ) -> VaultResult<Vec<RetrievedMemory>> {
        let Some(semantic) = &self.semantic else {
            return Ok(hits);
        };
        let sem_hits = semantic
            .retrieve(RetrievalQuery {
                query_text: query.to_string(),
                authorized_boundaries: boundaries.to_vec(),
                max_results: self.max_candidates,
                options: RetrievalOptions::default(),
            })
            .await?;
        let present: HashSet<MemoryId> = hits.iter().map(|h| h.memory.id).collect();
        for h in sem_hits {
            if !present.contains(&h.memory.id) {
                hits.push(h);
            }
        }
        Ok(hits)
    }

    /// Rerank the candidate pool with the cross-encoder and re-sort by relevance
    /// DESC. **Reorder-only (ADR-073, 2026-06-08): nothing is dropped.** Every
    /// retrieved candidate is mapped from its logit to a `[0,1]` relevance via
    /// [`relevance_score`] (the same sigmoid `memory_search` uses) and returned;
    /// the reranker's job here is purely to pull the right answer to the top. The
    /// abstain decision is a HINT computed downstream in [`Self::read`]
    /// (separation via [`search_hint`] + the [`READ_NO_SIGNAL_FLOOR`]) and by the
    /// calling agent — never a drop here.
    ///
    /// **Why never drop (ADR-073).** This previously hard-dropped candidates
    /// below `reranker.relevance_floor()` and the read abstained when all were
    /// dropped. The 1k live dogfood proved that false-abstains on stored facts: a
    /// real answer ("runs 10km" for "how do I stay fit") scored logit −3.21, fell
    /// below the −2.5 floor, and was discarded → the vault hid a memory it held —
    /// the cardinal sin ([[project_1k_live_read_false_abstain]]). The drop is
    /// removed; read now mirrors [`crate::RerankedRetriever`]'s reorder-only
    /// `rerank_pool`, and abstain is a separation/no-signal HINT that keeps the
    /// facts.
    ///
    /// The input pool may be a hybrid ∪ semantic union (see
    /// [`Self::union_semantic_recall`]); [`RERANK_CANDIDATE_CAP`] bounds the
    /// reranker batch (cost, not recall) — candidates are assumed sorted by
    /// retriever score DESC (the [`Retriever`] invariant) so truncating keeps the
    /// strongest before the reranker re-sorts.
    async fn apply_reranker(
        &self,
        query: &str,
        mut candidates: Vec<RetrievedMemory>,
    ) -> VaultResult<Vec<RetrievedMemory>> {
        let Some(reranker) = &self.reranker else {
            return Ok(candidates);
        };
        if candidates.is_empty() {
            return Ok(candidates);
        }
        // Split rather than truncate — see the matching note in
        // `RerankedRetriever::rerank_pool`. On the ADR-089 degrade path the
        // whole pool is returned in retriever order; on success the tail is
        // dropped exactly as `truncate` did.
        let tail = candidates.split_off(RERANK_CANDIDATE_CAP.min(candidates.len()));

        let docs: Vec<String> = candidates
            .iter()
            .map(|c| c.memory.content.clone())
            .collect();
        let scores = match reranker.rerank(query, &docs).await {
            Ok(scores) => scores,
            // ADR-089: ranking model not downloaded yet — degrade to the
            // retriever's own order, which is precisely what this function
            // already returns when `self.reranker` is `None` (see the early
            // return above). The downstream abstain HINT is unaffected: it is
            // computed in `read`, and already has to cope with the no-reranker
            // case. Failing the read instead would hide memories the vault
            // holds — the cardinal sin (ADR-073).
            Err(VaultError::ModelUnavailable { component }) => {
                tracing::warn!(
                    target: "vault_retrieval::reranker",
                    component = %component,
                    candidates = candidates.len() + tail.len(),
                    "ranking model unavailable — read falls back to retriever order \
                     (degraded, not failed; ADR-089)"
                );
                candidates.extend(tail);
                return Ok(candidates);
            }
            Err(e) => return Err(e),
        };
        if scores.len() != candidates.len() {
            return Err(VaultError::Embedding(format!(
                "reranker returned {} scores for {} candidates",
                scores.len(),
                candidates.len()
            )));
        }

        // Reorder-only: map every logit to [0,1] and KEEP all candidates.
        let mut reordered: Vec<RetrievedMemory> = candidates
            .into_iter()
            .zip(scores)
            .map(|(mut c, logit)| {
                c.score = relevance_score(logit);
                c.explanation = format!("reranked: logit={logit:.4} → relevance={:.4}", c.score);
                c
            })
            .collect();
        // Re-sort by relevance DESC (the reranker, not the retriever, owns
        // ordering now). `relevance_score` is strictly monotonic + NaN-free, so
        // this preserves the logit order.
        reordered.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        tracing::info!(
            target: "vault_retrieval::reranker",
            returned = reordered.len(),
            top_relevance = reordered.first().map(|c| c.score).unwrap_or(0.0),
            "recall-first rerank: reorder-only (no drop); abstain decided downstream as a hint"
        );
        Ok(reordered)
    }
}

// =============================================================================
// Helpers — pure functions over warning construction + aggregation
// =============================================================================

fn report_missing_warning(boundary: &Boundary) -> HealthWarning {
    HealthWarning {
        code: WarningCode::ReportMissing,
        severity: WarningSeverity::Warn,
        detail: format!(
            "no REPORT artifact exists for boundary '{}'",
            boundary.as_str()
        ),
        recovery_hint: "Run `vault-cli consolidate run` to generate the per-boundary REPORT."
            .into(),
    }
}

fn staleness_warning(boundary: &Boundary, age: Duration) -> Option<HealthWarning> {
    if age >= STALE_CRITICAL_THRESHOLD {
        Some(HealthWarning {
            code: WarningCode::ReportStaleCritical,
            severity: WarningSeverity::Critical,
            detail: format!(
                "REPORT for boundary '{}' is {} days old (≥ 7d)",
                boundary.as_str(),
                age.as_secs() / 86_400
            ),
            recovery_hint: "Run `vault-cli consolidate run` to refresh the REPORT.".into(),
        })
    } else if age >= STALE_WARN_THRESHOLD {
        Some(HealthWarning {
            code: WarningCode::ReportStaleWarn,
            severity: WarningSeverity::Warn,
            detail: format!(
                "REPORT for boundary '{}' is {} hours old (72h-7d band)",
                boundary.as_str(),
                age.as_secs() / 3_600
            ),
            recovery_hint: "Run `vault-cli consolidate run` to refresh the REPORT.".into(),
        })
    } else if age >= STALE_INFO_THRESHOLD {
        Some(HealthWarning {
            code: WarningCode::ReportStaleInfo,
            severity: WarningSeverity::Info,
            detail: format!(
                "REPORT for boundary '{}' is {} hours old (24-72h band)",
                boundary.as_str(),
                age.as_secs() / 3_600
            ),
            recovery_hint: "Run `vault-cli consolidate run` to refresh the REPORT.".into(),
        })
    } else {
        None
    }
}

fn aggregate_status(warnings: &[HealthWarning]) -> HealthStatus {
    if warnings
        .iter()
        .any(|w| w.severity == WarningSeverity::Critical)
    {
        HealthStatus::Critical
    } else if warnings.is_empty() {
        HealthStatus::Ok
    } else {
        HealthStatus::Degraded
    }
}

/// Mean of the top-`k` semantic cosine scores in `hits` (fewer if the slice
/// is shorter; 0.0 if empty). The relevance gate's signal — see
/// [`RELEVANCE_GATE_TOP_K`] / [`RELEVANCE_COSINE_FLOOR`] (ADR-057). Assumes
/// `hits` is sorted by score DESC (the `Retriever` trait invariant), so the
/// first `k` are the highest-cosine hits.
fn mean_top_k_cosine(hits: &[RetrievedMemory], k: usize) -> f32 {
    if hits.is_empty() || k == 0 {
        return 0.0;
    }
    let n = k.min(hits.len());
    let sum: f32 = hits.iter().take(n).map(|h| h.score).sum();
    sum / n as f32
}

impl std::fmt::Debug for StructuredReadPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredReadPipeline")
            .field("max_candidates", &self.max_candidates)
            // retriever / report_loader / clock intentionally omitted: trait
            // objects with no Debug impl.
            .finish_non_exhaustive()
    }
}

// =============================================================================
// Tests — scaffolded Commit 6 (failing until Task 5 implements `read()`)
// =============================================================================

#[cfg(test)]
mod tests {
    //! Unit tests pinning the locked-next-arc Plan Iteration 3 contracts:
    //! - Contract 2 (this module): MCP `memory_read` response shape +
    //!   7 health-warning codes + severity / aggregate-status rules.
    //! - Contract 3 (light coverage): consolidator-produced REPORT shape
    //!   that this pipeline consumes.
    //!
    //! Heavy quality assertions (multi-boundary correctness against real
    //! LanceDB / Tantivy / BGE) live at the application layer; this
    //! module's tests use mock implementations of `Retriever` +
    //! `ReportLoader` to exercise the pipeline-shape contracts.
    //!
    //! All tests scaffolded Commit 6 (locked-next-arc, 2026-05-26).
    //! Tests panic at `todo!()` until Task 5 implements `read()`.

    use super::*;
    use crate::report_io::{LoadedReport, LoadedReportFact, ReportLoader};
    use crate::retriever::{RetrievalQuery, RetrievedMemory};
    use crate::search_hint::STRONG_RELEVANCE;
    use async_trait::async_trait;
    use chrono::TimeZone;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;
    use vault_core::{Memory, MemoryId, MemoryType, NewMemory};

    // ---------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------

    fn boundary(name: &str) -> Boundary {
        Boundary::new(name).expect("static-valid test boundary")
    }

    /// Construct a Memory with explicit valid_from so `as_of` assertions
    /// are deterministic. Other fields are filled in as plausible defaults.
    fn fake_memory(
        id_n: u128,
        content: &str,
        boundary_name: &str,
        valid_from: DateTime<Utc>,
        confidence: f32,
        source_agent: Option<&str>,
    ) -> Memory {
        let mut m = Memory::try_new(NewMemory {
            content: content.to_string(),
            memory_type: MemoryType::Semantic,
            boundary: boundary(boundary_name),
            source_agent: source_agent.map(str::to_string),
            confidence,
            valid_from: Some(valid_from),
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("static-valid test memory");
        m.id = MemoryId(uuid_from_id(id_n));
        m
    }

    fn uuid_from_id(n: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(n)
    }

    fn retrieved(memory: Memory, score: f32) -> RetrievedMemory {
        RetrievedMemory {
            memory,
            score,
            explanation: format!("test: score={score:.4}"),
        }
    }

    /// Build a LoadedReport with one topic containing the supplied memory
    /// IDs. Useful for the topic-lookup tests.
    fn loaded_report_with_topic(
        boundary_name: &str,
        generated_at: DateTime<Utc>,
        topic_label: &str,
        member_ids: &[MemoryId],
        topic_names_unavailable: bool,
    ) -> LoadedReport {
        let facts: Vec<LoadedReportFact> = member_ids
            .iter()
            .map(|id| LoadedReportFact {
                fact: format!("fact-for-{id}"),
                memory_id: *id,
                as_of: generated_at,
                confidence: 0.9,
                source_agent: None,
            })
            .collect();
        let mut facts_by_topic = BTreeMap::new();
        facts_by_topic.insert(topic_label.to_string(), facts);
        LoadedReport {
            schema_version: 1,
            boundary: boundary(boundary_name),
            generated_at,
            consolidator_run_id: "00000000-0000-0000-0000-000000000000".into(),
            facts_by_topic,
            topic_names_unavailable,
        }
    }

    /// Mock retriever — returns canned candidates regardless of query.
    /// Tests that need query-shape inspection can call `observed_query()`.
    struct MockRetriever {
        canned: Vec<RetrievedMemory>,
        last_query: Mutex<Option<RetrievalQuery>>,
    }

    impl MockRetriever {
        fn new(canned: Vec<RetrievedMemory>) -> Arc<Self> {
            Arc::new(Self {
                canned,
                last_query: Mutex::new(None),
            })
        }
    }

    #[async_trait]
    impl Retriever for MockRetriever {
        async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
            *self.last_query.lock().unwrap() = Some(query);
            Ok(self.canned.clone())
        }
    }

    /// Mock REPORT loader — returns canned `Option<LoadedReport>` keyed
    /// by boundary name. Boundaries without a canned entry return Ok(None)
    /// (the REPORT_MISSING surfacing path).
    struct MockReportLoader {
        canned: HashMap<String, LoadedReport>,
    }

    impl MockReportLoader {
        fn new(reports: HashMap<String, LoadedReport>) -> Arc<Self> {
            Arc::new(Self { canned: reports })
        }

        fn empty() -> Arc<Self> {
            Arc::new(Self {
                canned: HashMap::new(),
            })
        }
    }

    #[async_trait]
    impl ReportLoader for MockReportLoader {
        async fn load(&self, boundary: &Boundary) -> VaultResult<Option<LoadedReport>> {
            Ok(self.canned.get(boundary.as_str()).cloned())
        }
    }

    /// Fixed-point Clock for staleness-tier tests.
    #[derive(Debug, Clone)]
    struct FixedClock {
        now: DateTime<Utc>,
    }

    impl FixedClock {
        fn arc(now: DateTime<Utc>) -> Arc<Self> {
            Arc::new(Self { now })
        }
    }

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.now
        }
    }

    /// 2026-06-01T12:00:00Z — fixed read-time anchor for deterministic
    /// staleness math. Pick a date well after Batch A's 2026-05-26 ship
    /// to avoid accidental overlap with any side data.
    fn read_clock_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap()
    }

    // ---------------------------------------------------------------------
    // Group A — Query validation + abstain short-circuits
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn empty_query_text_returns_invalid_input_error() {
        let pipeline =
            StructuredReadPipeline::new(MockRetriever::new(vec![]), MockReportLoader::empty());
        let err = pipeline
            .read(ReadQuery {
                query_text: "   ".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .expect_err("empty-after-trim MUST surface as InvalidInput, not pass to retriever");
        assert!(
            matches!(err, VaultError::InvalidInput(_)),
            "expected VaultError::InvalidInput; got {err:?}"
        );
    }

    #[tokio::test]
    async fn zero_authorized_boundaries_short_circuits_to_abstain_ok_health() {
        // Per BRD §11.4.3 the auth gate is short-circuit, not an error.
        // The agent might legitimately ask the vault when no boundaries
        // are authorized; the right response is "no relevant content".
        let pipeline =
            StructuredReadPipeline::new(MockRetriever::new(vec![]), MockReportLoader::empty());
        let resp = pipeline
            .read(ReadQuery {
                query_text: "anything".into(),
                authorized_boundaries: vec![],
            })
            .await
            .expect("zero-boundaries MUST short-circuit, never error");
        assert!(resp.abstain, "zero boundaries MUST set abstain=true");
        assert!(resp.relevant_facts.is_empty());
        assert_eq!(resp.health.status, HealthStatus::Ok);
        assert!(
            resp.health.warnings.is_empty(),
            "zero-boundaries short-circuit MUST NOT emit warnings (no REPORT to check)"
        );
    }

    #[tokio::test]
    async fn empty_retrieval_returns_abstain_true_with_facts_empty() {
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![]), // retriever returns 0 candidates
            MockReportLoader::empty(),
        );
        let resp = pipeline
            .read(ReadQuery {
                query_text: "anything".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .expect("empty retrieval MUST succeed with abstain=true");
        assert!(resp.abstain);
        assert!(resp.relevant_facts.is_empty());
    }

    #[tokio::test]
    async fn query_text_is_trimmed_before_echoing_to_response() {
        let mem = fake_memory(1, "a fact", "personal", read_clock_now(), 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(mem, 0.9)]),
            MockReportLoader::empty(),
        );
        let resp = pipeline
            .read(ReadQuery {
                query_text: "  whitespace-padded  ".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(
            resp.query, "whitespace-padded",
            "response.query MUST be the trimmed query text"
        );
    }

    // ---------------------------------------------------------------------
    // Group B — Boundary field semantics
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn single_boundary_query_sets_response_boundary_to_that_name() {
        let mem = fake_memory(1, "fact", "personal", read_clock_now(), 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(mem, 0.9)]),
            MockReportLoader::empty(),
        );
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(
            resp.boundary.as_deref(),
            Some("personal"),
            "single-boundary read MUST set response.boundary = Some(boundary_name)"
        );
    }

    #[tokio::test]
    async fn multi_boundary_query_sets_response_boundary_to_none() {
        let m1 = fake_memory(1, "fact1", "personal", read_clock_now(), 0.9, None);
        let m2 = fake_memory(2, "fact2", "work", read_clock_now(), 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m1, 0.9), retrieved(m2, 0.8)]),
            MockReportLoader::empty(),
        );
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal"), boundary("work")],
            })
            .await
            .unwrap();
        assert!(
            resp.boundary.is_none(),
            "multi-boundary read MUST set response.boundary = None; got {:?}",
            resp.boundary
        );
    }

    // ---------------------------------------------------------------------
    // Group C — Filter + pack: relevant_facts construction
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn retrieved_memories_become_relevant_facts_in_retrieval_order() {
        let now = read_clock_now();
        let m1 = fake_memory(1, "first", "personal", now, 0.95, Some("claude"));
        let m2 = fake_memory(2, "second", "personal", now, 0.80, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![
                retrieved(m1.clone(), 0.95),
                retrieved(m2.clone(), 0.80),
            ]),
            MockReportLoader::empty(),
        );
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(resp.relevant_facts.len(), 2);
        assert_eq!(resp.relevant_facts[0].fact, "first");
        assert_eq!(
            resp.relevant_facts[0].source_agent.as_deref(),
            Some("claude")
        );
        assert_eq!(resp.relevant_facts[0].confidence, 0.95);
        assert_eq!(resp.relevant_facts[1].fact, "second");
        assert!(resp.relevant_facts[1].source_agent.is_none());
        assert!(!resp.abstain);
    }

    #[tokio::test]
    async fn topic_lookup_from_report_when_memory_id_matches() {
        let now = read_clock_now();
        let m = fake_memory(7, "BP 132/85", "personal", now, 0.95, None);
        let report = loaded_report_with_topic(
            "personal",
            now,
            "blood_pressure",
            &[m.id],
            false, // topic_names_unavailable=false
        );
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(resp.relevant_facts.len(), 1);
        assert_eq!(
            resp.relevant_facts[0].topic.as_deref(),
            Some("blood_pressure"),
            "memory present in REPORT.facts_by_topic MUST get topic=Some(label)"
        );
    }

    #[tokio::test]
    async fn topic_is_none_when_memory_not_in_report() {
        // Memory written since last consolidation — not in REPORT yet.
        let now = read_clock_now();
        let m_new = fake_memory(99, "fresh fact", "personal", now, 0.9, None);
        let m_old = fake_memory(1, "old fact", "personal", now, 0.9, None);
        let report = loaded_report_with_topic("personal", now, "old_topic", &[m_old.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m_new, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(resp.relevant_facts.len(), 1);
        assert!(
            resp.relevant_facts[0].topic.is_none(),
            "memory NOT in REPORT MUST get topic=None"
        );
    }

    #[tokio::test]
    async fn topic_is_none_when_no_report_for_boundary() {
        let now = read_clock_now();
        let m = fake_memory(1, "fact", "personal", now, 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::empty(), // no REPORT
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert!(
            resp.relevant_facts[0].topic.is_none(),
            "REPORT_MISSING case MUST set topic=None on all facts"
        );
    }

    // ---------------------------------------------------------------------
    // Group C.5 — relevance gate (ADR-057): no-signal abstention via the
    // semantic top-1 cosine floor. Both directions pinned + anti-dilution.
    // ---------------------------------------------------------------------

    /// Mock semantic probe channel returning `RELEVANCE_GATE_TOP_K` hit(s) all
    /// at `cosine`, so the gate's top-1 equals `cosine` exactly.
    fn semantic_probe_at(cosine: f32) -> Arc<MockRetriever> {
        let now = read_clock_now();
        let hits: Vec<RetrievedMemory> = (0..RELEVANCE_GATE_TOP_K)
            .map(|i| {
                let m = fake_memory(900 + i as u128, "probe", "personal", now, 0.9, None);
                retrieved(m, cosine)
            })
            .collect();
        MockRetriever::new(hits)
    }

    #[test]
    fn relevance_floor_and_top_k_are_pinned() {
        // ADR-057 calibration pins (n=5 no-signal probes, 2026-05-28). A future
        // embedder swap that shifts the cosine distribution MUST re-break these
        // consciously, not drift silently.
        assert!(
            (RELEVANCE_COSINE_FLOOR - 0.66).abs() < f32::EPSILON,
            "relevance floor pinned at 0.66 (top-1 calibration: in the 0.642-0.696 gap)"
        );
        assert_eq!(
            RELEVANCE_GATE_TOP_K, 1,
            "top-1 (NOT top-K mean — avoids sparse-vault dilution; see over-abstain finding)"
        );
    }

    #[tokio::test]
    async fn no_signal_query_abstains_when_top_k_mean_below_floor() {
        // The A6 ship-gate. The main retriever WOULD return a candidate (so the
        // abstain is the GATE firing, not empty retrieval), but the semantic
        // probe's top-1 (0.61) is below the 0.66 floor → abstain, no facts.
        let now = read_clock_now();
        let candidate = fake_memory(1, "an unrelated fact", "personal", now, 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(candidate, 0.0145)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_relevance_gate(semantic_probe_at(0.61));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "what is the user's blood type".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .expect("gate abstain MUST succeed, not error");
        assert!(
            resp.abstain,
            "top-1 0.61 < floor 0.66 MUST abstain (no-signal ship-gate)"
        );
        assert!(
            resp.relevant_facts.is_empty(),
            "abstain MUST return empty facts even though the retriever had a candidate"
        );
    }

    #[tokio::test]
    async fn genuine_content_proceeds_when_top_k_mean_above_floor() {
        // No over-abstain: a real query (semantic top-1 0.72) clears the
        // floor → candidates returned, abstain=false.
        let now = read_clock_now();
        let m = fake_memory(
            1,
            "the user prefers dark roast",
            "personal",
            now,
            0.95,
            None,
        );
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.0328)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_relevance_gate(semantic_probe_at(0.72));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "what coffee does the user like".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert!(
            !resp.abstain,
            "top-1 0.72 >= floor 0.66 MUST proceed (no over-abstain)"
        );
        assert_eq!(resp.relevant_facts.len(), 1);
    }

    #[tokio::test]
    async fn contradiction_band_proceeds_at_lowest_measured_cosine() {
        // The lowest must-proceed contradiction measured at top-1 cosine 0.696
        // (Q26) MUST clear the 0.66 floor — contradiction detection is not
        // silently gated off.
        let now = read_clock_now();
        let m = fake_memory(1, "GA launch is Q1 2027", "work", now, 0.95, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.0300)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_relevance_gate(semantic_probe_at(0.696));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "when did we decide GA launch".into(),
                authorized_boundaries: vec![boundary("work")],
            })
            .await
            .unwrap();
        assert!(
            !resp.abstain,
            "lowest contradiction cosine 0.696 >= 0.66 MUST proceed"
        );
    }

    #[tokio::test]
    async fn top1_proceeds_on_single_strong_match_amid_weak_fillers() {
        // The over-abstain regression (live dogfood 2026-05-28): on a sparse
        // vault a real query has ONE strong match (e.g. zafflang ~0.72) plus
        // weak unrelated fillers (~0.45). top-3 mean would be ~0.54 → wrongly
        // abstain; top-1 (0.72) clears the 0.66 floor → proceeds. This is WHY
        // the gate uses top-1 (RELEVANCE_GATE_TOP_K=1), not top-K mean.
        let now = read_clock_now();
        let m = fake_memory(
            1,
            "the user prefers the zafflang language",
            "personal",
            now,
            0.95,
            None,
        );
        // Probe with one strong hit + two weak fillers, in score-DESC order
        // (mirrors the SemanticRetriever's sorted output).
        let strong = fake_memory(901, "strong match", "personal", now, 0.9, None);
        let filler1 = fake_memory(902, "filler one", "personal", now, 0.9, None);
        let filler2 = fake_memory(903, "filler two", "personal", now, 0.9, None);
        let probe: Arc<dyn Retriever> = MockRetriever::new(vec![
            retrieved(strong, 0.72),
            retrieved(filler1, 0.45),
            retrieved(filler2, 0.45),
        ]);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.0328)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_relevance_gate(probe);
        let resp = pipeline
            .read(ReadQuery {
                query_text: "what language do I prefer".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert!(
            !resp.abstain,
            "top-1 (0.72) MUST proceed despite weak fillers — top-3 mean (0.54) would over-abstain"
        );
        assert_eq!(resp.relevant_facts.len(), 1);
    }

    #[tokio::test]
    async fn gate_disabled_by_default_does_not_abstain_on_low_cosine() {
        // Opt-in semantics: a pipeline built WITHOUT `with_relevance_gate`
        // never runs the probe, so it returns candidates regardless of cosine
        // (the existing pipeline contract the other tests rely on).
        let now = read_clock_now();
        let m = fake_memory(1, "fact", "personal", now, 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.0145)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now));
        // NOTE: no .with_relevance_gate(...)
        let resp = pipeline
            .read(ReadQuery {
                query_text: "anything".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert!(
            !resp.abstain,
            "gate-off pipeline MUST NOT abstain on a returned candidate"
        );
        assert_eq!(resp.relevant_facts.len(), 1);
    }

    // ---------------------------------------------------------------------
    // Group C.6 — reranker gate (ADR-057 amendment): cross-encoder relevance.
    // Filter-by-floor, re-sort-by-score, abstain-when-none-pass, top-N cap.
    // (RerankProvider is in scope via `use super::*`.)
    // ---------------------------------------------------------------------

    /// Mock reranker — returns a score per doc from a content→score map
    /// (unknown content → `default`), with a configurable floor.
    struct MockReranker {
        scores: HashMap<String, f32>,
        default: f32,
        floor: f32,
    }

    impl MockReranker {
        fn new(scores: Vec<(&str, f32)>, default: f32, floor: f32) -> Arc<Self> {
            Arc::new(Self {
                scores: scores
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
                default,
                floor,
            })
        }
    }

    #[async_trait]
    impl RerankProvider for MockReranker {
        async fn rerank(&self, _query: &str, docs: &[String]) -> VaultResult<Vec<f32>> {
            Ok(docs
                .iter()
                .map(|d| self.scores.get(d).copied().unwrap_or(self.default))
                .collect())
        }
        fn relevance_floor(&self) -> f32 {
            self.floor
        }
    }

    /// Mock reranker reporting the model as absent — the ADR-089 pre-download
    /// state of a fresh install.
    struct UnavailableReranker;

    #[async_trait]
    impl RerankProvider for UnavailableReranker {
        async fn rerank(&self, _query: &str, _docs: &[String]) -> VaultResult<Vec<f32>> {
            Err(VaultError::ModelUnavailable {
                component: "reranker-model".to_string(),
            })
        }
        fn relevance_floor(&self) -> f32 {
            0.0
        }
    }

    /// Mock reranker failing for a reason that is NOT "not downloaded yet".
    struct TamperedReranker;

    #[async_trait]
    impl RerankProvider for TamperedReranker {
        async fn rerank(&self, _query: &str, _docs: &[String]) -> VaultResult<Vec<f32>> {
            Err(VaultError::ModelIntegrityFailed {
                file: "reranker-model".to_string(),
                expected: "aa".to_string(),
                actual: "bb".to_string(),
            })
        }
        fn relevance_floor(&self) -> f32 {
            0.0
        }
    }

    /// ADR-089 — `memory_read` degrades rather than failing when the ranking
    /// model has not been downloaded yet.
    ///
    /// Failing here would be worse than on the search path: the read is the
    /// primary answer path, so an error means the vault refuses to answer
    /// about memories it actually holds — the cardinal sin ADR-073 was written
    /// to prevent, arrived at from a different direction.
    #[tokio::test]
    async fn read_degrades_to_retriever_order_when_the_model_is_not_downloaded_yet() {
        let now = read_clock_now();
        let first = fake_memory(1, "the user plays the cello", "personal", now, 0.9, None);
        let second = fake_memory(
            2,
            "the user likes strong coffee",
            "personal",
            now,
            0.9,
            None,
        );
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(first, 0.9), retrieved(second, 0.4)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_reranker(Arc::new(UnavailableReranker));

        let resp = pipeline
            .read(ReadQuery {
                query_text: "what instrument does the user play?".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .expect("an absent ranking model must NOT fail the read");

        assert_eq!(
            resp.relevant_facts.len(),
            2,
            "the degrade path must keep every candidate the retriever found"
        );
        assert_eq!(
            resp.relevant_facts[0].fact, "the user plays the cello",
            "degrade preserves the retriever's own order"
        );
    }

    /// The other half of ADR-089 on the read path: a tampered model file must
    /// still fail loudly, never quietly fall back to un-reranked results.
    #[tokio::test]
    async fn read_with_a_tampered_model_fails_and_does_not_degrade() {
        let now = read_clock_now();
        let m = fake_memory(1, "a fact", "personal", now, 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_reranker(Arc::new(TamperedReranker));

        let err = pipeline
            .read(ReadQuery {
                query_text: "anything".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .expect_err("an integrity failure must propagate, never degrade");
        assert!(
            matches!(err, VaultError::ModelIntegrityFailed { .. }),
            "expected the integrity failure to reach the caller, got {err:?}"
        );
    }

    #[tokio::test]
    async fn reranker_reorders_lower_candidate_instead_of_dropping_it() {
        // ADR-073 (was `reranker_filters_candidates_below_floor`): the read path
        // is now REORDER-ONLY. A lower-scoring candidate is KEPT and ranked below
        // the strong one — never dropped. The old floor-drop false-abstained on
        // real facts (see [[project_1k_live_read_false_abstain]]); recall is now
        // unconditional and the agent judges relevance.
        let now = read_clock_now();
        let keep = fake_memory(
            1,
            "the user finds light themes straining",
            "personal",
            now,
            0.9,
            None,
        );
        let lower = fake_memory(
            2,
            "the user collects vintage keyboards",
            "personal",
            now,
            0.9,
            None,
        );
        let reranker = MockReranker::new(
            vec![
                ("the user finds light themes straining", 4.2), // sigmoid ≈ 0.985
                ("the user collects vintage keyboards", -3.1),  // sigmoid ≈ 0.043
            ],
            -10.0,
            0.0,
        );
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(keep, 0.5), retrieved(lower, 0.4)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_reranker(reranker);
        let resp = pipeline
            .read(ReadQuery {
                query_text: "is the user bothered by bright screens?".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert!(
            !resp.abstain,
            "a strong, separated match present MUST NOT abstain"
        );
        assert_eq!(
            resp.relevant_facts.len(),
            2,
            "reorder-only: both candidates kept, none dropped"
        );
        assert_eq!(
            resp.relevant_facts[0].fact, "the user finds light themes straining",
            "the strong match ranks first"
        );
        assert_eq!(
            resp.relevant_facts[1].fact, "the user collects vintage keyboards",
            "the weaker fact is kept + ranked below, NOT dropped (recall-safe)"
        );
    }

    #[tokio::test]
    async fn reranker_abstains_via_no_signal_floor_but_still_returns_the_fact() {
        // ADR-073 (was `reranker_abstains_when_all_candidates_below_floor`): the
        // A7-guard case — a lone adjacent-but-wrong fact scoring deep no-signal
        // (logit −5.4 → relevance ≈ 0.0045, below READ_NO_SIGNAL_FLOOR) sets
        // `abstain=true`. But the fact is STILL returned (recall-safe): the honest
        // "not in vault" signal is the `abstain` flag + the low `top_relevance`,
        // NOT an empty list. A capable agent inspects the fact and declines (the
        // live cat→dog behaviour); a weak agent trusts `abstain`.
        let now = read_clock_now();
        let guard = fake_memory(
            1,
            "the user relocated to Lisbon",
            "personal",
            now,
            0.9,
            None,
        );
        let reranker = MockReranker::new(vec![("the user relocated to Lisbon", -5.4)], -10.0, 0.0);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(guard, 0.7)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_reranker(reranker);
        let resp = pipeline
            .read(ReadQuery {
                query_text: "what city did the user grow up in?".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert!(
            resp.abstain,
            "deep no-signal (top < READ_NO_SIGNAL_FLOOR) MUST set abstain=true"
        );
        assert_eq!(
            resp.relevant_facts.len(),
            1,
            "recall-safe: the candidate is STILL returned even when abstaining (ADR-073)"
        );
        assert!(
            resp.top_relevance < READ_NO_SIGNAL_FLOOR,
            "top_relevance ({}) MUST be below the no-signal floor here",
            resp.top_relevance
        );
    }

    #[tokio::test]
    async fn reranker_reorders_by_rerank_score_not_retrieval_order() {
        // Retriever ranks A above B; the reranker disagrees (B more relevant).
        // The response MUST be reranker-ordered: B first.
        let now = read_clock_now();
        let a = fake_memory(1, "fact A", "personal", now, 0.9, None);
        let b = fake_memory(2, "fact B", "personal", now, 0.9, None);
        let reranker = MockReranker::new(vec![("fact A", 1.0), ("fact B", 6.0)], -10.0, 0.0);
        let pipeline = StructuredReadPipeline::new(
            // retriever order: A (0.9) before B (0.8)
            MockRetriever::new(vec![retrieved(a, 0.9), retrieved(b, 0.8)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_reranker(reranker);
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(resp.relevant_facts.len(), 2);
        assert_eq!(
            resp.relevant_facts[0].fact, "fact B",
            "reranker score MUST decide order, not retrieval rank"
        );
        assert_eq!(resp.relevant_facts[1].fact, "fact A");
    }

    #[tokio::test]
    async fn reranker_caps_at_rerank_candidate_cap() {
        // `RERANK_CANDIDATE_CAP + 5` retrieved candidates; only the top
        // RERANK_CANDIDATE_CAP are reranked. The overflow are dropped even
        // though the mock would score them above the floor — they never reach
        // the reranker. (The cap was raised 8→DEFAULT_MAX_CANDIDATES at the
        // 2026-06-01 dogfood: BGE's weak ranking must not displace a relevant
        // fact out of the reranker's view; see RERANK_CANDIDATE_CAP docs.)
        let n = RERANK_CANDIDATE_CAP + 5;
        let now = read_clock_now();
        let canned: Vec<RetrievedMemory> = (0..n)
            .map(|i| {
                let m = fake_memory(
                    100 + i as u128,
                    &format!("candidate {i}"),
                    "personal",
                    now,
                    0.9,
                    None,
                );
                // retriever score DESC so truncate keeps candidates 0..CAP
                retrieved(m, 1.0 - (i as f32) * 0.01)
            })
            .collect();
        // Mock scores every candidate above the floor.
        let reranker = MockReranker::new(vec![], 5.0, 0.0);
        let pipeline =
            StructuredReadPipeline::new(MockRetriever::new(canned), MockReportLoader::empty())
                .with_clock(FixedClock::arc(now))
                .with_reranker(reranker);
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(
            resp.relevant_facts.len(),
            RERANK_CANDIDATE_CAP,
            "only top-{RERANK_CANDIDATE_CAP} candidates are reranked + surfaced"
        );
        let facts: Vec<&str> = resp
            .relevant_facts
            .iter()
            .map(|f| f.fact.as_str())
            .collect();
        // The overflow candidates (indices CAP..n) MUST be dropped.
        for i in RERANK_CANDIDATE_CAP..n {
            let dropped = format!("candidate {i}");
            assert!(
                !facts.contains(&dropped.as_str()),
                "candidate {i} (beyond the cap) MUST be dropped (never reranked)"
            );
        }
    }

    #[tokio::test]
    async fn union_semantic_recall_rescues_keyword_starved_fact() {
        // Scale finding (ADR-069, 2026-06-04): a strong PURE-semantic match
        // (subject-less, no query-keyword overlap) is starved out of the
        // hybrid's RRF top-N by facts with incidental keyword overlap. Here the
        // hybrid mock returns ONLY the keyword-y distractor (the cello is beyond
        // its returned pool, modelling the RRF burial); the semantic channel
        // ranks the cello #1. The recall-union must feed the cello to the
        // reranker, which surfaces it. Pre-fix (no union) the cello never
        // reaches the reranker and the read abstains (distractor below floor).
        let now = read_clock_now();
        let cello = fake_memory(
            1,
            "plays the cello in a community orchestra",
            "personal",
            now,
            0.9,
            None,
        );
        let distractor = fake_memory(
            2,
            "the user listens to jazz while working",
            "personal",
            now,
            0.9,
            None,
        );
        // Hybrid returns ONLY the distractor — the cello is RRF-starved out.
        let hybrid = MockRetriever::new(vec![retrieved(distractor.clone(), 0.5)]);
        // Semantic channel ranks the cello highly (its true pure-BGE strength).
        let semantic = MockRetriever::new(vec![retrieved(cello, 0.8), retrieved(distractor, 0.4)]);
        // Reranker: cello above floor, jazz distractor below.
        let reranker = MockReranker::new(
            vec![
                ("plays the cello in a community orchestra", 3.0),
                ("the user listens to jazz while working", -3.0),
            ],
            -10.0,
            0.0,
        );
        let pipeline = StructuredReadPipeline::new(hybrid, MockReportLoader::empty())
            .with_clock(FixedClock::arc(now))
            .with_relevance_gate(semantic)
            .with_reranker(reranker);
        let resp = pipeline
            .read(ReadQuery {
                query_text: "what instrument does the user play?".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert!(
            !resp.abstain,
            "the recall-union MUST rescue the keyword-starved semantic match"
        );
        assert_eq!(
            resp.relevant_facts.len(),
            2,
            "reorder-only (ADR-073): the unioned-in cello AND the distractor are both kept"
        );
        assert_eq!(
            resp.relevant_facts[0].fact, "plays the cello in a community orchestra",
            "the recall-union match reranks to #1"
        );
    }

    #[tokio::test]
    async fn union_semantic_recall_dedups_overlap() {
        // When the same fact is returned by BOTH the hybrid and the semantic
        // channel, the union must NOT duplicate it in the response.
        let now = read_clock_now();
        let shared = fake_memory(1, "the user owns a rivian", "personal", now, 0.9, None);
        let hybrid = MockRetriever::new(vec![retrieved(shared.clone(), 0.6)]);
        let semantic = MockRetriever::new(vec![retrieved(shared, 0.9)]);
        let reranker = MockReranker::new(vec![("the user owns a rivian", 2.0)], -10.0, 0.0);
        let pipeline = StructuredReadPipeline::new(hybrid, MockReportLoader::empty())
            .with_clock(FixedClock::arc(now))
            .with_relevance_gate(semantic)
            .with_reranker(reranker);
        let resp = pipeline
            .read(ReadQuery {
                query_text: "what does the user drive?".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(
            resp.relevant_facts.len(),
            1,
            "a fact in both channels must appear once, not twice"
        );
    }

    // ---------------------------------------------------------------------
    // Group C.7 — ADR-073 recall-safe abstain hint (1k live-dogfood regressions)
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn low_but_separated_top_proceeds_and_returns_the_fact() {
        // THE Gap-1 bug, pinned. "how do I stay fit" → "runs 10km" scored logit
        // −3.21 (relevance ≈ 0.039) live and was DROPPED by the old floor →
        // false-abstain. Now: low absolute score but clearly separated from the
        // pool ⇒ abstain=false, the real fact is returned.
        let now = read_clock_now();
        let answer = fake_memory(
            1,
            "the user runs ten kilometres three times a week",
            "personal",
            now,
            0.93,
            None,
        );
        let filler = fake_memory(
            2,
            "the user keeps a detailed food diary",
            "personal",
            now,
            0.9,
            None,
        );
        let reranker = MockReranker::new(
            vec![
                ("the user runs ten kilometres three times a week", -3.2), // ≈ 0.0392
                ("the user keeps a detailed food diary", -5.5),            // ≈ 0.0041
            ],
            -10.0,
            0.0,
        );
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(answer, 0.5), retrieved(filler, 0.4)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_reranker(reranker);
        let resp = pipeline
            .read(ReadQuery {
                query_text: "how do i stay fit".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert!(
            !resp.abstain,
            "a low-but-separated top (0.039 vs 0.004, ≈9×) MUST proceed — the Gap-1 false-abstain fix"
        );
        assert_eq!(
            resp.relevant_facts[0].fact,
            "the user runs ten kilometres three times a week"
        );
        assert!(
            resp.top_relevance > READ_NO_SIGNAL_FLOOR && resp.top_relevance < STRONG_RELEVANCE,
            "top_relevance ({}) is the low-but-real band",
            resp.top_relevance
        );
    }

    #[tokio::test]
    async fn flat_distractor_cluster_abstains_but_keeps_facts() {
        // The salary-trap class: query has no real answer, but several adjacent
        // distractors score similarly (a FLAT cluster, e.g. vendor "$6,500
        // annual" rows ≈ 0.025/0.023/0.022 live). No separation ⇒ weak_match ⇒
        // abstain=true; but the facts are STILL returned so the agent can say
        // "those are vendor rates, not a salary".
        let now = read_clock_now();
        let a = fake_memory(
            1,
            "booked Mercury Logistics, rate approximately 6500 annual",
            "personal",
            now,
            0.9,
            None,
        );
        let b = fake_memory(
            2,
            "booked Vertex Furniture, rate approximately 6500 annual",
            "personal",
            now,
            0.9,
            None,
        );
        let c = fake_memory(
            3,
            "booked Cascade Cleaning, rate approximately 6500 annual",
            "personal",
            now,
            0.9,
            None,
        );
        let reranker = MockReranker::new(
            vec![
                (
                    "booked Mercury Logistics, rate approximately 6500 annual",
                    -3.66,
                ), // ≈ 0.0251
                (
                    "booked Vertex Furniture, rate approximately 6500 annual",
                    -3.76,
                ), // ≈ 0.0228
                (
                    "booked Cascade Cleaning, rate approximately 6500 annual",
                    -3.79,
                ), // ≈ 0.0221
            ],
            -10.0,
            0.0,
        );
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![
                retrieved(a, 0.5),
                retrieved(b, 0.49),
                retrieved(c, 0.48),
            ]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_reranker(reranker);
        let resp = pipeline
            .read(ReadQuery {
                query_text: "what is the user's annual salary".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert!(
            resp.abstain,
            "a flat cluster (no separation, all ≈0.022-0.025) MUST abstain — the salary-trap class"
        );
        assert_eq!(
            resp.relevant_facts.len(),
            3,
            "recall-safe: the closest facts are STILL returned for the agent to judge"
        );
    }

    #[tokio::test]
    async fn strong_match_sets_high_top_relevance_and_no_abstain() {
        // Field-population pin: a confidently-relevant fact yields abstain=false
        // and a top_relevance well above STRONG_RELEVANCE.
        let now = read_clock_now();
        let m = fake_memory(
            1,
            "plays the cello in a community orchestra",
            "personal",
            now,
            0.93,
            None,
        );
        let reranker = MockReranker::new(
            vec![("plays the cello in a community orchestra", 6.5)],
            -10.0,
            0.0,
        );
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.5)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now))
        .with_reranker(reranker);
        let resp = pipeline
            .read(ReadQuery {
                query_text: "what instrument does the user play".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert!(!resp.abstain);
        assert!(
            resp.top_relevance > STRONG_RELEVANCE,
            "a +6.5 logit maps near 1.0 (got {})",
            resp.top_relevance
        );
    }

    // ---------------------------------------------------------------------
    // Group D — 7 warning codes (ADR-054 Contract 2)
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn report_missing_emits_report_missing_warning_with_warn_severity() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::ReportMissing)
            .expect("REPORT_MISSING warning MUST be emitted when no REPORT exists for boundary");
        assert_eq!(w.severity, WarningSeverity::Warn);
        assert!(!w.recovery_hint.is_empty(), "recovery_hint MUST be set");
    }

    #[tokio::test]
    async fn report_age_24_to_72_hours_emits_stale_info_with_info_severity() {
        let now = read_clock_now();
        let generated = now - chrono::Duration::hours(36); // 36h → INFO band
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let report = loaded_report_with_topic("personal", generated, "t", &[m.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::ReportStaleInfo)
            .expect("REPORT_STALE_INFO MUST fire for 24h ≤ age < 72h");
        assert_eq!(w.severity, WarningSeverity::Info);
    }

    #[tokio::test]
    async fn report_age_72_hours_to_7_days_emits_stale_warn_with_warn_severity() {
        let now = read_clock_now();
        let generated = now - chrono::Duration::days(4); // 4d → WARN band
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let report = loaded_report_with_topic("personal", generated, "t", &[m.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::ReportStaleWarn)
            .expect("REPORT_STALE_WARN MUST fire for 72h ≤ age < 7d");
        assert_eq!(w.severity, WarningSeverity::Warn);
    }

    #[tokio::test]
    async fn report_age_7_plus_days_emits_stale_critical_with_critical_severity() {
        let now = read_clock_now();
        let generated = now - chrono::Duration::days(14); // 14d → CRITICAL band
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let report = loaded_report_with_topic("personal", generated, "t", &[m.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::ReportStaleCritical)
            .expect("REPORT_STALE_CRITICAL MUST fire for age ≥ 7d");
        assert_eq!(w.severity, WarningSeverity::Critical);
    }

    #[tokio::test]
    async fn report_with_topic_names_unavailable_emits_topic_names_unavailable_info_warning() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let report = loaded_report_with_topic(
            "personal",
            now,
            "topic_0", // placeholder label
            &[m.id],
            true, // topic_names_unavailable=true
        );
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::TopicNamesUnavailable)
            .expect("TOPIC_NAMES_UNAVAILABLE MUST fire when REPORT.topic_names_unavailable=true");
        assert_eq!(w.severity, WarningSeverity::Info);
    }

    #[tokio::test]
    async fn report_generated_at_in_future_emits_clock_skew_critical_warning() {
        let now = read_clock_now();
        let generated = now + chrono::Duration::hours(2); // 2h in the FUTURE
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let report = loaded_report_with_topic("personal", generated, "t", &[m.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::ClockSkewDetected)
            .expect("CLOCK_SKEW_DETECTED MUST fire when REPORT.generated_at > now()");
        assert_eq!(w.severity, WarningSeverity::Critical);
    }

    // ---------------------------------------------------------------------
    // Group E — Aggregate status rules
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn aggregate_status_is_ok_when_no_warnings() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        // Fresh REPORT (generated_at == now), labels available — no warning
        // surfaces.
        let report = loaded_report_with_topic("personal", now, "t", &[m.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(
            resp.health.status,
            HealthStatus::Ok,
            "no warnings MUST → HealthStatus::Ok; got warnings: {:?}",
            resp.health.warnings
        );
        assert!(resp.health.warnings.is_empty());
    }

    #[tokio::test]
    async fn aggregate_status_is_degraded_when_only_info_warnings() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        // 36h-old REPORT with topic_names_unavailable=true → two Info warnings.
        let report = loaded_report_with_topic(
            "personal",
            now - chrono::Duration::hours(36),
            "topic_0",
            &[m.id],
            true,
        );
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(resp.health.status, HealthStatus::Degraded);
        assert!(resp
            .health
            .warnings
            .iter()
            .all(|w| w.severity == WarningSeverity::Info));
    }

    #[tokio::test]
    async fn aggregate_status_is_degraded_when_warn_present_no_critical() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        // 4d-old REPORT → REPORT_STALE_WARN (severity Warn). No Critical.
        let report = loaded_report_with_topic(
            "personal",
            now - chrono::Duration::days(4),
            "t",
            &[m.id],
            false,
        );
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(resp.health.status, HealthStatus::Degraded);
        assert!(resp
            .health
            .warnings
            .iter()
            .any(|w| w.severity == WarningSeverity::Warn));
        assert!(!resp
            .health
            .warnings
            .iter()
            .any(|w| w.severity == WarningSeverity::Critical));
    }

    #[tokio::test]
    async fn aggregate_status_is_critical_when_any_critical_warning_present() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        // 14d-old REPORT → REPORT_STALE_CRITICAL (severity Critical).
        let report = loaded_report_with_topic(
            "personal",
            now - chrono::Duration::days(14),
            "t",
            &[m.id],
            false,
        );
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(
            resp.health.status,
            HealthStatus::Critical,
            "any Critical warning MUST escalate aggregate status to Critical"
        );
    }
}
