//! Recently deleted (ADR-114, `notes/RECENTLY-DELETED-DESIGN.md`).
//!
//! A memory deleted or rewritten by the user or an AI app is copied here, in
//! the same transaction as the change, and kept for
//! [`RECENTLY_DELETED_DAYS`] days so the owner can put it back. The copy is the
//! whole memory as it was, sealed in vault.db by SQLCipher like every row; no
//! embedding is kept, so the memory leaves the search files at once (ADR-039
//! unchanged) and a restore re-embeds it.
//!
//! The plain [`StorageBackend::delete_memory`] / [`StorageBackend::update_memory`]
//! stay hard and copy nothing: the nightly tidy-up's checkpoint rollback and
//! enrichment use them, and they are not deletes anyone asked for.
//!
//! **The AI-app limit (ADR-SEC-049).** Rows with `by_actor = 'app'` in the last
//! hour ARE the count of changes AI apps made, so the limit is checked inside
//! the same transaction as the change (no race between two calls), and a
//! reconnect or a restart cannot reset it. Counted across all apps together:
//! an app's name is whatever it says it is.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, OptionalExtension, Row, Transaction};
use tracing::instrument;

use vault_core::{Memory, VaultError, VaultResult};

use crate::audit::{ActorKind, AuditEventType, PendingAuditEvent};
use crate::cascading::{tx_enqueue_cascade, Ack, CascadePayloadV1, StorageBackend};
use crate::metadata_store::{tx_append_audit, tx_get_memory, tx_insert_memory, tx_update_memory};
use crate::retry_queue::CascadeOperation;

/// How long an entry stays in Recently deleted (BRD §11.5.3; founder s109).
pub const RECENTLY_DELETED_DAYS: i64 = 30;

/// How many memories AI apps together may delete or change in any rolling
/// hour (ADR-SEC-049; founder s109). The owner has no limit.
pub const APP_CHANGES_PER_HOUR: usize = 30;

/// Who made the change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinBy {
    /// The owner: the desktop app or the CLI, over the keeper's admin
    /// connection (ADR-108). No limit.
    User,
    /// An AI app, over MCP. Limited to [`APP_CHANGES_PER_HOUR`].
    App,
}

impl BinBy {
    /// The stored `by_actor` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::App => "app",
        }
    }

    /// The audit actor for a change this side made.
    pub fn actor(self) -> ActorKind {
        match self {
            Self::User => ActorKind::User,
            Self::App => ActorKind::Agent,
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "app" => Some(Self::App),
            _ => None,
        }
    }
}

/// What happened to the memory an entry holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinKind {
    /// The memory was deleted.
    Deleted,
    /// An edit replaced this wording; the memory itself still exists.
    Replaced,
}

impl BinKind {
    /// The stored `kind` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::Replaced => "replaced",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "deleted" => Some(Self::Deleted),
            "replaced" => Some(Self::Replaced),
            _ => None,
        }
    }
}

/// One Recently deleted entry.
#[derive(Clone, Debug, PartialEq)]
pub struct BinEntry {
    /// The entry's own id (a UUID string).
    pub entry_id: String,
    /// Deleted, or replaced by an edit.
    pub kind: BinKind,
    /// Who made the change.
    pub by: BinBy,
    /// When the change was made.
    pub at: DateTime<Utc>,
    /// The memory as it was, embedding left out.
    pub memory: Memory,
}

/// Fixed-width UTC text, so text order is time order in SQL.
fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Refuse an AI app's change once AI apps have made
/// [`APP_CHANGES_PER_HOUR`] changes in the hour before `now`. The owner
/// always passes.
pub(crate) fn tx_check_app_limit(
    tx: &Transaction<'_>,
    by: BinBy,
    now: DateTime<Utc>,
) -> VaultResult<()> {
    if by != BinBy::App {
        return Ok(());
    }
    let since = stamp(now - Duration::hours(1));
    let n: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM recently_deleted WHERE by_actor = 'app' AND at > ?1",
            params![since],
            |row| row.get(0),
        )
        .map_err(|e| VaultError::Storage(format!("count app changes: {e}")))?;
    if n >= APP_CHANGES_PER_HOUR as i64 {
        return Err(VaultError::RateLimited(format!(
            "AI apps made {n} changes in the last hour (limit {APP_CHANGES_PER_HOUR})"
        )));
    }
    Ok(())
}

/// Copy `memory` into Recently deleted. Called inside the transaction that
/// deletes or rewrites it.
pub(crate) fn tx_insert_bin(
    tx: &Transaction<'_>,
    memory: &Memory,
    kind: BinKind,
    by: BinBy,
    at: DateTime<Utc>,
) -> VaultResult<String> {
    let mut kept = memory.clone();
    kept.embedding = None;
    let json = serde_json::to_string(&kept)?;
    let entry_id = uuid::Uuid::now_v7().to_string();
    tx.execute(
        "INSERT INTO recently_deleted (entry_id, memory_id, kind, by_actor, at, memory_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            entry_id,
            memory.id.to_string(),
            kind.as_str(),
            by.as_str(),
            stamp(at),
            json
        ],
    )
    .map_err(|e| VaultError::Storage(format!("insert recently_deleted: {e}")))?;
    Ok(entry_id)
}

