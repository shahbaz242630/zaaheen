//! Tamper-evident audit log primitives (BRD §11.9).
//!
//! Every security-relevant operation appends one [`AuditEvent`] to the
//! `audit_log` table. Events form a hash chain: each event's `event_hash` is
//! `BLAKE3(prev_event_hash || canonical_event_bytes)`. The genesis event's
//! `prev_event_hash` is [`AUDIT_GENESIS_HASH`] (64 zero hex chars).
//!
//! Tampering with any event breaks the chain at validation time
//! ([`verify_chain`]). The user can verify chain integrity from any point.
//!
//! Canonical bytes are produced by serialising a fixed-field-order struct
//! to JSON via [`serde_json`]. `details_json` is a verbatim string supplied
//! by the caller — we hash exactly what the caller passed, no
//! re-serialisation, so the hash is deterministic without needing JSON
//! object-key canonicalisation. Callers that want structured details
//! must canonicalise (e.g., sort keys) before passing the string.

use blake3;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use vault_core::{Boundary, VaultError, VaultResult};

/// 64 hex zeros — the `prev_event_hash` of the first event in a fresh chain.
pub const AUDIT_GENESIS_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// Categories of audit events. Extend as needed; treat unknown variants as
/// non-existent (don't add `Other(String)` — it would defeat type-safe matching).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    MemoryCreate,
    MemoryRead,
    MemoryUpdate,
    MemoryDelete,
    MemoryList,
    SchemaMigration,
    /// A cascading write exhausted retries or hit a permanent failure and
    /// landed in the dead-letter table. CRITICAL — the founder must know.
    /// Per ADR-009 amendment + ADR-018 (T0.1.6 Phase C).
    CascadeDeadLetter,
    /// `StorageBackend::open` ran `validate_readable` on a downstream store
    /// (LanceDB or DuckDB) and the read failed. The vault is opened in
    /// degraded mode so vault-cli triage still works, but search will not.
    /// Per ADR-018 + Phase A Change 1 (T0.1.6 Phase C).
    StoreCorruption,
    /// The cascading retry queue is at the 10,000-entry cap and the user
    /// write fell back to `pending_sync`. Fired on the *transition* into
    /// overflow (one event per wave), not per overflowing write —
    /// debouncing keeps the audit log readable. Per Phase C plan Q2.
    CascadeQueueOverflow,
    /// An MCP tool invocation (`memory_search` / `memory_write` /
    /// `memory_update` / `memory_delete`) completed (success or error).
    /// Records the call shape — tool name, duration, boundary count,
    /// result count, optional structured error — but **never** raw
    /// argument text or any hash of it. Per `T0.1.9_PLAN.md` §6 v1.3:
    /// vault-mcp is the single audit layer for tool invocation
    /// accounting; `vault_retrieval::Retriever::retrieve` is downstream
    /// and emits operational `tracing::info!` only.
    /// `details_json` schema locked in ADR-024 alongside the JSON-RPC
    /// error mapping table. Superset of T0.1.8's v1.2 retrieval shape
    /// so search calls retain full diagnostic detail at the MCP layer.
    McpToolInvoke,

    /// **ADR-024 amendment 2026-05-05 (T0.1.11 Phase 4b — Decision 5(γ)).**
    /// Recorded when vault-tauri dispatches a Tauri command (add_memory
    /// / search_memories / update_memory / delete_memory) through
    /// `Adapter`. Tauri commands are vault-state changes that don't go
    /// through the MCP wire layer, so the existing `mcp.tool_invoke`
    /// semantic doesn't apply. This variant gives Tauri commands their
    /// own audit-chain shape per ADR-024 amendment Decision 5(γ): "ADR-024's
    /// audit chain is the authoritative record of vault-state changes;
    /// Tauri commands ARE vault-state changes; they belong in the audit
    /// chain with their own discriminator." Reusing `mcp.tool_invoke`
    /// would create semantic debt at V0.2 cloud sync where MCP vs UI
    /// origin needs disambiguation.
    /// `details_json` shape: same as `mcp.tool_invoke` (ToolInvokeDetails
    /// schema) — duration_ms, result_count, boundary_count, max_results
    /// / score_threshold / include_archived / query_length / error per
    /// command type. Tools are the Tauri command names ("add_memory",
    /// "search_memories", "update_memory", "delete_memory"). Reusing
    /// ToolInvokeDetails keeps the audit-row schema simple; only the
    /// event_type discriminates UI commands from MCP tools.
    TauriCommandInvoke,

    /// Recorded when [`StorageBackend::mark_superseded`] marks a memory as
    /// superseded by another (Phase 3 consolidator primitive per ADR-046,
    /// T0.2.3 commit 2). Distinct from [`Self::MemoryUpdate`] to preserve
    /// provenance fidelity per BRD §5.6 line 948 ("do not delete — preserve
    /// provenance"): consolidator-driven supersession is a semantically
    /// distinct event class from user-driven content edits, and the audit
    /// viewer (T0.2.15) filters supersession events by `event_type` rather
    /// than by JSON-path query against the `details` field.
    ///
    /// `details_json` shape: `{"superseded_by":"<MemoryId>"}` — the
    /// `resource_id` field carries the old (superseded) MemoryId; the
    /// `details_json.superseded_by` field carries the new (superseding)
    /// MemoryId. Downstream queries join `resource_id` ↔
    /// `details.superseded_by` to render the supersession chain.
    ///
    /// [`StorageBackend::mark_superseded`]: crate::cascading::StorageBackend::mark_superseded
    MemorySuperseded,

    /// Recorded when [`StorageBackend::invalidate`] sets a memory's `valid_until`
    /// to mark its content as no longer true (bi-temporal invalidation per
    /// ADR-051, T0.2.7 Phase B). Distinct from [`Self::MemorySuperseded`]:
    /// supersession links a memory to its consolidated/replacement memory
    /// (lineage), while invalidation marks the fact as false-in-the-world at a
    /// specific time. Both fields may be set on the same memory by the
    /// write-time `UPDATE` decision in Phase C.
    ///
    /// `details_json` shape:
    /// `{"valid_until":"<ISO-8601>","reason":"<free-text>"}` — the
    /// `resource_id` field carries the invalidated MemoryId.
    ///
    /// [`StorageBackend::invalidate`]: crate::cascading::StorageBackend::invalidate
    MemoryInvalidated,

    /// Recorded on the SURVIVOR of a deterministic dedup
    /// ([`StorageBackend::apply_dedup`], consolidator Phase 2-pre per ADR-063).
    /// Distinct from [`Self::MemorySuperseded`] (which fires on each LOSER) and
    /// from [`Self::MemoryUpdate`] (user-driven content edits): a dedup roll-up
    /// is a consolidator-driven aggregate bump (summed `access_count`, max
    /// `confidence`) onto an existing canonical memory whose CONTENT is
    /// unchanged — no re-embed. The audit viewer (T0.2.15) filters these
    /// distinctly so a near-identical-dedup roll-up is never mistaken for a
    /// content edit.
    ///
    /// `details_json` shape:
    /// `{"absorbed":["<MemoryId>",...],"summed_access_count":<u32>,"max_confidence":<f32>}`
    /// — the `resource_id` field carries the survivor MemoryId; `absorbed`
    /// lists the superseded (loser) ids rolled into it.
    ///
    /// [`StorageBackend::apply_dedup`]: crate::cascading::StorageBackend::apply_dedup
    MemoryDeduped,

    /// Recorded when the sleep consolidator's Phase 4 decays a cold fact's
    /// `confidence` ([`StorageBackend::apply_decay`], BRD §5.6 line 994).
    /// Distinct from [`Self::MemoryUpdate`] (user content edit) and
    /// [`Self::MemoryDeduped`] (merge roll-up): a decay is a system-driven,
    /// metadata-only confidence multiply (×0.9) on an untouched fact — content
    /// and vector are unchanged. The audit viewer (T0.2.15) surfaces these in
    /// the run's Decay section.
    ///
    /// `details_json` shape: `{"old_confidence":<f32>,"new_confidence":<f32>}`
    /// — `resource_id` carries the decayed MemoryId.
    ///
    /// [`StorageBackend::apply_decay`]: crate::cascading::StorageBackend::apply_decay
    MemoryDecayed,

    /// Recorded when the sleep consolidator's Phase 4 moves a cold fact to
    /// archive ([`StorageBackend::apply_archive`], BRD §5.6 lines 995-996;
    /// ADR-084). A fact untouched past `archive_after_days` has its
    /// `archived_at` marker set, dropping it OUT of default retrieval.
    /// Distinct from [`Self::MemoryDecayed`] (confidence fade, still
    /// retrievable) and [`Self::MemoryInvalidated`] (became false): an archive
    /// is a system-driven, metadata-only state change — content and vector are
    /// unchanged, and it is reversible (un-archiving clears the marker). The
    /// audit viewer surfaces these in the run's Decay/Archive section.
    ///
    /// `details_json` shape: `{"archived_at":"<rfc3339>"}` — `resource_id`
    /// carries the archived MemoryId.
    ///
    /// [`StorageBackend::apply_archive`]: crate::cascading::StorageBackend::apply_archive
    MemoryArchived,

    /// A memory came back out of Recently deleted (ADR-114): a deleted memory
    /// written back, or a replaced wording put back. Owner only.
    MemoryRestored,

    /// Recently deleted entries erased for good (ADR-114): by the owner
    /// (`"reason":"user"`) or after 30 days (`"reason":"expired"`).
    /// `details_json` carries the reason and the count, never content.
    BinErased,
}

