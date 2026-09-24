//! `VaultAdapter` — production impl of [`vault_mcp::Adapter`].
//!
//! Wires four trait deps into the concrete vault operations the MCP
//! tool layer dispatches:
//!
//! - **[`Retriever`]** — `Adapter::search` delegates here. Trust-
//!   boundary auth-gating already enforced at the StdioServer layer
//!   (Step 4); VaultAdapter passes through.
//! - **[`EmbeddingProvider`]** — `Adapter::write` / `::update` embed
//!   `content` before calling [`StorageBackend`]'s cascade entry
//!   points (which take a pre-computed embedding).
//! - **[`StorageBackend`]** — write / update / delete cascade across
//!   SQLCipher + LanceDB + DuckDB per ADR-009.
//! - **[`MetadataStore`]** — separate handle (sharing the
//!   `Arc<Inner>` SQLCipher connection at construction time) used
//!   by `append_tool_invoke_audit` for the `mcp.tool_invoke` audit
//!   chain row. Holding a separate handle (rather than calling
//!   `StorageBackend::metadata()`) avoids widening StorageBackend's
//!   public API surface for one consumer; the caller (T0.1.10
//!   `Application::spawn_retry_worker`) wires both at startup.
//!
//! ## Trust boundary (ADR-025)
//!
//! `authorized_boundaries` enforcement lives at
//! [`vault_mcp::StdioServer`] (Step 4). Every method on `VaultAdapter`
//! receives already-trusted shapes — `RetrievalQuery` with the
//! application-supplied boundary slice, `NewMemory` with a boundary
//! the handler has verified against the trusted slice. **VaultAdapter
//! MUST NOT** re-derive boundaries from request data; the handler is
//! the single auth-gate site per ADR-025.
//!
//! `append_tool_invoke_audit` records `actor_kind = ActorKind::Agent`
//! per the ADR-025 Step 6 application: the MCP client is an
//! untrusted agent per the trust-boundary contract; user attribution
//! lives in the boundary scope (`authorized_boundaries`), not the
//! audit-row actor field.
//!
//! ## Update semantics (ADR-028)
//!
//! `Adapter::update` does **read-before-write** to preserve
//! provenance and lineage. The full per-field classification of
//! preserved vs overwritten lives in **ADR-028** (HANDOFF.md). The
//! invariant the implementation upholds:
//!
//! - **OVERWRITE** only fields exposed by the MCP write/update wire
//!   schema (per ADR-024 tool-param contract: `content`,
//!   `memory_type`, `boundary`, `confidence`) PLUS system-managed
//!   fields update is expected to advance (`last_accessed = now`,
//!   `embedding` re-computed from new content).
//! - **PRESERVE** everything else: `id`, `source_agent`,
//!   `created_at`, `valid_from`, `valid_until`, `access_count`,
//!   `superseded_by`, `metadata`.
//!
//! `metadata.get_memory(id)` returning `None` produces
//! `VaultError::NotFound` — surfaces to the MCP client as `-32602
//! "not found"` per ADR-024's mapping.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use vault_core::{Boundary, Memory, MemoryId, NewMemory, VaultError, VaultResult};
use vault_embedding::EmbeddingProvider;
use vault_mcp::{Adapter, ToolInvokeDetails};
use vault_retrieval::{
    KeywordIndex, ReadQuery, RetrievalQuery, RetrievedMemory, Retriever, StructuredReadPipeline,
    StructuredReadResponse,
};
use vault_storage::{
    ActorKind, AgentToken, AuditEventType, AuditResult, BoundaryInfo, MemoryFilter, MetadataStore,
    PendingAuditEvent, StorageBackend,
};

/// Production `vault_mcp::Adapter` impl. Constructed by
/// `Application::spawn_retry_worker` at startup (T0.1.10) with concrete trait deps.
///
/// Cheap to clone — the Retriever and EmbeddingProvider are
/// `Arc`-shared, StorageBackend and MetadataStore both clone via
/// `Arc<Inner>` internals. Multiple StdioServer instances (V0.2+)
/// can hold clones without locking.
pub struct VaultAdapter {
    retriever: Arc<dyn Retriever>,
    /// Read pipeline for the `memory_read` MCP tool. Commit 6 (locked-
    /// next-arc, 2026-05-26 — ADR-052) replaced the V0.2-era
    /// `Option<ReadPipeline>` with a concrete [`StructuredReadPipeline`]
    /// that has no fallible setup. The field is no longer `Option`:
    /// the pipeline is always wired by `Application::new` against the
    /// vault root REPORT directory, and `Adapter::read` dispatches
    /// directly with no config-error fallback.
    read_pipeline: StructuredReadPipeline,
    embedding: Arc<dyn EmbeddingProvider>,
    storage: StorageBackend,
    metadata: MetadataStore,
    /// In-RAM BM25 keyword index, shared (`Arc`) with the retriever's
    /// keyword channel. Maintained inline on write/update/delete so a
    /// freshly written memory is searchable immediately, not only after
    /// the next server restart's bulk-load. Read-after-write fix
    /// (2026-05-28): the retriever queries this same `Arc<KeywordIndex>`,
    /// so an inline upsert here makes the new memory visible to the very
    /// next search/read in the same process.
    keyword_index: Arc<KeywordIndex>,
}

