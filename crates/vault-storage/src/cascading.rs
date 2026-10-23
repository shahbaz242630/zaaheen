//! [`StorageBackend`] — the cascading-write orchestrator (BRD §5.2, T0.1.6
//! Phase C1b).
//!
//! ## Responsibilities
//!
//! - User-write entry points (`write_memory` / `update_memory` /
//!   `delete_memory`) commit the **SQLite-side authoritative state** —
//!   `memories` row + audit chain entry + retry-queue (or pending-sync)
//!   bookkeeping — atomically in **one** SQLite transaction.
//! - Returns [`Ack`] as soon as that commit lands. Cascading writes to
//!   LanceDB and DuckDB run **asynchronously** through the retry worker
//!   (see `retry_worker.rs`).
//! - Eager `validate_readable` probe on `open()` surfaces hard fragment
//!   corruption immediately as a CRITICAL audit event + [`DegradedMode`]
//!   flag. Per ADR-018 + Phase A Change 1, the backend stays open in
//!   degraded mode so vault-cli triage still works.
//!
//! ## Lockstep + idempotency contract (load-bearing — ADR-017)
//!
//! Every cascading write produces **one** retry-queue row carrying both
//! the LanceDB and DuckDB sub-ops as a single unit (Phase C plan v2 Issue
//! 1). The worker re-runs both sub-ops idempotently per attempt; either
//! failure → whole entry reschedules. Retrieval-quality during the
//! divergence window is unaffected for V0.1 because retrieval is
//! LanceDB-only.
//!
//! ## Eager validation rejects the permanent-failure classes (ADR-009 amendment)
//!
//! `is_permanent` covers `DimensionMismatch` / `AccessDenied` /
//! `Storage(msg).contains("schema")`. The orchestrator validates dim +
//! boundary at `write_memory` / `update_memory` entry **before** the
//! SQLite write — those classes shouldn't reach the queue by
//! construction. If they do (because the worker observed a concurrent
//! schema drift), the worker dead-letters loudly with
//! [`crate::retry_queue::DeadLetterReason::Permanent`].
//!
//! ## Graph-cascade scope (still no-op in production at T0.2.3 commit 1)
//!
//! Memory writes do not extract entities at write time. The original V0.1
//! comment in this slot said "ships at T0.2.2 (consolidator)" — that
//! forward-reference is now stale: T0.2.2 (`a889931` + `a53e3a5`) shipped
//! Phase 1 clustering only, and T0.2.3 commit 1 ships Phase 2 LLM
//! merge-decisions but **without** entity extraction or graph-relationship
//! rewriting. T0.2.3 commit 2's Phase 3 `apply_merge` will emit a `WARN`
//! and skip graph updates pending a future entity-extraction task.
//!
//! See HANDOFF.md tech-debt entry on T0.2.x entity-extraction-at-consolidation
//! and the `GraphStore` relationship-rewrite primitive on merge (added at
//! T0.2.3 commit 1) for the forward-pointer.
//!
//! The orchestrator's "graph-side" cascade is therefore still a no-op in
//! production for memory writes. The worker still consults the test-only
//! `FaultInjector::graph_decision()` so adversarial tests can drive
//! graph-side failure scenarios.

#![allow(dead_code)] // RetryWorker lands in retry_worker.rs; some accessors are consumed there.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};

use vault_core::{Memory, MemoryId, VaultError, VaultResult};

use crate::audit::{ActorKind, AuditEventType, PendingAuditEvent};
use crate::dead_letter::DeadLetter;
use crate::graph_store::{DuckDbGraphStore, GraphStore};
use crate::key::SqlCipherKey;
use crate::metadata_store::{
    tx_append_audit, tx_get_memory, tx_insert_memory, tx_update_memory, MemoryFilter, MetadataStore,
};
use crate::pending_sync::PendingSync;
use crate::recently_deleted::{tx_check_app_limit, tx_insert_bin, BinBy, BinKind};
use crate::retry_queue::{CascadeOperation, RetryQueue, PAYLOAD_FORMAT_VERSION};
use crate::vector_store::{LanceVectorStore, VectorStore};

/// Hard cap on `retry_queue` entries before new cascading writes spill
/// into `pending_sync` for catch-up. Per ADR-009 amendment / Phase C plan
/// Q2.
pub const MAX_RETRY_QUEUE_DEPTH: usize = 10_000;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Returned from [`StorageBackend::write_memory`] et al. The cascading
/// downstream writes (LanceDB + DuckDB) have **not** completed by the time
/// this is returned — they are async via the retry worker. The SQLite-side
/// state (memory row + audit chain entry + retry-queue / pending-sync row)
/// IS durably committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ack {
    pub memory_id: MemoryId,
    pub sqlite_committed_at: DateTime<Utc>,
}

/// Reported readability state of the downstream stores. Set on `open()`
/// after `validate_readable` runs. UI banner + vault-cli surface this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegradedMode {
    /// Both downstream stores validated readable on open.
    Healthy,
    /// LanceDB's `validate_readable` failed — vector search will not work
    /// until repair. SQLite + DuckDB still operational.
    LanceUnreadable,
    /// DuckDB's `validate_readable` failed — graph traversal will not work.
    /// SQLite + LanceDB still operational.
    GraphUnreadable,
    /// Both downstream stores failed `validate_readable`.
    BothUnreadable,
}

impl DegradedMode {
    /// True when at least one downstream store is unreadable.
    pub fn is_degraded(self) -> bool {
        !matches!(self, Self::Healthy)
    }
}

// ---------------------------------------------------------------------------
// Internal payload shape
// ---------------------------------------------------------------------------

/// On-disk shape of `retry_queue.payload` for V0.1. Schema-versioned via
/// `retry_queue.payload_format_version`. The worker dispatches on
/// `retry_queue.operation` (Write / Update / Delete) and reads only the
/// fields relevant for that op.
///
/// For [`CascadeOperation::Delete`], the `embedding` field is an empty
/// `Vec<f32>` and `boundary` carries the boundary of the deleted memory
/// for audit-logging purposes (not used by `VectorStore::delete` itself).
///
/// Public so `vault-cli` (Phase C1b operator binary) can decode dead-letter
/// payloads when running an operator-driven retry. Format-version
/// dispatch on `retry_queue.payload_format_version` /
/// `dead_letter.payload_format_version` is the long-term shape;
/// V0.1 has only `V1`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CascadePayloadV1 {
    /// Embedding for LanceDB upsert. Empty for `Delete` cascades.
    pub embedding: Vec<f32>,
    /// Boundary the memory belongs to (defense-in-depth — also stored on
    /// the memory row + LanceDB row).
    pub boundary: String,
}

// ---------------------------------------------------------------------------
// StorageBackend
// ---------------------------------------------------------------------------

/// The cascading orchestrator. Cheap to clone (it holds `Arc`s of every
/// component internally); share freely across tasks.
///
/// Intentionally does **not** implement `Debug`: it owns the
/// `MetadataStore` (which holds the SQLCipher connection per ADR-007).
#[derive(Clone)]
pub struct StorageBackend {
    metadata: MetadataStore,
    vector: Arc<dyn VectorStore>,
    graph: Arc<dyn GraphStore>,
    retry_queue: RetryQueue,
    dead_letter: DeadLetter,
    pending_sync: PendingSync,
    degraded: DegradedMode,
    /// Tracks whether the most recent enqueue attempt found the queue at
    /// cap. When the state transitions Healthy→Overflow we fire one
    /// CRITICAL `cascade.queue_overflow` audit event; Overflow→Healthy
    /// fires nothing (the next write succeeds normally and the audit log
    /// shows the gap). Per Phase C plan Q2 ("debounced, not on every
    /// overflow write — so logs don't get drowned").
    in_cap_overflow: Arc<AtomicBool>,
}