impl AuditEventType {
    /// Stable wire-format string. Hand-written to avoid coupling to serde's
    /// (which we still verify in tests round-trips through both).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MemoryCreate => "memory.create",
            Self::MemoryRead => "memory.read",
            Self::MemoryUpdate => "memory.update",
            Self::MemoryDelete => "memory.delete",
            Self::MemoryList => "memory.list",
            Self::SchemaMigration => "schema.migration",
            Self::CascadeDeadLetter => "cascade.dead_letter",
            Self::StoreCorruption => "store.corruption",
            Self::CascadeQueueOverflow => "cascade.queue_overflow",
            Self::McpToolInvoke => "mcp.tool_invoke",
            Self::TauriCommandInvoke => "ui.tauri_command_invoke",
            Self::MemorySuperseded => "memory.superseded",
            Self::MemoryInvalidated => "memory.invalidated",
            Self::MemoryDeduped => "memory.deduped",
            Self::MemoryDecayed => "memory.decayed",
            Self::MemoryArchived => "memory.archived",
            Self::MemoryRestored => "memory.restored",
            Self::BinErased => "bin.erased",
        }
    }

    /// Parse the wire-format string. Returns `None` for unknown values so
    /// callers can decide what to do (skip, escalate, fail closed).
    ///
    /// The `"retrieval.query"` parse arm from T0.1.8 v1.2 is **not** carried
    /// forward — T0.1.9 §6.2 rule 1 specifies the audit chain rename is
    /// non-backward-compatible by design (V0.1 has no shipped users; founder
    /// development databases regenerate on next test run).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "memory.create" => Some(Self::MemoryCreate),
            "memory.read" => Some(Self::MemoryRead),
            "memory.update" => Some(Self::MemoryUpdate),
            "memory.delete" => Some(Self::MemoryDelete),
            "memory.list" => Some(Self::MemoryList),
            "schema.migration" => Some(Self::SchemaMigration),
            "cascade.dead_letter" => Some(Self::CascadeDeadLetter),
            "store.corruption" => Some(Self::StoreCorruption),
            "cascade.queue_overflow" => Some(Self::CascadeQueueOverflow),
            "mcp.tool_invoke" => Some(Self::McpToolInvoke),
            "ui.tauri_command_invoke" => Some(Self::TauriCommandInvoke),
            "memory.superseded" => Some(Self::MemorySuperseded),
            "memory.invalidated" => Some(Self::MemoryInvalidated),
            "memory.deduped" => Some(Self::MemoryDeduped),
            "memory.decayed" => Some(Self::MemoryDecayed),
            "memory.archived" => Some(Self::MemoryArchived),
            "memory.restored" => Some(Self::MemoryRestored),
            "bin.erased" => Some(Self::BinErased),
            _ => None,
        }
    }
}

