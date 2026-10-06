//! Keyword (BM25) retrieval strategy — vault-retrieval's lexical channel.
//!
//! [`KeywordIndex`] owns a Tantivy in-RAM full-text index over memory
//! content. [`KeywordRetriever`] is the [`crate::Retriever`] trait impl
//! that BM25-searches the index, hydrates memories from the SQLite
//! metadata store, applies boundary filtering, and returns ranked
//! `RetrievedMemory`s.
//!
//! ## V0.2 design (T0.2.7 Phase 1)
//!
//! **In-memory only.** The index is rebuilt at startup from
//! [`MetadataStore`] via [`KeywordIndex::bulk_insert`]. No on-disk sealed
//! sidecar at Phase 1 — defers sealed-storage complexity until profiling
//! shows the startup-rebuild cost matters (~1 sec at 10K memories, ~10
//! sec at 100K; the V0.2 beta target of 30 users is well below the
//! crossover).
//!
//! **Async API via `tokio::sync::Mutex<IndexWriter>`.** Tantivy is sync,
//! but in-RAM index ops are sub-millisecond at V0.2 beta scale — we hold
//! the writer mutex briefly across the commit + reader-reload sequence
//! and never block a non-tokio-aware blocking call. A future
//! profiling-driven enhancement may move this to `spawn_blocking` per
//! BRD §2.7 if commit costs grow.
//!
//! **Schema** — two fields:
//!
//! - `content`: tokenized full-text index for BM25 search using the
//!   `vault_text` analyzer — `SimpleTokenizer` → `RemoveLongFilter(40)`
//!   → `LowerCaser` → `StopWordFilter(English)`. The stopword filter is
//!   load-bearing: without it, Q21-style hard-neg queries
//!   ("What did we decide about the Kubernetes migration?") accumulate
//!   BM25 score from common-word matches on long natural-prose memories
//!   that contain zero content-word matches, leaking past the
//!   [`crate::AbstainingRetriever`] gate. See T0.2.7 Phase 5 Step 2
//!   diagnostic (2026-05-21) and `tests/abstain_q21_focused.rs`.
//! - `memory_id` ([`STRING`] + [`STORED`]): UUID, exact-match (untokenized)
//!   so [`Term::from_field_text`] can target it for `delete_term`, AND
//!   stored so we can round-trip the id from a search hit without an
//!   external lookup table.
//!
//! **Query sanitization.** Lucene operator characters (`'`, `+`, `-`,
//! `:`, `"`, `(`, `)`, `[`, `]`, `{`, `}`, `^`, `~`, `*`, `?`, `\`, `/`,
//! `!`, `&`, `|`) are stripped before parsing. Confirmed empirically by
//! the T0.2.7 spike: SCALE=100 smoke run failed `parse_query: Syntax Error`
//! on 4/9 queries because of `'` in `What's`. For natural-language
//! queries we want pure term-token matching; operator semantics aren't
//! intentionally exposed at this surface.
//!
//! **Boundary filter** is applied post-hydration in [`KeywordRetriever`].
//! Tantivy filter-clause optimisation is deferred — at V0.2 beta scale
//! the hydration cost dominates regardless.
//!
//! **Reader reload policy** is [`ReloadPolicy::Manual`]; we explicitly
//! call `reader.reload()` after each commit so post-mutation reads see
//! the new state deterministically (no auto-reload-delay races in
//! tests).
//!
//! ## What this module does NOT do (deferred to later phases)
//!
//! - No hybrid fusion with semantic — Phase 2.
//! - No top-1 BM25 abstain gate — Phase 3.
//! - No on-disk persistence / sealed sidecar — deferred indefinitely.
//! - No vault-app write-path wiring — Phase 1.5 / Phase 4 (depending on
//!   when [`crate::ReadPipeline`] gets exposed via MCP).

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, STORED, STRING,
};
use tantivy::tokenizer::{
    Language, LowerCaser, RemoveLongFilter, SimpleTokenizer, StopWordFilter, TextAnalyzer,
    TokenStream,
};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};
use tokio::sync::Mutex;
use tracing::instrument;

use vault_core::{Memory, MemoryId, VaultError, VaultResult};
use vault_storage::MetadataStore;

use crate::retriever::{
    RetrievalQuery, RetrievedMemory, Retriever, MAX_QUERY_BYTES, MAX_RESULTS_CAP,
};