impl StorageBackend {
    /// Open all three stores at the given paths via the sealed at-rest
    /// LanceDB path, then run `validate_readable` on the downstream
    /// stores. Returns `Ok(Self)` even on validation failure — the
    /// backend is reported via [`Self::degraded`] so vault-cli triage
    /// can still run (per ADR-018 / Phase A Change 1).
    ///
    /// Hard failures (couldn't open SQLCipher, couldn't open LanceDB /
    /// DuckDB at all) still return `Err` — those mean the vault-cli
    /// can't even read the SQLite metadata, so degraded mode wouldn't
    /// help.
    ///
    /// **Caller MUST pass the already-derived at-rest key** (`K3(master_key)`
    /// per ADR-008 amendment K3 KDF). Canonical production derivation
    /// site: [`vault_app::keychain::derive_at_rest_key`] per ADR-040
    /// amendment.
    ///
    /// The V0.1 plaintext `open()` constructor was removed at T0.2.0
    /// Phase 3 sub-task (e) (2026-05-12) alongside the rest of the V0.1
    /// migration code.
    #[instrument(
        skip(metadata_path, vector_data_dir, graph_path, key, at_rest_key),
        fields(
            metadata_path = %metadata_path.display(),
            vector_data_dir = %vector_data_dir.display(),
            graph_path = %graph_path.display(),
            dimension,
        )
    )]
    pub async fn open_with_at_rest_key(
        metadata_path: &Path,
        vector_data_dir: &Path,
        graph_path: &Path,
        key: SqlCipherKey,
        dimension: usize,
        at_rest_key: &[u8; 32],
    ) -> VaultResult<Self> {
        let metadata = MetadataStore::open(metadata_path, key).await?;
        let vector =
            LanceVectorStore::open_with_at_rest_key(vector_data_dir, dimension, at_rest_key)
                .await?;
        // ADR-SEC-002: the graph runs in-memory + sealed at rest with the same
        // at_rest_key as the vector store (no plaintext DuckDB file).
        let graph = DuckDbGraphStore::open_with_at_rest_key(graph_path, at_rest_key).await?;
        Self::assemble(metadata, vector, graph).await
    }

    /// Persist the graph store to its sealed at-rest snapshot (ADR-SEC-002).
    /// No-op for graph backends that don't seal. Invoked by the consolidator
    /// after each enrichment pass (the graph's only writer).
    pub async fn flush_graph(&self) -> VaultResult<()> {
        self.graph_store().flush().await
    }

    /// Shared assembly path used by [`Self::open_with_at_rest_key`].
    /// Runs `validate_readable` on the downstream stores, computes
    /// [`DegradedMode`], emits the per-store `store.corruption` audit
    /// events on failure, and builds the [`StorageBackend`] struct with
    /// shared metadata-store clones.
    async fn assemble(
        metadata: MetadataStore,
        vector: LanceVectorStore,
        graph: DuckDbGraphStore,
    ) -> VaultResult<Self> {
        let vector: Arc<dyn VectorStore> = Arc::new(vector);
        let graph: Arc<dyn GraphStore> = Arc::new(graph);

        let lance_ok = vector.validate_readable().await;
        let graph_ok = graph.validate_readable().await;

        let degraded = match (lance_ok.is_ok(), graph_ok.is_ok()) {
            (true, true) => DegradedMode::Healthy,
            (false, true) => DegradedMode::LanceUnreadable,
            (true, false) => DegradedMode::GraphUnreadable,
            (false, false) => DegradedMode::BothUnreadable,
        };

        // Emit a single `store.corruption` audit event per failed store,
        // before construction returns — so the chain captures the open
        // outcome even if the caller never reads the degraded flag.
        if let Err(e) = &lance_ok {
            warn!(error = %e, "LanceDB validate_readable failed at open — entering degraded mode (see ADR-018)");
            metadata
                .append_audit_event(
                    PendingAuditEvent::success(AuditEventType::StoreCorruption, ActorKind::System)
                        .error()
                        .with_resource("store", "lancedb")
                        .with_details_json(format!(
                            r#"{{"tag":"lancedb_corruption_at_open","error":{}}}"#,
                            json_string(&e.to_string()),
                        )),
                )
                .await?;
        }
        if let Err(e) = &graph_ok {
            warn!(error = %e, "DuckDB validate_readable failed at open — entering degraded mode (see ADR-018)");
            metadata
                .append_audit_event(
                    PendingAuditEvent::success(AuditEventType::StoreCorruption, ActorKind::System)
                        .error()
                        .with_resource("store", "duckdb")
                        .with_details_json(format!(
                            r#"{{"tag":"duckdb_corruption_at_open","error":{}}}"#,
                            json_string(&e.to_string()),
                        )),
                )
                .await?;
        }

        Ok(Self {
            retry_queue: RetryQueue::new(metadata.clone()),
            dead_letter: DeadLetter::new(metadata.clone()),
            pending_sync: PendingSync::new(metadata.clone()),
            metadata,
            vector,
            graph,
            degraded,
            in_cap_overflow: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Current degraded-mode state captured at `open()`. Stable for the
    /// lifetime of this backend instance.
    pub fn degraded(&self) -> DegradedMode {
        self.degraded
    }

    /// Metadata store handle. Crate-private — the cascading orchestrator
    /// owns all SQLite-side audit + memory writes; outside callers go
    /// through `write_memory` / `update_memory` / `delete_memory`.
    pub(crate) fn metadata(&self) -> &MetadataStore {
        &self.metadata
    }

    /// LanceDB-backed vector store handle. Public because `vault-cli`
    /// drives operator-initiated cascade retries directly against the
    /// vector store (production cascades go through the worker).
    pub fn vector_store(&self) -> &Arc<dyn VectorStore> {
        &self.vector
    }

    /// DuckDB-backed graph store handle. Public for symmetry with
    /// `vector_store()` — V0.2 consolidator + divergence-check use this
    /// for traversal validation.
    pub fn graph_store(&self) -> &Arc<dyn GraphStore> {
        &self.graph
    }

    /// Retry-queue handle. Crate-private — the cascading orchestrator
    /// owns enqueue; the retry worker owns drain. External callers
    /// observe outcomes via the `dead_letter` table.
    pub(crate) fn retry_queue(&self) -> &RetryQueue {
        &self.retry_queue
    }

    /// Dead-letter table handle. Public so vault-cli can list / inspect
    /// / resolve unresolved rows.
    pub fn dead_letter(&self) -> &DeadLetter {
        &self.dead_letter
    }

    /// Pending-sync table handle. Crate-private — V0.2 divergence-check
    /// owns the drain path.
    pub(crate) fn pending_sync(&self) -> &PendingSync {
        &self.pending_sync
    }

    /// Cascading user write. Atomic on the SQLite side: `memories` row +
    /// audit event + retry-queue (or pending-sync) row commit together.
    ///
    /// Eager validation rejects [`VaultError::DimensionMismatch`] and
    /// invalid-memory failures **before** any SQLite write — those classes
    /// would otherwise be permanent-failure dead-letters in the worker
    /// (wasted work + a confusing dead-letter row).
    #[instrument(skip(self, memory, embedding), fields(memory_id = %memory.id, dim = embedding.len()))]
    pub async fn write_memory(&self, memory: &Memory, embedding: &[f32]) -> VaultResult<Ack> {
        self.eager_validate(memory, embedding)?;
        self.cascading_write(
            memory.clone(),
            embedding.to_vec(),
            CascadeOperation::Write,
            None,
        )
        .await
    }

    /// Cascading update. Same atomicity contract as [`Self::write_memory`].
    /// Returns [`VaultError::NotFound`] if the memory id doesn't exist.
    #[instrument(skip(self, memory, embedding), fields(memory_id = %memory.id, dim = embedding.len()))]
    pub async fn update_memory(&self, memory: &Memory, embedding: &[f32]) -> VaultResult<Ack> {
        self.eager_validate(memory, embedding)?;
        self.cascading_write(
            memory.clone(),
            embedding.to_vec(),
            CascadeOperation::Update,
            None,
        )
        .await
    }

    /// [`Self::update_memory`] for an edit by the user or an AI app (ADR-114):
    /// the wording it replaces goes into Recently deleted as `replaced`, in
    /// the same transaction. An AI app's edit is refused with
    /// [`VaultError::RateLimited`] once AI apps have made
    /// [`crate::recently_deleted::APP_CHANGES_PER_HOUR`] changes in the last
    /// hour (ADR-SEC-049); nothing changes then.
    #[instrument(skip(self, memory, embedding), fields(memory_id = %memory.id, by = by.as_str()))]
    pub async fn update_memory_keeping_previous(
        &self,
        memory: &Memory,
        embedding: &[f32],
        by: BinBy,
    ) -> VaultResult<Ack> {
        self.eager_validate(memory, embedding)?;
        self.cascading_write(
            memory.clone(),
            embedding.to_vec(),
            CascadeOperation::Update,
            Some(by),
        )
        .await
    }

    /// Cascading delete. Idempotent: deleting a non-existent id still
    /// returns `Ok` and records an audit event with `details.deleted =
    /// false` (matches the [`MetadataStore::delete_memory`] contract). For
    /// non-existent ids, NO retry-queue / pending-sync row is enqueued
    /// (nothing to cascade).
    #[instrument(skip(self), fields(memory_id = %id))]
    pub async fn delete_memory(&self, id: &MemoryId) -> VaultResult<Ack> {
        self.delete_cascading(id, None).await
    }

    /// [`Self::delete_memory`] for a delete by the user or an AI app
    /// (ADR-114): the memory is copied into Recently deleted as `deleted` in
    /// the same transaction as the delete, so it can be restored for
    /// [`crate::recently_deleted::RECENTLY_DELETED_DAYS`] days. It still leaves
    /// the search files at once (ADR-039). An AI app's delete is refused with
    /// [`VaultError::RateLimited`] past the hourly limit (ADR-SEC-049); a
    /// missing id stays a quiet success and counts for nothing.
    #[instrument(skip(self), fields(memory_id = %id, by = by.as_str()))]
    pub async fn move_to_recently_deleted(&self, id: &MemoryId, by: BinBy) -> VaultResult<Ack> {
        self.delete_cascading(id, Some(by)).await
    }

    async fn delete_cascading(&self, id: &MemoryId, bin: Option<BinBy>) -> VaultResult<Ack> {
        let id_owned = *id;
        let metadata = self.metadata.clone();
        let in_cap = self.in_cap_overflow.clone();
        let id_for_closure = id_owned;

        let (committed_at, audit_seq, deleted_boundary): (DateTime<Utc>, i64, Option<String>) =
            metadata
                .with_transaction(move |tx| {
                    // Look up the existing memory so we know its boundary for
                    // the audit event AND so the cascade payload can carry it
                    // forward to the worker. The boundary read happens inside
                    // the same transaction as the delete — atomic.
                    let existing = tx_get_memory(tx, &id_for_closure)?;
                    let boundary_owned = existing.as_ref().map(|m| m.boundary.clone());

                    // ADR-114: keep a copy first, in this transaction, so the
                    // delete and the copy land together or not at all.
                    if let (Some(by), Some(memory)) = (bin, existing.as_ref()) {
                        let now = Utc::now();
                        tx_check_app_limit(tx, by, now)?;
                        tx_insert_bin(tx, memory, BinKind::Deleted, by, now)?;
                    }

                    let rows = tx
                        .execute(
                            "DELETE FROM memories WHERE id = ?1",
                            params![id_for_closure.to_string()],
                        )
                        .map_err(|e| VaultError::Storage(format!("delete memory: {e}")))?;

                    let mut pending = PendingAuditEvent::success(
                        AuditEventType::MemoryDelete,
                        bin.map_or(ActorKind::System, BinBy::actor),
                    )
                    .with_resource("memory", id_for_closure.to_string());
                    if let Some(b) = &boundary_owned {
                        pending = pending.with_boundary(b.clone());
                    }
                    pending.details_json = match bin {
                        None => format!(r#"{{"deleted":{}}}"#, rows > 0),
                        Some(_) => format!(r#"{{"deleted":{},"recently_deleted":true}}"#, rows > 0),
                    };
                    let event = tx_append_audit(tx, pending)?;
                    let committed_at = event.timestamp;
                    let audit_seq = event.seq;

                    // Only enqueue the cascade if a row was actually deleted.
                    // No row → nothing to cascade downstream.
                    if rows > 0 {
                        if let Some(boundary) = &boundary_owned {
                            let payload = CascadePayloadV1 {
                                embedding: Vec::new(),
                                boundary: boundary.as_str().to_string(),
                            };
                            let payload_bytes = serde_json::to_vec(&payload)?;

                            let queue_len = tx_count_retry_queue(tx)?;
                            if queue_len < MAX_RETRY_QUEUE_DEPTH {
                                tx_insert_retry_queue(
                                    tx,
                                    id_for_closure,
                                    CascadeOperation::Delete,
                                    audit_seq,
                                    &payload_bytes,
                                )?;
                                // Falling out of overflow if we were in it.
                                if in_cap.swap(false, Ordering::AcqRel) {
                                    // No audit event on transition out — the
                                    // gap in `cascade.queue_overflow` events
                                    // is the signal.
                                }
                            } else {
                                tx_upsert_pending_sync(
                                    tx,
                                    id_for_closure,
                                    CascadeOperation::Delete,
                                    committed_at,
                                    audit_seq,
                                    &payload_bytes,
                                )?;
                                // Transition into overflow: emit one audit event.
                                if !in_cap.swap(true, Ordering::AcqRel) {
                                    tx_append_audit(
                                        tx,
                                        PendingAuditEvent::success(
                                            AuditEventType::CascadeQueueOverflow,
                                            ActorKind::System,
                                        )
                                        .error()
                                        .with_details_json(
                                            r#"{"cap":10000,"action":"pending_sync_fallback"}"#
                                                .to_string(),
                                        ),
                                    )?;
                                }
                            }
                        }
                    }

                    Ok::<_, VaultError>((
                        committed_at,
                        audit_seq,
                        boundary_owned.map(|b| b.as_str().to_string()),
                    ))
                })
                .await?;

        let _ = audit_seq;
        let _ = deleted_boundary;
        Ok(Ack {
            memory_id: id_owned,
            sqlite_committed_at: committed_at,
        })
    }

    /// Mark a memory as superseded by another. **Phase 3 consolidator
    /// primitive per ADR-046 (T0.2.3 commit 2).**
    ///
    /// Sets `superseded_by = Some(new_id)` on the memory identified by
    /// `old_id`; no other fields change. Atomic with the audit event
    /// emission via [`MetadataStore::with_transaction`].
    ///
    /// **Explicitly NOT a cascading write.** Unlike [`Self::update_memory`],
    /// this:
    /// - Does NOT enqueue a `retry_queue` / `pending_sync` row.
    /// - Does NOT touch the vector store. A naive re-embed-then-update path
    ///   would produce a byte-identical vector under BGE-small's fp32
    ///   determinism — the LanceDB upsert would be a no-op. Bypassing the
    ///   cascade avoids spurious `retry_queue` enqueue + LanceDB write +
    ///   divergence-detection counter increment for a vector-layer no-op.
    /// - Emits the dedicated [`AuditEventType::MemorySuperseded`] variant
    ///   (NOT [`AuditEventType::MemoryUpdate`]) — preserves provenance
    ///   fidelity per BRD §5.6 line 948 "do not delete — preserve
    ///   provenance." Downstream audit viewer (T0.2.15) filters supersession
    ///   events by `event_type` rather than by JSON-path query on `details`.
    ///
    /// **`details_json` shape:** `{"superseded_by":"<new_id>"}`. The
    /// `resource_id` field carries the old (superseded) `MemoryId`; the
    /// `details.superseded_by` field carries the new (superseding)
    /// `MemoryId`. Audit viewer joins `resource_id` ↔
    /// `details.superseded_by`.
    ///
    /// **Errors:**
    /// - [`VaultError::NotFound`] if `old_id` doesn't exist.
    /// - [`VaultError::Storage`] on transaction-side failure.
    ///
    /// **Single-supersession assumption:** caller is responsible for not
    /// invoking this on a memory that's already superseded. Production
    /// callers (Phase 3 `apply_merge` in `vault-consolidator`) filter
    /// superseded memories via `MemoryFilter::include_superseded = false`
    /// (default) before clustering, so production code never hits the
    /// already-superseded path. If invoked on an already-superseded memory,
    /// the new `superseded_by` value overwrites the existing one (last-
    /// write-wins; the schema supports a chain via repeated single-Option
    /// writes) and a `tracing::warn!` records the case for observability.
    /// V0.3+ revisits iterative-supersession semantics if real
    /// consolidation-run data shows the need.
    #[instrument(skip(self), fields(old_id = %old_id, new_id = %new_id))]
    pub async fn mark_superseded(&self, old_id: MemoryId, new_id: MemoryId) -> VaultResult<Ack> {
        let metadata = self.metadata.clone();
        let committed_at: DateTime<Utc> = metadata
            .with_transaction(move |tx| {
                let memory = tx_get_memory(tx, &old_id)?.ok_or_else(|| {
                    VaultError::NotFound(format!("memory {old_id} does not exist"))
                })?;

                if let Some(existing) = memory.superseded_by {
                    warn!(
                        old_id = %old_id,
                        existing_supersedence = %existing,
                        new_supersedence = %new_id,
                        "mark_superseded called on already-superseded memory — \
                         chain extending (V0.3+ iterative-supersession behaviour; \
                         production code shouldn't hit this path because Phase 1 \
                         clustering filters superseded memories)"
                    );
                }

                let mut updated = memory;
                updated.superseded_by = Some(new_id);
                let boundary_for_audit = updated.boundary.clone();
                tx_update_memory(tx, &updated)?;

                let mut pending =
                    PendingAuditEvent::success(AuditEventType::MemorySuperseded, ActorKind::System)
                        .with_resource("memory", old_id.to_string())
                        .with_boundary(boundary_for_audit);
                pending.details_json = format!(
                    r#"{{"superseded_by":{}}}"#,
                    json_string(&new_id.to_string())
                );
                let event = tx_append_audit(tx, pending)?;

                Ok::<_, VaultError>(event.timestamp)
            })
            .await?;

        Ok(Ack {
            memory_id: old_id,
            sqlite_committed_at: committed_at,
        })
    }

    /// Mark a memory's content as no longer true in the world by setting
    /// `valid_until`. Bi-temporal invalidation primitive per ADR-051
    /// (T0.2.7 Phase B, merged-consolidator arc).
    ///
    /// `valid_until_at` is **fact-time** — the timestamp at which the
    /// memory's content stopped being true. NOT vault-deletion time. NOT
    /// garbage-collection time. Future-dated values (`valid_until_at >
    /// now()`) are allowed and represent planned expirations; retrieval
    /// continues to surface the memory until the timestamp passes.
    ///
    /// Orthogonal to [`Self::mark_superseded`]: both fields may be set on
    /// the same memory by the Phase C write-time `UPDATE` decision (fact
    /// stopped being true AND was replaced). `invalidate` does NOT touch
    /// `superseded_by`.
    ///
    /// Returns [`Ack`] once the SQLite-side state (memory row + audit
    /// chain entry) is durably committed. Metadata-only mutation —
    /// downstream stores (LanceDB / DuckDB) are untouched (no
    /// retry-queue row). The retrieval-side `valid_until` filter (Phase
    /// B.2, semantic.rs / keyword.rs / list_memories) excludes the
    /// memory by default after this call.
    ///
    /// **Boundary check is the caller's responsibility** per ADR-051 —
    /// MCP-layer callers (vault-mcp tool handlers) authorize against
    /// `authorized_boundaries` before invoking; internal callers
    /// (consolidator, Phase C write-time loop) pre-filter by boundary
    /// in their workflows. Mirrors the existing convention on
    /// [`Self::mark_superseded`].
    ///
    /// **Latest-wins on repeat invalidation:** calling on an
    /// already-invalidated memory overwrites `valid_until` with the new
    /// timestamp + emits a `tracing::warn!`. The earliest-known
    /// false-time edge case (rare — caller would need historical
    /// knowledge of when the fact actually became false) is handled by
    /// direct field write + admin path; V0.3+ revisits if telemetry
    /// shows the case. Per ADR-051 §Decision — invalidation API surface.
    ///
    /// **Audit-logged:** emits exactly one [`AuditEventType::MemoryInvalidated`]
    /// event per BRD §11.9.2. `details_json` shape:
    /// `{"valid_until":"<ISO-8601>","reason":"<free-text>"}`.
    ///
    /// **Errors:**
    /// - [`VaultError::NotFound`] if `memory_id` doesn't exist.
    /// - [`VaultError::InvalidInput`] if `valid_until_at < memory.valid_from`
    ///   (would violate the Memory bi-temporal invariant at
    ///   `crates/vault-core/src/memory.rs:198-204`).
    /// - [`VaultError::Storage`] on transaction-side failure.
    #[instrument(
        skip(self, reason),
        fields(memory_id = %memory_id, valid_until_at = %valid_until_at)
    )]
    pub async fn invalidate(
        &self,
        memory_id: MemoryId,
        valid_until_at: DateTime<Utc>,
        reason: String,
    ) -> VaultResult<Ack> {
        let metadata = self.metadata.clone();
        let committed_at: DateTime<Utc> = metadata
            .with_transaction(move |tx| {
                let memory = tx_get_memory(tx, &memory_id)?.ok_or_else(|| {
                    VaultError::NotFound(format!("memory {memory_id} does not exist"))
                })?;

                if let Some(existing) = memory.valid_until {
                    warn!(
                        memory_id = %memory_id,
                        existing_valid_until = %existing,
                        new_valid_until = %valid_until_at,
                        "invalidate called on already-invalidated memory — \
                         latest-wins per ADR-051; earlier timestamp overwritten"
                    );
                }

                // Enforce the Memory bi-temporal invariant (valid_until >=
                // valid_from) before mutating. Memory::validate() also
                // checks this, but rejecting early gives a clearer error
                // and avoids the surprise of a validate() failure after
                // tx_update_memory's body has already run.
                if valid_until_at < memory.valid_from {
                    return Err(VaultError::InvalidInput(format!(
                        "valid_until_at {valid_until_at} precedes valid_from {} for memory {memory_id}",
                        memory.valid_from
                    )));
                }

                let mut updated = memory;
                updated.valid_until = Some(valid_until_at);
                let boundary_for_audit = updated.boundary.clone();
                updated.validate()?;
                tx_update_memory(tx, &updated)?;

                let mut pending = PendingAuditEvent::success(
                    AuditEventType::MemoryInvalidated,
                    ActorKind::System,
                )
                .with_resource("memory", memory_id.to_string())
                .with_boundary(boundary_for_audit);
                pending.details_json = format!(
                    r#"{{"valid_until":{},"reason":{}}}"#,
                    json_string(&valid_until_at.to_rfc3339()),
                    json_string(&reason)
                );
                let event = tx_append_audit(tx, pending)?;

                Ok::<_, VaultError>(event.timestamp)
            })
            .await?;

        Ok(Ack {
            memory_id,
            sqlite_committed_at: committed_at,
        })
    }

    /// Apply a deterministic dedup atomically: supersede every loser into the
    /// survivor AND roll the cluster's aggregates onto the survivor, in ONE
    /// transaction (all-or-nothing — no partial dedup on mid-loop failure).
    /// Consolidator Phase 2-pre primitive per ADR-063.
    ///
    /// For each `loser_ids` member: set `superseded_by = survivor_id` and emit
    /// [`AuditEventType::MemorySuperseded`] (same per-loser semantics as
    /// [`Self::mark_superseded`]). For the survivor: set `access_count =
    /// summed_access_count` and `confidence = max_confidence` (BRD §5.6 line
    /// 988) and emit [`AuditEventType::MemoryDeduped`]. The survivor's CONTENT
    /// is unchanged, so this is **metadata-only** — no LanceDB/DuckDB cascade,
    /// no re-embed (the existing vector stays valid). Mirrors `mark_superseded`
    /// / `invalidate`'s metadata-only contract.
    ///
    /// **Boundary check is the caller's responsibility** (consolidator clusters
    /// within one boundary) — same convention as `mark_superseded` / `invalidate`.
    ///
    /// **Errors:**
    /// - [`VaultError::InvalidInput`] if `loser_ids` is empty or contains
    ///   `survivor_id` (a memory cannot supersede itself).
    /// - [`VaultError::NotFound`] if the survivor or any loser does not exist.
    /// - [`VaultError::Storage`] on transaction-side failure.
    #[instrument(
        skip(self),
        fields(survivor = %survivor_id, loser_count = loser_ids.len())
    )]
    pub async fn apply_dedup(
        &self,
        survivor_id: MemoryId,
        loser_ids: &[MemoryId],
        summed_access_count: u32,
        max_confidence: f32,
    ) -> VaultResult<Ack> {
        if loser_ids.is_empty() {
            return Err(VaultError::InvalidInput(
                "apply_dedup requires at least one loser".into(),
            ));
        }
        if loser_ids.contains(&survivor_id) {
            return Err(VaultError::InvalidInput(format!(
                "apply_dedup survivor {survivor_id} cannot also be a loser"
            )));
        }

        let losers: Vec<MemoryId> = loser_ids.to_vec();
        let metadata = self.metadata.clone();
        let committed_at: DateTime<Utc> = metadata
            .with_transaction(move |tx| {
                // 1. Supersede each loser → survivor (one MemorySuperseded each).
                for loser_id in &losers {
                    let loser = tx_get_memory(tx, loser_id)?.ok_or_else(|| {
                        VaultError::NotFound(format!("loser memory {loser_id} does not exist"))
                    })?;
                    if let Some(existing) = loser.superseded_by {
                        warn!(
                            loser_id = %loser_id,
                            existing_supersedence = %existing,
                            new_supersedence = %survivor_id,
                            "apply_dedup: loser already superseded — chain extending \
                             (production clustering filters superseded rows; unexpected)"
                        );
                    }
                    let boundary_for_audit = loser.boundary.clone();
                    let mut updated = loser;
                    updated.superseded_by = Some(survivor_id);
                    tx_update_memory(tx, &updated)?;

                    let mut pending = PendingAuditEvent::success(
                        AuditEventType::MemorySuperseded,
                        ActorKind::System,
                    )
                    .with_resource("memory", loser_id.to_string())
                    .with_boundary(boundary_for_audit);
                    pending.details_json =
                        format!(r#"{{"superseded_by":{}}}"#, json_string(&survivor_id.to_string()));
                    tx_append_audit(tx, pending)?;
                }

                // 2. Roll aggregates onto the survivor (one MemoryDeduped).
                let survivor = tx_get_memory(tx, &survivor_id)?.ok_or_else(|| {
                    VaultError::NotFound(format!("survivor memory {survivor_id} does not exist"))
                })?;
                let boundary_for_audit = survivor.boundary.clone();
                let mut updated = survivor;
                updated.access_count = summed_access_count;
                updated.confidence = max_confidence;
                updated.validate()?;
                tx_update_memory(tx, &updated)?;

                let absorbed_json = losers
                    .iter()
                    .map(|id| json_string(&id.to_string()))
                    .collect::<Vec<_>>()
                    .join(",");
                let mut pending =
                    PendingAuditEvent::success(AuditEventType::MemoryDeduped, ActorKind::System)
                        .with_resource("memory", survivor_id.to_string())
                        .with_boundary(boundary_for_audit);
                pending.details_json = format!(
                    r#"{{"absorbed":[{absorbed_json}],"summed_access_count":{summed_access_count},"max_confidence":{max_confidence}}}"#
                );
                let event = tx_append_audit(tx, pending)?;

                Ok::<_, VaultError>(event.timestamp)
            })
            .await?;

        Ok(Ack {
            memory_id: survivor_id,
            sqlite_committed_at: committed_at,
        })
    }

    /// Decay one cold fact's `confidence` — the sleep consolidator's Phase 4
    /// primitive (BRD §5.6 line 994; T0.2.4).
    ///
    /// Sets `confidence = new_confidence` and replaces `metadata` with
    /// `new_metadata` (which the caller composes as the fact's existing
    /// metadata plus the decay idempotency marker — so no metadata key is
    /// lost). **Metadata-only** — the fact's CONTENT and stored vector are
    /// untouched, so the (possibly ADR-074-enriched) embedding is preserved and
    /// there is no LanceDB/DuckDB cascade and no re-embed. Mirrors the
    /// `invalidate` / `apply_dedup` metadata-only contract.
    ///
    /// Emits exactly one [`AuditEventType::MemoryDecayed`] event with
    /// `details_json` = `{"old_confidence":<f32>,"new_confidence":<f32>}`.
    ///
    /// **Boundary check is the caller's responsibility** — the consolidator
    /// decays within the active set it enumerated, same convention as
    /// `invalidate` / `apply_dedup`.
    ///
    /// **Errors:**
    /// - [`VaultError::NotFound`] if `memory_id` does not exist.
    /// - [`VaultError::InvalidInput`] if `new_confidence` violates the
    ///   [`vault_core::Memory`] invariant (finite, in `[0.0, 1.0]`).
    /// - [`VaultError::Storage`] on transaction-side failure.
    #[instrument(skip(self, new_metadata), fields(memory = %memory_id))]
    pub async fn apply_decay(
        &self,
        memory_id: MemoryId,
        new_confidence: f32,
        new_metadata: serde_json::Value,
    ) -> VaultResult<Ack> {
        let store = self.metadata.clone();
        let committed_at: DateTime<Utc> = store
            .with_transaction(move |tx| {
                let memory = tx_get_memory(tx, &memory_id)?.ok_or_else(|| {
                    VaultError::NotFound(format!("memory {memory_id} does not exist"))
                })?;

                let old_confidence = memory.confidence;
                let boundary_for_audit = memory.boundary.clone();
                let mut updated = memory;
                updated.confidence = new_confidence;
                updated.metadata = new_metadata;
                updated.validate()?;
                tx_update_memory(tx, &updated)?;

                let mut pending =
                    PendingAuditEvent::success(AuditEventType::MemoryDecayed, ActorKind::System)
                        .with_resource("memory", memory_id.to_string())
                        .with_boundary(boundary_for_audit);
                pending.details_json = format!(
                    r#"{{"old_confidence":{old_confidence},"new_confidence":{new_confidence}}}"#
                );
                let event = tx_append_audit(tx, pending)?;

                Ok::<_, VaultError>(event.timestamp)
            })
            .await?;

        Ok(Ack {
            memory_id,
            sqlite_committed_at: committed_at,
        })
    }

    /// Apply a cold-archive move: set the memory's `archived_at` marker so it
    /// drops OUT of default retrieval (consolidator Phase 4, BRD §5.6 lines
    /// 995-996; ADR-084).
    ///
    /// **Metadata-only** — the fact's CONTENT and stored vector are untouched,
    /// so the (possibly ADR-074-enriched) embedding is preserved and there is
    /// no LanceDB/DuckDB cascade and no re-embed. The fact is NOT deleted: the
    /// archived row still lives in `memories`, retrievable via an explicit
    /// archive search (`MemoryFilter::include_archived = true`). Archive is
    /// reversible — checkpoint rollback restores `archived_at = None`. Mirrors
    /// the `invalidate` / `apply_decay` metadata-only contract.
    ///
    /// Emits exactly one [`AuditEventType::MemoryArchived`] event with
    /// `details_json` = `{"archived_at":"<rfc3339>"}`.
    ///
    /// **Boundary check is the caller's responsibility** — the consolidator
    /// archives within the active set it enumerated, same convention as
    /// `invalidate` / `apply_decay`.
    ///
    /// **Errors:**
    /// - [`VaultError::NotFound`] if `memory_id` does not exist.
    /// - [`VaultError::Storage`] on transaction-side failure.
    #[instrument(skip(self), fields(memory = %memory_id))]
    pub async fn apply_archive(
        &self,
        memory_id: MemoryId,
        archived_at: DateTime<Utc>,
    ) -> VaultResult<Ack> {
        let store = self.metadata.clone();
        let committed_at: DateTime<Utc> = store
            .with_transaction(move |tx| {
                let memory = tx_get_memory(tx, &memory_id)?.ok_or_else(|| {
                    VaultError::NotFound(format!("memory {memory_id} does not exist"))
                })?;

                let boundary_for_audit = memory.boundary.clone();
                let mut updated = memory;
                updated.archived_at = Some(archived_at);
                updated.validate()?;
                tx_update_memory(tx, &updated)?;

                let mut pending =
                    PendingAuditEvent::success(AuditEventType::MemoryArchived, ActorKind::System)
                        .with_resource("memory", memory_id.to_string())
                        .with_boundary(boundary_for_audit);
                pending.details_json = format!(
                    r#"{{"archived_at":{}}}"#,
                    json_string(&archived_at.to_rfc3339())
                );
                let event = tx_append_audit(tx, pending)?;

                Ok::<_, VaultError>(event.timestamp)
            })
            .await?;

        Ok(Ack {
            memory_id,
            sqlite_committed_at: committed_at,
        })
    }

    /// Drain `pending_sync` back into `retry_queue` (the cap-overflow recovery
    /// path; migration 0003 + T0.2.4 sync ship-gate). Oldest-first, atomically
    /// per entry: while `retry_queue` has capacity, re-enqueue the stored
    /// cascade (its `sequence_id` + `payload`) and remove the `pending_sync`
    /// row in the same transaction. Stops when the queue refills to
    /// [`MAX_RETRY_QUEUE_DEPTH`] or after `limit` rows have been examined.
    /// Returns the number of entries re-enqueued.
    ///
    /// A row with no stored payload (a pre-0003 legacy row — not expected at
    /// V0.1 scale, where cap-overflow never fired) is logged and skipped:
    /// re-enqueueing without the embedding would write a broken cascade. The
    /// [`crate::DivergenceDetector`] calls this as its Tier-0 sweep.
    ///
    /// # Errors
    /// - [`VaultError::Storage`] on a transaction-side failure.
    #[instrument(skip(self), fields(limit))]
    pub async fn drain_pending_sync(&self, limit: usize) -> VaultResult<usize> {
        let entries = self.pending_sync.oldest_first(limit).await?;
        let mut drained = 0usize;
        for entry in entries {
            let Some(payload_bytes) = entry.payload else {
                tracing::warn!(
                    memory_id = %entry.memory_id,
                    "pending_sync row has no cascade payload (pre-0003 legacy?) — skipping; \
                     operator must reconcile this entry"
                );
                continue;
            };
            let memory_id = entry.memory_id;
            let operation = entry.operation;
            let sequence_id = entry.sequence_id;
            let store = self.metadata.clone();
            let reenqueued: bool = store
                .with_transaction(move |tx| {
                    if tx_count_retry_queue(tx)? >= MAX_RETRY_QUEUE_DEPTH {
                        return Ok::<bool, VaultError>(false); // queue full — stop draining
                    }
                    tx_insert_retry_queue(tx, memory_id, operation, sequence_id, &payload_bytes)?;
                    tx_delete_pending_sync(tx, memory_id)?;
                    Ok::<bool, VaultError>(true)
                })
                .await?;
            if !reenqueued {
                break; // retry_queue is at cap; leave the rest for the next sweep
            }
            drained += 1;
        }
        if drained > 0 {
            tracing::info!(drained, "pending_sync drained into retry_queue");
        }
        Ok(drained)
    }

    /// List memories matching `filter`. The `limit` parameter accepts
    /// `None` (return ALL matching rows — no SQL `LIMIT` clause) or
    /// `Some(N)` (cap at `N`).
    ///
    /// `limit: None` is locked as intentional unboundedness — used by
    /// `vault-consolidator`'s clustering primitive (BRD §5.6 Phase 1)
    /// where the algorithm needs every memory in the boundary for
    /// union-find. Callers MUST NOT treat `None` as a "page size of
    /// zero" footgun. At V0.2 alpha scale (100-1000 memories per
    /// vault) the unbounded case is bounded by the vault size itself.
    /// V0.3+ revisits if vaults grow to 10k+ memories with measurable
    /// memory pressure — see ADR-045 §f pagination forward-compat call.
    ///
    /// Emits exactly one `memory.list` audit event regardless of how
    /// many rows match (same contract as the underlying
    /// [`MetadataStore::list_memories`]).
    ///
    /// First public read-side API on [`StorageBackend`] (T0.2.2
    /// Amendment 2). Prior reads went through the type-specific store
    /// handles (`vector_store()`, `graph_store()`) or the
    /// `MetadataStore` was kept `pub(crate)`-gated. The
    /// boundary-scoped enumeration use case is the first cross-crate
    /// consumer (vault-consolidator); future consumers
    /// (vault-cli triage, T0.2.4 decay/archive) can compose with the
    /// same surface.
    #[instrument(skip(self), fields(limit = ?limit, boundary = ?filter.boundary))]
    pub async fn list_memories(
        &self,
        filter: MemoryFilter,
        limit: Option<usize>,
    ) -> VaultResult<Vec<Memory>> {
        self.metadata.list_memories(filter, limit).await
    }

    /// Eager-validate memory + embedding before any SQLite write. Drops
    /// [`VaultError::DimensionMismatch`] / invalid-memory cases on the
    /// floor so they never reach the cascading queue.
    pub(crate) fn eager_validate(&self, memory: &Memory, embedding: &[f32]) -> VaultResult<()> {
        memory.validate()?; // content / confidence / etc.
        let expected = self.vector.dimension();
        if embedding.len() != expected {
            return Err(VaultError::DimensionMismatch {
                expected,
                actual: embedding.len(),
            });
        }
        if embedding.is_empty() {
            return Err(VaultError::InvalidInput("embedding is empty".into()));
        }
        if embedding.iter().any(|x| !x.is_finite()) {
            return Err(VaultError::InvalidInput(
                "embedding contains non-finite values".into(),
            ));
        }
        Ok(())
    }

    /// The overflow flag shared by every cascading write (Recently deleted's
    /// restore enqueues through [`tx_enqueue_cascade`] too).
    pub(crate) fn in_cap_overflow(&self) -> &Arc<AtomicBool> {
        &self.in_cap_overflow
    }

    /// Shared SQLite-side body for write + update. The only difference is
    /// (a) the SQL (INSERT vs UPDATE) and (b) the `CascadeOperation` written
    /// onto the retry-queue row. With `bin` set (ADR-114), an update first
    /// copies the wording it replaces into Recently deleted.
    async fn cascading_write(
        &self,
        memory: Memory,
        embedding: Vec<f32>,
        op: CascadeOperation,
        bin: Option<BinBy>,
    ) -> VaultResult<Ack> {
        debug_assert!(matches!(
            op,
            CascadeOperation::Write | CascadeOperation::Update
        ));

        let memory_id = memory.id;
        let boundary_str = memory.boundary.as_str().to_string();
        let payload = CascadePayloadV1 {
            embedding,
            boundary: boundary_str,
        };
        let payload_bytes = serde_json::to_vec(&payload)?;
        let in_cap = self.in_cap_overflow.clone();

        let committed_at: DateTime<Utc> = self
            .metadata
            .with_transaction(move |tx| {
                let event_kind = match op {
                    CascadeOperation::Write => {
                        tx_insert_memory(tx, &memory)?;
                        AuditEventType::MemoryCreate
                    }
                    CascadeOperation::Update => {
                        if let Some(by) = bin {
                            if let Some(previous) = tx_get_memory(tx, &memory_id)? {
                                let now = Utc::now();
                                tx_check_app_limit(tx, by, now)?;
                                tx_insert_bin(tx, &previous, BinKind::Replaced, by, now)?;
                            }
                        }
                        let rows = tx_update_memory(tx, &memory)?;
                        if rows == 0 {
                            return Err(VaultError::NotFound(format!(
                                "memory {memory_id} does not exist",
                            )));
                        }
                        AuditEventType::MemoryUpdate
                    }
                    CascadeOperation::Delete => unreachable!("delete uses delete_memory"),
                };

                let mut pending = PendingAuditEvent::success(
                    event_kind,
                    bin.map_or(ActorKind::System, BinBy::actor),
                )
                .with_resource("memory", memory_id.to_string())
                .with_boundary(memory.boundary.clone());
                if bin.is_some() {
                    pending.details_json = r#"{"recently_deleted":true}"#.to_string();
                }
                let event = tx_append_audit(tx, pending)?;
                let committed_at = event.timestamp;
                let audit_seq = event.seq;

                tx_enqueue_cascade(
                    tx,
                    memory_id,
                    op,
                    committed_at,
                    audit_seq,
                    &payload_bytes,
                    &in_cap,
                )?;
                Ok::<DateTime<Utc>, VaultError>(committed_at)
            })
            .await?;

        Ok(Ack {
            memory_id,
            sqlite_committed_at: committed_at,
        })
    }
}

// ---------------------------------------------------------------------------
// SQL helpers (private to this module — all run inside a caller-supplied
// `&Transaction<'_>`)
// ---------------------------------------------------------------------------

/// Queue a write/update cascade (LanceDB + DuckDB) for the retry worker, or
/// spill it into `pending_sync` at the cap, emitting the one debounced
/// `cascade.queue_overflow` event on the way into overflow (Phase C plan Q2).
pub(crate) fn tx_enqueue_cascade(
    tx: &rusqlite::Transaction<'_>,
    memory_id: MemoryId,
    op: CascadeOperation,
    committed_at: DateTime<Utc>,
    audit_seq: i64,
    payload_bytes: &[u8],
    in_cap: &AtomicBool,
) -> VaultResult<()> {
    let queue_len = tx_count_retry_queue(tx)?;
    if queue_len < MAX_RETRY_QUEUE_DEPTH {
        tx_insert_retry_queue(tx, memory_id, op, audit_seq, payload_bytes)?;
        in_cap.store(false, Ordering::Release);
    } else {
        tx_upsert_pending_sync(tx, memory_id, op, committed_at, audit_seq, payload_bytes)?;
        if !in_cap.swap(true, Ordering::AcqRel) {
            tx_append_audit(
                tx,
                PendingAuditEvent::success(AuditEventType::CascadeQueueOverflow, ActorKind::System)
                    .error()
                    .with_details_json(
                        r#"{"cap":10000,"action":"pending_sync_fallback"}"#.to_string(),
                    ),
            )?;
        }
    }
    Ok(())
}

fn tx_count_retry_queue(tx: &rusqlite::Transaction<'_>) -> VaultResult<usize> {
    let n: i64 = tx
        .query_row("SELECT COUNT(*) FROM retry_queue", [], |row| row.get(0))
        .map_err(|e| VaultError::Storage(format!("count retry_queue (in tx): {e}")))?;
    Ok(n as usize)
}

fn tx_insert_retry_queue(
    tx: &rusqlite::Transaction<'_>,
    memory_id: MemoryId,
    operation: CascadeOperation,
    sequence_id: i64,
    payload_bytes: &[u8],
) -> VaultResult<()> {
    use crate::retry_queue::{base_backoff_secs, compute_next_attempt};
    let now = Utc::now();
    let next_at = compute_next_attempt(0, now, 0.0)
        .expect("base_backoff_secs(0) must yield Some — invariant of retry_queue schedule");
    let entry_id = uuid::Uuid::now_v7();
    let _ = base_backoff_secs(0); // sanity reference for the audit reader

    tx.execute(
        "INSERT INTO retry_queue (
            id, memory_id, operation, payload_format_version,
            payload, sequence_id, attempts_made,
            next_attempt_at, created_at, last_error
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, NULL)",
        params![
            entry_id.as_bytes().to_vec(),
            memory_id.0.as_bytes().to_vec(),
            operation.as_str(),
            PAYLOAD_FORMAT_VERSION,
            payload_bytes,
            sequence_id,
            next_at.to_rfc3339(),
            now.to_rfc3339(),
        ],
    )
    .map_err(|e| VaultError::Storage(format!("enqueue retry (in tx): {e}")))?;
    Ok(())
}

