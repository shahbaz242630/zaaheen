//! The desktop's command bodies, moved from `vault-tauri/src/commands/` into
//! the keeper (ADR-108 D2). Each keeps the clamps, audit rows (the same
//! `tauri_command` names), JSON shapes and error texts it had in the desktop,
//! so the screens see exactly what they saw before. Only the process they run
//! in changed.
//!
//! Entitlement is not asked here: the desktop asks once per command, and the
//! admin server checks the keeper's own view before dispatching
//! (`server.rs`).

use std::time::Instant;

use chrono::{DateTime, Utc};
use rmcp::schemars;
use serde_json::{json, Value};
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory, VaultError};
use vault_mcp::{Adapter, ToolInvokeDetails, ToolInvokeError};
use vault_retrieval::{RetrievalOptions, RetrievalQuery};

use super::store::OwnerStore;
use crate::VaultAdapter;

/// Upper bound on `list_recent_memories`' page size (BRD §11.7.1).
pub const MAX_RECENT_MEMORIES: usize = 200;

/// Upper bound on an agent name, mirroring the boundary-name cap in
/// BRD §11.7.1 — the closest specified analogue for a short identifier.
pub const MAX_AGENT_NAME_LEN: usize = 64;

/// Largest export page (ADR-108 D2): small enough that one page is a modest
/// message, large enough that a big vault is a few dozen round trips.
pub const MAX_EXPORT_PAGE: usize = 2_000;

/// A `TauriCommandInvoke` row with no search-specific fields.
fn row(
    tool: &'static str,
    started: Instant,
    result_count: u32,
    boundary_count: u32,
) -> ToolInvokeDetails {
    ToolInvokeDetails {
        tool,
        duration_ms: started.elapsed().as_millis() as u64,
        result_count,
        boundary_count,
        max_results: None,
        score_threshold: None,
        include_archived: None,
        query_length: None,
        error: None,
    }
}

fn parse_memory_type(memory_type: &str) -> Result<MemoryType, String> {
    match memory_type {
        "semantic" => Ok(MemoryType::Semantic),
        "episodic" => Ok(MemoryType::Episodic),
        "procedural" => Ok(MemoryType::Procedural),
        other => Err(format!("invalid memory_type: '{other}'")),
    }
}

fn desktop_memory(content: String, memory_type: MemoryType, boundary: Boundary) -> NewMemory {
    NewMemory {
        content,
        memory_type,
        boundary,
        source_agent: Some("vault-tauri".to_string()),
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: json!({}),
    }
}

