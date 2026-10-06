//! Phase 2 (LLM merge decisions) + Phase 3 (Apply merges) — BRD §5.6 lines 940-950.
//!
//! T0.2.3 commit 1 ships **Phase 2 only** — the [`decide_merge`] primitive
//! that takes a [`Cluster`] (from Phase 1) and asks the configured
//! [`LlmProvider`] to decide whether the cluster merges, stays separate,
//! or contains a contradiction. The output is a [`MergeOutcome`] enum
//! value; the orchestrator consumes it at commit 2 via the `apply_merge`
//! primitive (also landing in this module).
//!
//! Cluster member content is hydrated from `MetadataStore` at decision time
//! — BRD §5.6 line 941 verbatim "send the cluster contents to Phi-4-mini" —
//! same re-read pattern as T0.2.2 Phase 1's re-embed-at-consolidation
//! (`Memory.embedding = None` after metadata-side read; embeddings live in
//! LanceDB). For Phase 2 the cluster's `member_row_ids` are looked up by ID
//! and their `content` strings collected into the LLM prompt.
//!
//! See ADR-044 Amendment 1 (rides with T0.2.3 commit 1) for the
//! [`CompletionParams::system_prompt`] override that Phase 2 uses to swap
//! the default merge-classifier system message for the N-ary cluster
//! merge-decision system message.
//!
//! [`Cluster`]: crate::phases::cluster::Cluster
//! [`LlmProvider`]: vault_llm::LlmProvider
//! [`CompletionParams::system_prompt`]: vault_llm::CompletionParams

use crate::prompt_guard::guarded_system_prompt;
use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};
use vault_core::{MemoryId, VaultError, VaultResult};
use vault_embedding::EmbeddingProvider;
use vault_llm::{CompletionParams, LlmProvider};
use vault_storage::{MemoryFilter, StorageBackend};

use crate::phases::cluster::Cluster;

/// Outcome of one Phase 2 merge decision per BRD §5.6 line 942.
///
/// Per ADR-045 §e + T0.2.3 iteration 2 Q2 (α single-decision-per-cluster),
/// **one decision per cluster** — the LLM does not split a cluster into
/// partial merges at T0.2.3. If V0.2 dogfood shows the LLM systematically
/// wants partial splits, amend to β multi-decision shape at T0.2.3 close
/// or T0.2.7.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum MergeOutcome {
    /// LLM decided the cluster's memories should be consolidated into one.
    /// `merged_text` is the LLM-produced consolidated content; the orchestrator
    /// (Phase 3 `apply_merge`, T0.2.3 commit 2) writes a new memory with this
    /// content and marks the cluster members as `superseded_by` the new row.
    Merge {
        merged_text: String,
        reasoning: String,
    },
    /// LLM decided the cluster's memories are NOT duplicates despite the
    /// vector-similarity edge from Phase 1. No action — originals stay.
    KeepSeparate { reasoning: String },
    /// LLM decided the cluster's memories conflict (e.g., "Mom's birthday
    /// is June 15" vs "Mom's birthday is July 15"). Per BRD §5.6 line 944,
    /// the **default** behaviour is to queue a `ConflictReview` row for
    /// user review — the LLM does not auto-resolve.
    ///
    /// **Exception (T0.3.x Batch A, locked-next-arc Step 4):** when the LLM
    /// can identify a clear winner (one fact is unambiguously newer,
    /// more specific, or explicitly supersedes the other), it MAY populate
    /// `clear_winner` with the winning memory's id. The orchestrator
    /// calls [`vault_storage::StorageBackend::invalidate`] on each loser
    /// (`cluster.member_row_ids - clear_winner`) per ADR-051's bi-temporal
    /// `valid_until` semantics; retrieval thereafter skips the invalidated
    /// rows by default. The reasoning text is recorded in the invalidate
    /// audit event.
    ///
    /// When `clear_winner` is `None`, the legacy ConflictReview path
    /// remains in effect — no auto-resolution.
    Contradiction {
        reasoning: String,
        #[serde(default)]
        clear_winner: Option<MemoryId>,
    },
}

/// JSON schema (string form) the LLM is constrained to emit. GBNF-compiled
/// in `Phi4MiniProvider`; ignored by `MockLlmProvider`. Locked at T0.2.3
/// iteration 2 §Q2 (α single-decision-per-cluster).
const MERGE_DECISION_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "decision": {
            "type": "string",
            "enum": ["merge", "keep_separate", "contradiction"]
        },
        "merged_text": { "type": "string" },
        "reasoning": { "type": "string" },
        "clear_winner": {
            "type": ["string", "null"],
            "description": "For contradiction decisions ONLY: the memory_id of the unambiguously-correct fact if one exists. Null otherwise."
        }
    },
    "required": ["decision", "reasoning"],
    "additionalProperties": false
}"#;

