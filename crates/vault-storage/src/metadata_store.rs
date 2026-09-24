//! [`MetadataStore`] — the SQLite/SQLCipher-backed durable record-of-truth
//! for memory metadata, audit events, and migration state (BRD §5.2).
//!
//! Vector embeddings live in LanceDB (T0.1.4). Graph entities and
//! relationships live in DuckDB (T0.1.5). This store is the *metadata*
//! authority — when those other stores disagree with this one, this one
//! wins (cascading writes in T0.1.6 will reconcile).
//!
//! ## Concurrency
//!
//! `rusqlite::Connection` is single-threaded by design. We wrap a single
//! connection in `std::sync::Mutex` and run all DB work inside
//! [`tokio::task::spawn_blocking`] so async callers don't block the
//! runtime. For V0.1's expected throughput (handfuls of writes per
//! second, perhaps tens) this is enough; a real connection pool can
//! land later if profiling demands it.
//!
//! ## Atomicity
//!
//! Every CRUD operation runs inside a transaction that *also* appends
//! the corresponding audit event. Either both happen or neither — there
//! is no observable state where a memory was written but its audit event
//! was lost.

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row, Transaction};
use tracing::{debug, instrument, warn};
use uuid::Uuid;

use vault_core::{Boundary, Memory, MemoryId, MemoryType, VaultError, VaultResult};

use crate::audit::{
    seal, ActorKind, AuditEvent, AuditEventType, AuditResult, PendingAuditEvent, AUDIT_GENESIS_HASH,
};
use crate::key::SqlCipherKey;
use crate::migrations;

/// Filter for [`MetadataStore::list_memories`]. All fields default to "no filter."
#[derive(Clone, Debug, Default)]
pub struct MemoryFilter {
    /// If set, only return memories in this boundary.
    pub boundary: Option<Boundary>,
    /// If set, only return memories of this type.
    pub memory_type: Option<MemoryType>,
    /// If `false` (default), exclude memories that have been superseded by a
    /// consolidator merge. Set to `true` to include them (used by audit /
    /// debugging tooling, never by retrieval).
    pub include_superseded: bool,
    /// If set, only return memories with `created_at >= since`. Used by
    /// T0.2.5 (Checkpoint & Rollback) to drive incremental consolidation
    /// scope — the consolidator processes only memories added since the
    /// last checkpoint. T0.2.2 ships with the parameter forward-compat
    /// but always passes `None` (full-scan); T0.2.5 wires actual values.
    /// See ADR-045 §f.
    pub since: Option<DateTime<Utc>>,
    /// If set, only return memories whose content was true (per
    /// bi-temporal `valid_until`) at this point in time. Specifically:
    /// `valid_until IS NULL` (still valid) OR `valid_until > valid_at`
    /// (becomes false in the future). When `None` (default), no
    /// `valid_until` filter is applied — both expired and non-expired
    /// memories are returned. Per ADR-051 (T0.2.7 Phase B). V0.2
    /// retrieval consumers pass `Some(Utc::now())` for "currently true"
    /// queries; admin/audit callers leave it `None`.
    pub valid_at: Option<DateTime<Utc>>,
    /// If `false` (default), exclude cold-archived memories (`archived_at IS
    /// NOT NULL`) — the consolidator Phase-4 archive state (ADR-084). Set to
    /// `true` for the explicit "search archive" path or admin/audit tooling.
    /// Mirrors `include_superseded`: default retrieval and consolidator passes
    /// never want archived facts; only an explicit archive search does.
    pub include_archived: bool,
    /// Keyset cursor for paging (ADR-108 D2, the desktop's export): only
    /// memories that come strictly AFTER `(created_at, id)` in the list's
    /// order, newest first. A cursor, not an offset, so a memory saved
    /// mid-export is neither repeated nor makes another one skipped.
    pub before: Option<(DateTime<Utc>, MemoryId)>,
}

/// Async, encrypted SQLite-backed metadata store. Cheap to clone (it holds
/// an `Arc` internally), share freely across tasks.
///
/// Intentionally does **not** implement `Debug`: it owns a live SQLCipher
/// connection holding key-derived state, and we don't want a habit of
/// stubbing `Debug` on types that hold sensitive handles. See ADR-007.
#[derive(Clone)]
pub struct MetadataStore {
    inner: Arc<Inner>,
}

struct Inner {
    conn: Mutex<Connection>,
}

impl Inner {
    fn lock(&self) -> VaultResult<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| VaultError::Storage(format!("connection mutex poisoned: {e}")))
    }
}