/// Who triggered the operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    User,
    Agent,
    System,
}

impl ActorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
            Self::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "agent" => Some(Self::Agent),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

/// Outcome of the audited operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditResult {
    Success,
    Denied,
    Error,
}

impl AuditResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Denied => "denied",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "success" => Some(Self::Success),
            "denied" => Some(Self::Denied),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// Caller-supplied audit event content. The store assigns `event_id`,
/// `timestamp`, `prev_event_hash`, and `event_hash` at insertion time.
#[derive(Clone, Debug)]
pub struct PendingAuditEvent {
    pub event_type: AuditEventType,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub boundary: Option<Boundary>,
    pub actor_kind: ActorKind,
    pub actor_name: Option<String>,
    pub user_id: Option<String>,
    pub device_id: Option<String>,
    pub result: AuditResult,
    /// Verbatim JSON string. Caller is responsible for canonicalisation if
    /// they need cross-process determinism. Defaults to `"{}"`.
    pub details_json: String,
}

impl PendingAuditEvent {
    /// Convenience constructor for a successful, no-detail event.
    pub fn success(event_type: AuditEventType, actor_kind: ActorKind) -> Self {
        Self {
            event_type,
            resource_type: None,
            resource_id: None,
            boundary: None,
            actor_kind,
            actor_name: None,
            user_id: None,
            device_id: None,
            result: AuditResult::Success,
            details_json: "{}".to_string(),
        }
    }