const ENTRY_SELECT_SQL: &str =
    "SELECT entry_id, kind, by_actor, at, memory_json FROM recently_deleted";

fn row_to_entry(row: &Row<'_>) -> rusqlite::Result<VaultResult<BinEntry>> {
    let entry_id: String = row.get(0)?;
    let kind: String = row.get(1)?;
    let by: String = row.get(2)?;
    let at: String = row.get(3)?;
    let json: String = row.get(4)?;
    Ok((|| {
        let kind = BinKind::parse(&kind)
            .ok_or_else(|| VaultError::Storage(format!("recently_deleted kind {kind:?}")))?;
        let by = BinBy::parse(&by)
            .ok_or_else(|| VaultError::Storage(format!("recently_deleted by_actor {by:?}")))?;
        let at = DateTime::parse_from_rfc3339(&at)
            .map_err(|e| VaultError::Storage(format!("recently_deleted at: {e}")))?
            .with_timezone(&Utc);
        let memory: Memory = serde_json::from_str(&json)?;
        Ok(BinEntry {
            entry_id,
            kind,
            by,
            at,
            memory,
        })
    })())
}

fn tx_get_entry(tx: &Transaction<'_>, entry_id: &str) -> VaultResult<Option<BinEntry>> {
    tx.query_row(
        &format!("{ENTRY_SELECT_SQL} WHERE entry_id = ?1"),
        params![entry_id],
        row_to_entry,
    )
    .optional()
    .map_err(|e| VaultError::Storage(format!("get recently_deleted entry: {e}")))?
    .transpose()
}