/// IndexWriter heap budget. Tantivy 0.26 mandates ≥15 MB; 100 MB matches
/// the T0.2.7 spike's tested-known-good value for 10K-doc workloads
/// (see `examples/t028g_hybrid_retrieval_spike.rs::build_bm25_index`).
const WRITER_HEAP_BYTES: usize = 100_000_000;

/// Name under which the vault's stopword-filtered tokenizer is registered
/// with the Tantivy [`Index`]. Referenced by the `content` field's
/// [`TextFieldIndexing::set_tokenizer`] so indexing AND search both apply
/// the same analyzer chain.
const VAULT_TOKENIZER_NAME: &str = "vault_text";

/// Tantivy/Lucene operator characters stripped from natural-language
/// queries before parsing. Empirical basis: spike apostrophe failures
/// at SCALE=100 smoke (T0.2.7 Phase 0.b, 2026-05-19).
const LUCENE_OPERATOR_CHARS: &[char] = &[
    '+', '-', '!', '&', '|', '(', ')', '{', '}', '[', ']', '^', '~', '*', '?', ':', '\\', '/', '"',
    '\'',
];

/// Tantivy-backed BM25 full-text index over memory content. In-memory
/// only at Phase 1 — vault-app rebuilds via [`Self::bulk_insert`] at
/// startup from the (sealed) metadata store.
///
/// Cheap to clone — wraps the underlying handles in `Arc` so cloning
/// is just refcount bumps. Share freely across tasks.
///
/// Per ADR-007 precedent: does NOT implement `Debug` (holds live index
/// handles).
#[derive(Clone)]
pub struct KeywordIndex {
    index: Index,
    reader: IndexReader,
    writer: Arc<Mutex<IndexWriter>>,
    content_field: Field,
    memory_id_field: Field,
}