impl MetadataStore {
    /// Open or create an encrypted SQLite database at `path`.
    ///
    /// On a fresh database, schema migrations are applied automatically
    /// (idempotent — safe to call repeatedly).
    ///
    /// # Errors
    ///
    /// - [`VaultError::Storage`] if the path is unreachable, the key is
    ///   wrong (the verification query fails), or migrations fail.
    pub async fn open(path: impl AsRef<Path>, key: SqlCipherKey) -> VaultResult<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || {
            Self::open_blocking(
                &path,
                &key,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            )
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Open the database only if it is already there: `Ok(None)` when there is
    /// no file, and nothing is ever created (ADR-108's amendment to ADR-104 —
    /// a locked keeper serving the export must never make a vault). Read-write,
    /// because the audit chain is appended, with migrations as [`Self::open`].
    ///
    /// # Errors
    ///
    /// As [`Self::open`]; also when whether the file exists cannot be told.
    pub async fn open_existing(
        path: impl AsRef<Path>,
        key: SqlCipherKey,
    ) -> VaultResult<Option<Self>> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || {
            let present = path
                .try_exists()
                .map_err(|e| VaultError::Storage(format!("check {}: {e}", path.display())))?;
            if !present {
                return Ok(None);
            }
            // No CREATE flag: a file removed in the meantime is an error,
            // never a fresh database.
            Self::open_blocking(&path, &key, OpenFlags::SQLITE_OPEN_READ_WRITE).map(Some)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    fn open_blocking(path: &Path, key: &SqlCipherKey, flags: OpenFlags) -> VaultResult<Self> {
        let mut conn = Connection::open_with_flags(path, flags)
            .map_err(|e| VaultError::Storage(format!("open {}: {e}", path.display())))?;

        // SQLCipher: set the key BEFORE any other PRAGMA / query.
        // pragma_update with `key` accepts the raw passphrase; SQLCipher
        // derives the AES key internally via PBKDF2.
        conn.pragma_update(None, "key", key.as_str())
            .map_err(|e| VaultError::Storage(format!("set sqlcipher key: {e}")))?;

        // Verify the key by issuing a query that must succeed when the
        // database is correctly decrypted. Without a valid key this
        // returns "file is not a database" (or similar).
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|e| VaultError::Storage(format!("sqlcipher key verification failed: {e}")))?;

        // Sensible defaults: WAL for concurrent readers, foreign keys on,
        // synchronous = NORMAL (durable enough for a personal device DB).
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| VaultError::Storage(format!("enable WAL: {e}")))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| VaultError::Storage(format!("enable foreign_keys: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| VaultError::Storage(format!("set synchronous: {e}")))?;

        migrations::run(&mut conn)?;

        Ok(Self {
            inner: Arc::new(Inner {
                conn: Mutex::new(conn),
            }),
        })
    }

    /// Insert a new memory. Atomic with the corresponding audit event.
    ///
    /// # Errors
    ///
    /// - [`VaultError::InvalidInput`] if `memory.validate()` rejects
    /// - [`VaultError::Storage`] if the row already exists or insertion fails
    #[instrument(skip(self, memory), fields(memory_id = %memory.id))]
    pub async fn create_memory(&self, memory: &Memory) -> VaultResult<()> {
        memory.validate()?;
        let memory = memory.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            tx_insert_memory(&tx, &memory)?;
            tx_append_audit(
                &tx,
                PendingAuditEvent::success(AuditEventType::MemoryCreate, ActorKind::System)
                    .with_resource("memory", memory.id.to_string())
                    .with_boundary(memory.boundary.clone()),
            )?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Look up a memory by ID. Atomic with a `memory.read` audit event.
    ///
    /// Returns `Ok(None)` for a missing ID (still records a `Success` audit
    /// event with `result.found = false` so the access pattern remains visible).
    #[instrument(skip(self), fields(memory_id = %id))]
    pub async fn get_memory(&self, id: &MemoryId) -> VaultResult<Option<Memory>> {
        let id = *id;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            let memory = tx_get_memory(&tx, &id)?;
            let mut pending =
                PendingAuditEvent::success(AuditEventType::MemoryRead, ActorKind::System)
                    .with_resource("memory", id.to_string());
            pending.details_json = format!(r#"{{"found":{}}}"#, memory.is_some());
            tx_append_audit(&tx, pending)?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))?;
            Ok(memory)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Replace an existing memory. Atomic with the corresponding audit event.
    ///
    /// # Errors
    ///
    /// - [`VaultError::InvalidInput`] if `memory.validate()` rejects
    /// - [`VaultError::NotFound`] if no memory with that ID exists
    /// - [`VaultError::Storage`] on transaction or row-update failure
    #[instrument(skip(self, memory), fields(memory_id = %memory.id))]
    pub async fn update_memory(&self, memory: &Memory) -> VaultResult<()> {
        memory.validate()?;
        let memory = memory.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            let rows = tx_update_memory(&tx, &memory)?;
            if rows == 0 {
                return Err(VaultError::NotFound(format!(
                    "memory {} does not exist",
                    memory.id
                )));
            }
            tx_append_audit(
                &tx,
                PendingAuditEvent::success(AuditEventType::MemoryUpdate, ActorKind::System)
                    .with_resource("memory", memory.id.to_string())
                    .with_boundary(memory.boundary.clone()),
            )?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Delete a memory by ID. Atomic with the corresponding audit event.
    ///
    /// Idempotent: deleting a non-existent ID is a no-op success. The
    /// audit log records every call with `details.deleted = true|false`.
    #[instrument(skip(self), fields(memory_id = %id))]
    pub async fn delete_memory(&self, id: &MemoryId) -> VaultResult<()> {
        let id = *id;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            let rows = tx
                .execute(
                    "DELETE FROM memories WHERE id = ?1",
                    params![id.to_string()],
                )
                .map_err(|e| VaultError::Storage(format!("delete memory: {e}")))?;
            let mut pending =
                PendingAuditEvent::success(AuditEventType::MemoryDelete, ActorKind::System)
                    .with_resource("memory", id.to_string());
            pending.details_json = format!(r#"{{"deleted":{}}}"#, rows > 0);
            tx_append_audit(&tx, pending)?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// List memories matching `filter`, capped at `limit` results.
    /// Always emits exactly one `memory.list` audit event regardless of
    /// how many rows match.
    ///
    /// `limit: None` returns ALL matching rows (no SQL `LIMIT` clause).
    /// Used by `vault-consolidator`'s clustering primitive for
    /// boundary-scoped enumeration where the algorithm needs every
    /// memory in scope (BRD §5.6 Phase 1). `limit: Some(N)` caps results
    /// at `N`. The semantic is locked here — `None` is intentional
    /// unboundedness, NOT a "page size of zero" footgun.
    ///
    /// At V0.2 alpha scale (100-1000 memories per vault) the unbounded
    /// case is bounded by the vault size itself. V0.3+ revisits if
    /// vaults grow to 10k+ memories with measurable memory pressure —
    /// see ADR-045 §f pagination forward-compat call.
    #[instrument(skip(self), fields(limit = ?limit))]
    pub async fn list_memories(
        &self,
        filter: MemoryFilter,
        limit: Option<usize>,
    ) -> VaultResult<Vec<Memory>> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            let memories = tx_list_memories(&tx, &filter, limit)?;
            let mut pending =
                PendingAuditEvent::success(AuditEventType::MemoryList, ActorKind::System);
            if let Some(b) = filter.boundary.clone() {
                pending = pending.with_boundary(b);
            }
            pending.details_json = format!(r#"{{"count":{}}}"#, memories.len());
            tx_append_audit(&tx, pending)?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))?;
            Ok(memories)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Hydrate a batch of memories by ID, preserving input order. IDs not
    /// present in the store are silently omitted from the returned `Vec`
    /// — a `warn!` is logged for each so an operator reviewing logs sees
    /// the orphan IDs. Used by `vault_retrieval::SemanticRetriever::retrieve`
    /// to hydrate the top-k vector hits in a single round-trip
    /// (Q10 in `T0.1.8_PLAN.md`); also intended for T0.2.7 reranking which
    /// fetches top-50 candidates.
    ///
    /// **Order preservation contract.** SQLite returns `IN (...)` rows in
    /// unspecified order; this method re-sorts in-process so the caller can
    /// zip the returned `Memory` rows against parallel score / distance
    /// vectors. With N ≤ 100 (V0.1's `max_results` cap), the in-process
    /// `O(N^2)` reconstruction is trivially fast — no `HashMap` needed.
    ///
    /// **Audit policy: this method does NOT append a per-ID `memory.read`
    /// audit event.** Doing so would inflate the audit log linearly with
    /// every retrieval. Per T0.1.9 §6 audit-removal sub-phase, the
    /// retrieval pipeline (`SemanticRetriever::retrieve`) does NOT
    /// audit either — it emits operational `tracing::info!` only.
    /// Audit accounting moved up to vault-mcp, which appends a single
    /// `AuditEventType::McpToolInvoke` event per tool dispatch (see
    /// ADR-024 for the locked details_json schema).
    ///
    /// **SQLite parameter limit.** SQLite's default `SQLITE_LIMIT_VARIABLE_NUMBER`
    /// is 32,766 in modern builds. We reject `ids.len() > 32_000` as
    /// `InvalidInput` rather than silently splitting into multiple
    /// queries — V0.2's caller is bounded by `max_results = 200` (post
    /// 2026-05-18 raise from 100 for value-aware retrieval), and any
    /// caller approaching the SQLite limit deserves to see the error.
    ///
    /// # Errors
    ///
    /// - [`VaultError::InvalidInput`] if `ids.len() > 32_000`.
    /// - [`VaultError::Storage`] on SQL prepare / execute / row-decode failure.
    #[instrument(skip(self, ids), fields(id_count = ids.len()))]
    pub async fn get_memories_batch(&self, ids: &[MemoryId]) -> VaultResult<Vec<Memory>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        if ids.len() > 32_000 {
            return Err(VaultError::InvalidInput(format!(
                "get_memories_batch: {} ids exceeds SQLite parameter cap (32,000)",
                ids.len()
            )));
        }
        let ids = ids.to_vec();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock()?;
            // Build "?1, ?2, ?3, ..." placeholders. We never splice ID
            // strings into the query — UUIDs are bound as parameters.
            let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
            let sql = format!(
                "SELECT id, content, memory_type, source_agent, boundary,
                        created_at, valid_from, valid_until,
                        confidence, access_count, last_accessed,
                        superseded_by, metadata_json, archived_at
                 FROM memories WHERE id IN ({})",
                placeholders.join(", ")
            );
            let bindings: Vec<rusqlite::types::Value> =
                ids.iter().map(|id| id.to_string().into()).collect();
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| VaultError::Storage(format!("prepare get_memories_batch: {e}")))?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(bindings.iter()), row_to_memory)
                .map_err(|e| VaultError::Storage(format!("query get_memories_batch: {e}")))?;
            let mut found: Vec<Memory> = Vec::with_capacity(ids.len());
            for r in rows {
                found.push(r.map_err(|e| VaultError::Storage(format!("decode memory row: {e}")))?);
            }
            // Reconstruct in input order. `swap_remove` is O(1); the
            // outer loop is O(N) over input ids, inner `position` is
            // O(remaining) — overall O(N^2). Fine for N ≤ 100.
            let mut out = Vec::with_capacity(ids.len());
            for id in &ids {
                if let Some(pos) = found.iter().position(|m| m.id == *id) {
                    out.push(found.swap_remove(pos));
                } else {
                    warn!(
                        memory_id = %id,
                        "get_memories_batch: id not found in metadata store; omitting"
                    );
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Append a caller-supplied audit event. Used by other crates
    /// (vault-mcp, vault-sync, etc.) when they need to record their own
    /// security-relevant events.
    pub async fn append_audit_event(&self, pending: PendingAuditEvent) -> VaultResult<AuditEvent> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            let event = tx_append_audit(&tx, pending)?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))?;
            Ok(event)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Read the entire audit log up to `limit` entries, ordered by `seq`
    /// ascending (oldest first).
    pub async fn list_audit_events(&self, limit: usize) -> VaultResult<Vec<AuditEvent>> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock()?;
            let mut stmt = conn
                .prepare(AUDIT_SELECT_SQL)
                .map_err(|e| VaultError::Storage(format!("prepare audit select: {e}")))?;
            let rows = stmt
                .query_map(params![limit as i64], row_to_audit_event)
                .map_err(|e| VaultError::Storage(format!("query audit events: {e}")))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| VaultError::Storage(format!("read audit row: {e}")))?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Walk the entire audit log and verify the BLAKE3 hash chain.
    /// Returns `Ok(())` for a healthy chain, or a [`VaultError::Storage`]
    /// pinpointing the first inconsistency.
    pub async fn verify_audit_chain(&self) -> VaultResult<()> {
        // Implementation note: we read the chain in pages so a giant log
        // doesn't have to fit in memory. For V0.1 a single fetch is fine.
        let events = self.list_audit_events(usize::MAX).await?;
        crate::audit::verify_chain(&events)
    }

    /// Run `f` against the underlying SQLite [`Connection`] inside
    /// [`tokio::task::spawn_blocking`]. Used by sibling modules in the same
    /// crate (`retry_queue`, `dead_letter`, `pending_sync`) so they can
    /// CRUD their own tables without each opening a separate connection.
    ///
    /// `f` receives `&mut Connection` and is responsible for any transaction
    /// scoping itself. For atomic cross-table writes, prefer
    /// [`Self::with_transaction`].
    pub(crate) async fn with_conn_blocking<F, R>(&self, f: F) -> VaultResult<R>
    where
        F: FnOnce(&mut Connection) -> VaultResult<R> + Send + 'static,
        R: Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            f(&mut conn)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Run `f` inside a single SQLite transaction on the underlying
    /// [`Connection`]. Per ADR-016 (T0.1.6 Phase C), this is the helper the
    /// cascading orchestrator uses to span atomic writes across `memories`
    /// + `audit_log` + `retry_queue` / `pending_sync`.
    ///
    /// **Closure contract:** receives `&Transaction<'_>`, **not**
    /// `&mut Connection` and **not** `Transaction<'_>` by value. The
    /// closure cannot call `commit()` or `rollback()` itself — the helper
    /// owns commit on `Ok` and lets the transaction drop (i.e., rollback)
    /// on `Err`. This means the closure can't accidentally half-commit a
    /// cross-table write. The trade-off: closures that need to interleave
    /// SQL with non-DB work (e.g., spawning subtasks) should not use this
    /// helper — keep them in `with_conn_blocking` where they own scope.
    ///
    /// **`#[allow(dead_code)]` rationale:** the production consumer is the
    /// cascading orchestrator that lands in C1b. C1a ships this helper +
    /// its tests so the API surface is locked, but no production caller
    /// exists yet. Consumed in the next commit; allow until then.
    #[allow(dead_code)]
    pub(crate) async fn with_transaction<F, T>(&self, f: F) -> VaultResult<T>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> VaultResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            let out = f(&tx)?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit tx: {e}")))?;
            Ok(out)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }
}

// ---------------------------------------------------------------------------
// SQLCipher passphrase primitives — for ADR-041 V0.1 → V0.2 bridge.
// Free fns (not on MetadataStore) because they're one-shot transition
// operations distinct from normal store ops. Sync — caller wraps in
// spawn_blocking if needed.
// ---------------------------------------------------------------------------

/// Open a SQLCipher file and verify the given passphrase unlocks it.
///
/// Returns `Ok(())` if the passphrase decrypts the file (verified by a
/// schema query — SQLCipher defers verification to first query, NOT to
/// the `PRAGMA key` set itself). Returns `VaultError::Storage` with a
/// descriptive message on any failure (open, set-key, query).
///
/// **Used by ADR-041 bridge step 1** (per HANDOFF.md ADR-041 plan
/// iteration 2 §3) to fail-fast on a wrong V0.1 VAULT_KEY env value
/// BEFORE writing a new keychain entry — avoids creating a keychain
/// entry that points to a master_key for a vault the bridge can't
/// actually rekey.
///
/// Connection drops at function return — file lock released. Same
/// `OpenFlags::SQLITE_OPEN_NO_MUTEX` shape as `rekey_in_place` (single-
/// threaded one-shot use; no concurrent access).
#[instrument(skip(path, passphrase), fields(path = %path.display()))]
pub fn verify_sqlcipher_passphrase(path: &Path, passphrase: &SqlCipherKey) -> VaultResult<()> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| VaultError::Storage(format!("verify: open {}: {e}", path.display())))?;
    conn.pragma_update(None, "key", passphrase.as_str())
        .map_err(|e| VaultError::Storage(format!("verify: set key: {e}")))?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|e| {
        VaultError::Storage(format!(
            "verify: passphrase verification failed (wrong passphrase or file not \
             a SQLCipher database): {e}"
        ))
    })?;
    Ok(())
}

