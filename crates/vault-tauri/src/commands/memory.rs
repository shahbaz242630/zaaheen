//! Memory CRUD commands — BRD §5.11 `commands/memory.rs`.
//!
//! Since ADR-108 (D4) the bodies run in the keeper (`vault_app::admin::ops`),
//! which is the only process that opens the vault; each command here asks
//! the desktop's guard, then forwards over the admin connection. The names,
//! arguments, answers and audit rows are exactly what they were.
//!
//! Shared context (audit posture, auth-gating, testability pattern) is
//! documented on the parent module.

use serde_json::{json, Value};
use tauri::State;

use crate::guard::{Entitled, Entitlement};
use crate::link::{decoded, KeeperLink, Kind};

/// Upper bound on `list_recent_memories`' page size (BRD §11.7.1); the
/// keeper clamps to it.
pub const MAX_RECENT_MEMORIES: usize = vault_app::admin::ops::MAX_RECENT_MEMORIES;

/// `add_memory`: the new memory's id.
pub async fn add_memory_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<String, String> {
    let text = link
        .call(
            "admin_memory_add",
            json!({ "content": content, "memory_type": memory_type, "boundary": boundary }),
            Kind::Write,
        )
        .await?;
    decoded(&text)
}

#[tauri::command]
pub async fn add_memory(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<String, String> {
    let entitled = entitlement.require().await?;
    add_memory_inner(link.inner(), &entitled, content, memory_type, boundary).await
}

/// `search_memories`: every registered boundary (ADR-SEC-003), at the
/// keeper's read desk; a newer search cancels this one.
pub async fn search_memories_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
    query: String,
    limit: usize,
) -> Result<Vec<Value>, String> {
    let text = link
        .call_search(
            "admin_memory_search",
            json!({ "query": query, "limit": limit }),
        )
        .await?;
    decoded(&text)
}

#[tauri::command]
pub async fn search_memories(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
    query: String,
    limit: usize,
) -> Result<Vec<Value>, String> {
    // Asked BEFORE anything reaches the keeper: a locked computer must not
    // reach the vault at all.
    let entitled = entitlement.require().await?;
    search_memories_inner(link.inner(), &entitled, query, limit).await
}

/// `update_memory`.
pub async fn update_memory_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
    id: String,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<(), String> {
    link.call(
        "admin_memory_update",
        json!({ "id": id, "content": content, "memory_type": memory_type, "boundary": boundary }),
        Kind::Write,
    )
    .await
    .map(|_| ())
}

#[tauri::command]
pub async fn update_memory(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
    id: String,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<(), String> {
    let entitled = entitlement.require().await?;
    update_memory_inner(link.inner(), &entitled, id, content, memory_type, boundary).await
}

/// `delete_memory`. The desktop acts as the owner (`ActorKind::User`).
pub async fn delete_memory_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
    id: String,
) -> Result<(), String> {
    link.call("admin_memory_delete", json!({ "id": id }), Kind::Write)
        .await
        .map(|_| ())
}

#[tauri::command]
pub async fn delete_memory(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
    id: String,
) -> Result<(), String> {
    let entitled = entitlement.require().await?;
    delete_memory_inner(link.inner(), &entitled, id).await
}

/// `list_recent_memories`, newest first across every boundary.
pub async fn list_recent_memories_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
    limit: usize,
) -> Result<Vec<Value>, String> {
    let text = link
        .call(
            "admin_memory_list_recent",
            json!({ "limit": limit }),
            Kind::Read,
        )
        .await?;
    decoded(&text)
}

#[tauri::command]
pub async fn list_recent_memories(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
    limit: usize,
) -> Result<Vec<Value>, String> {
    let entitled = entitlement.require().await?;
    list_recent_memories_inner(link.inner(), &entitled, limit).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recent_limit_is_the_keepers() {
        assert_eq!(MAX_RECENT_MEMORIES, 200);
    }

    #[test]
    fn an_answer_of_the_wrong_shape_is_a_stable_code() {
        assert_eq!(
            decoded::<String>("{not json").err().as_deref(),
            Some(crate::link::ERR_KEEPER_UNREACHABLE)
        );
        assert_eq!(decoded::<String>("\"abc\"").ok().as_deref(), Some("abc"));
    }
}