fn tx_upsert_pending_sync(
    tx: &rusqlite::Transaction<'_>,
    memory_id: MemoryId,
    operation: CascadeOperation,
    queued_at: DateTime<Utc>,
    sequence_id: i64,
    payload_bytes: &[u8],
) -> VaultResult<()> {
    // Carry the full cascade payload + sequence anchor (migration 0003) so the
    // divergence sweep can re-enqueue a faithful retry_queue row when capacity
    // returns. ON CONFLICT, the latest overflowing op's payload wins.
    tx.execute(
        "INSERT INTO pending_sync (memory_id, operation, queued_at, sequence_id, payload)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(memory_id) DO UPDATE SET
            operation = excluded.operation,
            queued_at = excluded.queued_at,
            sequence_id = excluded.sequence_id,
            payload = excluded.payload",
        params![
            memory_id.0.as_bytes().to_vec(),
            operation.as_str(),
            queued_at.to_rfc3339(),
            sequence_id,
            payload_bytes,
        ],
    )
    .map_err(|e| VaultError::Storage(format!("upsert pending_sync (in tx): {e}")))?;
    Ok(())
}

fn tx_delete_pending_sync(tx: &rusqlite::Transaction<'_>, memory_id: MemoryId) -> VaultResult<()> {
    tx.execute(
        "DELETE FROM pending_sync WHERE memory_id = ?1",
        params![memory_id.0.as_bytes().to_vec()],
    )
    .map_err(|e| VaultError::Storage(format!("delete pending_sync (in tx): {e}")))?;
    Ok(())
}