impl KeywordIndex {
    /// Create a fresh, empty in-RAM index with the V0.2 schema.
    pub fn new() -> VaultResult<Self> {
        let mut schema_builder = Schema::builder();
        let content_options = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(VAULT_TOKENIZER_NAME)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );
        let content_field = schema_builder.add_text_field("content", content_options);
        // STRING + STORED: untokenized so `Term::from_field_text` matches
        // the full UUID exactly; stored so search hits round-trip the id
        // back without an external `corpus_idx_lookup` Vec.
        let memory_id_field = schema_builder.add_text_field("memory_id", STRING | STORED);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema);

        // Register the vault tokenizer: SimpleTokenizer → RemoveLong(40)
        // → LowerCaser → StopWordFilter(English). The first three steps
        // mirror Tantivy's `default` analyzer; the stopword filter (Lucene
        // standard 33-word English list) is the V0.2 addition. See module
        // docs for the empirical basis (T0.2.7 Phase 5 Step 2 Q21 leak).
        let stopword_filter = StopWordFilter::new(Language::English).ok_or_else(|| {
            vault_err(
                "StopWordFilter::new(Language::English)",
                "Tantivy 0.26.1 returned None for the English stopword list",
            )
        })?;
        let analyzer = TextAnalyzer::builder(SimpleTokenizer::default())
            .filter(RemoveLongFilter::limit(40))
            .filter(LowerCaser)
            .filter(stopword_filter)
            .build();
        index.tokenizers().register(VAULT_TOKENIZER_NAME, analyzer);

        let writer = index
            .writer(WRITER_HEAP_BYTES)
            .map_err(|e| vault_err("IndexWriter::writer", e))?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| vault_err("IndexReader build", e))?;

        Ok(Self {
            index,
            reader,
            writer: Arc::new(Mutex::new(writer)),
            content_field,
            memory_id_field,
        })
    }

    /// Bulk-load memories at startup. Single writer-lock + many
    /// `add_document` + one commit + one reader reload — minimises
    /// overhead vs N individual inserts. Existing index contents are
    /// preserved (this is additive). For a clean rebuild call
    /// [`Self::new`] and discard the prior index.
    ///
    /// No-op (returns `Ok(())`) when `memories` is empty.
    #[instrument(skip(self, memories), fields(count = memories.len()))]
    pub async fn bulk_insert(&self, memories: &[Memory]) -> VaultResult<()> {
        if memories.is_empty() {
            return Ok(());
        }

        let content_field = self.content_field;
        let memory_id_field = self.memory_id_field;

        let mut writer_guard = self.writer.lock().await;
        for m in memories {
            let id_str = m.id.to_string();
            writer_guard
                .add_document(doc!(
                    content_field => m.content.as_str(),
                    memory_id_field => id_str.as_str(),
                ))
                .map_err(|e| vault_err("add_document", e))?;
        }
        writer_guard.commit().map_err(|e| vault_err("commit", e))?;
        drop(writer_guard);

        self.reader
            .reload()
            .map_err(|e| vault_err("reader reload", e))?;
        Ok(())
    }

    /// Insert (or replace) one memory in the index. Semantically
    /// equivalent to [`Self::upsert`]: identical content yields a
    /// single hit; new content for the same id replaces the old.
    /// Provided as a separate entry point so callers can express
    /// intent at the write site (insert vs explicit replace).
    pub async fn insert(&self, id: MemoryId, content: &str) -> VaultResult<()> {
        self.upsert(id, content).await
    }

    /// Replace any existing index entry for `id` with one carrying
    /// `content`. Tantivy has no native upsert primitive — we
    /// delete-by-term then add fresh, all under one writer lock + one
    /// commit + one reload.
    #[instrument(skip(self, content))]
    pub async fn upsert(&self, id: MemoryId, content: &str) -> VaultResult<()> {
        let id_str = id.to_string();
        let content_field = self.content_field;
        let memory_id_field = self.memory_id_field;
        let term = Term::from_field_text(memory_id_field, &id_str);

        let mut writer_guard = self.writer.lock().await;
        writer_guard.delete_term(term);
        writer_guard
            .add_document(doc!(
                content_field => content,
                memory_id_field => id_str.as_str(),
            ))
            .map_err(|e| vault_err("add_document", e))?;
        writer_guard.commit().map_err(|e| vault_err("commit", e))?;
        drop(writer_guard);

        self.reader
            .reload()
            .map_err(|e| vault_err("reader reload", e))?;
        Ok(())
    }

    /// Delete the index entry for `id`. Idempotent: deleting an absent
    /// id is not an error (matches [`vault_storage::VectorStore::delete`]
    /// semantics).
    #[instrument(skip(self))]
    pub async fn delete(&self, id: MemoryId) -> VaultResult<()> {
        let id_str = id.to_string();
        let memory_id_field = self.memory_id_field;
        let term = Term::from_field_text(memory_id_field, &id_str);

        let mut writer_guard = self.writer.lock().await;
        writer_guard.delete_term(term);
        writer_guard.commit().map_err(|e| vault_err("commit", e))?;
        drop(writer_guard);

        self.reader
            .reload()
            .map_err(|e| vault_err("reader reload", e))?;
        Ok(())
    }

    /// Returns `true` if `text` yields at least one token under the
    /// `vault_text` analyzer (SimpleTokenizer → RemoveLong → LowerCaser →
    /// StopWordFilter).
    ///
    /// A query that reduces to zero tokens — e.g. an all-stopword query
    /// like `"the a is of and to"` — has no searchable terms. Tantivy
    /// 0.26.1's `QueryParser` does NOT return an empty query for this; it
    /// rejects it with `AllButQueryForbidden` ("Only excluding terms
    /// given"), which we wrap as `VaultError::Storage` (→ JSON-RPC
    /// `-32603 internal error` at the MCP boundary). Callers short-circuit
    /// on `!has_searchable_terms(..)` BEFORE `parse_query` so a degenerate
    /// query is a graceful empty result, not an internal error. Surfaced
    /// in §7 live dogfood 2026-05-30.
    fn has_searchable_terms(&self, text: &str) -> bool {
        match self.index.tokenizers().get(VAULT_TOKENIZER_NAME) {
            Some(mut analyzer) => analyzer.token_stream(text).advance(),
            // The tokenizer is always registered in `new()`. If it were
            // somehow absent we don't suppress the query — fall through to
            // the parser so any genuine error still surfaces. Defensive.
            None => true,
        }
    }

    /// BM25 search for `query` over indexed content. Returns up to
    /// `limit` `(MemoryId, score)` pairs in descending score order.
    /// Empty / whitespace queries, `limit == 0`, and queries with no
    /// searchable terms (all-stopword / all-punctuation) short-circuit to
    /// an empty `Vec` without error.
    ///
    /// Query text is sanitised (Lucene operator chars stripped) before
    /// parsing — see module-level docs for the empirical basis.
    #[instrument(skip(self, query), fields(limit, query_len = query.len()))]
    pub async fn search(&self, query: &str, limit: usize) -> VaultResult<Vec<(MemoryId, f32)>> {
        let trimmed = query.trim();
        if trimmed.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let sanitized = sanitize_query(trimmed);
        if sanitized.trim().is_empty() {
            return Ok(Vec::new());
        }
        // Degenerate-query guard: a query that tokenizes to zero terms
        // under the stopword-filtered analyzer (e.g. "the a is of and to")
        // is rejected by `parse_query` as AllButQueryForbidden. Treat it
        // as "no searchable terms → matches nothing" — see
        // `has_searchable_terms`.
        if !self.has_searchable_terms(&sanitized) {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();
        let parser = QueryParser::for_index(&self.index, vec![self.content_field]);
        let parsed = parser
            .parse_query(&sanitized)
            // The parser's message can quote the query, and errors reach the
            // log file: name the step only (ADR-SEC-014; audit s89).
            .map_err(|_| vault_err("parse_query", "the query could not be parsed"))?;

        let top: Vec<(f32, tantivy::DocAddress)> = searcher
            .search(&parsed, &TopDocs::with_limit(limit).order_by_score())
            .map_err(|e| vault_err("search", e))?;

        let memory_id_field = self.memory_id_field;
        let mut out: Vec<(MemoryId, f32)> = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let tdoc: TantivyDocument = searcher
                .doc(addr)
                .map_err(|e| vault_err("searcher.doc", e))?;
            let Some(id_str) = tdoc.get_first(memory_id_field).and_then(|v| v.as_str()) else {
                continue;
            };
            let Ok(id) = MemoryId::from_str(id_str) else {
                continue;
            };
            out.push((id, score));
        }
        Ok(out)
    }

    /// Number of documents currently indexed. Cheap snapshot from a
    /// fresh searcher. Used by tests + telemetry.
    pub async fn len(&self) -> VaultResult<usize> {
        let searcher = self.reader.searcher();
        Ok(searcher.num_docs() as usize)
    }

    /// `len() == 0`. Convenience for callers.
    pub async fn is_empty(&self) -> VaultResult<bool> {
        Ok(self.len().await? == 0)
    }
}