impl VaultAdapter {
    /// Construct from the trait deps + a [`StructuredReadPipeline`] + the
    /// shared [`KeywordIndex`]. Caller (`Application::new`) wires concrete
    /// implementations and passes a `MetadataStore` handle that points at
    /// the same encrypted SQLite file used in the `StorageBackend` open.
    ///
    /// **Commit 6 (2026-05-26) — signature change:** the `read_pipeline`
    /// parameter is now a concrete [`StructuredReadPipeline`] (not
    /// `Option<ReadPipeline>`). The new pipeline has no model-load cost
    /// and no fallible setup, so the `Option` wrapper is unnecessary.
    ///
    /// **Commit 8 (2026-05-28) — signature change:** the `keyword_index`
    /// parameter was added so write/update/delete maintain the BM25 index
    /// inline (read-after-write fix). Pass the same `Arc<KeywordIndex>`
    /// that the retriever's keyword channel holds.
    pub fn new(
        retriever: Arc<dyn Retriever>,
        read_pipeline: StructuredReadPipeline,
        embedding: Arc<dyn EmbeddingProvider>,
        storage: StorageBackend,
        metadata: MetadataStore,
        keyword_index: Arc<KeywordIndex>,
    ) -> Self {
        Self {
            retriever,
            read_pipeline,
            embedding,
            storage,
            metadata,
            keyword_index,
        }
    }

    /// Maintain the in-RAM BM25 keyword index after a durable write/update.
    /// Best-effort: the SQLite write already committed and the index is
    /// rebuilt from SQLite on every restart, so a transient index failure
    /// is self-healing — log loudly, never fail the operation (the memory
    /// IS saved). Read-after-write fix (2026-05-28): without this, a fresh
    /// write was invisible to search/read until the next restart's bulk-load.
    async fn maintain_keyword_index_upsert(&self, id: MemoryId, content: &str) {
        if let Err(e) = self.keyword_index.upsert(id, content).await {
            tracing::warn!(
                target: "vault_app::keyword_index",
                memory_id = %id,
                error = %e,
                "keyword-index upsert failed after durable write; the memory is saved \
                 and will be indexed on the next server restart"
            );
        }
    }

    /// Mirror of [`Self::maintain_keyword_index_upsert`] for the delete
    /// path. Best-effort with the same self-healing rationale.
    async fn maintain_keyword_index_delete(&self, id: MemoryId) {
        if let Err(e) = self.keyword_index.delete(id).await {
            tracing::warn!(
                target: "vault_app::keyword_index",
                memory_id = %id,
                error = %e,
                "keyword-index delete failed after durable delete; the index will be \
                 reconciled on the next server restart"
            );
        }
    }
}

#[async_trait]
impl Adapter for VaultAdapter {
    async fn search(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        // Trust-boundary auth-gating already done at the StdioServer
        // layer (Step 4). Pass through to the Retriever.
        self.retriever.retrieve(query).await
    }

    async fn read(&self, query: ReadQuery) -> VaultResult<StructuredReadResponse> {
        // Trust-boundary auth-gating already done at the StdioServer
        // layer per ADR-025 (handle_read populates query.authorized_
        // boundaries from the trusted slice). Pass through to the
        // wired StructuredReadPipeline; Commit 6 (ADR-052) removed the
        // Option wrapper — pipeline is always present.
        self.read_pipeline.read(query).await
    }

    async fn write(&self, mut new_memory: NewMemory) -> VaultResult<MemoryId> {
        // Normalize content to canonical-save shape BEFORE validation.
        // Belt-and-braces companion to the `memory_write` MCP tool
        // description's canonical-save contract (T0.2.7 close,
        // 2026-05-25): the description teaches agents the six rules;
        // this catches the most common drift cases server-side so
        // smaller / cheaper LLMs that don't follow the description
        // perfectly still produce canonical-shape memories. See
        // `crate::normalization` for the applied rules.
        //
        // `Adapter::update` is also normalized — same rationale as the
        // write path. Closed as the small tech-debt follow-up shipping
        // alongside this admin-cleanup ride-along.
        new_memory.content =
            crate::normalization::normalize_for_canonical_save(&new_memory.content)?;

        // Memory::try_new applies validation + generates a fresh
        // MemoryId. Embedding is computed from validated content.
        let memory = Memory::try_new(new_memory)?;
        let embedding = self.embedding.embed(&memory.content).await?;
        self.storage.write_memory(&memory, &embedding).await?;
        self.maintain_keyword_index_upsert(memory.id, &memory.content)
            .await;
        Ok(memory.id)
    }