/// Encode `s` as a JSON string literal (with surrounding quotes, embedded
/// quotes / backslashes / control chars escaped). Used for the
/// `details_json` field — keeps the audit body well-formed JSON without
/// pulling `serde_json::Value` round-trips.
fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"<unserialisable>\"".to_string())
}

// ---------------------------------------------------------------------------
// PendingAuditEvent builder extension — small convenience for the
// orchestrator's audit emissions.
// ---------------------------------------------------------------------------

/// Trait extension so we can chain `.with_details_json("...")` like the
/// other builder methods. Kept private to avoid leaking the helper outside
/// this module.
trait PendingAuditEventExt {
    fn with_details_json(self, json: String) -> Self;
}

impl PendingAuditEventExt for PendingAuditEvent {
    fn with_details_json(mut self, json: String) -> Self {
        self.details_json = json;
        self
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};

    use tempfile::TempDir;

    use vault_core::{Boundary, MemoryType, NewMemory};

    use crate::audit::{AuditEvent, AuditEventType};

    const DIM: usize = 4;

    /// Test-only at-rest key (32 bytes, fixed pattern). Per-mod local
    /// const per HANDOFF sub-task (d) §"Const placement" decision lock;
    /// matches the convention in `tests/migration_v0_1_to_sealed.rs:96`.
    const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