/// BM25 retrieval strategy. Wraps a [`KeywordIndex`] handle plus a
/// metadata-store handle for hydration. Implements [`Retriever`] so it
/// composes with [`crate::SemanticRetriever`] under future hybrid
/// fusion (Phase 2).
///
/// Per ADR-007 / [`crate::SemanticRetriever`] precedent: does NOT
/// implement `Debug` (holds live storage handles).
#[derive(Clone)]
pub struct KeywordRetriever {
    index: Arc<KeywordIndex>,
    metadata_store: Arc<MetadataStore>,
}

impl KeywordRetriever {
    /// Construct a new retriever from a shared `KeywordIndex` handle
    /// and a metadata-store handle. Both are `Arc`-shared by
    /// convention — vault-app holds the canonical handles.
    pub fn new(index: Arc<KeywordIndex>, metadata_store: Arc<MetadataStore>) -> Self {
        Self {
            index,
            metadata_store,
        }
    }
}

#[async_trait]
impl Retriever for KeywordRetriever {
    #[instrument(
        skip(self, query),
        fields(
            query_len = query.query_text.len(),
            boundary_count = query.authorized_boundaries.len(),
            max_results = query.max_results,
        )
    )]
    async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        // Q2: query-text validation. Empty/whitespace → InvalidInput,
        // matching SemanticRetriever's surface for parity.
        let trimmed = query.query_text.trim();
        if trimmed.is_empty() {
            return Err(VaultError::InvalidInput(
                "query_text must be non-empty after trim".into(),
            ));
        }
        if query.query_text.len() > MAX_QUERY_BYTES {
            return Err(VaultError::InvalidInput(format!(
                "query_text exceeds MAX_QUERY_BYTES={MAX_QUERY_BYTES}"
            )));
        }
        // Q3: max_results in 1..=MAX_RESULTS_CAP.
        if query.max_results == 0 || query.max_results > MAX_RESULTS_CAP {
            return Err(VaultError::InvalidInput(format!(
                "max_results must be in 1..={MAX_RESULTS_CAP}"
            )));
        }
        // Q1: empty authorized_boundaries → empty result (access denied
        // semantics, never an error, never information-leaking).
        if query.authorized_boundaries.is_empty() {
            return Ok(Vec::new());
        }

        // BM25 search. Pull a generous pool — we trim post-hydration
        // after boundary + archived filters. The pool size matches
        // `max_results` for Phase 1 (no widening needed until hybrid
        // fusion lands at Phase 2).
        let hits = self
            .index
            .search(&query.query_text, query.max_results)
            .await?;
        if hits.is_empty() {
            return Ok(Vec::new());
        }

        // Hydrate memories from SQLite.
        let ids: Vec<MemoryId> = hits.iter().map(|(id, _)| *id).collect();
        let memories = self.metadata_store.get_memories_batch(&ids).await?;
        let mut by_id: HashMap<MemoryId, Memory> = HashMap::with_capacity(memories.len());
        for m in memories {
            by_id.insert(m.id, m);
        }

        // Boundary + archived filter.
        let allowed: HashSet<&str> = query
            .authorized_boundaries
            .iter()
            .map(|b| b.as_str())
            .collect();
        let include_archived = query.options.include_archived;

        let mut out: Vec<RetrievedMemory> = Vec::with_capacity(hits.len());
        let now = Utc::now();
        for (id, score) in hits {
            let Some(m) = by_id.remove(&id) else { continue };
            if !allowed.contains(m.boundary.as_str()) {
                continue;
            }
            // Default (include_archived=false): skip both superseded AND
            // expired memories. Per ADR-051 (T0.2.7 Phase B), single flag
            // controls both behaviors.
            if !include_archived && (m.is_superseded() || m.is_expired_at(now)) {
                continue;
            }
            let explanation = format!("keyword(bm25) score={score:.4}");
            out.push(RetrievedMemory {
                memory: m,
                score,
                explanation,
            });
        }

        // Sort: score DESC, tiebreak created_at DESC (Retriever trait
        // invariant #3).
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.memory.created_at.cmp(&a.memory.created_at))
        });
        out.truncate(query.max_results);

        Ok(out)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Strip Lucene/Tantivy `QueryParser` operator characters from a