    async fn update(&self, id: MemoryId, mut new_memory: NewMemory) -> VaultResult<()> {
        // ADR-028 read-before-write contract: read existing → patch
        // preserved fields → re-compute embedding → cascade update.

        // Normalize content to canonical-save shape BEFORE validation.
        // Mirrors the write path so all stored memories — regardless of
        // entry route — land in canonical shape (per the cross-platform
        // thesis in [[mcp-descriptions-cross-platform-lever]]). The
        // earlier T0.2.7 close commit deferred this; the admin
        // ride-along of 2026-05-26 closes it.
        new_memory.content =
            crate::normalization::normalize_for_canonical_save(&new_memory.content)?;

        // Read existing. NotFound surfaces as VaultError::NotFound,
        // which maps to `-32602 "not found"` at the MCP wire layer
        // per ADR-024.
        let existing = self
            .metadata
            .get_memory(&id)
            .await?
            .ok_or_else(|| VaultError::NotFound(format!("memory {id} not found")))?;

        // Build the candidate updated Memory through try_new_with_id
        // so MCP-supplied fields go through the canonical validation
        // path. Then patch the ADR-028 PRESERVED fields from the
        // existing row.
        let mut updated = Memory::try_new_with_id(id, new_memory)?;

        // ADR-028 PRESERVED — keep from existing row.
        updated.source_agent = existing.source_agent;
        updated.created_at = existing.created_at;
        updated.valid_from = existing.valid_from;
        updated.valid_until = existing.valid_until;
        updated.access_count = existing.access_count;
        updated.superseded_by = existing.superseded_by;
        updated.metadata = existing.metadata;

        // ADR-028 OVERWRITE → now.
        updated.last_accessed = Utc::now();

        // Re-validate after the patches. try_new_with_id validated
        // the new fields (content / memory_type / boundary /
        // confidence) but the patched temporal fields could in
        // theory introduce a `valid_until < valid_from` violation if
        // the existing row violates the invariant (a regression
        // somewhere upstream). Treat that as a hard failure rather
        // than silently shipping invalid data.
        updated.validate()?;

        // Re-compute embedding from the new content. ADR-028:
        // stale embedding is a correctness bug.
        let embedding = self.embedding.embed(&updated.content).await?;

        self.storage.update_memory(&updated, &embedding).await?;
        self.maintain_keyword_index_upsert(id, &updated.content)
            .await;
        Ok(())
    }

    async fn delete(&self, id: MemoryId) -> VaultResult<()> {
        // StorageBackend::delete_memory is idempotent at the
        // cascade layer per cascading.rs:323-329 — deleting a
        // non-existent id still returns Ok with details.deleted =
        // false. Pass through.
        self.storage.delete_memory(&id).await?;
        self.maintain_keyword_index_delete(id).await;
        Ok(())
    }

    /// ADR-025 amendment 2026-05-05: returns the memory's stored
    /// boundary so the StdioServer handler can auth-gate `memory_delete`
    /// before dispatching to `delete`. Reads through the same
    /// `MetadataStore` handle used by `update`'s read-before-write path.
    async fn lookup_boundary(&self, id: MemoryId) -> VaultResult<Option<Boundary>> {
        Ok(self.metadata.get_memory(&id).await?.map(|m| m.boundary))
    }

    /// ADR-SEC-001 D3/D4: resolve a presented capability token's hash to the
    /// connecting agent's authorized boundaries via the agent-token store
    /// (migration 0007). `None` = no active agent for that token → the daemon
    /// returns a generic 401 (SP-4 fail-secure). The daemon is the sole caller;
    /// the stdio path uses the trusted construction-time slice instead.
    async fn resolve_token_boundaries(
        &self,
        token_hash: &str,
    ) -> VaultResult<Option<(String, Vec<Boundary>)>> {
        Ok(self
            .storage
            .lookup_agent_by_token_hash(token_hash)
            .await?
            .map(|agent| (agent.agent_name, agent.boundaries)))
    }

    async fn append_tool_invoke_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        self.append_audit_with_event_type(AuditEventType::McpToolInvoke, details, ActorKind::Agent)
            .await
    }
}

/// One page of the active memories, newest first, after `before`. Shared by
/// the adapter and the locked keeper's store so both page identically.
pub(crate) async fn list_page(
    metadata: &MetadataStore,
    before: Option<(chrono::DateTime<Utc>, MemoryId)>,
    limit: usize,
) -> VaultResult<Vec<Memory>> {
    metadata
        .list_memories(
            MemoryFilter {
                before,
                ..MemoryFilter::default()
            },
            Some(limit),
        )
        .await
}

/// The audit row for one tool or command invocation, built the one way the
/// chain's hash determinism relies on (ADR-024, BRD §11.9.2). Shared by the
/// adapter and by the locked keeper's database-only store (ADR-108), so both
/// write byte-identical rows.
pub(crate) fn invoke_audit_event(
    event_type: AuditEventType,
    details: ToolInvokeDetails,
    actor_kind: ActorKind,
) -> VaultResult<PendingAuditEvent> {
    let result = if details.error.is_some() {
        AuditResult::Error
    } else {
        AuditResult::Success
    };
    let details_json = details.to_canonical_json()?;
    Ok(PendingAuditEvent {
        event_type,
        resource_type: None,
        resource_id: None,
        boundary: None,
        actor_kind,
        actor_name: None,
        user_id: None,
        device_id: None,
        result,
        details_json,
    })
}

impl VaultAdapter {
    /// **ADR-024 amendment 2026-05-05 (T0.1.11 Phase 4b — Decision 5(γ)).**
    /// Generic audit-write helper used by both the trait method
    /// `append_tool_invoke_audit` (event_type = McpToolInvoke) AND the
    /// inherent method `append_tauri_command_audit` below. Encapsulates
    /// the PendingAuditEvent construction shape per ADR-024 + BRD §11.9.2
    /// audit-chain hash determinism.
    async fn append_audit_with_event_type(
        &self,
        event_type: AuditEventType,
        details: ToolInvokeDetails,
        actor_kind: ActorKind,
    ) -> VaultResult<()> {
        let pending = invoke_audit_event(event_type, details, actor_kind)?;
        self.metadata.append_audit_event(pending).await?;
        Ok(())
    }