/// Re-encrypt a SQLCipher file in place from `old_passphrase` to
/// `new_passphrase`.
///
/// **Sequence (per ADR-041 plan iteration 2 §3 + §10 post-write
/// verification invariant):**
///
/// 1. Open `path` with `old_passphrase`. Schema-query verify (catches
///    "wrong old passphrase" before any rekey attempt).
/// 2. `PRAGMA rekey '<new_passphrase>'` — re-encrypts every page in
///    place with the new key.
/// 3. Close the connection.
/// 4. **Reopen with `new_passphrase` + schema-query verify** — load-bearing
///    per ADR-041 §10 post-write verification invariant: SQLCipher's
///    `PRAGMA rekey` may return Ok with the changes living only in the
///    connection's memory (spike Read-only probe 2026-05-11 — Outcome 3).
///    Without this verify step, multiple silent-failure modes (read-only
///    file, disk-full, antivirus locks, ACL changes, network filesystem
///    flush quirks) would leave `path` actually still keyed by
///    `old_passphrase` while the caller proceeds as though it's now keyed
///    by `new_passphrase`.
///
/// **Caller responsibility (per ADR-041 §2 cross-store snapshot-commit
/// invariant Pattern B):** the caller MUST take a pre-rekey snapshot of
/// `path` BEFORE invoking this primitive, AND MUST roll back via snapshot
/// restore on Err return. This primitive does NOT take or manage the
/// snapshot — that's the caller's composition responsibility.
///
/// # Errors
///
/// - `VaultError::Storage` wrapping any rusqlite open / pragma / query
///   failure. Error messages include the failing step name for triage.
/// - Wrong `old_passphrase` surfaces as schema-query Err at step 1
///   (NOT as PRAGMA-key Err — SQLCipher defers verification to first
///   query).
///
/// # Idempotency
///
/// Calling `rekey_in_place(path, K, K)` (same passphrase both sides) is
/// safe but pointless — SQLCipher rewrites every page with the same key.
/// Caller should compare passphrases before invoking if avoiding the
/// rewrite is important.
///
/// # See also
///
/// - ADR-041 plan iteration 2 §3 step 6-7 (the bridge sequence calling
///   this primitive).
/// - HANDOFF.md ADR-041 §10 post-write verification invariant
///   (rationale for the close+reopen+verify step).
/// - `crates/vault-storage/examples/sqlcipher_rekey_spike.rs` (compile-
///   and-run runtime evidence the primitive works on this dep chain).
#[instrument(skip(path, old_passphrase, new_passphrase), fields(path = %path.display()))]
pub fn rekey_in_place(
    path: &Path,
    old_passphrase: &SqlCipherKey,
    new_passphrase: &SqlCipherKey,
) -> VaultResult<()> {
    // Step 1: open + verify with old passphrase.
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| VaultError::Storage(format!("rekey: open {}: {e}", path.display())))?;
    conn.pragma_update(None, "key", old_passphrase.as_str())
        .map_err(|e| VaultError::Storage(format!("rekey: set old key: {e}")))?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|e| {
        VaultError::Storage(format!(
            "rekey: old-passphrase verification failed (wrong old passphrase or \
             file not a SQLCipher database): {e}"
        ))
    })?;

    // Step 2: PRAGMA rekey to new passphrase.
    conn.pragma_update(None, "rekey", new_passphrase.as_str())
        .map_err(|e| VaultError::Storage(format!("rekey: PRAGMA rekey: {e}")))?;

    // Step 3: close the connection so the next open sees committed state.
    drop(conn);

    // Step 4: reopen with NEW passphrase + verify (post-write verification
    // invariant per ADR-041 §10 — load-bearing, not belt-and-suspenders).
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| VaultError::Storage(format!("rekey: post-rekey reopen: {e}")))?;
    conn.pragma_update(None, "key", new_passphrase.as_str())
        .map_err(|e| VaultError::Storage(format!("rekey: post-rekey set new key: {e}")))?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|e| {
        VaultError::Storage(format!(
            "rekey: post-rekey verification failed (PRAGMA rekey returned Ok but \
             reopen with new passphrase fails — silent in-memory rekey or write \
             not persisted): {e}"
        ))
    })?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Transaction-bound helpers (sync). Centralised so the test harness can