    fn embedding(fill: f32) -> Vec<f32> {
        vec![fill; DIM]
    }

    fn sample_memory(boundary: &str, content: &str) -> Memory {
        Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new(boundary).unwrap(),
            source_agent: Some("test".into()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .unwrap()
    }

    /// Open a backend rooted at a fresh tempdir. Returns the tempdir guard
    /// (must be kept alive — it owns the directory) plus the backend.
    async fn make_backend() -> (TempDir, StorageBackend) {
        let tmp = TempDir::new().unwrap();
        let metadata_path = tmp.path().join("vault.db");
        let vector_dir = tmp.path().join("lance");
        let graph_path = tmp.path().join("graph.duckdb");
        let key = SqlCipherKey::new("cascading-test-key");
        let backend = StorageBackend::open_with_at_rest_key(
            &metadata_path,
            &vector_dir,
            &graph_path,
            key,
            DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .unwrap();
        (tmp, backend)
    }

    // ------------------------------------------------------------------
    // open() degraded-mode reporting
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn open_on_clean_store_is_healthy() {
        let (_tmp, backend) = make_backend().await;
        assert_eq!(backend.degraded(), DegradedMode::Healthy);
    }

    #[tokio::test]
    async fn open_on_corrupted_lance_fragments_returns_lance_unreadable() {
        // Per ADR-018 / Phase A Q5 test 3b: open MUST return Ok with
        // degraded == LanceUnreadable, audit log MUST contain a
        // `store.corruption` event tagged `lancedb_corruption_at_open`,
        // and SQLite-side state MUST remain operational.
        let tmp = TempDir::new().unwrap();
        let metadata_path = tmp.path().join("vault.db");
        let vector_dir = tmp.path().join("lance");
        let graph_path = tmp.path().join("graph.duckdb");
        let key = SqlCipherKey::new("corruption-test-key");

        // First open: write 5 memories so there's data to corrupt.
        {
            let backend = StorageBackend::open_with_at_rest_key(
                &metadata_path,
                &vector_dir,
                &graph_path,
                key.clone(),
                DIM,
                &TEST_AT_REST_KEY,
            )
            .await
            .unwrap();
            for i in 0..5 {
                let m = sample_memory("work", &format!("memory-{i}"));
                backend
                    .write_memory(&m, &embedding(0.1 * i as f32))
                    .await
                    .unwrap();
            }
            // Force the cascade through manually so LanceDB has fragments to corrupt.
            // We do this by upserting directly into the vector store on this path.
            for i in 0..5 {
                let id = MemoryId::new();
                let b = Boundary::new("work").unwrap();
                backend
                    .vector
                    .upsert(&id, &embedding(0.2 * i as f32), &b)
                    .await
                    .unwrap();
            }
        }
        // Drop the backend (releases the LanceDB connection's file locks).

        // Find a Lance fragment file under the vector_dir and clobber its
        // last 64 bytes with 0xAB. Mirrors the spike's corruption shape.
        //
        // Why the FOOTER, not the header: lance v1 (0.8 era) used a
        // header-based file format — the magic + version bytes were at
        // offset 0, so corrupting the first 64 bytes tripped magic-check
        // fast-fail. Lance v2 (4.0+) moved to a footer-based format —
        // the magic ("LANC") + length-prefixed metadata live at the END
        // of the file. Phase 0a-fix (2026-05-07) discovered that
        // header-corruption on a lance v2 file does NOT fail fast: the
        // first 64 bytes are interpreted as data, and a corrupted size
        // field downstream triggers a 32 GB allocation attempt that
        // OOM-aborts the test process. Footer-corruption fails
        // lance's magic-check immediately, no allocation. Same intent
        // (file is unreadable), different offset for the format change.
        // ADR-018's `validate_readable` path still surfaces the
        // corruption regardless.
        let lance_path = find_first_lance_fragment(&vector_dir)
            .expect("expected at least one .lance fragment under the vector data dir");
        {
            let mut f = OpenOptions::new().write(true).open(&lance_path).unwrap();
            f.seek(SeekFrom::End(-64)).unwrap();
            f.write_all(&[0xAB; 64]).unwrap();
            f.sync_all().unwrap();
        }

        // Reopen — must succeed with degraded mode.
        let backend2 = StorageBackend::open_with_at_rest_key(
            &metadata_path,
            &vector_dir,
            &graph_path,
            key,
            DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .unwrap();
        assert_eq!(
            backend2.degraded(),
            DegradedMode::LanceUnreadable,
            "open on corrupted Lance must report LanceUnreadable, not fail"
        );

        // SQLite side must still work: list memories returns the rows we
        // wrote on the first open.
        let memories = backend2
            .metadata
            .list_memories(Default::default(), Some(100))
            .await
            .unwrap();
        assert_eq!(memories.len(), 5);

        // Audit log includes the corruption event, tagged lancedb.
        let events = backend2
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let corruption: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::StoreCorruption)
            .collect();
        assert_eq!(
            corruption.len(),
            1,
            "exactly one store.corruption event expected on second open"
        );
        let detail = &corruption[0].details_json;
        assert!(
            detail.contains("lancedb_corruption_at_open"),
            "details_json should tag the failing store: {detail}"
        );
        assert_eq!(corruption[0].resource_type.as_deref(), Some("store"));
        assert_eq!(corruption[0].resource_id.as_deref(), Some("lancedb"));
    }

    fn find_first_lance_fragment(root: &Path) -> Option<std::path::PathBuf> {
        for entry in walkdir_min(root) {
            let p = entry;
            if p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s == "lance")
                    .unwrap_or(false)
            {
                return Some(p);
            }
        }
        None
    }

    /// Tiny walkdir replacement so we don't pull a workspace dep just for tests.
    fn walkdir_min(root: &Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(p) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&p) else {
                continue;
            };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out
    }