    /// **ADR-024 amendment 2026-05-05 (T0.1.11 Phase 4b — Decision 5(γ)).**
    /// Append a `TauriCommandInvoke` audit row for vault-state-changing
    /// Tauri commands (add_memory / search_memories / update_memory /
    /// delete_memory). Tauri commands aren't MCP — reusing
    /// `mcp.tool_invoke` would create semantic debt at V0.2 cloud sync;
    /// the new variant gives Tauri commands their own discriminator.
    /// `actor_kind = User` (founder is the actor; not an untrusted agent
    /// like ADR-025 specifies for MCP).
    pub async fn append_tauri_command_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        self.append_audit_with_event_type(
            AuditEventType::TauriCommandInvoke,
            details,
            ActorKind::User,
        )
        .await
    }

    // ---------------------------------------------------------------------
    // UI-facing reads (desktop UI slice 2)
    //
    // These deliberately live on the concrete `VaultAdapter`, NOT on the
    // `vault_mcp::Adapter` trait. The trait is the surface every connected AI
    // agent can reach; these are desktop-shell reads for the vault OWNER.
    // Widening the agent-facing trait to carry them would hand every agent a
    // vault-wide boundary enumeration for no benefit — a least-privilege
    // regression (BRD §11.2 SP-2). `append_tauri_command_audit` above sits
    // here for the same reason.
    //
    // ## ADR-SEC-003 — the desktop UI reads across all boundaries
    //
    // Boundaries are a mandatory-access-control mechanism scoping *agents*
    // (BRD §11.4.3 rule 5: "the LLM/agent never sees memories outside its
    // authorized boundary"). The Tauri layer acts as `ActorKind::User` — the
    // vault owner, who is the party that GRANTS boundary access to agents in
    // the first place. Fencing the owner out of their own boundaries would
    // protect nothing and make the Boundaries tab decorative. So these reads
    // are owner-scoped, not agent-scoped.
    //
    // This does NOT weaken §11.4.3: agent-facing paths still resolve their
    // authorized slice per-request from the capability token (ADR-SEC-001 D4),
    // and none of them route through here.
    // ---------------------------------------------------------------------

    /// Newest-first memories across every boundary, capped at `limit`.
    ///
    /// Backs the home tab's "recently remembered" list, which previously read
    /// a browser-local cache and therefore showed only UI-added memories —
    /// anything an agent wrote was invisible. Excludes superseded and
    /// cold-archived rows (the `MemoryFilter` defaults), matching what default
    /// retrieval considers active.
    pub async fn list_recent_memories(&self, limit: usize) -> VaultResult<Vec<Memory>> {
        self.metadata
            .list_memories(MemoryFilter::default(), Some(limit))
            .await
    }

    /// One page of [`Self::list_recent_memories`]' order, after `before`
    /// (ADR-108: the desktop's export, page by page).
    pub async fn list_memories_page(
        &self,
        before: Option<(chrono::DateTime<Utc>, MemoryId)>,
        limit: usize,
    ) -> VaultResult<Vec<Memory>> {
        list_page(&self.metadata, before, limit).await
    }

    /// Every registered boundary with its active-memory count (name-ordered).
    pub async fn list_boundaries(&self) -> VaultResult<Vec<BoundaryInfo>> {
        self.storage.list_boundaries().await
    }

    /// Register a named boundary. `Ok(false)` means it already existed.
    pub async fn create_boundary(
        &self,
        boundary: &Boundary,
        description: Option<&str>,
    ) -> VaultResult<bool> {
        self.storage.create_boundary(boundary, description).await
    }

    /// Every registered agent (active and revoked) — the ADR-SEC-001
    /// capability-token registry. Carries no secret: only the token's hash is
    /// ever stored, and `AgentToken` does not expose even that.
    pub async fn list_agents(&self) -> VaultResult<Vec<AgentToken>> {
        self.storage.list_agent_tokens().await
    }

    /// Revoke an agent's capability token. `Ok(false)` means no ACTIVE agent
    /// of that name existed. Revocation is soft, so the audit trail survives.
    pub async fn revoke_agent(&self, agent_name: &str) -> VaultResult<bool> {
        self.storage.revoke_agent_token(agent_name).await
    }

    /// Verify the tamper-evident audit chain end to end (BRD §11.9.2).
    /// `Ok(())` means every link validated.
    pub async fn verify_audit_chain(&self) -> VaultResult<()> {
        self.metadata.verify_audit_chain().await
    }

    /// Count of active memories across all boundaries — the number the
    /// Settings tab reports.
    pub async fn total_memory_count(&self) -> VaultResult<u64> {
        let boundaries = self.storage.list_boundaries().await?;
        Ok(boundaries.iter().map(|b| b.memory_count).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::TempDir;
    use vault_core::{Boundary, MemoryType};
    use vault_embedding::EMBEDDING_DIM;
    use vault_storage::SqlCipherKey;

    /// Test-only at-rest key (32 bytes, fixed pattern). Per-mod local
    /// const per HANDOFF sub-task (d) §"Const placement" decision lock;
    /// matches the convention in `vault-storage/tests/migration_v0_1_to_sealed.rs:96`
    /// and `vault-cli/src/main.rs:497`.
    const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

    // -----------------------------------------------------------------
    // Stub trait impls used across tests
    // -----------------------------------------------------------------

    /// Stub retriever returning a caller-supplied response. Records
    /// the queries it received for later assertion.
    struct StubRetriever {
        response: Vec<RetrievedMemory>,
        queries: std::sync::Mutex<Vec<RetrievalQuery>>,
    }

    impl StubRetriever {
        fn with_response(response: Vec<RetrievedMemory>) -> Self {
            Self {
                response,
                queries: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn recorded_queries(&self) -> Vec<RetrievalQuery> {
            self.queries.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Retriever for StubRetriever {
        async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
            self.queries.lock().unwrap().push(query);
            Ok(self.response.clone())
        }
    }

    /// Stub embedder returning a deterministic L2-normalised vector.
    /// First slot is `1.0`, rest zeros — unit norm. Different inputs
    /// produce the same vector (irrelevant for these tests; the
    /// real embedder is exercised in vault-embedding).
    struct StubEmbedder {
        calls: AtomicU64,
    }

    impl StubEmbedder {
        fn new() -> Self {
            Self {
                calls: AtomicU64::new(0),
            }
        }

        fn call_count(&self) -> u64 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _text: &str) -> VaultResult<Vec<f32>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut v = vec![0.0_f32; EMBEDDING_DIM];
            v[0] = 1.0;
            Ok(v)
        }
    }

    /// Test-only no-op REPORT loader for constructing a
    /// [`StructuredReadPipeline`] in fixtures that don't exercise the
    /// read path. Returns `Ok(None)` for every boundary so the pipeline
    /// surfaces `REPORT_MISSING` (which these tests don't assert on).
    /// Mirrors the canned-empty pattern from
    /// `crates/vault-mcp/tests/common/mock_adapter.rs::MockAdapter::read`.
    struct NoopReportLoader;

    #[async_trait]
    impl vault_retrieval::ReportLoader for NoopReportLoader {
        async fn load(&self, _: &Boundary) -> VaultResult<Option<vault_retrieval::LoadedReport>> {
            Ok(None)
        }
    }

    // -----------------------------------------------------------------
    // Test fixture: open fresh tempdir-backed StorageBackend + a
    // SECOND MetadataStore handle for VaultAdapter. The two
    // MetadataStore handles are SEPARATE SQLCipher connections to
    // the same physical DB file (V0.1 single-user serial MCP =
    // never concurrent appends; V0.2+ revisit per ADR-028).
    // -----------------------------------------------------------------

    struct Fixture {
        _tmp: TempDir,
        adapter: VaultAdapter,
        retriever: Arc<StubRetriever>,
        embedder: Arc<StubEmbedder>,
        // Held for direct read-back assertions in tests.
        metadata_for_assert: MetadataStore,
        // Same Arc the adapter holds — lets tests assert the keyword
        // index was maintained inline on write/update/delete (#0).
        keyword_index: Arc<KeywordIndex>,
    }

    async fn make_fixture(retriever_response: Vec<RetrievedMemory>) -> Fixture {
        let tmp = TempDir::new().unwrap();
        let metadata_path = tmp.path().join("vault.db");
        let vector_dir = tmp.path().join("lance");
        let graph_path = tmp.path().join("graph.duckdb");
        let key = SqlCipherKey::new("vault-app-adapter-test-key");

        // StorageBackend opens its own MetadataStore internally. Open
        // a SECOND MetadataStore for VaultAdapter.metadata. Both
        // point at the same SQLCipher file via separate connections.
        let storage = StorageBackend::open_with_at_rest_key(
            &metadata_path,
            &vector_dir,
            &graph_path,
            key.clone(),
            EMBEDDING_DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .unwrap();
        let metadata = MetadataStore::open(&metadata_path, key.clone())
            .await
            .unwrap();
        let metadata_for_assert = MetadataStore::open(&metadata_path, key).await.unwrap();

        let retriever = Arc::new(StubRetriever::with_response(retriever_response));
        let embedder = Arc::new(StubEmbedder::new());

        // Commit 6 (ADR-052, 2026-05-26): VaultAdapter::new now takes a
        // concrete StructuredReadPipeline (not Option<ReadPipeline>). Build
        // a minimal pipeline backed by a fresh StubRetriever + NoopReportLoader
        // — these tests don't exercise read(), but the field must hold a
        // valid pipeline to compile.
        let pipeline_retriever: Arc<dyn Retriever> =
            Arc::new(StubRetriever::with_response(Vec::new()));
        let pipeline_loader: Arc<dyn vault_retrieval::ReportLoader> = Arc::new(NoopReportLoader);
        let read_pipeline = StructuredReadPipeline::new(pipeline_retriever, pipeline_loader);

        // Commit 8 (2026-05-28): VaultAdapter::new now takes the shared
        // KeywordIndex for inline write/update/delete maintenance. These
        // tests don't exercise keyword search, so a fresh empty index is
        // fine — the maintenance calls upsert/delete into it harmlessly.
        let keyword_index =
            Arc::new(vault_retrieval::KeywordIndex::new().expect("KeywordIndex::new in test"));

        let adapter = VaultAdapter::new(
            retriever.clone() as Arc<dyn Retriever>,
            read_pipeline,
            embedder.clone() as Arc<dyn EmbeddingProvider>,
            storage,
            metadata,
            keyword_index.clone(),
        );

        Fixture {
            _tmp: tmp,
            adapter,
            retriever,
            embedder,
            metadata_for_assert,
            keyword_index,
        }
    }

    fn sample_new_memory(content: &str, boundary: &str) -> NewMemory {
        NewMemory {
            content: content.to_string(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new(boundary).unwrap(),
            source_agent: Some("update-agent".to_string()),
            confidence: 0.7,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        }
    }

    fn sample_query(boundary: &str) -> RetrievalQuery {
        RetrievalQuery {
            query_text: "anything".to_string(),
            authorized_boundaries: vec![Boundary::new(boundary).unwrap()],
            max_results: 10,
            options: vault_retrieval::RetrievalOptions {
                score_threshold: None,
                include_archived: false,
            },
        }
    }

    fn sample_search_audit_details(error: bool) -> ToolInvokeDetails {
        ToolInvokeDetails {
            tool: "memory_search",
            duration_ms: 12,
            result_count: if error { 0 } else { 3 },
            boundary_count: 1,
            max_results: Some(10),
            score_threshold: None,
            include_archived: Some(false),
            query_length: Some(8),
            error: if error {
                Some(vault_mcp::ToolInvokeError::DimensionMismatch {
                    expected: 384,
                    actual: 256,
                })
            } else {
                None
            },
        }
    }

    // ==================================================================
    // 0. keyword-index maintenance on write/update/delete (#0)
    // ==================================================================

    #[tokio::test]
    async fn write_makes_memory_searchable_in_keyword_index_without_restart() {
        // #0 regression (2026-05-28): before this fix a fresh write went
        // to SQLite + Lance but NOT the in-RAM BM25 index, so it was
        // invisible to search/read until the next server restart's
        // bulk-load. The adapter now upserts into the shared keyword
        // index inline on write — so the memory is findable in the SAME
        // process, no restart.
        let fx = make_fixture(Vec::new()).await;
        let id = fx
            .adapter
            .write(sample_new_memory(
                "The user prefers dark mode in their code editors.",
                "personal",
            ))
            .await
            .expect("write succeeds");

        let hits = fx
            .keyword_index
            .search("dark mode editors", 10)
            .await
            .expect("keyword search succeeds");
        assert!(
            hits.iter().any(|(hit_id, _score)| *hit_id == id),
            "freshly written memory must be searchable in the keyword index \
             without a restart; got {hits:?}"
        );
    }

    #[tokio::test]
    async fn delete_removes_memory_from_keyword_index() {
        // Companion to the write test: delete must also maintain the
        // index inline so a removed memory stops surfacing immediately.
        let fx = make_fixture(Vec::new()).await;
        let id = fx
            .adapter
            .write(sample_new_memory(
                "The user enjoys hiking on weekends.",
                "personal",
            ))
            .await
            .expect("write succeeds");
        // Present after write.
        let before = fx
            .keyword_index
            .search("hiking weekends", 10)
            .await
            .unwrap();
        assert!(before.iter().any(|(h, _)| *h == id), "present after write");

        fx.adapter.delete(id).await.expect("delete succeeds");

        let after = fx
            .keyword_index
            .search("hiking weekends", 10)
            .await
            .unwrap();
        assert!(
            !after.iter().any(|(h, _)| *h == id),
            "deleted memory must be gone from the keyword index; got {after:?}"
        );
    }

    // ==================================================================
    // 1. search → retriever
    // ==================================================================

    #[tokio::test]
    async fn search_dispatches_to_retriever() {
        let f = make_fixture(vec![]).await;
        let q = sample_query("work");
        let results = f.adapter.search(q.clone()).await.unwrap();
        assert_eq!(results.len(), 0, "empty stub response → empty results");

        let recorded = f.retriever.recorded_queries();
        assert_eq!(recorded.len(), 1, "exactly one retriever dispatch per call");
        assert_eq!(recorded[0].query_text, q.query_text);
        assert_eq!(
            recorded[0].authorized_boundaries, q.authorized_boundaries,
            "trusted boundary slice passed through verbatim"
        );
    }

    // ==================================================================
    // 2. write → embed + cascade
    // ==================================================================

    #[tokio::test]
    async fn write_embeds_then_cascades() {
        let f = make_fixture(vec![]).await;
        let id = f
            .adapter
            .write(sample_new_memory("remember the milk", "work"))
            .await
            .unwrap();

        // Embedder was called exactly once.
        assert_eq!(f.embedder.call_count(), 1);

        // Row exists in metadata store. Content reflects canonical-save
        // normalization per T0.2.7 close (2026-05-25): the input
        // "remember the milk" was capitalized + terminated with a period
        // by `normalize_for_canonical_save` before storage. This
        // assertion pins that normalization runs on the write path.
        let stored = f
            .metadata_for_assert
            .get_memory(&id)
            .await
            .unwrap()
            .expect("memory written by VaultAdapter::write must be readable");
        assert_eq!(stored.content, "Remember the milk.");
        assert_eq!(stored.boundary.as_str(), "work");
        assert_eq!(stored.access_count, 0);
    }

    // ==================================================================
    // 3. update — ADR-028 preservation invariant
    // ==================================================================

    /// **Pinning test for ADR-028.** Pre-populate a memory with non-
    /// default `created_at`, `access_count > 0`, and
    /// `superseded_by = Some(...)`. Call `Adapter::update` with a
    /// new content + memory_type + boundary + confidence. Assert:
    ///
    /// - PRESERVED fields unchanged (id, source_agent, created_at,
    ///   valid_from, valid_until, access_count, superseded_by,
    ///   metadata)
    /// - OVERWRITTEN fields match input (content, memory_type,
    ///   boundary, confidence)
    /// - last_accessed advanced to ≥ pre-update timestamp
    /// - embedder called once during update (re-computation per
    ///   ADR-028)
    #[tokio::test]
    async fn update_preserves_provenance_per_adr_028() {
        let f = make_fixture(vec![]).await;

        // Pre-populate via VaultAdapter::write.
        let original_id = f
            .adapter
            .write(sample_new_memory("original content", "work"))
            .await
            .unwrap();

        // Mutate the row's fields directly via the metadata store
        // (simulating a memory that's accumulated history through
        // some other path — e.g. consolidator). We need
        // non-default `access_count` and `superseded_by` so the
        // preservation assertions are non-trivial.
        let original_created_at = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let other_id = MemoryId::new();
        let original_metadata = serde_json::json!({"source": "consolidator", "weight": 0.42});
        let original_valid_from = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();

        // Read, mutate, write back.
        let mut row = f
            .metadata_for_assert
            .get_memory(&original_id)
            .await
            .unwrap()
            .expect("pre-populated memory must exist");
        row.created_at = original_created_at;
        row.valid_from = original_valid_from;
        row.access_count = 42;
        row.superseded_by = Some(other_id);
        row.metadata = original_metadata.clone();
        row.source_agent = Some("genesis-agent".to_string());

        f.metadata_for_assert.update_memory(&row).await.unwrap();

        // Snapshot pre-update timestamp so we can assert
        // `last_accessed` advanced.
        let pre_update_timestamp = Utc::now();
        // Sleep a millisecond so `last_accessed > pre_update_timestamp`
        // can be a strict comparison even on fast clocks.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;

        // Reset embedder call count so we can pin "update calls
        // embed exactly once."
        let pre_update_embed_calls = f.embedder.call_count();

        // The update payload — note the values that should land
        // (content / memory_type / boundary / confidence) and the
        // values that should NOT (source_agent on the new payload
        // is "update-agent" but should be preserved as
        // "genesis-agent"; metadata on the new payload should be
        // discarded in favor of the existing).
        let update_payload = NewMemory {
            content: "updated content".to_string(),
            memory_type: MemoryType::Procedural,
            boundary: Boundary::new("personal").unwrap(),
            source_agent: Some("update-agent".to_string()),
            confidence: 0.95,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({"this_should_not_appear": true}),
        };

        f.adapter.update(original_id, update_payload).await.unwrap();

        // Read back.
        let after = f
            .metadata_for_assert
            .get_memory(&original_id)
            .await
            .unwrap()
            .expect("memory still exists after update");

        // OVERWRITTEN. Content arrives normalized per the canonical-save
        // contract — first-char capitalized + terminal period appended —
        // mirroring the write path's normalization (2026-05-26 admin
        // ride-along).
        assert_eq!(
            after.content, "Updated content.",
            "content overwritten and normalized"
        );
        assert_eq!(
            after.memory_type,
            MemoryType::Procedural,
            "memory_type overwritten"
        );
        assert_eq!(after.boundary.as_str(), "personal", "boundary overwritten");
        assert_eq!(after.confidence, 0.95, "confidence overwritten");

        // PRESERVED.
        assert_eq!(after.id, original_id, "id preserved (identity)");
        assert_eq!(
            after.source_agent.as_deref(),
            Some("genesis-agent"),
            "ADR-028: source_agent preserved (genesis attribution)"
        );
        assert_eq!(
            after.created_at, original_created_at,
            "ADR-028: created_at preserved (provenance)"
        );
        assert_eq!(
            after.valid_from, original_valid_from,
            "ADR-028: valid_from preserved (bi-temporal)"
        );
        assert_eq!(
            after.access_count, 42,
            "ADR-028: access_count preserved (read-history)"
        );
        assert_eq!(
            after.superseded_by,
            Some(other_id),
            "ADR-028: superseded_by preserved (consolidation lineage)"
        );
        assert_eq!(
            after.metadata, original_metadata,
            "ADR-028: metadata preserved"
        );

        // ADVANCED.
        assert!(
            after.last_accessed > pre_update_timestamp,
            "ADR-028: last_accessed advanced to now (update IS an access)"
        );

        // RE-COMPUTED.
        assert_eq!(
            f.embedder.call_count(),
            pre_update_embed_calls + 1,
            "ADR-028: embedding re-computed exactly once during update"
        );
    }

    // ==================================================================
    // 3b. update — content normalization mirrors the write path
    // ==================================================================

    /// **Pinning test for the 2026-05-26 admin ride-along.** The
    /// canonical-save normalization that the write path runs through
    /// `vault_app::normalization::normalize_for_canonical_save` MUST
    /// also run on the update path. Calls `Adapter::update` with raw
    /// agent-style content (lowercase first char, no terminal period,
    /// first-person framing) and asserts the stored row carries the
    /// normalized form. Mirrors `write_embeds_then_cascades`'s
    /// content-shape assertion.
    #[tokio::test]
    async fn update_normalizes_content_like_write() {
        let f = make_fixture(vec![]).await;

        // Seed a memory we can update.
        let id = f
            .adapter
            .write(sample_new_memory("seed content", "work"))
            .await
            .unwrap();

        // Update with raw agent-style content. The write path normalizes
        // "i prefer dark mode" → "The user prefers dark mode." — assert
        // the update path matches.
        let update_payload = NewMemory {
            content: "i prefer dark mode".to_string(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new("work").unwrap(),
            source_agent: Some("update-agent".to_string()),
            confidence: 0.8,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        };
        f.adapter.update(id, update_payload).await.unwrap();

        let after = f
            .metadata_for_assert
            .get_memory(&id)
            .await
            .unwrap()
            .expect("updated memory exists");
        assert_eq!(
            after.content, "The user prefers dark mode.",
            "update path MUST run normalize_for_canonical_save (first-person → third-person, \
             capitalize, terminal period); see crate::normalization"
        );
    }

    // ==================================================================
    // 4. update — NotFound for missing id
    // ==================================================================

    #[tokio::test]
    async fn update_returns_not_found_for_missing_id() {
        let f = make_fixture(vec![]).await;
        let missing = MemoryId::new();
        let err = f
            .adapter
            .update(missing, sample_new_memory("anything", "work"))
            .await
            .expect_err("missing id must surface as Err");
        assert!(
            matches!(err, VaultError::NotFound(_)),
            "missing id MUST surface as VaultError::NotFound (maps to -32602 \"not found\" \
             at MCP wire layer per ADR-024); got {err:?}"
        );
    }

    // ==================================================================
    // 5. delete — cascade idempotent
    // ==================================================================

    #[tokio::test]
    async fn delete_cascades_idempotently() {
        let f = make_fixture(vec![]).await;
        let id = f
            .adapter
            .write(sample_new_memory("doomed memory", "work"))
            .await
            .unwrap();

        // First delete: row removed.
        f.adapter.delete(id).await.unwrap();
        assert!(
            f.metadata_for_assert
                .get_memory(&id)
                .await
                .unwrap()
                .is_none(),
            "first delete removes the row"
        );

        // Second delete (same id): also Ok per cascade idempotency.
        f.adapter.delete(id).await.unwrap();
    }

    // ==================================================================
    // 6. append_tool_invoke_audit — chain row
    // ==================================================================

    #[tokio::test]
    async fn append_tool_invoke_audit_writes_chain_row() {
        let f = make_fixture(vec![]).await;
        let details = sample_search_audit_details(false);

        f.adapter.append_tool_invoke_audit(details).await.unwrap();

        // Read back via the assertion handle. The test just checks
        // a row exists with the expected event_type; the per-field
        // assertions are below.
        let events = f.metadata_for_assert.list_audit_events(10).await.unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.event_type == AuditEventType::McpToolInvoke),
            "audit_log must contain a `mcp.tool_invoke` row after append"
        );
    }

    // ==================================================================
    // 7. append_tool_invoke_audit — canonical JSON
    // ==================================================================

    #[tokio::test]
    async fn append_tool_invoke_audit_uses_canonical_json() {
        let f = make_fixture(vec![]).await;
        let details = sample_search_audit_details(false);
        let expected_canonical = details
            .to_canonical_json()
            .expect("canonical JSON serialisation succeeds");

        f.adapter.append_tool_invoke_audit(details).await.unwrap();

        let events = f.metadata_for_assert.list_audit_events(10).await.unwrap();
        let row = events
            .iter()
            .find(|e| e.event_type == AuditEventType::McpToolInvoke)
            .expect("mcp.tool_invoke row present");

        // BRD §11.9.2: details_json byte-string must match the
        // canonical-JSON output verbatim. The audit chain hashes
        // these bytes; any drift from to_canonical_json's output
        // breaks hash determinism.
        assert_eq!(
            row.details_json, expected_canonical,
            "details_json must equal ToolInvokeDetails::to_canonical_json output verbatim \
             (BRD §11.9.2 audit chain hash determinism)"
        );
    }

    // ==================================================================
    // 8. append_tool_invoke_audit — actor_kind = Agent (ADR-025)
    // ==================================================================

    /// **Stand-alone test for the ADR-025 Step 6 application.** Pinned
    /// separately from the chain-row + canonical-JSON tests so a
    /// regression on actor-kind classification fails at its own
    /// assertion line rather than getting tangled with the broader
    /// audit-row assertion.
    #[tokio::test]
    async fn append_tool_invoke_audit_records_actor_as_agent_per_adr_025() {
        let f = make_fixture(vec![]).await;
        f.adapter
            .append_tool_invoke_audit(sample_search_audit_details(true))
            .await
            .unwrap();

        let events = f.metadata_for_assert.list_audit_events(10).await.unwrap();
        let row = events
            .iter()
            .find(|e| e.event_type == AuditEventType::McpToolInvoke)
            .expect("mcp.tool_invoke row present");
        assert_eq!(
            row.actor_kind,
            ActorKind::Agent,
            "ADR-025 Step 6 application: append_tool_invoke_audit MUST record \
             actor_kind = ActorKind::Agent. The MCP client is an untrusted agent per \
             ADR-025; user attribution lives in the boundary scope (authorized_boundaries), \
             not the audit-row actor field."
        );
        // Result reflects the error path (details.error.is_some()).
        assert_eq!(
            row.result,
            AuditResult::Error,
            "details.error.is_some() → AuditResult::Error"
        );
    }
}
