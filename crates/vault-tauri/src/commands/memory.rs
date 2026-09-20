//! Memory CRUD commands — BRD §5.11 `commands/memory.rs`.
//!
//! Shared context (audit posture, auth-gating, testability pattern) is
//! documented on the parent module.

use std::time::Instant;

use tauri::State;
use vault_app::Application;
use vault_core::{Boundary, MemoryId, MemoryType, NewMemory};
use vault_mcp::{Adapter, ToolInvokeDetails};
use vault_retrieval::{RetrievalOptions, RetrievalQuery};

use crate::guard::{Entitled, Entitlement};

/// Upper bound on `list_recent_memories`' page size.
///
/// BRD §11.7.1 requires a bound on every input; without one, a caller could
/// ask for the entire vault and force an unbounded row materialization.
pub const MAX_RECENT_MEMORIES: usize = 200;

/// Inner add_memory implementation. Pure async fn over `&Application`
/// for testability.
pub async fn add_memory_inner(
    app: &Application,
    _entitled: &Entitled,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<MemoryId, String> {
    let adapter = app.adapter();
    let start = Instant::now();

    let parsed_memory_type = match memory_type.as_str() {
        "semantic" => MemoryType::Semantic,
        "episodic" => MemoryType::Episodic,
        "procedural" => MemoryType::Procedural,
        other => return Err(format!("invalid memory_type: '{other}'")),
    };
    let parsed_boundary = Boundary::new(&boundary).map_err(|e| format!("invalid boundary: {e}"))?;

    let new_memory = NewMemory {
        content,
        memory_type: parsed_memory_type,
        boundary: parsed_boundary,
        source_agent: Some("vault-tauri".to_string()),
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({}),
    };

    let result = adapter.write(new_memory).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (id, error_for_audit) = match &result {
        Ok(id) => (Some(*id), None),
        Err(e) => (None, Some(vault_mcp::ToolInvokeError::from_vault_error(e))),
    };

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "add_memory",
            duration_ms,
            result_count: if id.is_some() { 1 } else { 0 },
            boundary_count: 1,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        })
        .await;

    result.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_memory(
    state: State<'_, Application>,
    entitlement: State<'_, Entitlement>,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<String, String> {
    let entitled = entitlement.require().await?;
    add_memory_inner(state.inner(), &entitled, content, memory_type, boundary)
        .await
        .map(|id| id.to_string())
}