    /// Attach a resource (`type` + `id`) to the event.
    #[must_use]
    pub fn with_resource(mut self, kind: impl Into<String>, id: impl Into<String>) -> Self {
        self.resource_type = Some(kind.into());
        self.resource_id = Some(id.into());
        self
    }

    /// Attach a boundary scope to the event.
    #[must_use]
    pub fn with_boundary(mut self, boundary: Boundary) -> Self {
        self.boundary = Some(boundary);
        self
    }

    /// Mark the event as a denial.
    #[must_use]
    pub fn denied(mut self) -> Self {
        self.result = AuditResult::Denied;
        self
    }

    /// Mark the event as an error.
    #[must_use]
    pub fn error(mut self) -> Self {
        self.result = AuditResult::Error;
        self
    }
}

/// A persisted audit event read back from the database.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub seq: i64,
    pub event_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub user_id: Option<String>,
    pub device_id: Option<String>,
    pub event_type: AuditEventType,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    /// Boundary as the raw string read from disk. We don't re-validate via
    /// `Boundary` here because boundary-name validation rules may evolve;
    /// the audit log is a historical record.
    pub boundary: Option<String>,
    pub actor_kind: ActorKind,
    pub actor_name: Option<String>,
    pub result: AuditResult,
    pub details_json: String,
    pub prev_event_hash: String,
    pub event_hash: String,
}

/// Fields that contribute to the canonical bytes hashed into `event_hash`.
/// Field order is fixed and load-bearing — never reorder. New fields go at
/// the end (with `#[serde(skip_serializing_if = "Option::is_none")]` to
/// keep old chains valid is **not** sufficient on its own — see
/// HANDOFF.md ADR before evolving this).
#[derive(Serialize)]
struct CanonicalBody<'a> {
    event_id: String,
    timestamp: String,
    user_id: Option<&'a str>,
    device_id: Option<&'a str>,
    event_type: &'static str,
    resource_type: Option<&'a str>,
    resource_id: Option<&'a str>,
    boundary: Option<&'a str>,
    actor_kind: &'static str,
    actor_name: Option<&'a str>,
    result: &'static str,
    details_json: &'a str,
}

/// Compute the canonical byte representation of the event content. Used as
/// the second input to BLAKE3 alongside `prev_event_hash`.
///
/// The 12 arguments correspond exactly to the 12 fields of [`CanonicalBody`].
/// This function is private and consumed by exactly two callers ([`seal`] and
/// [`verify_chain`]); extracting to a struct would just hide the argument
/// count without reducing complexity. Keeping the explicit arg list makes the
/// chain-input contract visible at every call site, which is precisely what
/// we want for tamper-evidence — any change to this signature is a chain
/// schema change and must be treated as such.
#[allow(clippy::too_many_arguments)]
fn canonical_bytes(
    event_id: Uuid,
    timestamp: DateTime<Utc>,
    user_id: Option<&str>,
    device_id: Option<&str>,
    event_type: AuditEventType,
    resource_type: Option<&str>,
    resource_id: Option<&str>,
    boundary: Option<&str>,
    actor_kind: ActorKind,
    actor_name: Option<&str>,
    result: AuditResult,
    details_json: &str,
) -> Vec<u8> {
    let body = CanonicalBody {
        event_id: event_id.hyphenated().to_string(),
        timestamp: timestamp.to_rfc3339(),
        user_id,
        device_id,
        event_type: event_type.as_str(),
        resource_type,
        resource_id,
        boundary,
        actor_kind: actor_kind.as_str(),
        actor_name,
        result: result.as_str(),
        details_json,
    };
    // Serialisation of primitive-only fields cannot fail. The expect message
    // documents the invariant per BRD §0.2.
    serde_json::to_vec(&body).expect("CanonicalBody serialisation cannot fail")
}

