//! Recently deleted (ADR-114, `notes/RECENTLY-DELETED-DESIGN.md`).
//!
//! The four commands behind Memories › Recently deleted. Like every memory
//! command since ADR-108 (D4) the bodies run in the keeper
//! (`vault_app::admin::bin_ops`); each command here asks the desktop's guard,
//! then forwards over the admin connection. Gated: past the lock, like the
//! memories themselves.

use serde_json::{json, Value};
use tauri::State;

use crate::guard::{Entitled, Entitlement};
use crate::link::{decoded, KeeperLink, Kind};

/// `list_recently_deleted`: `{"total": n, "entries": [...]}`, newest first.
pub async fn list_recently_deleted_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
    limit: usize,
    offset: usize,
) -> Result<Value, String> {
    let text = link
        .call(
            "admin_bin_list",
            json!({ "limit": limit, "offset": offset }),
            Kind::Read,
        )
        .await?;
    decoded(&text)
}

#[tauri::command]
pub async fn list_recently_deleted(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
    limit: usize,
    offset: usize,
) -> Result<Value, String> {
    let entitled = entitlement.require().await?;
    list_recently_deleted_inner(link.inner(), &entitled, limit, offset).await
}

/// `restore_recently_deleted`: puts the entry back; `{"id": memory id}`.
pub async fn restore_recently_deleted_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
    entry_id: String,
) -> Result<Value, String> {
    let text = link
        .call(
            "admin_bin_restore",
            json!({ "entry_id": entry_id }),
            Kind::Write,
        )
        .await?;
    decoded(&text)
}

#[tauri::command]
pub async fn restore_recently_deleted(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
    entry_id: String,
) -> Result<Value, String> {
    let entitled = entitlement.require().await?;
    restore_recently_deleted_inner(link.inner(), &entitled, entry_id).await
}

/// `erase_recently_deleted` ("Delete now"): whether the entry was there.
pub async fn erase_recently_deleted_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
    entry_id: String,
) -> Result<bool, String> {
    let text = link
        .call(
            "admin_bin_erase",
            json!({ "entry_id": entry_id }),
            Kind::Write,
        )
        .await?;
    decoded(&text)
}

#[tauri::command]
pub async fn erase_recently_deleted(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
    entry_id: String,
) -> Result<bool, String> {
    let entitled = entitlement.require().await?;
    erase_recently_deleted_inner(link.inner(), &entitled, entry_id).await
}

/// `empty_recently_deleted`: how many entries were erased.
pub async fn empty_recently_deleted_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
) -> Result<usize, String> {
    let text = link.call("admin_bin_empty", json!({}), Kind::Write).await?;
    decoded(&text)
}

#[tauri::command]
pub async fn empty_recently_deleted(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
) -> Result<usize, String> {
    let entitled = entitlement.require().await?;
    empty_recently_deleted_inner(link.inner(), &entitled).await
}