// exercise them too if needed and so all SQL lives in one place.
//
// `pub(crate)` so the cascading orchestrator (`cascading.rs`) can invoke
// them inside [`MetadataStore::with_transaction`] — the cascading-write
// atomicity contract spans `memories` + `audit_log` + `retry_queue` /
// `pending_sync` and must run inside one SQLite transaction.
// ---------------------------------------------------------------------------

pub(crate) fn tx_insert_memory(tx: &Transaction<'_>, m: &Memory) -> VaultResult<()> {
    tx.execute(
        "INSERT INTO memories (
            id, content, memory_type, source_agent, boundary,
            created_at, valid_from, valid_until,
            confidence, access_count, last_accessed,
            superseded_by, metadata_json, archived_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            m.id.to_string(),
            m.content,
            memory_type_to_str(m.memory_type),
            m.source_agent,
            m.boundary.as_str(),
            m.created_at.to_rfc3339(),
            m.valid_from.to_rfc3339(),
            m.valid_until.map(|d| d.to_rfc3339()),
            m.confidence as f64,
            m.access_count as i64,
            m.last_accessed.to_rfc3339(),
            m.superseded_by.map(|id| id.to_string()),
            m.metadata.to_string(),
            m.archived_at.map(|d| d.to_rfc3339()),
        ],
    )
    .map(|_| ())
    .map_err(|e| VaultError::Storage(format!("insert memory {}: {e}", m.id)))
}

pub(crate) fn tx_get_memory(tx: &Transaction<'_>, id: &MemoryId) -> VaultResult<Option<Memory>> {
    tx.query_row(MEMORY_SELECT_SQL, params![id.to_string()], row_to_memory)
        .optional()
        .map_err(|e| VaultError::Storage(format!("get memory {id}: {e}")))
}

pub(crate) fn tx_update_memory(tx: &Transaction<'_>, m: &Memory) -> VaultResult<usize> {
    tx.execute(
        "UPDATE memories SET
            content = ?2, memory_type = ?3, source_agent = ?4, boundary = ?5,
            created_at = ?6, valid_from = ?7, valid_until = ?8,
            confidence = ?9, access_count = ?10, last_accessed = ?11,
            superseded_by = ?12, metadata_json = ?13, archived_at = ?14
         WHERE id = ?1",
        params![
            m.id.to_string(),
            m.content,
            memory_type_to_str(m.memory_type),
            m.source_agent,
            m.boundary.as_str(),
            m.created_at.to_rfc3339(),
            m.valid_from.to_rfc3339(),
            m.valid_until.map(|d| d.to_rfc3339()),
            m.confidence as f64,
            m.access_count as i64,
            m.last_accessed.to_rfc3339(),
            m.superseded_by.map(|id| id.to_string()),
            m.metadata.to_string(),
            m.archived_at.map(|d| d.to_rfc3339()),
        ],
    )
    .map_err(|e| VaultError::Storage(format!("update memory {}: {e}", m.id)))
}

fn tx_list_memories(
    tx: &Transaction<'_>,
    filter: &MemoryFilter,
    limit: Option<usize>,
) -> VaultResult<Vec<Memory>> {
    // Build SQL dynamically — but keep WHERE pieces parameterised. We never
    // splice user data into the query string.
    let mut sql = String::from(
        "SELECT id, content, memory_type, source_agent, boundary,
                created_at, valid_from, valid_until,
                confidence, access_count, last_accessed,
                superseded_by, metadata_json, archived_at
         FROM memories WHERE 1 = 1",
    );

    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();

    if let Some(b) = &filter.boundary {
        sql.push_str(&format!(" AND boundary = ?{}", bindings.len() + 1));
        bindings.push(b.as_str().to_string().into());
    }
    if let Some(mt) = filter.memory_type {
        sql.push_str(&format!(" AND memory_type = ?{}", bindings.len() + 1));
        bindings.push(memory_type_to_str(mt).to_string().into());
    }
    if !filter.include_superseded {
        sql.push_str(" AND superseded_by IS NULL");
    }
    // ADR-084: cold-archived facts (archived_at IS NOT NULL) are out of default
    // retrieval; the consolidator's active-set enumerations (cluster/decay/
    // contradiction) and default reads exclude them. Only an explicit archive
    // search sets include_archived = true.
    if !filter.include_archived {
        sql.push_str(" AND archived_at IS NULL");
    }
    // `created_at` is stored as RFC3339 with consistent UTC offset (see
    // `tx_create_memory`'s `m.created_at.to_rfc3339()` binding). RFC3339
    // with constant offset is lexicographically sortable, so a TEXT
    // comparison is correct for the `>=` ordering.
    if let Some(since) = filter.since {
        sql.push_str(&format!(" AND created_at >= ?{}", bindings.len() + 1));
        bindings.push(since.to_rfc3339().into());
    }
    // ADR-051: bi-temporal validity filter — when set, exclude memories
    // whose `valid_until` has already passed `valid_at` (the fact stopped
    // being true). `valid_until IS NULL` (still true) and future-dated
    // `valid_until > valid_at` (will expire later) both pass. Same
    // RFC3339-with-UTC-offset lexicographic-sort rationale as `since`.
    if let Some(valid_at) = filter.valid_at {
        sql.push_str(&format!(
            " AND (valid_until IS NULL OR valid_until > ?{})",
            bindings.len() + 1
        ));
        bindings.push(valid_at.to_rfc3339().into());
    }

    if let Some((at, id)) = &filter.before {
        let (a, b) = (bindings.len() + 1, bindings.len() + 2);
        sql.push_str(&format!(
            " AND (created_at < ?{a} OR (created_at = ?{a} AND id < ?{b}))"
        ));
        bindings.push(at.to_rfc3339().into());
        bindings.push(id.to_string().into());
    }

    // The id breaks ties, so the order is total: pages never overlap or
    // leave a gap between memories saved in the same instant.
    sql.push_str(" ORDER BY created_at DESC, id DESC");
    if let Some(n) = limit {
        sql.push_str(&format!(" LIMIT ?{}", bindings.len() + 1));
        bindings.push((n as i64).into());
    }

    let mut stmt = tx
        .prepare(&sql)
        .map_err(|e| VaultError::Storage(format!("prepare list memories: {e}")))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), row_to_memory)
        .map_err(|e| VaultError::Storage(format!("query list memories: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| VaultError::Storage(format!("read memory row: {e}")))?);
    }
    Ok(out)
}

/// Append a [`PendingAuditEvent`] to the chain inside an existing
/// [`Transaction`] and return the persisted [`AuditEvent`] (with its
/// assigned `seq` and `event_hash`).
///
/// `pub(crate)` so the cascading orchestrator (`cascading.rs`) can append
/// audit events inside [`MetadataStore::with_transaction`] alongside its
/// `memories` / `retry_queue` / `pending_sync` writes — atomicity of the
/// cascade-write requires the audit append to happen in the same SQLite
/// transaction.
///
/// External callers go through [`MetadataStore::append_audit_event`] which
/// owns its own transaction.
pub(crate) fn tx_append_audit(
    tx: &Transaction<'_>,
    pending: PendingAuditEvent,
) -> VaultResult<AuditEvent> {
    let prev_hash: String = tx
        .query_row(
            "SELECT event_hash FROM audit_log ORDER BY seq DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| VaultError::Storage(format!("read chain tip: {e}")))?
        .unwrap_or_else(|| AUDIT_GENESIS_HASH.to_string());

    let event_id = Uuid::now_v7();
    let timestamp = Utc::now();
    let (_canonical, event_hash) = seal(event_id, timestamp, &prev_hash, &pending);

    tx.execute(
        "INSERT INTO audit_log (
            event_id, timestamp, user_id, device_id,
            event_type, resource_type, resource_id, boundary,
            actor_kind, actor_name, result, details_json,
            prev_event_hash, event_hash
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            event_id.hyphenated().to_string(),
            timestamp.to_rfc3339(),
            pending.user_id,
            pending.device_id,
            pending.event_type.as_str(),
            pending.resource_type,
            pending.resource_id,
            pending.boundary.as_ref().map(|b| b.as_str().to_string()),
            pending.actor_kind.as_str(),
            pending.actor_name,
            pending.result.as_str(),
            &pending.details_json,
            &prev_hash,
            &event_hash,
        ],
    )
    .map_err(|e| VaultError::Storage(format!("insert audit event: {e}")))?;

    let seq: i64 = tx.last_insert_rowid();
    debug!(seq, %event_id, "audit event recorded");

    Ok(AuditEvent {
        seq,
        event_id,
        timestamp,
        user_id: pending.user_id,
        device_id: pending.device_id,
        event_type: pending.event_type,
        resource_type: pending.resource_type,
        resource_id: pending.resource_id,
        boundary: pending.boundary.map(|b| b.into_inner()),
        actor_kind: pending.actor_kind,
        actor_name: pending.actor_name,
        result: pending.result,
        details_json: pending.details_json,
        prev_event_hash: prev_hash,
        event_hash,
    })
}

// ---------------------------------------------------------------------------
// Row decoders + enum/string helpers
// ---------------------------------------------------------------------------

const MEMORY_SELECT_SQL: &str = "SELECT id, content, memory_type, source_agent, boundary,
            created_at, valid_from, valid_until,
            confidence, access_count, last_accessed,
            superseded_by, metadata_json, archived_at
         FROM memories WHERE id = ?1";

const AUDIT_SELECT_SQL: &str = "SELECT seq, event_id, timestamp, user_id, device_id,
            event_type, resource_type, resource_id, boundary,
            actor_kind, actor_name, result, details_json,
            prev_event_hash, event_hash
         FROM audit_log ORDER BY seq ASC LIMIT ?1";

fn row_to_memory(row: &Row<'_>) -> rusqlite::Result<Memory> {
    let id_s: String = row.get(0)?;
    let id = MemoryId(parse_uuid_field(&id_s, "memories.id")?);
    let memory_type_s: String = row.get(2)?;
    let memory_type = parse_memory_type(&memory_type_s)?;
    let boundary_s: String = row.get(4)?;
    let boundary = Boundary::new(boundary_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("memories.boundary: {e}"),
            )),
        )
    })?;
    let metadata_s: String = row.get(12)?;
    let metadata: serde_json::Value = serde_json::from_str(&metadata_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(12, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let superseded_s: Option<String> = row.get(11)?;
    let superseded_by = superseded_s
        .map(|s| parse_uuid_field(&s, "memories.superseded_by").map(MemoryId))
        .transpose()?;

    let confidence: f64 = row.get(8)?;
    let access_count: i64 = row.get(9)?;

    Ok(Memory {
        id,
        content: row.get(1)?,
        memory_type,
        source_agent: row.get(3)?,
        boundary,
        created_at: row.get(5)?,
        valid_from: row.get(6)?,
        valid_until: row.get(7)?,
        confidence: confidence as f32,
        access_count: access_count as u32,
        last_accessed: row.get(10)?,
        superseded_by,
        // Column 13: archived_at (ADR-084). rusqlite's chrono FromSql parses
        // the RFC3339 text; NULL maps to None (active).
        archived_at: row.get(13)?,
        embedding: None, // lives in LanceDB, not SQLite
        metadata,
    })
}

fn row_to_audit_event(row: &Row<'_>) -> rusqlite::Result<AuditEvent> {
    let event_id_s: String = row.get(1)?;
    let event_id = parse_uuid_field(&event_id_s, "audit_log.event_id")?;

    let event_type_s: String = row.get(5)?;
    let event_type = AuditEventType::parse(&event_type_s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown audit event_type: {event_type_s}"),
            )),
        )
    })?;

    let actor_kind_s: String = row.get(9)?;
    let actor_kind = ActorKind::parse(&actor_kind_s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            9,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown actor_kind: {actor_kind_s}"),
            )),
        )
    })?;

    let result_s: String = row.get(11)?;
    let result = AuditResult::parse(&result_s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            11,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown audit result: {result_s}"),
            )),
        )
    })?;

    Ok(AuditEvent {
        seq: row.get(0)?,
        event_id,
        timestamp: row.get(2)?,
        user_id: row.get(3)?,
        device_id: row.get(4)?,
        event_type,
        resource_type: row.get(6)?,
        resource_id: row.get(7)?,
        boundary: row.get(8)?,
        actor_kind,
        actor_name: row.get(10)?,
        result,
        details_json: row.get(12)?,
        prev_event_hash: row.get(13)?,
        event_hash: row.get(14)?,
    })
}