/// Compute `BLAKE3(prev_hash_hex_bytes || canonical_bytes)` as hex.
pub fn compute_event_hash(prev_hash_hex: &str, canonical: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(prev_hash_hex.as_bytes());
    hasher.update(canonical);
    hasher.finalize().to_hex().to_string()
}

/// Compute the canonical bytes + event hash for a [`PendingAuditEvent`]
/// using the supplied identity (`event_id`, `timestamp`, `prev_hash`).
///
/// Returns `(canonical_bytes, event_hash_hex)`. The store inserts both
/// into the `audit_log` table.
pub fn seal(
    event_id: Uuid,
    timestamp: DateTime<Utc>,
    prev_hash_hex: &str,
    pending: &PendingAuditEvent,
) -> (Vec<u8>, String) {
    let canonical = canonical_bytes(
        event_id,
        timestamp,
        pending.user_id.as_deref(),
        pending.device_id.as_deref(),
        pending.event_type,
        pending.resource_type.as_deref(),
        pending.resource_id.as_deref(),
        pending.boundary.as_ref().map(Boundary::as_str),
        pending.actor_kind,
        pending.actor_name.as_deref(),
        pending.result,
        &pending.details_json,
    );
    let hash = compute_event_hash(prev_hash_hex, &canonical);
    (canonical, hash)
}