/// natural-language query so the parser doesn't choke on punctuation
/// like the apostrophe in `What's`. Each operator char is replaced
/// with a space so tokenisation still produces clean word boundaries.
fn sanitize_query(text: &str) -> String {
    text.chars()
        .map(|c| {
            if LUCENE_OPERATOR_CHARS.contains(&c) {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Wrap a Tantivy error as `VaultError::Storage`. Tantivy errors don't
/// implement `Into<VaultError>` and the cross-crate dep would be ugly;
/// `Storage` is the closest semantic match (an index failure IS a
/// storage-layer failure).
fn vault_err(ctx: &str, e: impl std::fmt::Display) -> VaultError {
    VaultError::Storage(format!("keyword index: {ctx}: {e}"))
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn sanitize_strips_apostrophe() {
        let cleaned = sanitize_query("What's the Comcast bill?");
        assert!(!cleaned.contains('\''));
        assert!(!cleaned.contains('?'));
        // Word boundaries preserved.
        assert!(cleaned.contains("What"));
        assert!(cleaned.contains("Comcast"));
    }

    #[test]
    fn sanitize_preserves_dollar_and_alphanumeric() {
        let cleaned = sanitize_query("Q1 2027 launch revenue $89");
        // `$` is NOT a Lucene operator; preserved.
        assert!(cleaned.contains('$'));
        assert!(cleaned.contains("Q1"));
        assert!(cleaned.contains("89"));
    }

    #[test]
    fn sanitize_strips_every_listed_operator() {
        let cleaned = sanitize_query("foo+bar-baz!qux&|()*?:\\/\"'~^[]{}");
        for c in LUCENE_OPERATOR_CHARS {
            assert!(
                !cleaned.contains(*c),
                "operator {c:?} not stripped (got {cleaned:?})"
            );
        }
    }

    #[test]
    fn sanitize_empty_yields_empty() {
        assert_eq!(sanitize_query(""), "");
    }

    #[test]
    fn vault_err_format_includes_ctx() {
        let e = vault_err("commit", "boom");
        match e {
            VaultError::Storage(msg) => {
                assert!(msg.contains("keyword index"));
                assert!(msg.contains("commit"));
                assert!(msg.contains("boom"));
            }
            other => panic!("expected VaultError::Storage, got {other:?}"),
        }
    }
}