fn parse_uuid_field(s: &str, field: &str) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{field}: {e}"),
            )),
        )
    })
}

fn memory_type_to_str(mt: MemoryType) -> &'static str {
    match mt {
        MemoryType::Episodic => "episodic",
        MemoryType::Semantic => "semantic",
        MemoryType::Procedural => "procedural",
    }
}

fn parse_memory_type(s: &str) -> rusqlite::Result<MemoryType> {
    match s {
        "episodic" => Ok(MemoryType::Episodic),
        "semantic" => Ok(MemoryType::Semantic),
        "procedural" => Ok(MemoryType::Procedural),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown memory_type: {other}"),
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use tempfile::TempDir;
    use vault_core::NewMemory;

    /// Async test helper. Must be `.await`-ed — we never construct a fresh
    /// tokio Runtime here because that would panic when called from a
    /// `#[tokio::test]` (already inside a runtime) or from `tokio_test::block_on`
    /// (already inside a single-thread runtime).
    async fn make_store() -> (TempDir, MetadataStore) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");
        let key = SqlCipherKey::new("correct-horse-battery-staple-test-key");
        let store = MetadataStore::open(&path, key).await.unwrap();
        (tmp, store)
    }

    fn sample_memory(boundary: &str, mt: MemoryType, content: &str) -> Memory {
        Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: mt,
            boundary: Boundary::new(boundary).unwrap(),
            source_agent: Some("test".into()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({"k": "v"}),
        })
        .unwrap()
    }

    // ----- D4 (ADR-108): export pages and the never-create open -----

    /// Pages walk the whole list exactly once, newest first, even when many
    /// memories share one `created_at` (the id breaks the tie), and a memory
    /// saved mid-export is not repeated or skipped for the others.
    #[tokio::test]
    async fn pages_before_a_cursor_cover_every_memory_exactly_once() {
        let (_tmp, store) = make_store().await;
        let same_moment = chrono::Utc::now();
        let mut all = Vec::new();
        for i in 0..7 {
            let mut m = sample_memory("work", MemoryType::Semantic, &format!("m{i}"));
            if i < 5 {
                m.created_at = same_moment;
            }
            store.create_memory(&m).await.unwrap();
            all.push(m.id);
        }

        let mut seen = Vec::new();
        let mut before = None;
        loop {
            let page = store
                .list_memories(
                    MemoryFilter {
                        before,
                        ..Default::default()
                    },
                    Some(2),
                )
                .await
                .unwrap();
            if page.is_empty() {
                break;
            }
            if seen.is_empty() {
                // Saved after the export began: newer than every cursor, so
                // it is never reached and never disturbs the walk.
                store
                    .create_memory(&sample_memory("work", MemoryType::Semantic, "late"))
                    .await
                    .unwrap();
            }
            let last = page.last().unwrap();
            before = Some((last.created_at, last.id));
            seen.extend(page.iter().map(|m| m.id));
        }

        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), seen.len(), "no memory twice");
        let mut expected = all.clone();
        expected.sort();
        assert_eq!(
            unique, expected,
            "every memory once, the late one not at all"
        );
    }

    /// The order is total: two lists of the same rows agree exactly.
    #[tokio::test]
    async fn the_list_order_is_total() {
        let (_tmp, store) = make_store().await;
        let same_moment = chrono::Utc::now();
        for i in 0..6 {
            let mut m = sample_memory("work", MemoryType::Semantic, &format!("t{i}"));
            m.created_at = same_moment;
            store.create_memory(&m).await.unwrap();
        }
        let ids = |v: Vec<Memory>| v.into_iter().map(|m| m.id).collect::<Vec<_>>();
        let first = ids(store
            .list_memories(MemoryFilter::default(), None)
            .await
            .unwrap());
        let second = ids(store
            .list_memories(MemoryFilter::default(), None)
            .await
            .unwrap());
        assert_eq!(first, second);
        let mut descending = first.clone();
        descending.sort_by_key(|id| std::cmp::Reverse(id.to_string()));
        assert_eq!(first, descending, "ties go newest id first");
    }

    /// Lock mode opens the database only if it exists: no file, no database,
    /// and nothing is created (ADR-108 ADR-104 amendment).
    #[tokio::test]
    async fn open_existing_never_creates_a_database() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");
        let key = SqlCipherKey::new("correct-horse-battery-staple-test-key");
        let opened = MetadataStore::open_existing(&path, key).await.unwrap();
        assert!(opened.is_none());
        assert!(!path.exists(), "nothing may be created");
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    /// An existing database opens, with its rows, and can take audit rows.
    #[tokio::test]
    async fn open_existing_opens_a_database_that_is_there() {
        let (tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "kept");
        store.create_memory(&m).await.unwrap();
        drop(store);

        let key = SqlCipherKey::new("correct-horse-battery-staple-test-key");
        let reopened = MetadataStore::open_existing(tmp.path().join("vault.db"), key)
            .await
            .unwrap()
            .expect("the database is there");
        assert!(reopened.get_memory(&m.id).await.unwrap().is_some());
        reopened.verify_audit_chain().await.unwrap();
    }

    /// Boundaries are listed from the metadata store alone, so lock mode can
    /// show counts without the rest of the vault.
    #[tokio::test]
    async fn boundaries_are_listed_from_the_metadata_store_alone() {
        let (_tmp, store) = make_store().await;
        store
            .create_memory(&sample_memory("work", MemoryType::Semantic, "w"))
            .await
            .unwrap();
        let listed = store.list_boundaries().await.unwrap();
        assert!(listed.iter().any(|b| b.boundary.as_str() == "default"));
    }

    #[tokio::test]
    async fn create_then_get_round_trips() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "hello");
        store.create_memory(&m).await.unwrap();
        let back = store.get_memory(&m.id).await.unwrap().unwrap();
        // embedding is intentionally None on read (lives in LanceDB)
        let mut expected = m.clone();
        expected.embedding = None;
        assert_eq!(back, expected);
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let (_tmp, store) = make_store().await;
        let id = MemoryId::new();
        let result = store.get_memory(&id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn update_modifies_existing_record() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "v1");
        store.create_memory(&m).await.unwrap();

        let mut updated = m.clone();
        updated.content = "v2 — updated".into();
        updated.confidence = 0.5;
        store.update_memory(&updated).await.unwrap();

        let back = store.get_memory(&m.id).await.unwrap().unwrap();
        assert_eq!(back.content, "v2 — updated");
        assert!((back.confidence - 0.5).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn update_missing_returns_not_found() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "ghost");
        let err = store.update_memory(&m).await.unwrap_err();
        assert!(matches!(err, VaultError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_removes_record_and_is_idempotent() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "doomed");
        store.create_memory(&m).await.unwrap();

        store.delete_memory(&m.id).await.unwrap();
        assert!(store.get_memory(&m.id).await.unwrap().is_none());

        // Second delete is a no-op success.
        store.delete_memory(&m.id).await.unwrap();
    }

    #[tokio::test]
    async fn invalid_memory_rejected_at_create() {
        let (_tmp, store) = make_store().await;
        let mut m = sample_memory("work", MemoryType::Semantic, "ok");
        m.confidence = 2.0; // invalid
        let err = store.create_memory(&m).await.unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn list_with_no_filter_returns_all_active() {
        let (_tmp, store) = make_store().await;
        for i in 0..5 {
            let m = sample_memory("work", MemoryType::Semantic, &format!("mem-{i}"));
            store.create_memory(&m).await.unwrap();
        }
        let list = store
            .list_memories(MemoryFilter::default(), Some(100))
            .await
            .unwrap();
        assert_eq!(list.len(), 5);
    }

    #[tokio::test]
    async fn list_filters_by_boundary() {
        let (_tmp, store) = make_store().await;
        store
            .create_memory(&sample_memory("work", MemoryType::Semantic, "w1"))
            .await
            .unwrap();
        store
            .create_memory(&sample_memory("personal", MemoryType::Semantic, "p1"))
            .await
            .unwrap();
        store
            .create_memory(&sample_memory("personal", MemoryType::Semantic, "p2"))
            .await
            .unwrap();

        let only_personal = store
            .list_memories(
                MemoryFilter {
                    boundary: Some(Boundary::new("personal").unwrap()),
                    ..Default::default()
                },
                Some(100),
            )
            .await
            .unwrap();
        assert_eq!(only_personal.len(), 2);
        assert!(only_personal
            .iter()
            .all(|m| m.boundary.as_str() == "personal"));
    }

    #[tokio::test]
    async fn list_filters_by_memory_type() {
        let (_tmp, store) = make_store().await;
        store
            .create_memory(&sample_memory("work", MemoryType::Semantic, "s"))
            .await
            .unwrap();
        store
            .create_memory(&sample_memory("work", MemoryType::Episodic, "e"))
            .await
            .unwrap();
        store
            .create_memory(&sample_memory("work", MemoryType::Procedural, "p"))
            .await
            .unwrap();

        let only_episodic = store
            .list_memories(
                MemoryFilter {
                    memory_type: Some(MemoryType::Episodic),
                    ..Default::default()
                },
                Some(100),
            )
            .await
            .unwrap();
        assert_eq!(only_episodic.len(), 1);
        assert_eq!(only_episodic[0].memory_type, MemoryType::Episodic);
    }

    #[tokio::test]
    async fn list_excludes_superseded_by_default() {
        let (_tmp, store) = make_store().await;
        let parent = sample_memory("work", MemoryType::Semantic, "merged-into");
        store.create_memory(&parent).await.unwrap();

        let mut child = sample_memory("work", MemoryType::Semantic, "old-version");
        store.create_memory(&child).await.unwrap();
        child.superseded_by = Some(parent.id);
        store.update_memory(&child).await.unwrap();

        let default_list = store
            .list_memories(MemoryFilter::default(), Some(100))
            .await
            .unwrap();
        assert_eq!(default_list.len(), 1);
        assert_eq!(default_list[0].id, parent.id);

        let with_superseded = store
            .list_memories(
                MemoryFilter {
                    include_superseded: true,
                    ..Default::default()
                },
                Some(100),
            )
            .await
            .unwrap();
        assert_eq!(with_superseded.len(), 2);
    }

    #[tokio::test]
    async fn list_respects_limit() {
        let (_tmp, store) = make_store().await;
        for i in 0..10 {
            let m = sample_memory("work", MemoryType::Semantic, &format!("m-{i}"));
            store.create_memory(&m).await.unwrap();
        }
        let limited = store
            .list_memories(MemoryFilter::default(), Some(3))
            .await
            .unwrap();
        assert_eq!(limited.len(), 3);
    }

    // ─── T0.2.2 Amendment 2 pins — list_memories Option<usize> + since filter ───

    /// `limit: None` returns ALL matching rows (no SQL `LIMIT` clause).
    /// Locks the unbounded-enumeration semantic used by
    /// `vault-consolidator`'s clustering primitive (BRD §5.6 Phase 1).
    /// Per ADR-045 §f — `None` is intentional unboundedness, not a
    /// "page size of zero" footgun.
    #[tokio::test]
    async fn list_with_limit_none_returns_all_matching_rows() {
        let (_tmp, store) = make_store().await;
        for i in 0..25 {
            let m = sample_memory("work", MemoryType::Semantic, &format!("m-{i}"));
            store.create_memory(&m).await.unwrap();
        }
        let unbounded = store
            .list_memories(MemoryFilter::default(), None)
            .await
            .unwrap();
        assert_eq!(
            unbounded.len(),
            25,
            "limit: None must return every row, not a default-page sized slice"
        );
    }

    /// `MemoryFilter::since` excludes memories with `created_at < since`.
    /// Forward-compat pin for T0.2.5 (Checkpoint & Rollback) incremental
    /// consolidation scope. T0.2.2 ships with `since: None` (full-scan);
    /// T0.2.5 wires actual checkpoint timestamps. The filter shape is
    /// locked at T0.2.2 even though the production caller passes `None`.
    #[tokio::test]
    async fn list_since_filter_excludes_older_memories() {
        let (_tmp, store) = make_store().await;
        // Insert one memory, sleep enough to span a millisecond, capture
        // a cutoff, then insert two more. Filter with `since = cutoff`
        // should return exactly the two later memories.
        let early = sample_memory("work", MemoryType::Semantic, "early");
        store.create_memory(&early).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let cutoff = chrono::Utc::now();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let late_a = sample_memory("work", MemoryType::Semantic, "late-a");
        let late_b = sample_memory("work", MemoryType::Semantic, "late-b");
        store.create_memory(&late_a).await.unwrap();
        store.create_memory(&late_b).await.unwrap();

        let after_cutoff = store
            .list_memories(
                MemoryFilter {
                    since: Some(cutoff),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(after_cutoff.len(), 2);
        assert!(
            after_cutoff.iter().all(|m| m.id != early.id),
            "memories with created_at < since must be excluded"
        );
    }

    /// `MemoryFilter::valid_at` excludes memories whose `valid_until` has
    /// already passed the supplied timestamp. Locks the bi-temporal
    /// filter contract from ADR-051 (T0.2.7 Phase B).
    #[tokio::test]
    async fn list_valid_at_filter_excludes_expired_memories() {
        let (_tmp, store) = make_store().await;

        // Memory A: still valid (valid_until=None).
        let live = sample_memory("work", MemoryType::Semantic, "still-true");
        store.create_memory(&live).await.unwrap();

        // Memory B: expired one hour ago. `valid_from` pinned to 2 days
        // ago so the Memory invariant (valid_until >= valid_from) holds
        // independent of microsecond-scale clock skew between this test's
        // `Utc::now()` and `sample_memory`'s internal default `now`.
        let mut expired = sample_memory("work", MemoryType::Semantic, "expired-yesterday");
        expired.valid_from = chrono::Utc::now() - chrono::Duration::days(2);
        expired.valid_until = Some(chrono::Utc::now() - chrono::Duration::hours(1));
        store.create_memory(&expired).await.unwrap();

        // Memory C: future-dated valid_until (still currently valid).
        let mut future = sample_memory("work", MemoryType::Semantic, "expires-next-year");
        future.valid_until = Some(chrono::Utc::now() + chrono::Duration::days(365));
        store.create_memory(&future).await.unwrap();

        // No filter — all three returned.
        let unfiltered = store
            .list_memories(MemoryFilter::default(), None)
            .await
            .unwrap();
        assert_eq!(unfiltered.len(), 3);

        // valid_at = now — exclude `expired`, keep `live` and `future`.
        let now = chrono::Utc::now();
        let filtered = store
            .list_memories(
                MemoryFilter {
                    valid_at: Some(now),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            filtered.len(),
            2,
            "valid_at filter must exclude memories whose valid_until has passed"
        );
        let ids: Vec<MemoryId> = filtered.iter().map(|m| m.id).collect();
        assert!(ids.contains(&live.id), "live memory must remain");
        assert!(ids.contains(&future.id), "future-dated memory must remain");
        assert!(
            !ids.contains(&expired.id),
            "expired memory must be filtered out per ADR-051"
        );
    }

    #[tokio::test]
    async fn get_memories_batch_empty_input_returns_empty_vec() {
        let (_tmp, store) = make_store().await;
        let out = store.get_memories_batch(&[]).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn get_memories_batch_preserves_input_order() {
        let (_tmp, store) = make_store().await;
        // Insert in one order; query in a different order; assert the
        // returned Vec follows the *query* order, not insertion order.
        let mems: Vec<Memory> = (0..5)
            .map(|i| sample_memory("work", MemoryType::Semantic, &format!("mem-{i}")))
            .collect();
        for m in &mems {
            store.create_memory(m).await.unwrap();
        }
        // Reverse-order query.
        let ids: Vec<MemoryId> = mems.iter().map(|m| m.id).rev().collect();
        let out = store.get_memories_batch(&ids).await.unwrap();
        assert_eq!(out.len(), 5);
        for (out_mem, expected_id) in out.iter().zip(ids.iter()) {
            assert_eq!(
                out_mem.id, *expected_id,
                "order mismatch: get_memories_batch must preserve input order"
            );
        }
    }

    #[tokio::test]
    async fn get_memories_batch_partial_hit_warns_and_omits() {
        let (_tmp, store) = make_store().await;
        let a = sample_memory("work", MemoryType::Semantic, "a");
        let c = sample_memory("work", MemoryType::Semantic, "c");
        store.create_memory(&a).await.unwrap();
        store.create_memory(&c).await.unwrap();
        let b_missing = MemoryId::new(); // never inserted
                                         // Query [a, b_missing, c] — middle ID has no row.
        let out = store
            .get_memories_batch(&[a.id, b_missing, c.id])
            .await
            .unwrap();
        // b_missing is omitted; a and c are returned in input order.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, a.id);
        assert_eq!(out[1].id, c.id);
        // The `warn!` log is emitted but not asserted here — capturing
        // tracing in tests requires a `tracing-subscriber` test fixture
        // not currently wired in vault-storage. The OMIT semantic is the
        // load-bearing contract for retrieval correctness; the WARN is
        // operational signal.
    }

    #[tokio::test]
    async fn get_memories_batch_does_not_append_audit_event() {
        // Per the doc-comment promise: get_memories_batch deliberately
        // does NOT append per-ID memory.read audit events. T0.1.9 §6
        // moved audit accounting up to vault-mcp's `mcp.tool_invoke`
        // event; vault-retrieval emits operational `tracing::info!` only.
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "no-audit");
        store.create_memory(&m).await.unwrap();
        // Snapshot audit count after the create (which DOES audit).
        let before = store.list_audit_events(100).await.unwrap().len();
        let out = store.get_memories_batch(&[m.id]).await.unwrap();
        assert_eq!(out.len(), 1);
        // After the batch fetch + an extra list_audit_events, the chain
        // has not grown from the batch fetch itself.
        let after = store.list_audit_events(100).await.unwrap().len();
        assert_eq!(
            after, before,
            "get_memories_batch must not append audit events"
        );
    }

    #[tokio::test]
    async fn get_memories_batch_rejects_oversized_input() {
        let (_tmp, store) = make_store().await;
        let too_many: Vec<MemoryId> = (0..32_001).map(|_| MemoryId::new()).collect();
        let err = store.get_memories_batch(&too_many).await.unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn audit_chain_is_maintained_across_operations() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "audited");
        store.create_memory(&m).await.unwrap();
        store.get_memory(&m.id).await.unwrap();
        store
            .update_memory(&{
                let mut x = m.clone();
                x.content = "v2".into();
                x
            })
            .await
            .unwrap();
        store.delete_memory(&m.id).await.unwrap();

        let events = store.list_audit_events(100).await.unwrap();
        // create, read, update, delete = 4 events
        assert_eq!(events.len(), 4);
        store.verify_audit_chain().await.unwrap();
    }

    #[tokio::test]
    async fn audit_chain_detects_tampering() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "trip-wire");
        store.create_memory(&m).await.unwrap();
        store
            .create_memory(&sample_memory("work", MemoryType::Semantic, "second"))
            .await
            .unwrap();

        // Reach into the DB directly and tamper with the boundary on the
        // first event. The chain should refuse to validate.
        {
            let inner = store.inner.clone();
            let conn = inner.lock().unwrap();
            conn.execute(
                "UPDATE audit_log SET boundary = 'tampered' WHERE seq = 1",
                [],
            )
            .unwrap();
        }

        let err = store.verify_audit_chain().await.unwrap_err();
        assert!(
            matches!(&err, VaultError::Storage(s) if s.contains("tampering detected")),
            "expected tampering detection, got {err:?}",
        );
    }

    #[tokio::test]
    async fn opening_with_wrong_key_fails() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");

        // Create with the right key.
        let store = MetadataStore::open(&path, SqlCipherKey::new("right"))
            .await
            .unwrap();
        store
            .create_memory(&sample_memory("work", MemoryType::Semantic, "secret"))
            .await
            .unwrap();
        drop(store);

        // Reopen with the wrong key — must fail at the verification query.
        // Note: `MetadataStore` does not impl Debug (ADR-007), so we cannot
        // print `result` directly; use static descriptions in panic messages.
        let result = MetadataStore::open(&path, SqlCipherKey::new("wrong")).await;
        match result {
            Err(VaultError::Storage(_)) => {}
            Err(_) => panic!("expected VaultError::Storage from wrong-key open, got a different VaultError variant"),
            Ok(_) => panic!("expected wrong-key open to fail, got Ok"),
        }
    }

    #[tokio::test]
    async fn reopening_with_correct_key_preserves_data() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");

        let store = MetadataStore::open(&path, SqlCipherKey::new("right"))
            .await
            .unwrap();
        let m = sample_memory("work", MemoryType::Semantic, "persistent");
        store.create_memory(&m).await.unwrap();
        drop(store);

        let store = MetadataStore::open(&path, SqlCipherKey::new("right"))
            .await
            .unwrap();
        let back = store.get_memory(&m.id).await.unwrap().unwrap();
        assert_eq!(back.content, "persistent");
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

        #[test]
        fn round_trip_integrity(
            content in "[a-zA-Z0-9 _.-]{1,200}",
            confidence in 0.0f32..=1.0,
            boundary_name in "[a-zA-Z0-9_-]{1,32}",
        ) {
            tokio_test::block_on(async {
                let (_tmp, store) = make_store().await;
                let m = Memory::try_new(NewMemory {
                    content: content.clone(),
                    memory_type: MemoryType::Semantic,
                    boundary: Boundary::new(boundary_name).unwrap(),
                    source_agent: None,
                    confidence,
                    valid_from: None,
                    valid_until: None,
                    metadata: serde_json::json!({}),
                }).unwrap();
                store.create_memory(&m).await.unwrap();
                let back = store.get_memory(&m.id).await.unwrap().unwrap();
                prop_assert_eq!(back.content, content);
                prop_assert!((back.confidence - confidence).abs() < 1e-6);
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }
    }

    proptest! {
        // Adversarial: for any chain of length N, tampering with any single
        // event breaks verify_audit_chain on that prefix; restoring the
        // original value re-validates the chain. This is the property-test
        // form of "the audit chain catches every tamper, every time."
        // Lower case count because each iteration creates a fresh on-disk
        // SQLCipher DB (slow due to PBKDF2).
        #![proptest_config(ProptestConfig { cases: 8, ..ProptestConfig::default() })]

        #[test]
        fn tampering_breaks_chain_at_every_seq(n_events in 2usize..6) {
            tokio_test::block_on(async {
                let (_tmp, store) = make_store().await;

                for i in 0..n_events {
                    let pending = PendingAuditEvent::success(
                        AuditEventType::MemoryRead,
                        ActorKind::System,
                    )
                    .with_resource("memory", format!("res-{i}"))
                    .with_boundary(Boundary::new(format!("b-{i}")).unwrap());
                    store.append_audit_event(pending).await.unwrap();
                }
                store.verify_audit_chain().await.unwrap();

                for tamper_seq in 1..=(n_events as i64) {
                    // Save the current value, tamper, expect verification to fail.
                    let inner = store.inner.clone();
                    let original: Option<String> = {
                        let conn = inner.lock().unwrap();
                        let v: Option<String> = conn
                            .query_row(
                                "SELECT boundary FROM audit_log WHERE seq = ?1",
                                rusqlite::params![tamper_seq],
                                |row| row.get(0),
                            )
                            .unwrap();
                        conn.execute(
                            "UPDATE audit_log SET boundary = 'tampered-by-test' WHERE seq = ?1",
                            rusqlite::params![tamper_seq],
                        )
                        .unwrap();
                        v
                    };

                    let err = store.verify_audit_chain().await.unwrap_err();
                    prop_assert!(
                        matches!(&err, VaultError::Storage(s) if s.contains("tampering detected")),
                        "expected tamper at seq {} to break chain, got {:?}",
                        tamper_seq, err,
                    );

                    // Restore original value, verify chain heals.
                    {
                        let conn = inner.lock().unwrap();
                        conn.execute(
                            "UPDATE audit_log SET boundary = ?2 WHERE seq = ?1",
                            rusqlite::params![tamper_seq, original],
                        )
                        .unwrap();
                    }
                    store.verify_audit_chain().await.unwrap();
                }
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }
    }

    #[tokio::test]
    async fn concurrent_writes_all_succeed_and_chain_stays_valid() {
        // 20 tasks concurrently create memories. The Mutex<Connection>
        // serialises them at the storage layer; we want to verify that
        // (a) every task succeeds, (b) every memory is retrievable,
        // (c) the audit chain validates after the dust settles.
        let (_tmp, store) = make_store().await;
        let n: u32 = 20;
        let mut handles = Vec::new();
        for i in 0..n {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                let m = sample_memory("work", MemoryType::Semantic, &format!("c-{i}"));
                store.create_memory(&m).await.map(|()| m.id)
            }));
        }
        let mut ids = Vec::new();
        for h in handles {
            ids.push(h.await.unwrap().unwrap());
        }
        assert_eq!(ids.len(), n as usize);

        // All retrievable.
        for id in &ids {
            assert!(store.get_memory(id).await.unwrap().is_some());
        }

        // Chain still valid (each create + each get appended an audit event).
        store.verify_audit_chain().await.unwrap();

        // Total audit events: n creates + n reads = 2n.
        let events = store.list_audit_events(1000).await.unwrap();
        assert_eq!(events.len() as u32, n * 2);
    }

    /// Honest perf measurement, reported via eprintln for visibility.
    /// Does NOT assert a tight 50ms budget — SQLCipher's default PBKDF2
    /// (256,000 SHA-512 iterations) makes first-open inherently 100-300ms
    /// on a modern CPU. That cost is *intentional* (brute-force resistance).
    /// Steady-state operations should be sub-millisecond.
    /// Asserts only a generous regression bound.
    #[tokio::test]
    async fn perf_budget_open_migrate_first_audit() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("perf.db");
        let key = SqlCipherKey::new("perf-test-key-not-secret");

        let t0 = std::time::Instant::now();
        let store = MetadataStore::open(&path, key.clone()).await.unwrap();
        let after_open = t0.elapsed();

        let pending =
            PendingAuditEvent::success(AuditEventType::SchemaMigration, ActorKind::System);
        store.append_audit_event(pending).await.unwrap();
        let after_first_audit = t0.elapsed();

        // Steady-state — second audit insert should be quick.
        let t_steady_start = std::time::Instant::now();
        store
            .append_audit_event(PendingAuditEvent::success(
                AuditEventType::MemoryRead,
                ActorKind::System,
            ))
            .await
            .unwrap();
        let steady_state_audit = t_steady_start.elapsed();

        eprintln!(
            "[perf] open+migrate={:?}  +first_audit={:?}  steady_audit={:?}",
            after_open, after_first_audit, steady_state_audit,
        );

        // Generous regression bound — anything over 5s is a real bug.
        // The 50ms target Shahbaz set is reported in HANDOFF.md alongside
        // these measured numbers so we can decide together whether to
        // tune kdf_iter (security trade-off) or accept the cost.
        assert!(
            after_first_audit.as_secs() < 5,
            "perf regression: open+migrate+first_audit took {:?}",
            after_first_audit,
        );
        assert!(
            steady_state_audit.as_millis() < 200,
            "steady-state audit insert took {:?} — expected sub-100ms",
            steady_state_audit,
        );
    }

    // ---------- with_transaction (ADR-016 helper for the orchestrator) ----------

    #[tokio::test]
    async fn with_transaction_commits_on_ok() {
        // Closure writes a row inside the transaction; helper commits
        // automatically on Ok return. Row visible after the helper completes.
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "tx-commit-target");
        let m_for_closure = m.clone();
        store
            .with_transaction(move |tx| {
                tx_insert_memory(tx, &m_for_closure)?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(store.get_memory(&m.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn with_transaction_rolls_back_on_closure_error() {
        // Closure writes a row, then returns Err — helper must NOT commit.
        // The transaction is dropped without a commit() call, so rusqlite
        // rolls it back. Verify nothing was persisted.
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "tx-rollback-target");
        let m_for_closure = m.clone();

        let err = store
            .with_transaction(move |tx| {
                tx_insert_memory(tx, &m_for_closure)?;
                // Deliberately fail so the helper drops the tx without
                // commit. The row's INSERT is rolled back.
                Err::<(), _>(VaultError::Storage("simulated rollback".into()))
            })
            .await
            .unwrap_err();

        assert!(
            matches!(&err, VaultError::Storage(s) if s.contains("simulated rollback")),
            "expected the closure's error to propagate verbatim, got {err:?}",
        );
        // Row must not exist — tx was rolled back.
        assert!(store.get_memory(&m.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn with_transaction_returns_closure_value_on_ok() {
        // Closure may compute and return a value; helper passes it through.
        // Verifies the generic T parameter.
        let (_tmp, store) = make_store().await;
        let row_count: i64 = store
            .with_transaction(|tx| {
                tx.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
                    .map_err(|e| VaultError::Storage(format!("query: {e}")))
            })
            .await
            .unwrap();
        assert_eq!(row_count, 0);
    }
}