/// `add_memory`: the new memory's id.
pub async fn memory_add(
    adapter: &VaultAdapter,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<String, String> {
    let start = Instant::now();
    let parsed_memory_type = parse_memory_type(&memory_type)?;
    let parsed_boundary = Boundary::new(&boundary).map_err(|e| format!("invalid boundary: {e}"))?;

    let result = adapter
        .write(desktop_memory(content, parsed_memory_type, parsed_boundary))
        .await;
    let mut details = row("add_memory", start, u32::from(result.is_ok()), 1);
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;

    result.map(|id| id.to_string()).map_err(|e| e.to_string())
}

/// `search_memories`: the owner searches every registered boundary
/// (ADR-SEC-003), falling back to the narrowest slice if the registry cannot
/// be read (SP-4).
pub async fn memory_search(
    adapter: &VaultAdapter,
    query: String,
    limit: usize,
) -> Result<Vec<Value>, String> {
    let authorized_boundaries: Vec<Boundary> = match adapter.list_boundaries().await {
        Ok(infos) => infos.into_iter().map(|i| i.boundary).collect(),
        Err(_) => vec![Boundary::default_name()],
    };
    let start = Instant::now();
    let query_length = query.len();
    let boundary_count = authorized_boundaries.len() as u32;

    let result = adapter
        .search(RetrievalQuery {
            query_text: query,
            authorized_boundaries,
            max_results: limit,
            options: RetrievalOptions {
                score_threshold: None,
                include_archived: false,
            },
        })
        .await;

    let count = result.as_ref().map_or(0, |m| m.len() as u32);
    let mut details = row("search_memories", start, count, boundary_count);
    details.max_results = Some(limit as u32);
    details.include_archived = Some(false);
    details.query_length = Some(query_length as u32);
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;

    result
        .map(|memories| {
            memories
                .into_iter()
                .map(|rm| {
                    json!({
                        "id": rm.memory.id.to_string(),
                        "content": rm.memory.content,
                        "memory_type": format!("{:?}", rm.memory.memory_type).to_lowercase(),
                        "boundary": rm.memory.boundary.as_str(),
                        "score": rm.score,
                        "explanation": rm.explanation,
                        "created_at": rm.memory.created_at.to_rfc3339(),
                    })
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

/// `update_memory`.
pub async fn memory_update(
    adapter: &VaultAdapter,
    id: String,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<(), String> {
    let start = Instant::now();
    let id: MemoryId = id.parse().map_err(|e| format!("invalid memory id: {e}"))?;
    let parsed_memory_type = parse_memory_type(&memory_type)?;
    let parsed_boundary = Boundary::new(&boundary).map_err(|e| format!("invalid boundary: {e}"))?;

    let result = adapter
        .update(
            id,
            desktop_memory(content, parsed_memory_type, parsed_boundary),
        )
        .await;
    let mut details = row("update_memory", start, u32::from(result.is_ok()), 1);
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;

    result.map_err(|e| e.to_string())
}

/// `delete_memory`. The desktop acts as the owner (`ActorKind::User`), so the
/// agent-facing boundary check of ADR-025 does not apply.
pub async fn memory_delete(adapter: &VaultAdapter, id: String) -> Result<(), String> {
    let start = Instant::now();
    let id: MemoryId = id.parse().map_err(|e| format!("invalid memory id: {e}"))?;

    let result = adapter.delete(id).await;
    let mut details = row("delete_memory", start, u32::from(result.is_ok()), 0);
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;

    result.map_err(|e| e.to_string())
}

/// `list_recent_memories`, clamped to 1..=[`MAX_RECENT_MEMORIES`].
pub async fn memory_list_recent(
    adapter: &VaultAdapter,
    limit: usize,
) -> Result<Vec<Value>, String> {
    let start = Instant::now();
    let effective_limit = limit.clamp(1, MAX_RECENT_MEMORIES);

    let result = adapter.list_recent_memories(effective_limit).await;
    let count = result.as_ref().map_or(0, |m| m.len() as u32);
    let mut details = row("list_recent_memories", start, count, 0);
    details.max_results = Some(effective_limit as u32);
    details.include_archived = Some(false);
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;

    result
        .map(|memories| {
            memories
                .into_iter()
                .map(|m| {
                    json!({
                        "id": m.id.to_string(),
                        "content": m.content,
                        "memory_type": format!("{:?}", m.memory_type).to_lowercase(),
                        "boundary": m.boundary.as_str(),
                        "created_at": m.created_at.to_rfc3339(),
                    })
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

/// `list_boundaries`.
pub async fn boundary_list(adapter: &VaultAdapter) -> Result<Vec<Value>, String> {
    let start = Instant::now();
    let result = adapter.list_boundaries().await;
    let count = result.as_ref().map_or(0, |b| b.len() as u32);
    let mut details = row("list_boundaries", start, count, count);
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;

    result
        .map(|boundaries| {
            boundaries
                .into_iter()
                .map(|b| {
                    json!({
                        "name": b.boundary.as_str(),
                        "description": b.description,
                        "created_at": b.created_at.to_rfc3339(),
                        "memory_count": b.memory_count,
                    })
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

/// `create_boundary`: `true` when a new boundary was registered, `false` when
/// it already existed (an idempotent no-op).
pub async fn boundary_create(
    adapter: &VaultAdapter,
    name: String,
    description: Option<String>,
) -> Result<bool, String> {
    let start = Instant::now();
    // Validated BEFORE touching the vault (BRD §11.7.1).
    let parsed = Boundary::new(&name).map_err(|e| format!("invalid boundary name: {e}"))?;

    let result = adapter
        .create_boundary(&parsed, description.as_deref())
        .await;
    let created = *result.as_ref().unwrap_or(&false);
    let mut details = row("create_boundary", start, u32::from(created), 1);
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;

    result.map_err(|e| e.to_string())
}

/// `list_agents`: active AND revoked (revocation is soft). An explicit
/// allowlist serialisation (BRD §11.7.2): no token, not even its hash.
pub async fn agent_list(adapter: &VaultAdapter) -> Result<Vec<Value>, String> {
    let start = Instant::now();
    let result = adapter.list_agents().await;
    let count = result.as_ref().map_or(0, |a| a.len() as u32);
    let mut details = row("list_agents", start, count, 0);
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;

    result
        .map(|agents| {
            agents
                .into_iter()
                .map(|a| {
                    json!({
                        "name": a.agent_name,
                        "boundaries": a.boundaries.iter().map(|b| b.as_str()).collect::<Vec<_>>(),
                        "created_at": a.created_at.to_rfc3339(),
                        "revoked_at": a.revoked_at.map(|t| t.to_rfc3339()),
                        "active": a.revoked_at.is_none(),
                    })
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

/// `revoke_agent`: `false` when no ACTIVE agent of that name existed.
pub async fn agent_revoke(adapter: &VaultAdapter, agent_name: String) -> Result<bool, String> {
    let start = Instant::now();
    if agent_name.is_empty() || agent_name.len() > MAX_AGENT_NAME_LEN {
        return Err("invalid agent name".to_string());
    }

    let result = adapter.revoke_agent(&agent_name).await;
    let revoked = *result.as_ref().unwrap_or(&false);
    let mut details = row("revoke_agent", start, u32::from(revoked), 0);
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;

    result.map_err(|e| e.to_string())
}

/// `get_settings_info`, less the `version` (the desktop adds its own, ADR-108
/// D2). `data_dir` is passed in as the person reads it.
///
/// A broken audit chain is a REPORTABLE STATE, not a failure: the tab exists
/// partly to show it.
pub async fn settings_info(store: &dyn OwnerStore, data_dir: String) -> Result<Value, String> {
    let start = Instant::now();
    let memory_count = store.total_memory_count().await;
    let boundary_count = store.list_boundaries().await.map(|b| b.len());
    let audit_chain_verified = store.verify_audit_chain().await.is_ok();

    let mut details = row(
        "get_settings_info",
        start,
        1,
        *boundary_count.as_ref().unwrap_or(&0) as u32,
    );
    details.error = memory_count
        .as_ref()
        .err()
        .or(boundary_count.as_ref().err())
        .map(ToolInvokeError::from_vault_error);
    let _ = store.append_tauri_command_audit(details).await;

    let memory_count = memory_count.map_err(|e| e.to_string())?;
    let boundary_count = boundary_count.map_err(|e| e.to_string())?;
    Ok(json!({
        "data_dir": data_dir,
        "memory_count": memory_count,
        "boundary_count": boundary_count,
        "audit_chain_verified": audit_chain_verified,
    }))
}

/// The empty Settings answer for a computer with no vault yet (lock mode, no
/// database): nothing counted, nothing to verify, nothing written.
pub fn settings_info_empty(data_dir: String) -> Value {
    json!({
        "data_dir": data_dir,
        "memory_count": 0,
        "boundary_count": 0,
        "audit_chain_verified": true,
    })
}

/// One export page (ADR-108 D2), at most [`MAX_EXPORT_PAGE`] memories, newest
/// first, after `before`. Embeddings are dropped: the export never contains
/// them, and they would make each page many times larger. No audit row here —
/// the desktop writes the one export row after the file is written, with the
/// final outcome.
///
/// # Errors
///
/// The storage error, as text for the log; the desktop answers its own code.
pub async fn export_page(
    store: &dyn OwnerStore,
    before: Option<(DateTime<Utc>, MemoryId)>,
    limit: usize,
) -> Result<Vec<Memory>, VaultError> {
    let mut page = store
        .list_memories_page(before, limit.clamp(1, MAX_EXPORT_PAGE))
        .await?;
    for memory in &mut page {
        memory.embedding = None;
    }
    Ok(page)
}

/// The desktop events that write their audit row through the keeper (ADR-108
/// D3). A closed list: the desktop cannot invent a row.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DesktopEvent {
    ExportMemories,
    ExportLogs,
    SetMaintenanceSchedule,
}

impl DesktopEvent {
    /// The `tauri_command` name the row has always carried.
    pub fn tool(self) -> &'static str {
        match self {
            Self::ExportMemories => "export_memories",
            Self::ExportLogs => "export_logs",
            Self::SetMaintenanceSchedule => "set_maintenance_schedule",
        }
    }

    /// Whether a locked computer may record it: the two exports are always
    /// available, the schedule is not (A-S1).
    pub fn open(self) -> bool {
        matches!(self, Self::ExportMemories | Self::ExportLogs)
    }

    /// The error each row has always recorded on failure, word for word.
    fn failure(self) -> VaultError {
        match self {
            Self::ExportMemories => VaultError::Storage("memory export failed".to_string()),
            Self::ExportLogs => VaultError::Storage("log export failed".to_string()),
            Self::SetMaintenanceSchedule => {
                VaultError::Scheduler("schedule change failed".to_string())
            }
        }
    }

    /// The fields only some rows have carried.
    fn shape(self, details: &mut ToolInvokeDetails) {
        if self == Self::ExportMemories {
            details.max_results = Some(EXPORT_AUDIT_MAX);
            details.include_archived = Some(false);
        }
    }
}

/// The `max_results` the export row has always carried: the old single-read
/// cap, kept so rows before and after D4 read the same.
const EXPORT_AUDIT_MAX: u32 = 100_000;

/// Append a desktop event's row, as the desktop used to: the desktop measured
/// the operation and reports its duration, count and outcome.
pub async fn audit_event(
    store: &dyn OwnerStore,
    event: DesktopEvent,
    duration_ms: u64,
    result_count: u32,
    failed: bool,
) -> Result<(), String> {
    let mut details = row(event.tool(), Instant::now(), result_count, 0);
    details.duration_ms = duration_ms;
    event.shape(&mut details);
    if failed {
        details.error = Some(ToolInvokeError::from_vault_error(&event.failure()));
    }
    store
        .append_tauri_command_audit(details)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_limit_clamps_to_the_documented_bounds() {
        assert_eq!(
            usize::MAX.clamp(1, MAX_RECENT_MEMORIES),
            MAX_RECENT_MEMORIES
        );
        assert_eq!(0_usize.clamp(1, MAX_RECENT_MEMORIES), 1);
        assert_eq!(20_usize.clamp(1, MAX_RECENT_MEMORIES), 20);
    }

    #[test]
    fn the_desktop_events_keep_their_audit_names() {
        assert_eq!(DesktopEvent::ExportMemories.tool(), "export_memories");
        assert_eq!(DesktopEvent::ExportLogs.tool(), "export_logs");
        assert_eq!(
            DesktopEvent::SetMaintenanceSchedule.tool(),
            "set_maintenance_schedule"
        );
    }

    #[test]
    fn only_the_exports_are_open_to_a_locked_computer() {
        assert!(DesktopEvent::ExportMemories.open());
        assert!(DesktopEvent::ExportLogs.open());
        assert!(!DesktopEvent::SetMaintenanceSchedule.open());
    }

    #[test]
    fn the_empty_settings_answer_names_no_stack_and_counts_nothing() {
        let v = settings_info_empty("C:\\Users\\x\\Zaaheen".to_string());
        assert_eq!(v["memory_count"], 0);
        assert_eq!(v["boundary_count"], 0);
        assert!(v.get("version").is_none(), "the desktop adds its own");
    }
}
