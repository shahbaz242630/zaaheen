//! The desktop's Recently deleted screen (ADR-114,
//! `notes/RECENTLY-DELETED-DESIGN.md`): the keeper's side of the four
//! commands. Owner only: these are admin tools, never MCP tools, so an AI app
//! cannot see, restore or empty the bin.
//!
//! Each call records one `ui.tauri_command_invoke` audit row, as every
//! desktop command does; the storage layer records what changed.

use std::time::Instant;

use serde_json::{json, Value};
use vault_mcp::ToolInvokeError;
use vault_storage::{BinBy, BinKind};

use super::ops::row;
use crate::VaultAdapter;

/// Upper bound on one page of the Recently deleted list (BRD §11.7.1).
pub const MAX_RECENTLY_DELETED_PAGE: usize = 200;

/// `list_recently_deleted`: `{"total": n, "entries": [...]}`, newest first.
pub async fn bin_list(
    adapter: &VaultAdapter,
    limit: usize,
    offset: usize,
) -> Result<Value, String> {
    let start = Instant::now();
    let limit = limit.clamp(1, MAX_RECENTLY_DELETED_PAGE);
    // Never show an entry past its 30 days, however long the keeper has run.
    if let Err(e) = adapter.purge_recently_deleted().await {
        tracing::warn!(target: "vault_app::admin", error = %e, "recently deleted: purge before list failed");
    }
    let result = async {
        let entries = adapter.list_recently_deleted(limit, offset).await?;
        let total = adapter.count_recently_deleted().await?;
        Ok::<_, vault_core::VaultError>((entries, total))
    }
    .await;
    let count = result.as_ref().map_or(0, |(e, _)| e.len() as u32);
    let mut details = row("list_recently_deleted", start, count, 0);
    details.max_results = Some(limit as u32);
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;

    let (entries, total) = result.map_err(|e| e.to_string())?;
    Ok(json!({
        "total": total,
        "entries": entries
            .into_iter()
            .map(|e| json!({
                "entry_id": e.entry_id,
                "kind": match e.kind {
                    BinKind::Deleted => "deleted",
                    BinKind::Replaced => "replaced",
                },
                "by": match e.by {
                    BinBy::User => "you",
                    BinBy::App => "app",
                },
                "at": e.at.to_rfc3339(),
                "memory_id": e.memory.id.to_string(),
                "content": e.memory.content,
                "memory_type": format!("{:?}", e.memory.memory_type).to_lowercase(),
                "boundary": e.memory.boundary.as_str(),
            }))
            .collect::<Vec<_>>(),
    }))
}

/// `restore_recently_deleted`: the restored memory's id.
pub async fn bin_restore(adapter: &VaultAdapter, entry_id: String) -> Result<Value, String> {
    let start = Instant::now();
    let result = adapter.restore_recently_deleted(&entry_id).await;
    let mut details = row(
        "restore_recently_deleted",
        start,
        u32::from(result.is_ok()),
        1,
    );
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;
    result
        .map(|id| json!({ "id": id.to_string() }))
        .map_err(|e| e.to_string())
}

/// `erase_recently_deleted` ("Delete now"): whether the entry was there.
pub async fn bin_erase(adapter: &VaultAdapter, entry_id: String) -> Result<bool, String> {
    let start = Instant::now();
    let result = adapter.erase_recently_deleted(&entry_id).await;
    let mut details = row(
        "erase_recently_deleted",
        start,
        u32::from(matches!(result, Ok(true))),
        0,
    );
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;
    result.map_err(|e| e.to_string())
}

/// `empty_recently_deleted`: how many entries were erased.
pub async fn bin_empty(adapter: &VaultAdapter) -> Result<usize, String> {
    let start = Instant::now();
    let result = adapter.empty_recently_deleted().await;
    let count = result.as_ref().map_or(0, |n| *n as u32);
    let mut details = row("empty_recently_deleted", start, count, 0);
    details.error = result.as_ref().err().map(ToolInvokeError::from_vault_error);
    let _ = adapter.append_tauri_command_audit(details).await;
    result.map_err(|e| e.to_string())
}