fn tx_erased_audit(tx: &Transaction<'_>, reason: &str, count: usize) -> VaultResult<()> {
    let mut pending = PendingAuditEvent::success(
        AuditEventType::BinErased,
        if reason == "user" {
            ActorKind::User
        } else {
            ActorKind::System
        },
    )
    .with_resource("recently_deleted", "entries");
    pending.details_json = format!(r#"{{"reason":"{reason}","count":{count}}}"#);
    tx_append_audit(tx, pending)?;
    Ok(())
}

impl StorageBackend {
    /// Recently deleted, newest first.
    #[instrument(skip(self))]
    pub async fn list_recently_deleted(
        &self,
        limit: usize,
        offset: usize,
    ) -> VaultResult<Vec<BinEntry>> {
        self.metadata()
            .with_transaction(move |tx| {
                let mut stmt = tx
                    .prepare(&format!(
                        "{ENTRY_SELECT_SQL} ORDER BY at DESC, entry_id DESC LIMIT ?1 OFFSET ?2"
                    ))
                    .map_err(|e| VaultError::Storage(format!("list recently_deleted: {e}")))?;
                let rows = stmt
                    .query_map(params![limit as i64, offset as i64], row_to_entry)
                    .map_err(|e| VaultError::Storage(format!("list recently_deleted: {e}")))?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(|e| {
                        VaultError::Storage(format!("read recently_deleted: {e}"))
                    })??);
                }
                Ok(out)
            })
            .await
    }

    /// How many entries Recently deleted holds.
    pub async fn count_recently_deleted(&self) -> VaultResult<u64> {
        self.metadata()
            .with_transaction(|tx| {
                let n: i64 = tx
                    .query_row("SELECT COUNT(*) FROM recently_deleted", [], |row| {
                        row.get(0)
                    })
                    .map_err(|e| VaultError::Storage(format!("count recently_deleted: {e}")))?;
                Ok(n.max(0) as u64)
            })
            .await
    }

    /// One entry, if it is still there.
    pub async fn recently_deleted_entry(&self, entry_id: &str) -> VaultResult<Option<BinEntry>> {
        let entry_id = entry_id.to_string();
        self.metadata()
            .with_transaction(move |tx| tx_get_entry(tx, &entry_id))
            .await
    }

    /// Put an entry back (owner only; vault-app gives AI apps no way here).
    /// `embedding` is the entry's memory content embedded by the caller.
    ///
    /// - The memory no longer exists → it is written back with its own id and
    ///   dates.
    /// - It exists (an edit replaced this wording, or it was restored before)
    ///   → its current wording goes into Recently deleted as `replaced`, then
    ///   the entry's wording is put back. So an undo can itself be undone.
    ///
    /// The entry leaves Recently deleted. One transaction; the vector cascade
    /// is queued like any write.
    ///
    /// # Errors
    /// [`VaultError::NotFound`] when the entry is gone (restored, erased or
    /// expired in the meantime).
    #[instrument(skip(self, embedding))]
    pub async fn restore_from_recently_deleted(
        &self,
        entry_id: &str,
        embedding: &[f32],
    ) -> VaultResult<Ack> {
        let entry = self
            .recently_deleted_entry(entry_id)
            .await?
            .ok_or_else(|| VaultError::NotFound("recently deleted entry".into()))?;
        self.eager_validate(&entry.memory, embedding)?;

        let entry_id = entry_id.to_string();
        let payload_bytes = serde_json::to_vec(&CascadePayloadV1 {
            embedding: embedding.to_vec(),
            boundary: entry.memory.boundary.as_str().to_string(),
        })?;
        let in_cap = self.in_cap_overflow().clone();
        let memory_id = entry.memory.id;

        let committed_at = self
            .metadata()
            .with_transaction(move |tx| {
                let entry = tx_get_entry(tx, &entry_id)?
                    .ok_or_else(|| VaultError::NotFound("recently deleted entry".into()))?;
                let now = Utc::now();
                // Past its 30 days it is gone, even before the next purge.
                if entry.at < now - Duration::days(RECENTLY_DELETED_DAYS) {
                    return Err(VaultError::NotFound("recently deleted entry".into()));
                }
                tx.execute(
                    "DELETE FROM recently_deleted WHERE entry_id = ?1",
                    params![entry_id],
                )
                .map_err(|e| VaultError::Storage(format!("take recently_deleted: {e}")))?;

                let memory = entry.memory;
                let op = match tx_get_memory(tx, &memory.id)? {
                    Some(current) => {
                        tx_insert_bin(tx, &current, BinKind::Replaced, BinBy::User, now)?;
                        tx_update_memory(tx, &memory)?;
                        CascadeOperation::Update
                    }
                    None => {
                        tx_insert_memory(tx, &memory)?;
                        CascadeOperation::Write
                    }
                };

                let mut pending =
                    PendingAuditEvent::success(AuditEventType::MemoryRestored, ActorKind::User)
                        .with_resource("memory", memory.id.to_string())
                        .with_boundary(memory.boundary.clone());
                pending.details_json = format!(r#"{{"kind":"{}"}}"#, entry.kind.as_str());
                let event = tx_append_audit(tx, pending)?;

                tx_enqueue_cascade(
                    tx,
                    memory.id,
                    op,
                    event.timestamp,
                    event.seq,
                    &payload_bytes,
                    &in_cap,
                )?;
                Ok(event.timestamp)
            })
            .await?;

        Ok(Ack {
            memory_id,
            sqlite_committed_at: committed_at,
        })
    }

    /// Erase one entry for good (owner's "Delete now"). `false` when it was
    /// already gone.
    #[instrument(skip(self))]
    pub async fn erase_from_recently_deleted(&self, entry_id: &str) -> VaultResult<bool> {
        let entry_id = entry_id.to_string();
        self.metadata()
            .with_transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM recently_deleted WHERE entry_id = ?1",
                        params![entry_id],
                    )
                    .map_err(|e| VaultError::Storage(format!("erase recently_deleted: {e}")))?;
                if n > 0 {
                    tx_erased_audit(tx, "user", n)?;
                }
                Ok(n > 0)
            })
            .await
    }

    /// Erase every entry for good (owner's "Empty Recently deleted").
    #[instrument(skip(self))]
    pub async fn empty_recently_deleted(&self) -> VaultResult<usize> {
        self.metadata()
            .with_transaction(|tx| {
                let n = tx
                    .execute("DELETE FROM recently_deleted", [])
                    .map_err(|e| VaultError::Storage(format!("empty recently_deleted: {e}")))?;
                if n > 0 {
                    tx_erased_audit(tx, "user", n)?;
                }
                Ok(n)
            })
            .await
    }

    /// Erase the entries older than [`RECENTLY_DELETED_DAYS`] before `now`.
    /// Runs when the keeper starts and with the nightly job.
    #[instrument(skip(self))]
    pub async fn purge_recently_deleted(&self, now: DateTime<Utc>) -> VaultResult<usize> {
        let cutoff = stamp(now - Duration::days(RECENTLY_DELETED_DAYS));
        self.metadata()
            .with_transaction(move |tx| {
                let n = tx
                    .execute(
                        "DELETE FROM recently_deleted WHERE at < ?1",
                        params![cutoff],
                    )
                    .map_err(|e| VaultError::Storage(format!("purge recently_deleted: {e}")))?;
                if n > 0 {
                    tx_erased_audit(tx, "expired", n)?;
                }
                Ok(n)
            })
            .await
    }
}

#[cfg(test)]
#[path = "recently_deleted_tests.rs"]
mod tests;