    // ------------------------------------------------------------------
    // Eager validation — DimensionMismatch / invalid-memory rejection
    // (Shahbaz observation #2: rejection-path test)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn write_memory_rejects_dimension_mismatch_with_no_rows_anywhere() {
        // The load-bearing rejection-path test. Per Phase C plan v2
        // observation #2: a wrong-dim write must NOT leave any state on
        // disk — no `memories` row, no `audit_log` entry, no `retry_queue`
        // entry. The lockstep contract (Issue 1) breaks if a permanent
        // failure can sneak past the orchestrator into the worker.
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "wrong-dim memory");

        let wrong_dim = [0.0_f32; DIM + 3];
        let err = backend.write_memory(&m, &wrong_dim).await.unwrap_err();
        match err {
            VaultError::DimensionMismatch { expected, actual } => {
                assert_eq!(expected, DIM);
                assert_eq!(actual, DIM + 3);
            }
            other => panic!("expected DimensionMismatch, got {other:?}"),
        }

        // No memory row.
        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_none());
        // No retry_queue row.
        assert_eq!(backend.retry_queue.len().await.unwrap(), 0);
        // No pending_sync row.
        assert_eq!(backend.pending_sync.len().await.unwrap(), 0);
        // Audit log only contains the eager `get_memory` read above —
        // anything else means the orchestrator wrote.
        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        assert!(
            !events
                .iter()
                .any(|e| e.event_type == AuditEventType::MemoryCreate),
            "no memory.create event should be present after a rejected write"
        );
    }

    #[tokio::test]
    async fn write_memory_rejects_invalid_memory_with_no_rows_anywhere() {
        let (_tmp, backend) = make_backend().await;
        let mut m = sample_memory("work", "bad confidence");
        m.confidence = 5.0; // outside [0, 1]

        let err = backend.write_memory(&m, &embedding(0.1)).await.unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));

        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_none());
        assert_eq!(backend.retry_queue.len().await.unwrap(), 0);
        assert_eq!(backend.pending_sync.len().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn write_memory_rejects_empty_embedding() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "empty embedding");
        // Length 0 takes the dimension-mismatch path (0 != DIM).
        let err = backend.write_memory(&m, &[]).await.unwrap_err();
        assert!(matches!(err, VaultError::DimensionMismatch { .. }));
    }

    #[tokio::test]
    async fn write_memory_rejects_non_finite_embedding() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "nan embedding");
        let mut e = embedding(0.1);
        e[1] = f32::NAN;
        let err = backend.write_memory(&m, &e).await.unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }

    // ------------------------------------------------------------------
    // Standard CRUD paths — write / update / delete
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn write_memory_round_trips_via_metadata_get() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "hello");
        let ack = backend.write_memory(&m, &embedding(0.1)).await.unwrap();
        assert_eq!(ack.memory_id, m.id);

        let back = backend
            .metadata
            .get_memory(&m.id)
            .await
            .unwrap()
            .expect("memory must be persisted");
        // embedding lives in LanceDB on the cascade — the metadata row's
        // embedding stays None.
        let mut expected = m.clone();
        expected.embedding = None;
        assert_eq!(back, expected);
    }

    #[tokio::test]
    async fn write_memory_emits_memory_create_audit_event() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "audited write");
        backend.write_memory(&m, &embedding(0.2)).await.unwrap();

        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let creates: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::MemoryCreate)
            .collect();
        assert_eq!(creates.len(), 1);
        assert_eq!(
            creates[0].resource_id.as_deref(),
            Some(m.id.to_string().as_str())
        );
        assert_eq!(creates[0].boundary.as_deref(), Some("work"));
    }

    #[tokio::test]
    async fn write_memory_enqueues_one_retry_queue_row_with_audit_seq_as_sequence_id() {
        // Cascade ordering invariant (ADR-017): retry_queue.sequence_id
        // equals the audit_log.seq of the corresponding memory.create
        // event. Concurrent writes serialise by SQLite commit order.
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "ordered write");
        backend.write_memory(&m, &embedding(0.3)).await.unwrap();

        assert_eq!(backend.retry_queue.len().await.unwrap(), 1);
        let due = backend
            .retry_queue
            .poll_due(Utc::now() + chrono::Duration::seconds(60), 100)
            .await
            .unwrap();
        assert_eq!(due.len(), 1);
        let entry = &due[0];
        assert_eq!(entry.memory_id, m.id);
        assert_eq!(entry.operation, CascadeOperation::Write);

        // Cross-check against the audit chain: the latest MemoryCreate
        // event's seq must equal the entry's sequence_id.
        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let create_event = events
            .iter()
            .find(|e| e.event_type == AuditEventType::MemoryCreate)
            .expect("memory.create event must exist");
        assert_eq!(entry.sequence_id, create_event.seq);
    }

    #[tokio::test]
    async fn update_memory_round_trip() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "v1 content");
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();

        let mut updated = m.clone();
        updated.content = "v2 content".into();
        backend
            .update_memory(&updated, &embedding(0.2))
            .await
            .unwrap();

        let back = backend.metadata.get_memory(&m.id).await.unwrap().unwrap();
        assert_eq!(back.content, "v2 content");

        // Two retry-queue entries: one Write, one Update. The Update's
        // sequence_id is greater than the Write's (audit seq is monotonic).
        let due = backend
            .retry_queue
            .poll_due(Utc::now() + chrono::Duration::seconds(60), 100)
            .await
            .unwrap();
        assert_eq!(due.len(), 2);
        let ops: Vec<CascadeOperation> = due.iter().map(|e| e.operation).collect();
        assert!(ops.contains(&CascadeOperation::Write));
        assert!(ops.contains(&CascadeOperation::Update));
        let mut seqs: Vec<i64> = due.iter().map(|e| e.sequence_id).collect();
        seqs.sort();
        assert!(seqs[0] < seqs[1]);
    }

    #[tokio::test]
    async fn update_missing_memory_returns_not_found_with_no_state_changes() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "ghost");
        let err = backend
            .update_memory(&m, &embedding(0.4))
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::NotFound(_)));
        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_none());
        assert_eq!(backend.retry_queue.len().await.unwrap(), 0);
    }

    // ------------------------------------------------------------------
    // mark_superseded — ADR-046 (T0.2.3 commit 2)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn mark_superseded_metadata_only_no_vector_write() {
        // ADR-046 invariant: mark_superseded MUST NOT enqueue a cascade
        // row for the vector store. The supersession is a metadata-only
        // state change; the vector layer is untouched. Two-point assertion
        // (baseline + post-call) proves the delta is zero, robust against
        // test-process state leakage per ADR-046 Q3 (locked α).
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "original content");
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();

        // Baseline: exactly one Write enqueue from write_memory.
        let baseline = backend.retry_queue.len().await.unwrap();
        assert_eq!(baseline, 1, "baseline: one Write enqueue from write_memory");

        // Synthetic new_id — mark_superseded only validates old_id's
        // existence, not new_id's. (In production the new memory has
        // already been written by the orchestrator via write_memory before
        // apply_merge calls mark_superseded; we don't re-validate it here.)
        let new_id = MemoryId::new();
        backend.mark_superseded(m.id, new_id).await.unwrap();

        // ADR-046 invariant: delta == 0. No cascade enqueue means no
        // LanceDB upsert + no divergence counter increment downstream.
        let after = backend.retry_queue.len().await.unwrap();
        assert_eq!(
            after, baseline,
            "mark_superseded MUST NOT enqueue a cascade row — \
             supersession is metadata-only"
        );

        // Metadata-side state: superseded_by is set + all other fields
        // unchanged.
        let back = backend.metadata.get_memory(&m.id).await.unwrap().unwrap();
        assert_eq!(back.superseded_by, Some(new_id));
        assert_eq!(back.content, m.content);
        assert_eq!(back.confidence, m.confidence);
        assert_eq!(back.access_count, m.access_count);
        assert_eq!(back.memory_type, m.memory_type);
        assert_eq!(back.boundary, m.boundary);
    }

    #[tokio::test]
    async fn mark_superseded_emits_memory_superseded_audit_event() {
        // ADR-046 invariant: mark_superseded emits the dedicated
        // AuditEventType::MemorySuperseded variant (NOT MemoryUpdate). The
        // event-class discrimination is the load-bearing property for
        // T0.2.15 audit-viewer filtering and for preserving BRD §5.6 line
        // 948 provenance fidelity.
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "supersedable");
        backend.write_memory(&m, &embedding(0.2)).await.unwrap();

        let new_id = MemoryId::new();
        backend.mark_superseded(m.id, new_id).await.unwrap();

        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let supersession = events
            .iter()
            .find(|e| e.event_type == AuditEventType::MemorySuperseded)
            .expect("memory.superseded event must exist after mark_superseded");

        // Resource pairing: old_id at resource_id; new_id at
        // details.superseded_by. Audit viewer joins these two fields to
        // render the supersession chain.
        assert_eq!(supersession.resource_type.as_deref(), Some("memory"));
        assert_eq!(
            supersession.resource_id.as_deref(),
            Some(m.id.to_string().as_str())
        );
        let expected_details_substring = format!(r#""superseded_by":"{new_id}""#);
        assert!(
            supersession
                .details_json
                .contains(&expected_details_substring),
            "details_json {} must contain {}",
            supersession.details_json,
            expected_details_substring
        );

        // Actor: System (consolidator runs as System actor per the
        // existing cascade-path convention).
        assert_eq!(supersession.actor_kind, ActorKind::System);

        // ADR-046 discrimination property: NO MemoryUpdate event was
        // emitted by mark_superseded. Only the initial write_memory's
        // MemoryCreate + the new MemorySuperseded are present. Pins
        // "MemorySuperseded NOT MemoryUpdate" at the test level so
        // future regressions surface immediately.
        let updates: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::MemoryUpdate)
            .collect();
        assert!(
            updates.is_empty(),
            "mark_superseded must NOT emit MemoryUpdate events; found {} update events",
            updates.len()
        );
    }

    // ------------------------------------------------------------------
    // apply_dedup — ADR-063 (deterministic dedup, T0.3.x)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn apply_dedup_supersedes_losers_and_bumps_survivor_metadata_only() {
        let (_tmp, backend) = make_backend().await;
        let survivor = sample_memory("work", "the user drives a Rivian R1T");
        let loser_a = sample_memory("work", "the user drives a Rivian R1T truck");
        let loser_b = sample_memory("work", "the user now drives a Rivian R1T");
        backend
            .write_memory(&survivor, &embedding(0.1))
            .await
            .unwrap();
        backend
            .write_memory(&loser_a, &embedding(0.1))
            .await
            .unwrap();
        backend
            .write_memory(&loser_b, &embedding(0.1))
            .await
            .unwrap();

        // Baseline: three Write enqueues from the three write_memory calls.
        let baseline = backend.retry_queue.len().await.unwrap();

        backend
            .apply_dedup(survivor.id, &[loser_a.id, loser_b.id], 12, 0.95)
            .await
            .unwrap();

        // Metadata-only: apply_dedup MUST NOT enqueue any cascade row
        // (content unchanged → no re-embed, no LanceDB upsert).
        let after = backend.retry_queue.len().await.unwrap();
        assert_eq!(
            after, baseline,
            "apply_dedup must be metadata-only (no cascade enqueue)"
        );

        // Losers superseded → survivor.
        for loser_id in [loser_a.id, loser_b.id] {
            let l = backend
                .metadata
                .get_memory(&loser_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                l.superseded_by,
                Some(survivor.id),
                "loser must be superseded → survivor"
            );
        }

        // Survivor: aggregates rolled, content + supersede-state untouched.
        let s = backend
            .metadata
            .get_memory(&survivor.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s.access_count, 12, "survivor access_count = summed");
        assert!(
            (s.confidence - 0.95).abs() < 1e-6,
            "survivor confidence = max"
        );
        assert_eq!(s.content, survivor.content, "survivor content unchanged");
        assert_eq!(s.superseded_by, None, "survivor is NOT superseded");
    }

    #[tokio::test]
    async fn apply_dedup_emits_deduped_on_survivor_and_superseded_on_losers() {
        let (_tmp, backend) = make_backend().await;
        let survivor = sample_memory("work", "canonical fact");
        let loser = sample_memory("work", "canonical fact dup");
        backend
            .write_memory(&survivor, &embedding(0.1))
            .await
            .unwrap();
        backend.write_memory(&loser, &embedding(0.1)).await.unwrap();

        backend
            .apply_dedup(survivor.id, &[loser.id], 5, 0.9)
            .await
            .unwrap();

        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();

        // Exactly one MemoryDeduped, on the survivor, listing the absorbed loser.
        let deduped: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::MemoryDeduped)
            .collect();
        assert_eq!(deduped.len(), 1, "exactly one memory.deduped event");
        assert_eq!(
            deduped[0].resource_id.as_deref(),
            Some(survivor.id.to_string().as_str())
        );
        assert_eq!(deduped[0].actor_kind, ActorKind::System);
        assert!(
            deduped[0].details_json.contains(&loser.id.to_string()),
            "deduped details must list the absorbed loser id: {}",
            deduped[0].details_json
        );

        // The loser carries a MemorySuperseded event (resource = loser).
        let superseded: Vec<_> = events
            .iter()
            .filter(|e| {
                e.event_type == AuditEventType::MemorySuperseded
                    && e.resource_id.as_deref() == Some(loser.id.to_string().as_str())
            })
            .collect();
        assert_eq!(
            superseded.len(),
            1,
            "loser must have one memory.superseded event"
        );
    }

    #[tokio::test]
    async fn apply_dedup_rejects_empty_losers_and_self_loser() {
        let (_tmp, backend) = make_backend().await;
        let survivor = sample_memory("work", "x");
        backend
            .write_memory(&survivor, &embedding(0.1))
            .await
            .unwrap();

        assert!(
            matches!(
                backend.apply_dedup(survivor.id, &[], 0, 0.9).await,
                Err(VaultError::InvalidInput(_))
            ),
            "empty losers must be rejected"
        );
        assert!(
            matches!(
                backend
                    .apply_dedup(survivor.id, &[survivor.id], 0, 0.9)
                    .await,
                Err(VaultError::InvalidInput(_))
            ),
            "survivor as its own loser must be rejected"
        );
    }

    #[tokio::test]
    async fn apply_dedup_is_atomic_unknown_loser_rolls_back_all() {
        let (_tmp, backend) = make_backend().await;
        let survivor = sample_memory("work", "canonical");
        let real_loser = sample_memory("work", "canonical dup");
        backend
            .write_memory(&survivor, &embedding(0.1))
            .await
            .unwrap();
        backend
            .write_memory(&real_loser, &embedding(0.1))
            .await
            .unwrap();

        let bogus = MemoryId::new();
        // real_loser is processed first (superseded inside the tx), then the
        // bogus id fails → the whole transaction must roll back.
        let result = backend
            .apply_dedup(survivor.id, &[real_loser.id, bogus], 9, 0.9)
            .await;
        assert!(
            matches!(result, Err(VaultError::NotFound(_))),
            "unknown loser → NotFound"
        );

        // Rollback proof: the real loser is NOT superseded and the survivor's
        // aggregates are unchanged.
        let l = backend
            .metadata
            .get_memory(&real_loser.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            l.superseded_by, None,
            "rollback: real loser must NOT be superseded"
        );
        let s = backend
            .metadata
            .get_memory(&survivor.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            s.access_count, survivor.access_count,
            "rollback: survivor aggregates unchanged"
        );
    }

    // ------------------------------------------------------------------
    // invalidate — ADR-051 (T0.2.7 Phase B, bi-temporal storage)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn invalidate_sets_valid_until_and_returns_ack() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "fact that becomes false");
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();

        let cutoff = Utc::now();
        let ack = backend
            .invalidate(m.id, cutoff, "test reason".into())
            .await
            .unwrap();
        assert_eq!(ack.memory_id, m.id);

        let back = backend.metadata.get_memory(&m.id).await.unwrap().unwrap();
        // RFC3339 round-trip can lose sub-millisecond precision; check
        // equality at second resolution to avoid timestamp-noise flakes.
        let stored = back.valid_until.expect("valid_until must be Some");
        assert!(
            (stored - cutoff).num_milliseconds().abs() < 1000,
            "stored {stored} ~ cutoff {cutoff}",
        );
        // ADR-051 orthogonality: invalidate MUST NOT touch superseded_by.
        assert_eq!(
            back.superseded_by, None,
            "invalidate must not touch superseded_by (orthogonal per ADR-051)"
        );
        assert_eq!(back.content, m.content);
    }

    #[tokio::test]
    async fn invalidate_returns_not_found_on_missing_memory() {
        let (_tmp, backend) = make_backend().await;
        let id = MemoryId::new();
        let err = backend
            .invalidate(id, Utc::now(), "no such memory".into())
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::NotFound(_)));
    }

    #[tokio::test]
    async fn invalidate_rejects_valid_until_before_valid_from() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "valid_from is now");
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();

        // valid_from defaults to now() at create; a yesterday cutoff
        // is before it, violating the Memory bi-temporal invariant.
        let yesterday = Utc::now() - chrono::Duration::days(1);
        let err = backend
            .invalidate(m.id, yesterday, "wrong order".into())
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));

        // Memory must remain valid (valid_until still None) after the rejection.
        let back = backend.metadata.get_memory(&m.id).await.unwrap().unwrap();
        assert_eq!(back.valid_until, None);
    }

    #[tokio::test]
    async fn invalidate_latest_wins_on_repeat() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "double-invalidated");
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();

        let first = Utc::now();
        backend
            .invalidate(m.id, first, "first".into())
            .await
            .unwrap();

        // ADR-051 §Decision: latest-wins on repeat invalidation.
        let second = first + chrono::Duration::seconds(60);
        backend
            .invalidate(m.id, second, "second".into())
            .await
            .unwrap();

        let back = backend.metadata.get_memory(&m.id).await.unwrap().unwrap();
        let stored = back.valid_until.expect("valid_until must be Some");
        // Second invalidation wins (later timestamp); first is overwritten.
        assert!(
            (stored - second).num_milliseconds().abs() < 1000,
            "stored {stored} must match second={second} (latest-wins per ADR-051)",
        );
    }

    // ------------------------------------------------------------------
    // apply_decay — Phase 4 confidence decay (T0.2.4)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn apply_decay_updates_confidence_and_marker_metadata_only() {
        let (_tmp, backend) = make_backend().await;
        let mut m = sample_memory("personal", "a fact that has gone cold");
        m.confidence = 0.8;
        backend.write_memory(&m, &embedding(0.3)).await.unwrap();

        let decayed_at = Utc::now();
        let new_metadata = serde_json::json!({
            "decay": { "last_decay_at": decayed_at.to_rfc3339() }
        });
        let ack = backend.apply_decay(m.id, 0.72, new_metadata).await.unwrap();
        assert_eq!(ack.memory_id, m.id);

        let back = backend.metadata.get_memory(&m.id).await.unwrap().unwrap();
        assert!(
            (back.confidence - 0.72).abs() < 1e-6,
            "confidence must be the decayed value, got {}",
            back.confidence
        );
        assert_eq!(
            back.metadata["decay"]["last_decay_at"],
            serde_json::Value::String(decayed_at.to_rfc3339()),
            "decay idempotency marker must be persisted"
        );
        // Metadata-only: content + bi-temporal fields are untouched.
        assert_eq!(back.content, m.content, "decay must not change content");
        assert_eq!(back.valid_until, None, "decay must not invalidate");
        assert_eq!(back.superseded_by, None, "decay must not supersede");
    }

    #[tokio::test]
    async fn apply_decay_returns_not_found_on_missing_memory() {
        let (_tmp, backend) = make_backend().await;
        let err = backend
            .apply_decay(MemoryId::new(), 0.5, serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::NotFound(_)));
    }

    #[tokio::test]
    async fn apply_decay_rejects_out_of_range_confidence() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("personal", "cold fact");
        backend.write_memory(&m, &embedding(0.3)).await.unwrap();
        // 1.5 violates the Memory invariant (confidence ∈ [0,1]); the in-tx
        // validate() must reject and leave the row unchanged.
        let err = backend
            .apply_decay(m.id, 1.5, serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));

        let back = backend.metadata.get_memory(&m.id).await.unwrap().unwrap();
        assert_eq!(
            back.confidence, m.confidence,
            "a rejected decay must not mutate confidence"
        );
    }

    // ------------------------------------------------------------------
    // apply_archive — Phase 4 cold archive (ADR-084)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn apply_archive_sets_marker_metadata_only_and_excludes_from_default_list() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("personal", "a fact untouched for a year");
        backend.write_memory(&m, &embedding(0.3)).await.unwrap();

        let archived_at = Utc::now();
        let ack = backend.apply_archive(m.id, archived_at).await.unwrap();
        assert_eq!(ack.memory_id, m.id);

        // The marker is persisted and the row is unchanged otherwise.
        let back = backend.metadata.get_memory(&m.id).await.unwrap().unwrap();
        assert!(back.is_archived(), "archived_at marker must be set");
        assert_eq!(
            back.archived_at.map(|d| d.to_rfc3339()),
            Some(archived_at.to_rfc3339()),
            "archived_at must round-trip"
        );
        assert_eq!(back.content, m.content, "archive must not change content");
        assert_eq!(back.confidence, m.confidence, "archive must not decay");
        assert_eq!(back.valid_until, None, "archive must not invalidate");
        assert_eq!(back.superseded_by, None, "archive must not supersede");

        // Default list excludes the archived fact; include_archived surfaces it.
        let default_listed = backend
            .list_memories(MemoryFilter::default(), None)
            .await
            .unwrap();
        assert!(
            !default_listed.iter().any(|x| x.id == m.id),
            "archived fact must be excluded from default list (out of retrieval)"
        );
        let with_archived = backend
            .list_memories(
                MemoryFilter {
                    include_archived: true,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert!(
            with_archived.iter().any(|x| x.id == m.id),
            "archived fact must surface when include_archived = true"
        );
    }

    #[tokio::test]
    async fn apply_archive_returns_not_found_on_missing_memory() {
        let (_tmp, backend) = make_backend().await;
        let err = backend
            .apply_archive(MemoryId::new(), Utc::now())
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::NotFound(_)));
    }

    #[tokio::test]
    async fn apply_archive_emits_memory_archived_audit_event() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "archive me");
        backend.write_memory(&m, &embedding(0.2)).await.unwrap();

        let archived_at = Utc::now();
        backend.apply_archive(m.id, archived_at).await.unwrap();

        let events = backend.metadata.list_audit_events(1000).await.unwrap();
        let archived = events
            .iter()
            .find(|e| e.event_type == AuditEventType::MemoryArchived)
            .expect("a memory.archived audit event must be emitted");
        assert_eq!(
            archived.resource_id.as_deref(),
            Some(m.id.to_string().as_str())
        );
        assert!(
            archived.details_json.contains(&archived_at.to_rfc3339()),
            "details_json must carry archived_at, got: {}",
            archived.details_json
        );
    }

    #[tokio::test]
    async fn invalidate_emits_memory_invalidated_audit_event() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "audit me");
        backend.write_memory(&m, &embedding(0.2)).await.unwrap();

        let cutoff = Utc::now();
        backend
            .invalidate(m.id, cutoff, "test audit shape".into())
            .await
            .unwrap();

        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let event = events
            .iter()
            .find(|e| e.event_type == AuditEventType::MemoryInvalidated)
            .expect("memory.invalidated event must exist after invalidate");

        assert_eq!(event.resource_type.as_deref(), Some("memory"));
        assert_eq!(
            event.resource_id.as_deref(),
            Some(m.id.to_string().as_str())
        );
        assert_eq!(event.actor_kind, ActorKind::System);
        // details_json shape per ADR-051: {"valid_until":<RFC3339>,"reason":<text>}.
        // RFC3339 second-resolution prefix is enough for the substring assert.
        let cutoff_prefix = cutoff.format("%Y-%m-%dT%H:%M:%S").to_string();
        assert!(
            event.details_json.contains(&cutoff_prefix),
            "details_json {} must contain cutoff {}",
            event.details_json,
            cutoff_prefix
        );
        assert!(
            event.details_json.contains("test audit shape"),
            "details_json {} must contain the supplied reason",
            event.details_json
        );

        // ADR-051 discrimination property: NO MemoryUpdate event from invalidate.
        let updates: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::MemoryUpdate)
            .collect();
        assert!(
            updates.is_empty(),
            "invalidate must NOT emit MemoryUpdate events; found {} update events",
            updates.len()
        );
    }

    #[tokio::test]
    async fn invalidate_and_mark_superseded_are_orthogonal() {
        // ADR-051 §Decision — relationship to mark_superseded:
        // both fields may be set on the same memory by the Phase C
        // write-time UPDATE decision. Test pins the orthogonality.
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "false-AND-replaced");
        backend.write_memory(&m, &embedding(0.3)).await.unwrap();

        let new_id = MemoryId::new();
        backend.mark_superseded(m.id, new_id).await.unwrap();

        let cutoff = Utc::now();
        backend
            .invalidate(m.id, cutoff, "phase-C compose".into())
            .await
            .unwrap();

        let back = backend.metadata.get_memory(&m.id).await.unwrap().unwrap();
        assert_eq!(back.superseded_by, Some(new_id));
        assert!(back.valid_until.is_some());
        assert_eq!(back.content, m.content);
    }

    #[tokio::test]
    async fn invalidate_does_not_enqueue_cascade_row() {
        // Mirrors mark_superseded's metadata-only contract — invalidate
        // is a metadata-only state change; LanceDB/DuckDB untouched.
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "no cascade");
        backend.write_memory(&m, &embedding(0.4)).await.unwrap();

        let baseline = backend.retry_queue.len().await.unwrap();
        assert_eq!(baseline, 1, "baseline: one Write enqueue from write_memory");

        backend
            .invalidate(m.id, Utc::now(), "no cascade".into())
            .await
            .unwrap();

        let after = backend.retry_queue.len().await.unwrap();
        assert_eq!(
            after, baseline,
            "invalidate MUST NOT enqueue a cascade row — metadata-only mutation"
        );
    }

    #[tokio::test]
    async fn invalidate_accepts_future_dated_valid_until() {
        // ADR-051 §Decision: future-dated valid_until (planned expiration)
        // is allowed. The retrieval-side filter is what decides whether
        // the memory currently surfaces — invalidate() only persists.
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "expires next year");
        backend.write_memory(&m, &embedding(0.5)).await.unwrap();

        let future = Utc::now() + chrono::Duration::days(365);
        backend
            .invalidate(m.id, future, "planned-expiration".into())
            .await
            .unwrap();

        let back = backend.metadata.get_memory(&m.id).await.unwrap().unwrap();
        assert!(back.valid_until.is_some());
        let stored = back.valid_until.unwrap();
        assert!(
            stored > Utc::now(),
            "future-dated valid_until must be preserved, got {stored}"
        );
    }

    #[tokio::test]
    async fn delete_memory_idempotent_on_missing_id() {
        let (_tmp, backend) = make_backend().await;
        let id = MemoryId::new();
        // First delete on a missing id: no error.
        let ack = backend.delete_memory(&id).await.unwrap();
        assert_eq!(ack.memory_id, id);
        // No retry-queue entry — nothing to cascade.
        assert_eq!(backend.retry_queue.len().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn delete_memory_removes_row_and_enqueues_delete_cascade() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "doomed");
        backend.write_memory(&m, &embedding(0.5)).await.unwrap();
        // Confirm preconditions.
        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_some());
        let pre_count = backend.retry_queue.len().await.unwrap();
        assert_eq!(pre_count, 1);

        backend.delete_memory(&m.id).await.unwrap();
        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_none());

        // Two entries now (Write + Delete).
        assert_eq!(backend.retry_queue.len().await.unwrap(), 2);
    }

    // ------------------------------------------------------------------
    // Cap-overflow → pending_sync (Phase A Q2 cap-overflow refinement)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn write_when_queue_at_cap_routes_to_pending_sync_and_emits_overflow_audit() {
        // Pre-fill retry_queue beyond the cap by inserting raw rows
        // (avoids 10k cascade calls in the test). We need exactly
        // MAX_RETRY_QUEUE_DEPTH rows.
        let (_tmp, backend) = make_backend().await;

        // Backdoor: fill retry_queue to cap by direct UPSERT inside a tx.
        backend
            .metadata
            .with_transaction(|tx| {
                use uuid::Uuid;
                let now = Utc::now();
                for i in 0..MAX_RETRY_QUEUE_DEPTH {
                    let id = Uuid::now_v7();
                    let mem = MemoryId::new();
                    let payload = serde_json::json!({"embedding": [], "boundary": "work"});
                    tx.execute(
                        "INSERT INTO retry_queue (
                            id, memory_id, operation, payload_format_version,
                            payload, sequence_id, attempts_made,
                            next_attempt_at, created_at, last_error
                        ) VALUES (?1, ?2, 'write', 1, ?3, ?4, 0, ?5, ?5, NULL)",
                        params![
                            id.as_bytes().to_vec(),
                            mem.0.as_bytes().to_vec(),
                            serde_json::to_vec(&payload).unwrap(),
                            i as i64 + 100, // arbitrary monotonic sequence_id
                            now.to_rfc3339(),
                        ],
                    )
                    .map_err(|e| VaultError::Storage(e.to_string()))?;
                }
                Ok::<_, VaultError>(())
            })
            .await
            .unwrap();
        assert_eq!(
            backend.retry_queue.len().await.unwrap(),
            MAX_RETRY_QUEUE_DEPTH
        );

        // Now write — the retry_queue is at cap, so the cascade row
        // should land in pending_sync and a CascadeQueueOverflow audit
        // event should fire (first overflow in this session).
        let m = sample_memory("work", "overflowed");
        backend.write_memory(&m, &embedding(0.7)).await.unwrap();

        // SQLite-side state: memory committed.
        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_some());
        // Retry queue length unchanged (still at cap).
        assert_eq!(
            backend.retry_queue.len().await.unwrap(),
            MAX_RETRY_QUEUE_DEPTH
        );
        // pending_sync now carries this memory_id.
        let entry = backend.pending_sync.get(m.id).await.unwrap().unwrap();
        assert_eq!(entry.operation, CascadeOperation::Write);

        // Audit log contains exactly one CascadeQueueOverflow event.
        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let overflows: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::CascadeQueueOverflow)
            .collect();
        assert_eq!(
            overflows.len(),
            1,
            "exactly one cascade.queue_overflow event expected on first overflow"
        );

        // Second overflow within the same session: NO additional overflow event
        // (debounced).
        let m2 = sample_memory("work", "overflowed-again");
        backend.write_memory(&m2, &embedding(0.8)).await.unwrap();
        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let overflows_after_second: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::CascadeQueueOverflow)
            .collect();
        assert_eq!(
            overflows_after_second.len(),
            1,
            "second overflowing write should not emit another overflow event (debounced)"
        );
        assert!(backend.pending_sync.get(m2.id).await.unwrap().is_some());
    }

    // ------------------------------------------------------------------
    // Mid-cascade abort recovery (Phase A Q5 test 1)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn mid_cascade_abort_recovers_via_retry_queue() {
        // Simulates process exit between SQLite ack and the LanceDB-side
        // cascade. After the orchestrator drops, a fresh orchestrator on
        // the same files must see the retry_queue row and be able to
        // re-drive it. We don't wire the worker here — that's
        // retry_worker.rs's territory — we just assert the persistence
        // surface that the worker consumes.
        let tmp = TempDir::new().unwrap();
        let metadata_path = tmp.path().join("vault.db");
        let vector_dir = tmp.path().join("lance");
        let graph_path = tmp.path().join("graph.duckdb");
        let key = SqlCipherKey::new("recover-test-key");

        let m = sample_memory("work", "recover me");
        let id = m.id;
        {
            let backend1 = StorageBackend::open_with_at_rest_key(
                &metadata_path,
                &vector_dir,
                &graph_path,
                key.clone(),
                DIM,
                &TEST_AT_REST_KEY,
            )
            .await
            .unwrap();
            backend1.write_memory(&m, &embedding(0.9)).await.unwrap();
        }
        // Simulated process exit: backend1 dropped here. Files persist.

        let backend2 = StorageBackend::open_with_at_rest_key(
            &metadata_path,
            &vector_dir,
            &graph_path,
            key,
            DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .unwrap();

        // SQLite has the memory.
        assert!(backend2.metadata.get_memory(&id).await.unwrap().is_some());
        // Retry queue still has the cascade entry waiting to be drained.
        let due = backend2
            .retry_queue
            .poll_due(Utc::now() + chrono::Duration::seconds(60), 100)
            .await
            .unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].memory_id, id);
        assert_eq!(due[0].operation, CascadeOperation::Write);
        let payload: CascadePayloadV1 = serde_json::from_value(due[0].payload.clone()).unwrap();
        assert_eq!(payload.embedding.len(), DIM);
        assert_eq!(payload.boundary, "work");
    }

    // ------------------------------------------------------------------
    // Ack contents
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn ack_carries_memory_id_and_recent_committed_at() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "ack content");
        let before = Utc::now() - chrono::Duration::seconds(1);
        let ack = backend.write_memory(&m, &embedding(0.1)).await.unwrap();
        let after = Utc::now() + chrono::Duration::seconds(1);
        assert_eq!(ack.memory_id, m.id);
        assert!(ack.sqlite_committed_at >= before);
        assert!(ack.sqlite_committed_at <= after);
    }

    // ------------------------------------------------------------------
    // DegradedMode helper
    // ------------------------------------------------------------------

    #[test]
    fn degraded_mode_is_degraded_helper() {
        assert!(!DegradedMode::Healthy.is_degraded());
        assert!(DegradedMode::LanceUnreadable.is_degraded());
        assert!(DegradedMode::GraphUnreadable.is_degraded());
        assert!(DegradedMode::BothUnreadable.is_degraded());
    }
}