/// System prompt for the N-ary cluster merge-decision call. Sent via
/// [`CompletionParams::system_prompt`] per ADR-044 Amendment 1 (rides with
/// T0.2.3 commit 1) — overrides `Phi4MiniProvider`'s default merge-classifier
/// system message which is pairwise-shaped and unfit for N-ary input.
const MERGE_DECISION_SYSTEM_PROMPT: &str =
    "You are a JSON-only memory consolidator. You receive a cluster of N memories \
     that a vector-similarity pass flagged as potential duplicates. Decide whether \
     to (a) merge them into one consolidated memory, (b) keep them separate \
     (vector similarity was a false positive), or (c) flag a contradiction (the \
     memories describe the same entity with conflicting facts). For contradictions, \
     if one memory is unambiguously newer / more specific / explicitly supersedes \
     the others (e.g., 'we moved to $109' explicitly replaces an older '$89'), set \
     clear_winner to that memory's id and the others will be auto-invalidated. \
     Otherwise leave clear_winner null and the user reviews. Respond with strict \
     JSON matching the schema. For merge decisions, set merged_text to the \
     consolidated content; for keep_separate and contradiction, omit merged_text.";

/// Output-token budget for the merge decision. A `Merge` echoes the
/// consolidated content in `merged_text`, so the provider default (256) is too
/// small and truncates the JSON on real merges. ~1K tokens (~4K chars) covers
/// merging near-duplicate facts (a cluster collapses to roughly one fact's
/// length, not the sum). Clusters whose merged_text still overflows are skipped
/// by the orchestrator rather than crashing the run.
const MERGE_MAX_TOKENS: u32 = 1024;

/// Phase 2 primitive — ask the LLM to classify a cluster.
///
/// **Inputs:**
/// - `cluster`: the cluster from Phase 1 ([`find_candidate_clusters`] output).
///   Members are read by [`MemoryId`]; content is hydrated from storage.
/// - `llm`: any [`LlmProvider`]. Production uses `Phi4MiniProvider`; tests
///   use `MockLlmProvider`.
/// - `storage`: source of memory-row content. The cluster carries only
///   `MemoryId`s per ADR-045 §a; this fn calls
///   [`StorageBackend::list_memories`] with a default filter and matches
///   by `id` membership.
///
/// **Output:** [`MergeOutcome`] — one of `Merge`/`KeepSeparate`/`Contradiction`.
///
/// **Errors:**
/// - [`VaultError::Storage`] propagated from `storage.list_memories`.
/// - [`VaultError::Llm`] propagated from the LLM call.
/// - [`VaultError::Llm`] wrapping a parse error if the LLM returns JSON the
///   schema accepts but our enum can't deserialize (shouldn't happen under
///   GBNF, but defense in depth per the [`LlmProvider`] trait docs).
/// - [`VaultError::Storage`] if a cluster member ID can't be found in storage
///   (would indicate a TOCTOU race between Phase 1 and Phase 2 — concurrent
///   delete; surfaced rather than silently dropped).
///
/// [`find_candidate_clusters`]: crate::find_candidate_clusters
#[instrument(skip(cluster, llm, storage), fields(cluster_id = cluster.id, cluster_size = cluster.size()))]
pub async fn decide_merge(
    cluster: &Cluster,
    llm: &dyn LlmProvider,
    storage: &StorageBackend,
) -> VaultResult<MergeOutcome> {
    // Hydrate cluster member content from storage. Cluster carries IDs only;
    // we need the .content strings to build the LLM prompt. Read all memories
    // in the workspace (filter=default) then filter by membership — at V0.2
    // scale (BRD §6.1 100-1000 memories) this is sub-millisecond; for V0.3+
    // a `MetadataStore::get_memories_by_ids` batch read would be cleaner.
    let all_memories = storage.list_memories(MemoryFilter::default(), None).await?;
    let member_set: std::collections::HashSet<MemoryId> =
        cluster.member_row_ids.iter().copied().collect();
    let mut hydrated: Vec<&vault_core::Memory> = all_memories
        .iter()
        .filter(|m| member_set.contains(&m.id))
        .collect();
    // Preserve cluster's locked-ascending ordering for deterministic prompt
    // construction (cluster.member_row_ids is sorted ascending per ADR-045 §a).
    hydrated.sort_by_key(|m| m.id);

    if hydrated.len() != cluster.size() {
        return Err(VaultError::Storage(format!(
            "Phase 2 cluster hydration mismatch: cluster has {} members, storage returned {} \
             (concurrent delete or replication lag suspected)",
            cluster.size(),
            hydrated.len()
        )));
    }

    // Build the JSON prompt body. Schema locked iteration 2 §Q1 (α structured).
    #[derive(Serialize)]
    struct PromptMemory<'a> {
        id: String,
        content: &'a str,
    }
    #[derive(Serialize)]
    struct PromptBody<'a> {
        memories: Vec<PromptMemory<'a>>,
    }
    let body = PromptBody {
        memories: hydrated
            .iter()
            .map(|m| PromptMemory {
                id: m.id.to_string(),
                content: &m.content,
            })
            .collect(),
    };
    let user_prompt = serde_json::to_string(&body)
        .map_err(|e| VaultError::Llm(format!("Phase 2 prompt JSON serialisation failed: {e}")))?;

    // Call the LLM with the N-ary merge-decision system prompt override.
    // ADR-044 Amendment 1: `system_prompt: Some(...)` swaps the provider's
    // default merge-classifier text for the N-ary cluster shape.
    //
    // `max_tokens`: a `Merge` outcome echoes the consolidated content in
    // `merged_text`, so the default 256 truncates the JSON mid-string on any
    // non-trivial merge → parse failure (observed live 2026-05-30 merging the
    // CAP_OK content-ceiling probes). MERGE_MAX_TOKENS gives normal merges room;
    // pathologically large clusters that still overflow are handled by the
    // orchestrator skipping the cluster (ADR-062 iter 2 — merge is now
    // log-and-continue, not run-aborting), so this never crashes the run.
    let params = CompletionParams {
        system_prompt: Some(guarded_system_prompt(MERGE_DECISION_SYSTEM_PROMPT)),
        max_tokens: MERGE_MAX_TOKENS,
        ..CompletionParams::default()
    };
    let raw = llm
        .complete_json(&user_prompt, MERGE_DECISION_SCHEMA, &params)
        .await
        .map_err(|e| VaultError::Llm(format!("Phase 2 LLM call failed: {e}")))?;

    // Parse the response. GBNF guarantees schema validity but enum-variant
    // mismatch is a real risk — defense in depth per LlmProvider trait
    // docs ("treats parse-failure as a hard error, NOT a retry case").
    let outcome: MergeOutcome = serde_json::from_str(&raw).map_err(|e| {
        warn!(raw_response = %raw, "Phase 2 LLM returned malformed JSON");
        VaultError::Llm(format!(
            "Phase 2 LLM response failed to parse as MergeOutcome: {e} (raw: {raw})"
        ))
    })?;

    Ok(outcome)
}