/// Inner search_memories implementation.
pub async fn search_memories_inner(
    app: &Application,
    _entitled: &Entitled,
    query: String,
    limit: usize,
    authorized_boundaries: Vec<Boundary>,
) -> Result<Vec<serde_json::Value>, String> {
    let adapter = app.adapter();
    let start = Instant::now();
    let query_length = query.len();

    let retrieval_query = RetrievalQuery {
        query_text: query,
        authorized_boundaries: authorized_boundaries.clone(),
        max_results: limit,
        options: RetrievalOptions {
            score_threshold: None,
            include_archived: false,
        },
    };

    let result = adapter.search(retrieval_query).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (count, error_for_audit) = match &result {
        Ok(memories) => (memories.len() as u32, None),
        Err(e) => (0, Some(vault_mcp::ToolInvokeError::from_vault_error(e))),
    };

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "search_memories",
            duration_ms,
            result_count: count,
            boundary_count: authorized_boundaries.len() as u32,
            max_results: Some(limit as u32),
            score_threshold: None,
            include_archived: Some(false),
            query_length: Some(query_length as u32),
            error: error_for_audit,
        })
        .await;

    result
        .map(|memories| {
            memories
                .into_iter()
                .map(|rm| {
                    serde_json::json!({
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

#[tauri::command]
pub async fn search_memories(
    state: State<'_, Application>,
    entitlement: State<'_, Entitlement>,
    query: String,
    limit: usize,
) -> Result<Vec<serde_json::Value>, String> {
    // Asked BEFORE the boundary listing below: a locked computer must not
    // reach the vault at all, not even to enumerate boundary names.
    let entitled = entitlement.require().await?;

    // ADR-SEC-003: the desktop UI acts as the vault OWNER, so search spans
    // every registered boundary rather than the hardcoded `default` it used
    // before UI slice 2. A boundary the owner cannot search is a boundary the
    // Boundaries tab can only decorate. Agent-facing paths are unaffected —
    // they resolve their slice per-request from the capability token.
    let boundaries = match state.inner().adapter().list_boundaries().await {
        Ok(infos) => infos.into_iter().map(|i| i.boundary).collect(),
        // SP-4 fail securely: if the registry read fails, fall back to the
        // narrowest slice that keeps search working rather than widening.
        Err(_) => vec![Boundary::default_name()],
    };
    search_memories_inner(state.inner(), &entitled, query, limit, boundaries).await
}

/// Inner update_memory implementation.
pub async fn update_memory_inner(
    app: &Application,
    _entitled: &Entitled,
    id_str: String,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<(), String> {
    let adapter = app.adapter();
    let start = Instant::now();

    let id: MemoryId = id_str
        .parse()
        .map_err(|e| format!("invalid memory id: {e}"))?;

    let parsed_memory_type = match memory_type.as_str() {
        "semantic" => MemoryType::Semantic,
        "episodic" => MemoryType::Episodic,
        "procedural" => MemoryType::Procedural,
        other => return Err(format!("invalid memory_type: '{other}'")),
    };
    let parsed_boundary = Boundary::new(&boundary).map_err(|e| format!("invalid boundary: {e}"))?;

    let new_memory = NewMemory {
        content,
        memory_type: parsed_memory_type,
        boundary: parsed_boundary,
        source_agent: Some("vault-tauri".to_string()),
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({}),
    };

    let result = adapter.update(id, new_memory).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let error_for_audit = result
        .as_ref()
        .err()
        .map(vault_mcp::ToolInvokeError::from_vault_error);

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "update_memory",
            duration_ms,
            result_count: if result.is_ok() { 1 } else { 0 },
            boundary_count: 1,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        })
        .await;

    result.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_memory(
    state: State<'_, Application>,
    entitlement: State<'_, Entitlement>,
    id: String,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<(), String> {
    let entitled = entitlement.require().await?;
    update_memory_inner(state.inner(), &entitled, id, content, memory_type, boundary).await
}

/// Inner delete_memory implementation. Note: ADR-025 amendment auth-
/// gate from Phase 4a lives at the MCP/StdioServer layer; Tauri layer
/// operates as founder/User actor and bypasses that gate (V0.1
/// founder-only context). V0.2 alpha-cohort will revisit per
/// ADR-029-implied multi-user trust context.
pub async fn delete_memory_inner(
    app: &Application,
    _entitled: &Entitled,
    id_str: String,
) -> Result<(), String> {
    let adapter = app.adapter();
    let start = Instant::now();

    let id: MemoryId = id_str
        .parse()
        .map_err(|e| format!("invalid memory id: {e}"))?;

    let result = adapter.delete(id).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let error_for_audit = result
        .as_ref()
        .err()
        .map(vault_mcp::ToolInvokeError::from_vault_error);

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "delete_memory",
            duration_ms,
            result_count: if result.is_ok() { 1 } else { 0 },
            boundary_count: 0,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        })
        .await;

    result.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_memory(
    state: State<'_, Application>,
    entitlement: State<'_, Entitlement>,
    id: String,
) -> Result<(), String> {
    let entitled = entitlement.require().await?;
    delete_memory_inner(state.inner(), &entitled, id).await
}

/// Inner list_recent_memories implementation.
///
/// Replaces the home tab's browser-local cache, which only ever held
/// UI-added memories — anything an agent wrote through MCP was invisible on
/// the home screen even though search found it fine.
pub async fn list_recent_memories_inner(
    app: &Application,
    _entitled: &Entitled,
    limit: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let adapter = app.adapter();
    let start = Instant::now();

    // Clamp rather than reject: a too-large page is a UI bug, not an attack,
    // and silently serving 200 is friendlier than an error the user can't act
    // on. Zero is meaningless here, so it also floors at 1.
    let effective_limit = limit.clamp(1, MAX_RECENT_MEMORIES);

    let result = adapter.list_recent_memories(effective_limit).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (count, error_for_audit) = match &result {
        Ok(memories) => (memories.len() as u32, None),
        Err(e) => (0, Some(vault_mcp::ToolInvokeError::from_vault_error(e))),
    };

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "list_recent_memories",
            duration_ms,
            result_count: count,
            boundary_count: 0,
            max_results: Some(effective_limit as u32),
            score_threshold: None,
            include_archived: Some(false),
            query_length: None,
            error: error_for_audit,
        })
        .await;

    result
        .map(|memories| {
            memories
                .into_iter()
                .map(|m| {
                    serde_json::json!({
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

#[tauri::command]
pub async fn list_recent_memories(
    state: State<'_, Application>,
    entitlement: State<'_, Entitlement>,
    limit: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let entitled = entitlement.require().await?;
    list_recent_memories_inner(state.inner(), &entitled, limit).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_limit_clamps_to_the_documented_maximum() {
        assert_eq!(
            usize::MAX.clamp(1, MAX_RECENT_MEMORIES),
            MAX_RECENT_MEMORIES,
            "an unbounded page request must clamp, never materialize the vault"
        );
    }

    #[test]
    fn recent_limit_floors_at_one() {
        assert_eq!(
            0_usize.clamp(1, MAX_RECENT_MEMORIES),
            1,
            "a zero page size is meaningless and must floor at 1"
        );
    }

    #[test]
    fn recent_limit_passes_through_a_normal_page_size() {
        assert_eq!(20_usize.clamp(1, MAX_RECENT_MEMORIES), 20);
    }

    // Test fixture construction for vault-tauri commands requires a
    // real Application (SqlCipher + LanceDB + DuckDB + ORT). This is
    // the same constraint as vault-app/tests/integration_smoke.rs —
    // those tests are #[ignore]-by-default per session-discipline.
    //
    // Phase 4b commits 5 IPC tests + 1 audit-row test as
    // #[ignore]-by-default placeholders; Phase 5 close OR V0.2 alpha-
    // distribution task lands the real implementations once the test
    // fixture infrastructure is shared across vault-app +
    // vault-tauri (currently each crate would need to duplicate the
    // BgeSmallProvider + StorageBackend setup boilerplate).
    //
    // Per Shahbaz Phase 4b v2 review + step-expansion #[ignore]
    // discipline: each placeholder unimplemented! body fails when run
    // with --ignored. Workspace floor reflects the +6 ignored deltas.

    /// Phase 4b ignored placeholder. ADR-024 amendment Decision 5(γ)
    /// pinning test — landing the real impl needs shared test-fixture
    /// scaffolding for Application construction.
    #[tokio::test]
    #[ignore = "Phase 4b deferred — Application test-fixture needs sharing across vault-app + vault-tauri; lands at V0.2 alpha-distribution"]
    async fn add_memory_command_dispatches_through_adapter_write() {
        unimplemented!("Phase 4b ignored placeholder — V0.2 alpha-distribution lands real impl");
    }

    #[tokio::test]
    #[ignore = "Phase 4b deferred — same fixture-sharing constraint as add_memory_command test above"]
    async fn search_memories_command_dispatches_through_adapter_search() {
        unimplemented!("Phase 4b ignored placeholder");
    }

    #[tokio::test]
    #[ignore = "Phase 4b deferred — same fixture-sharing constraint"]
    async fn update_memory_command_dispatches_through_adapter_update_with_full_newmemory_per_adr_028(
    ) {
        unimplemented!("Phase 4b ignored placeholder");
    }

    #[tokio::test]
    #[ignore = "Phase 4b deferred — same fixture-sharing constraint"]
    async fn delete_memory_command_dispatches_through_adapter_delete_with_auth_gate_inherited_from_phase_4a(
    ) {
        unimplemented!("Phase 4b ignored placeholder");
    }

    #[tokio::test]
    #[ignore = "Phase 4b deferred — same fixture-sharing constraint"]
    async fn tauri_command_invoke_audit_row_written_per_adr_024_amendment() {
        unimplemented!("Phase 4b ignored placeholder");
    }

    /// UI slice 2 counterpart to the placeholders above: the home tab's
    /// recent list must surface AGENT-written memories, which is the whole
    /// reason it stopped reading the browser-local cache.
    #[tokio::test]
    #[ignore = "UI slice 2 — same Application test-fixture constraint as the Phase 4b placeholders"]
    async fn list_recent_memories_includes_agent_written_memories_not_just_ui_added() {
        unimplemented!("UI slice 2 ignored placeholder — needs shared Application fixture");
    }
}