/// Walk an in-order slice of events and verify the hash chain.
///
/// # Errors
///
/// Returns [`VaultError::Storage`] at the first inconsistency:
/// - a mismatched `prev_event_hash` (the chain is broken)
/// - a mismatched `event_hash` (the event was tampered with)
///
/// `events` must be sorted by `seq` ascending.
pub fn verify_chain(events: &[AuditEvent]) -> VaultResult<()> {
    let mut prev = AUDIT_GENESIS_HASH.to_string();
    for ev in events {
        if ev.prev_event_hash != prev {
            return Err(VaultError::Storage(format!(
                "audit chain broken at seq {}: expected prev_hash {prev}, found {}",
                ev.seq, ev.prev_event_hash,
            )));
        }
        let canonical = canonical_bytes(
            ev.event_id,
            ev.timestamp,
            ev.user_id.as_deref(),
            ev.device_id.as_deref(),
            ev.event_type,
            ev.resource_type.as_deref(),
            ev.resource_id.as_deref(),
            ev.boundary.as_deref(),
            ev.actor_kind,
            ev.actor_name.as_deref(),
            ev.result,
            &ev.details_json,
        );
        let computed = compute_event_hash(&prev, &canonical);
        if computed != ev.event_hash {
            return Err(VaultError::Storage(format!(
                "audit event hash mismatch at seq {}: tampering detected (expected {computed}, found {})",
                ev.seq, ev.event_hash,
            )));
        }
        prev.clone_from(&ev.event_hash);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event(seq: i64, prev_hash: &str) -> AuditEvent {
        let event_id = Uuid::now_v7();
        let timestamp = DateTime::parse_from_rfc3339("2026-04-28T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let canonical = canonical_bytes(
            event_id,
            timestamp,
            None,
            None,
            AuditEventType::MemoryCreate,
            Some("memory"),
            Some("00000000-0000-0000-0000-000000000001"),
            Some("work"),
            ActorKind::User,
            None,
            AuditResult::Success,
            "{}",
        );
        let hash = compute_event_hash(prev_hash, &canonical);
        AuditEvent {
            seq,
            event_id,
            timestamp,
            user_id: None,
            device_id: None,
            event_type: AuditEventType::MemoryCreate,
            resource_type: Some("memory".into()),
            resource_id: Some("00000000-0000-0000-0000-000000000001".into()),
            boundary: Some("work".into()),
            actor_kind: ActorKind::User,
            actor_name: None,
            result: AuditResult::Success,
            details_json: "{}".into(),
            prev_event_hash: prev_hash.to_string(),
            event_hash: hash,
        }
    }

    #[test]
    fn genesis_hash_is_64_zeros() {
        assert_eq!(AUDIT_GENESIS_HASH.len(), 64);
        assert!(AUDIT_GENESIS_HASH.chars().all(|c| c == '0'));
    }

    #[test]
    fn event_type_string_round_trip() {
        for et in [
            AuditEventType::MemoryCreate,
            AuditEventType::MemoryRead,
            AuditEventType::MemoryUpdate,
            AuditEventType::MemoryDelete,
            AuditEventType::MemoryList,
            AuditEventType::SchemaMigration,
            AuditEventType::CascadeDeadLetter,
            AuditEventType::StoreCorruption,
            AuditEventType::CascadeQueueOverflow,
            AuditEventType::McpToolInvoke,
            AuditEventType::MemorySuperseded,
            AuditEventType::MemoryInvalidated,
            AuditEventType::MemoryDeduped,
            AuditEventType::MemoryDecayed,
            AuditEventType::MemoryArchived,
            AuditEventType::MemoryRestored,
            AuditEventType::BinErased,
        ] {
            assert_eq!(AuditEventType::parse(et.as_str()), Some(et));
        }
        assert_eq!(AuditEventType::parse("unknown.kind"), None);
        // Pin the v1.3 wire format string explicitly — the constant is
        // load-bearing for the MCP tool-invocation audit contract per
        // T0.1.9_PLAN.md §6 / ADR-024.
        assert_eq!(AuditEventType::McpToolInvoke.as_str(), "mcp.tool_invoke");
        // Pin the memory.superseded wire-format string per ADR-046 (T0.2.3
        // commit 2) — load-bearing for the T0.2.15 Tauri audit-viewer filter
        // (event_type == "memory.superseded" enumerates consolidator-driven
        // supersession events distinctly from user-driven memory updates).
        assert_eq!(
            AuditEventType::MemorySuperseded.as_str(),
            "memory.superseded"
        );
        // Pin the memory.deduped wire-format string per ADR-063 — load-bearing
        // for the audit-viewer filter (distinguishes dedup roll-ups from
        // user-driven memory.update edits and from memory.superseded losers).
        assert_eq!(AuditEventType::MemoryDeduped.as_str(), "memory.deduped");
        // Pin the memory.decayed wire-format string per T0.2.4 — load-bearing
        // for the audit-viewer Decay-section filter (distinguishes Phase-4
        // confidence decays from user-driven memory.update edits).
        assert_eq!(AuditEventType::MemoryDecayed.as_str(), "memory.decayed");
        // Pin the memory.archived wire-format string per ADR-084 — load-bearing
        // for the audit-viewer Archive-section filter (distinguishes Phase-4
        // cold-archive moves from decays and from user-driven memory.update
        // edits).
        assert_eq!(AuditEventType::MemoryArchived.as_str(), "memory.archived");
        // Confirm the old v1.2 retrieval wire-format string no longer
        // round-trips — T0.1.9 §6.2 rule 1 specifies non-backward-compat.
        assert_eq!(AuditEventType::parse("retrieval.query"), None);
    }

    #[test]
    fn actor_kind_string_round_trip() {
        for a in [ActorKind::User, ActorKind::Agent, ActorKind::System] {
            assert_eq!(ActorKind::parse(a.as_str()), Some(a));
        }
        assert_eq!(ActorKind::parse("nope"), None);
    }

    #[test]
    fn audit_result_string_round_trip() {
        for r in [
            AuditResult::Success,
            AuditResult::Denied,
            AuditResult::Error,
        ] {
            assert_eq!(AuditResult::parse(r.as_str()), Some(r));
        }
        assert_eq!(AuditResult::parse("indeterminate"), None);
    }

    #[test]
    fn event_hash_is_deterministic() {
        let id = Uuid::now_v7();
        let ts = Utc::now();
        let body1 = canonical_bytes(
            id,
            ts,
            None,
            None,
            AuditEventType::MemoryCreate,
            None,
            None,
            None,
            ActorKind::User,
            None,
            AuditResult::Success,
            "{}",
        );
        let body2 = canonical_bytes(
            id,
            ts,
            None,
            None,
            AuditEventType::MemoryCreate,
            None,
            None,
            None,
            ActorKind::User,
            None,
            AuditResult::Success,
            "{}",
        );
        assert_eq!(body1, body2);
        assert_eq!(
            compute_event_hash(AUDIT_GENESIS_HASH, &body1),
            compute_event_hash(AUDIT_GENESIS_HASH, &body2),
        );
    }

    #[test]
    fn event_hash_changes_on_any_field_change() {
        let id = Uuid::now_v7();
        let ts = Utc::now();
        let base = canonical_bytes(
            id,
            ts,
            None,
            None,
            AuditEventType::MemoryCreate,
            None,
            None,
            None,
            ActorKind::User,
            None,
            AuditResult::Success,
            "{}",
        );
        let changed = canonical_bytes(
            id,
            ts,
            None,
            None,
            AuditEventType::MemoryDelete, // different event type
            None,
            None,
            None,
            ActorKind::User,
            None,
            AuditResult::Success,
            "{}",
        );
        assert_ne!(
            compute_event_hash(AUDIT_GENESIS_HASH, &base),
            compute_event_hash(AUDIT_GENESIS_HASH, &changed),
        );
    }

    #[test]
    fn verify_chain_accepts_valid_chain() {
        let e1 = sample_event(1, AUDIT_GENESIS_HASH);
        let e2 = sample_event(2, &e1.event_hash);
        let e3 = sample_event(3, &e2.event_hash);
        verify_chain(&[e1, e2, e3]).unwrap();
    }

    #[test]
    fn verify_chain_rejects_broken_prev_hash() {
        let e1 = sample_event(1, AUDIT_GENESIS_HASH);
        let mut e2 = sample_event(2, &e1.event_hash);
        // Tamper with the chain link.
        e2.prev_event_hash = AUDIT_GENESIS_HASH.to_string();
        let err = verify_chain(&[e1, e2]).unwrap_err();
        assert!(matches!(err, VaultError::Storage(s) if s.contains("chain broken")));
    }

    #[test]
    fn verify_chain_rejects_tampered_event_field() {
        let e1 = sample_event(1, AUDIT_GENESIS_HASH);
        let mut e2 = sample_event(2, &e1.event_hash);
        // Modify a content field but leave event_hash unchanged — this is the
        // exact scenario the chain protects against.
        e2.boundary = Some("personal".into());
        let err = verify_chain(&[e1, e2]).unwrap_err();
        assert!(matches!(err, VaultError::Storage(s) if s.contains("tampering detected")));
    }

    #[test]
    fn empty_chain_verifies() {
        verify_chain(&[]).unwrap();
    }

    #[test]
    fn pending_event_builders_compose() {
        let boundary = Boundary::new("work").unwrap();
        let pe = PendingAuditEvent::success(AuditEventType::MemoryCreate, ActorKind::User)
            .with_resource("memory", "abc")
            .with_boundary(boundary.clone())
            .denied();
        assert_eq!(pe.event_type, AuditEventType::MemoryCreate);
        assert_eq!(pe.resource_type.as_deref(), Some("memory"));
        assert_eq!(pe.resource_id.as_deref(), Some("abc"));
        assert_eq!(pe.boundary, Some(boundary));
        assert_eq!(pe.result, AuditResult::Denied);
    }

    #[test]
    fn seal_produces_self_consistent_event() {
        let event_id = Uuid::now_v7();
        let timestamp = Utc::now();
        let pending = PendingAuditEvent::success(AuditEventType::MemoryCreate, ActorKind::User)
            .with_resource("memory", "abc");
        let (canonical, hash) = seal(event_id, timestamp, AUDIT_GENESIS_HASH, &pending);
        assert_eq!(hash, compute_event_hash(AUDIT_GENESIS_HASH, &canonical));
    }
}