/// Phase 3 output — describes one applied merge.
///
/// Returned by [`apply_merge`] so the orchestrator (T0.2.3 commit 2's
/// [`Consolidator::run_consolidation`]) has everything it needs to build
/// `ConsolidationReport.summary_markdown` at commit 3 (new memory id,
/// superseded ids, the two aggregated fields BRD §5.6 line 947 calls out)
/// without re-reading state from storage.
///
/// [`Consolidator::run_consolidation`]: crate::Consolidator::run_consolidation
#[derive(Clone, Debug)]
pub struct AppliedMerge {
    /// The id of the newly-written merged memory. Becomes the
    /// `superseded_by` value on each cluster member.
    pub new_memory_id: MemoryId,
    /// The ids of the cluster members that were marked superseded. Same
    /// length and order as `cluster.member_row_ids` (sort-by-id-ascending
    /// per ADR-045 §a).
    pub superseded_memory_ids: Vec<MemoryId>,
    /// `Σ(member.access_count)` per BRD §5.6 line 947. Surfaced for the
    /// summary markdown.
    pub summed_access_count: u32,
    /// `max(member.confidence)` per BRD §5.6 line 947. Surfaced for the
    /// summary markdown.
    pub max_confidence: f32,
}

/// Phase 3 primitive — write the merged memory + mark originals superseded.
///
/// Implements the steps at BRD §5.6 lines 946-950:
/// 1. Create new merged memory with summed `access_count`, max `confidence`,
///    fresh `created_at`.
/// 2. Mark original memories as `superseded_by` the new one (do not delete
///    — preserve provenance).
/// 3. Re-embed the merged content, update vector store.
/// 4. Update graph: the superseded originals' relationships are retired and the
///    merged memory's are re-extracted fresh.
///
/// **Step 4 is handled downstream, not here (ADR-SEC-002 Part 2).** `apply_merge`
/// marks the originals superseded; the graph rewrite happens in
/// [`crate::Consolidator::enrich_facts`], whose reconciliation pass retires every
/// live edge whose source fact left the active set (these superseded originals),
/// and whose enrichment pass writes the merged memory's fresh edges tagged with
/// its own `source_memory_id`. Keeping all graph mutation in the enrichment pass
/// is deliberate — it is the graph's sole writer (so the sealed-snapshot flush
/// stays co-located, ADR-SEC-002).
///
/// **Inputs:**
/// - `cluster`: from Phase 1's [`find_candidate_clusters`].
/// - `merged_text`: the consolidated content from Phase 2's
///   [`decide_merge`] (the `merged_text` field of `MergeOutcome::Merge`).
/// - `merged_reasoning`: the LLM's reasoning from `MergeOutcome::Merge`.
///   Captured by the orchestrator into `AppliedMergeWithContext` for the
///   summary markdown at commit 3; apply_merge itself does not store it
///   on the merged Memory.
/// - `storage`: source of cluster-member content (re-read at apply time so
///   we operate on current state) + sink for the new merged memory + the
///   supersession metadata updates (via ADR-046's `mark_superseded`).
/// - `embeddings`: re-embed primitive for the merged content per step 3.
///
/// **Output:** [`AppliedMerge`] with the new merged memory id + the list of
/// superseded original ids + the aggregated fields for the summary
/// markdown.
///
/// **Errors:**
/// - [`VaultError::Storage`] propagated from `list_memories` / `write_memory`
///   / `mark_superseded`.
/// - [`VaultError::Embedding`] propagated from `embeddings.embed`.
/// - [`VaultError::Storage`] if the cluster's members can't all be hydrated
///   from storage (concurrent delete or replication lag suspected).
///
/// [`find_candidate_clusters`]: crate::find_candidate_clusters
#[instrument(
    skip(cluster, merged_text, merged_reasoning, storage, embeddings),
    fields(cluster_id = cluster.id, cluster_size = cluster.size())
)]
pub async fn apply_merge(
    cluster: &Cluster,
    merged_text: &str,
    merged_reasoning: &str,
    storage: &StorageBackend,
    embeddings: &dyn EmbeddingProvider,
) -> VaultResult<AppliedMerge> {
    // Reasoning is captured by the orchestrator into AppliedMergeWithContext
    // for commit 3's summary markdown; apply_merge doesn't store it on the
    // merged Memory.
    let _ = merged_reasoning;

    // 1. Hydrate cluster members from storage. Cluster carries IDs only;
    //    we need the full Memory rows for access_count + confidence +
    //    memory_type + boundary. Same re-read pattern as decide_merge.
    let all_memories = storage.list_memories(MemoryFilter::default(), None).await?;
    let member_set: std::collections::HashSet<MemoryId> =
        cluster.member_row_ids.iter().copied().collect();
    let mut hydrated: Vec<vault_core::Memory> = all_memories
        .into_iter()
        .filter(|m| member_set.contains(&m.id))
        .collect();
    // Sort by id ascending so "first member" is deterministic. Matches
    // decide_merge's hydration ordering (cluster.member_row_ids is also
    // sorted ascending per ADR-045 §a).
    hydrated.sort_by_key(|m| m.id);

    if hydrated.len() != cluster.size() {
        return Err(VaultError::Storage(format!(
            "Phase 3 cluster hydration mismatch: cluster has {} members, \
             storage returned {} (concurrent delete or replication lag suspected)",
            cluster.size(),
            hydrated.len()
        )));
    }

    // 2. Aggregate fields per BRD §5.6 line 947.
    let summed_access_count: u32 = hydrated.iter().map(|m| m.access_count).sum();
    let max_confidence: f32 = hydrated
        .iter()
        .map(|m| m.confidence)
        .fold(0.0_f32, f32::max);

    // 3. Construct the merged Memory. `memory_type` is carried from the first
    //    cluster member after sort-by-id-ascending. BRD §5.6 §946-950 is
    //    silent on memory_type for the merged memory — first-member-by-id
    //    is deterministic and matches the typical semantic-cluster type
    //    homogeneity (a Phase 1 similarity cluster won't mix Procedural with
    //    Episodic memories at V0.2 scale; if it ever does, V0.3+ revisits
    //    via a mode-or-most-recent tiebreaker).
    let first = hydrated.first().expect("cluster.size() >= 2 invariant");
    let mut merged_memory = vault_core::Memory::try_new(vault_core::NewMemory {
        content: merged_text.to_string(),
        memory_type: first.memory_type,
        boundary: first.boundary.clone(),
        source_agent: None, // Consolidator runs as system per BRD §5.6 line 901.
        confidence: max_confidence,
        valid_from: None, // Defaults to now in Memory::try_new.
        valid_until: None,
        metadata: serde_json::json!({}),
    })?;
    // access_count is system-managed and try_new initialises it to 0;
    // override per BRD §5.6 line 947 "summed access_count."
    merged_memory.access_count = summed_access_count;
    let new_memory_id = merged_memory.id;

    // 4. Re-embed the merged content per BRD §5.6 line 949.
    let embedding = embeddings.embed(merged_text).await?;

    // 5. Write the merged memory via the cascade (DOES touch the vector
    //    store — the new memory needs its embedding in LanceDB for search).
    storage.write_memory(&merged_memory, &embedding).await?;

    // 6. Mark each original as superseded by the merged memory via the
    //    new mark_superseded primitive (ADR-046). Metadata-only, no
    //    cascade, emits MemorySuperseded audit events.
    let mut superseded_memory_ids = Vec::with_capacity(hydrated.len());
    for member in &hydrated {
        storage.mark_superseded(member.id, new_memory_id).await?;
        superseded_memory_ids.push(member.id);
    }

    // 7. Graph update is NOT done here (ADR-SEC-002 Part 2). The superseded
    //    originals' edges are retired by `Consolidator::enrich_facts`'s
    //    reconciliation pass (every live edge whose source fact left the active
    //    set), and the merged memory's fresh edges are written by the same
    //    enrichment pass — keeping the enrichment pass the graph's sole writer
    //    (so the sealed-snapshot flush stays co-located).

    Ok(AppliedMerge {
        new_memory_id,
        superseded_memory_ids,
        summed_access_count,
        max_confidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use vault_core::{Boundary, Memory, MemoryType, NewMemory};
    use vault_embedding::EMBEDDING_DIM;
    use vault_llm::MockLlmProvider;
    use vault_storage::{RetryWorker, SqlCipherKey, StepResult};

    /// Test-only at-rest key. Matches the cross-crate convention from
    /// `vault-storage/tests/migration_v0_1_to_sealed.rs:96`.
    const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

    async fn open_test_storage() -> (StorageBackend, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = SqlCipherKey::new("phase2-decide-merge-test");
        let storage = StorageBackend::open_with_at_rest_key(
            &dir.path().join("metadata.db"),
            &dir.path().join("vectors"),
            &dir.path().join("graph.duckdb"),
            key,
            EMBEDDING_DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .expect("open StorageBackend");
        (storage, dir)
    }

    fn make_memory(content: &str, boundary: &Boundary) -> Memory {
        Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: boundary.clone(),
            source_agent: None,
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("valid memory")
    }

    /// Insert memories via cascading write then drain so list_memories sees
    /// the committed SQLite-side rows. Phase 2 doesn't need vector-side
    /// readability — only metadata.content — but the cascade path is the
    /// realistic insertion shape.
    async fn insert_and_drain(storage: &StorageBackend, memories: &[Memory]) -> Vec<MemoryId> {
        let mut ids = Vec::new();
        let embedding = vec![1.0_f32 / (EMBEDDING_DIM as f32).sqrt(); EMBEDDING_DIM];
        for memory in memories {
            ids.push(memory.id);
            storage
                .write_memory(memory, &embedding)
                .await
                .expect("write_memory");
        }
        let mut worker = RetryWorker::new(storage.clone());
        let drain_at = Utc::now() + chrono::Duration::seconds(60);
        for _ in 0..(memories.len() * 2 + 10) {
            match worker.step_at(drain_at).await.expect("worker step") {
                StepResult::Idle => break,
                StepResult::SucceededEntry { .. } => continue,
                other => panic!("unexpected worker outcome: {other:?}"),
            }
        }
        ids
    }

    fn cluster_of(id: u32, member_ids: Vec<MemoryId>) -> Cluster {
        let mut sorted = member_ids;
        sorted.sort();
        Cluster {
            id,
            member_row_ids: sorted,
        }
    }

    // ─── floor 1: merge outcome round-trip ────────────────────────────────

    #[tokio::test]
    async fn decide_merge_returns_merge_outcome_on_merge_response() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let memories = vec![
            make_memory("Buy milk today", &boundary),
            make_memory("Buy milk later", &boundary),
        ];
        let ids = insert_and_drain(&storage, &memories).await;
        let cluster = cluster_of(0, ids);

        let mock = MockLlmProvider::new(
            "test-mock",
            r#"{"decision":"merge","merged_text":"Buy milk","reasoning":"identical intent"}"#,
        );
        let outcome = decide_merge(&cluster, &mock, &storage).await.unwrap();
        match outcome {
            MergeOutcome::Merge {
                merged_text,
                reasoning,
            } => {
                assert_eq!(merged_text, "Buy milk");
                assert_eq!(reasoning, "identical intent");
            }
            other => panic!("expected Merge, got {other:?}"),
        }
        assert_eq!(mock.call_count(), 1, "exactly one LLM call per cluster");
    }

    // ─── floor 2: keep_separate outcome round-trip ────────────────────────

    #[tokio::test]
    async fn decide_merge_returns_keep_separate_on_keep_response() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let memories = vec![
            make_memory("Buy milk today", &boundary),
            make_memory("Dentist appointment Tuesday", &boundary),
        ];
        let ids = insert_and_drain(&storage, &memories).await;
        let cluster = cluster_of(0, ids);

        let mock = MockLlmProvider::new(
            "test-mock",
            r#"{"decision":"keep_separate","reasoning":"unrelated topics"}"#,
        );
        let outcome = decide_merge(&cluster, &mock, &storage).await.unwrap();
        assert!(matches!(outcome, MergeOutcome::KeepSeparate { .. }));
    }

    // ─── floor 3: contradiction outcome round-trip ────────────────────────

    #[tokio::test]
    async fn decide_merge_returns_contradiction_on_contradiction_response() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let memories = vec![
            make_memory("Mom's birthday is June 15", &boundary),
            make_memory("Mom's birthday is July 15", &boundary),
        ];
        let ids = insert_and_drain(&storage, &memories).await;
        let cluster = cluster_of(0, ids);

        let mock = MockLlmProvider::new(
            "test-mock",
            r#"{"decision":"contradiction","reasoning":"same person, conflicting dates"}"#,
        );
        let outcome = decide_merge(&cluster, &mock, &storage).await.unwrap();
        assert!(matches!(outcome, MergeOutcome::Contradiction { .. }));
    }

    // ─── floor 4: malformed JSON surfaces as VaultError::Llm ──────────────

    #[tokio::test]
    async fn decide_merge_surfaces_malformed_json_as_llm_error() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let memories = vec![
            make_memory("Buy milk today", &boundary),
            make_memory("Buy milk later", &boundary),
        ];
        let ids = insert_and_drain(&storage, &memories).await;
        let cluster = cluster_of(0, ids);

        let mock = MockLlmProvider::new("test-mock", "not even close to JSON");
        let err = decide_merge(&cluster, &mock, &storage).await.unwrap_err();
        assert!(
            matches!(err, VaultError::Llm(_)),
            "expected VaultError::Llm, got {err:?}"
        );
    }

    // ─── T0.3.x Batch A: MergeOutcome::Contradiction::clear_winner pins ──
    //
    // Backwards-compat + present-path serde behaviour. The orchestrator's
    // dispatch on `clear_winner.is_some()` (auto-invalidate via ADR-051) vs
    // `None` (queue ConflictReview) is exercised end-to-end via the
    // integration tests at Batch B; these unit tests pin only the JSON
    // deserialisation contract.

    #[test]
    fn merge_outcome_contradiction_clear_winner_defaults_to_none_when_field_absent() {
        // Pre-T0.3.x JSON: no `clear_winner` field. With `#[serde(default)]`
        // on the field, deserialisation MUST default to None — keeps every
        // existing fixture + canned LLM response working unchanged.
        let json = r#"{"decision":"contradiction","reasoning":"same person, conflicting dates"}"#;
        let outcome: MergeOutcome =
            serde_json::from_str(json).expect("legacy contradiction JSON MUST still parse");
        match outcome {
            MergeOutcome::Contradiction {
                reasoning,
                clear_winner,
            } => {
                assert_eq!(reasoning, "same person, conflicting dates");
                assert_eq!(
                    clear_winner, None,
                    "missing clear_winner MUST default to None for backwards-compat"
                );
            }
            other => panic!("expected Contradiction, got: {other:?}"),
        }
    }

    #[test]
    fn merge_outcome_contradiction_deserializes_clear_winner_when_field_present() {
        // T0.3.x Batch A path: LLM populated clear_winner with the
        // winning memory's UUID. Orchestrator at consolidator.rs branches
        // on this being Some to invoke `storage.invalidate(loser, now, ...)`.
        let winner_uuid = "00000000-0000-0000-0000-000000000042";
        let json = format!(
            r#"{{"decision":"contradiction","reasoning":"newer bill supersedes older","clear_winner":"{winner_uuid}"}}"#
        );
        let outcome: MergeOutcome =
            serde_json::from_str(&json).expect("clear_winner-present JSON MUST parse");
        match outcome {
            MergeOutcome::Contradiction {
                reasoning,
                clear_winner,
            } => {
                assert_eq!(reasoning, "newer bill supersedes older");
                assert_eq!(
                    clear_winner,
                    Some(MemoryId(uuid::Uuid::parse_str(winner_uuid).unwrap())),
                    "clear_winner MUST round-trip as the supplied MemoryId"
                );
            }
            other => panic!("expected Contradiction, got: {other:?}"),
        }
    }

    #[test]
    fn merge_outcome_contradiction_clear_winner_explicit_null_is_equivalent_to_absent() {
        // Phi-4's GBNF grammar may emit `"clear_winner": null` rather than
        // omitting the field. Both shapes MUST deserialise to None.
        let json = r#"{"decision":"contradiction","reasoning":"r","clear_winner":null}"#;
        let outcome: MergeOutcome =
            serde_json::from_str(json).expect("explicit-null clear_winner MUST parse");
        match outcome {
            MergeOutcome::Contradiction { clear_winner, .. } => {
                assert_eq!(clear_winner, None, "explicit null MUST deserialise to None");
            }
            other => panic!("expected Contradiction, got: {other:?}"),
        }
    }

    // ─── floor 5: N-boundary cases (N=2 / N=5 / N=10) dispatch one call ──

    #[tokio::test]
    async fn decide_merge_handles_cluster_size_n2_n5_n10_with_one_llm_call() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        // Build 10 memories so we can slice into N=2, N=5, N=10 clusters.
        let memories: Vec<Memory> = (0..10)
            .map(|i| make_memory(&format!("Buy milk variant {i}"), &boundary))
            .collect();
        let ids = insert_and_drain(&storage, &memories).await;

        for n in [2usize, 5, 10] {
            let cluster = cluster_of(0, ids[..n].to_vec());
            let mock = MockLlmProvider::new(
                "test-mock",
                r#"{"decision":"merge","merged_text":"Buy milk","reasoning":"N-ary cluster"}"#,
            );
            let outcome = decide_merge(&cluster, &mock, &storage).await.unwrap();
            assert!(
                matches!(outcome, MergeOutcome::Merge { .. }),
                "N={n} cluster should merge"
            );
            assert_eq!(
                mock.call_count(),
                1,
                "N={n} cluster must produce exactly one LLM call (single-decision shape per Q2)"
            );
        }
    }

    // ───────────────────────────────────────────────────────────────────
    // Phase 3 — apply_merge (T0.2.3 commit 2, BRD §5.6 lines 946-950)
    // ───────────────────────────────────────────────────────────────────

    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use vault_embedding::EmbeddingProvider;

    /// Test-only embedder that records each call's text + count. Mirrors
    /// the vault-retrieval/tests/common/mod.rs:36-85 pattern but defined
    /// locally here per the T0.2.3 commit-2 opener's "define a local stub"
    /// lean.
    struct StubEmbedder {
        last_text: tokio::sync::Mutex<Option<String>>,
        call_count: AtomicU64,
    }

    impl StubEmbedder {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                last_text: tokio::sync::Mutex::new(None),
                call_count: AtomicU64::new(0),
            })
        }

        async fn last(&self) -> Option<String> {
            self.last_text.lock().await.clone()
        }

        fn calls(&self) -> u64 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, text: &str) -> VaultResult<Vec<f32>> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            *self.last_text.lock().await = Some(text.to_string());
            // Unit-norm vector — eager_validate accepts it (non-empty,
            // finite, correct dimension).
            let v = vec![1.0_f32 / (EMBEDDING_DIM as f32).sqrt(); EMBEDDING_DIM];
            Ok(v)
        }
    }

    /// Insert a memory with custom `access_count` + `confidence` so Phase 3
    /// tests can exercise the aggregation math without needing a cascade
    /// drain (apply_merge reads metadata-side state only; LanceDB is not
    /// consulted for originals).
    async fn insert_with_overrides(
        storage: &StorageBackend,
        boundary: &Boundary,
        content: &str,
        confidence: f32,
        access_count: u32,
    ) -> MemoryId {
        let mut m = Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: boundary.clone(),
            source_agent: None,
            confidence,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("valid memory");
        m.access_count = access_count;
        let embedding = vec![1.0_f32 / (EMBEDDING_DIM as f32).sqrt(); EMBEDDING_DIM];
        storage
            .write_memory(&m, &embedding)
            .await
            .expect("write_memory");
        m.id
    }

    // ─── floor 1: writes_merged_memory_and_returns_id ─────────────────

    #[tokio::test]
    async fn apply_merge_writes_merged_memory_and_returns_id() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let id1 = insert_with_overrides(&storage, &boundary, "milk a", 0.7, 1).await;
        let id2 = insert_with_overrides(&storage, &boundary, "milk b", 0.8, 2).await;
        let id3 = insert_with_overrides(&storage, &boundary, "milk c", 0.9, 3).await;
        let cluster = cluster_of(0, vec![id1, id2, id3]);

        let embedder = StubEmbedder::new();
        let applied = apply_merge(
            &cluster,
            "Buy milk",
            "all three were milk-shopping reminders",
            &storage,
            embedder.as_ref(),
        )
        .await
        .unwrap();

        // The new merged memory id + the three superseded ids are returned.
        assert_eq!(applied.superseded_memory_ids.len(), 3);
        assert!(applied.superseded_memory_ids.contains(&id1));
        assert!(applied.superseded_memory_ids.contains(&id2));
        assert!(applied.superseded_memory_ids.contains(&id3));

        // All three originals now have superseded_by == new_id (read with
        // include_superseded=true since the default filter excludes them).
        let all = storage
            .list_memories(
                MemoryFilter {
                    include_superseded: true,
                    ..MemoryFilter::default()
                },
                None,
            )
            .await
            .unwrap();
        for original_id in [id1, id2, id3] {
            let m = all
                .iter()
                .find(|m| m.id == original_id)
                .expect("original must still exist (superseded, not deleted)");
            assert_eq!(m.superseded_by, Some(applied.new_memory_id));
        }
    }

    // ─── floor 2: sums_access_count_across_members ────────────────────

    #[tokio::test]
    async fn apply_merge_sums_access_count_across_members() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        // 5 + 10 + 15 = 30
        let id1 = insert_with_overrides(&storage, &boundary, "a", 0.5, 5).await;
        let id2 = insert_with_overrides(&storage, &boundary, "b", 0.5, 10).await;
        let id3 = insert_with_overrides(&storage, &boundary, "c", 0.5, 15).await;
        let cluster = cluster_of(0, vec![id1, id2, id3]);

        let embedder = StubEmbedder::new();
        let applied = apply_merge(&cluster, "merged", "", &storage, embedder.as_ref())
            .await
            .unwrap();

        assert_eq!(
            applied.summed_access_count, 30,
            "BRD §5.6 line 947: summed access_count"
        );
    }

    // ─── floor 3: takes_max_confidence_across_members ─────────────────

    #[tokio::test]
    async fn apply_merge_takes_max_confidence_across_members() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let id1 = insert_with_overrides(&storage, &boundary, "a", 0.6, 0).await;
        let id2 = insert_with_overrides(&storage, &boundary, "b", 0.9, 0).await;
        let id3 = insert_with_overrides(&storage, &boundary, "c", 0.7, 0).await;
        let cluster = cluster_of(0, vec![id1, id2, id3]);

        let embedder = StubEmbedder::new();
        let applied = apply_merge(&cluster, "merged", "", &storage, embedder.as_ref())
            .await
            .unwrap();

        assert!(
            (applied.max_confidence - 0.9).abs() < 1e-6,
            "BRD §5.6 line 947: max confidence (expected 0.9, got {})",
            applied.max_confidence
        );
    }

    // ─── floor 4: re_embeds_merged_text_via_provider ──────────────────

    #[tokio::test]
    async fn apply_merge_re_embeds_merged_text_via_provider() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let id1 = insert_with_overrides(&storage, &boundary, "a", 0.5, 0).await;
        let id2 = insert_with_overrides(&storage, &boundary, "b", 0.5, 0).await;
        let cluster = cluster_of(0, vec![id1, id2]);

        let embedder = StubEmbedder::new();
        let merged_text = "consolidated content for re-embedding";
        apply_merge(&cluster, merged_text, "", &storage, embedder.as_ref())
            .await
            .unwrap();

        // Exactly one embed call — the merged text. Originals are NOT
        // re-embedded (mark_superseded is metadata-only per ADR-046).
        assert_eq!(
            embedder.calls(),
            1,
            "apply_merge must call embeddings.embed exactly once (for merged_text); \
             originals are marked superseded via mark_superseded (no re-embed)"
        );
        assert_eq!(embedder.last().await, Some(merged_text.to_string()));
    }

    // ─── floor 5: no longer defers the graph update (ADR-SEC-002 Part 2) ──

    /// The graph rewrite is no longer deferred at merge time — it moved to
    /// `Consolidator::enrich_facts` (retire the superseded originals' edges +
    /// re-extract the merged memory's). `apply_merge` must therefore NOT emit
    /// the old "graph update deferred to T0.2.x" WARN. (The positive
    /// retirement behaviour is tested in
    /// `consolidator::enrich_facts_reconciliation_retires_edges_of_superseded_fact`.)
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn apply_merge_no_longer_emits_graph_deferral_warn() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let id1 = insert_with_overrides(&storage, &boundary, "a", 0.5, 0).await;
        let id2 = insert_with_overrides(&storage, &boundary, "b", 0.5, 0).await;
        let cluster = cluster_of(0, vec![id1, id2]);

        let embedder = StubEmbedder::new();
        apply_merge(&cluster, "merged", "", &storage, embedder.as_ref())
            .await
            .unwrap();

        assert!(
            !logs_contain("graph update deferred"),
            "apply_merge must NOT emit the obsolete graph-deferral WARN; \
             the rewrite now happens in enrich_facts (ADR-SEC-002 Part 2)"
        );
    }
}
