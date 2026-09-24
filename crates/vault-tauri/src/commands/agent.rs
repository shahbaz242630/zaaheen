//! Agent registry commands — BRD §5.11 `commands/agent.rs`.
//!
//! Reads (and revokes against) the ADR-SEC-001 per-agent capability-token
//! registry, so the Agents tab shows the grants that actually exist rather
//! than the browser-local list of agents the user once clicked through in
//! onboarding. Since ADR-108 (D4) the registry is read by the keeper
//! (`vault_app::admin::ops`); these ask the guard and forward.
//!
//! **SP-1 / zero-knowledge:** a capability token's plaintext is never stored —
//! only its BLAKE3 hash — and `AgentToken` does not expose even the hash. These
//! commands therefore cannot leak a token, only the fact that one exists. The
//! keeper's serialization is an explicit allowlist per BRD §11.7.2.

use serde_json::{json, Value};
use tauri::State;

use crate::guard::{Entitled, Entitlement};
use crate::link::{decoded, KeeperLink, Kind};

/// Upper bound on an agent name, mirroring the boundary-name cap in
/// BRD §11.7.1 — the closest specified analogue for a short identifier.
pub const MAX_AGENT_NAME_LEN: usize = vault_app::admin::ops::MAX_AGENT_NAME_LEN;

/// `list_agents`: active AND revoked (revocation is soft).
pub async fn list_agents_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
) -> Result<Vec<Value>, String> {
    let text = link.call("admin_agent_list", json!({}), Kind::Read).await?;
    decoded(&text)
}

#[tauri::command]
pub async fn list_agents(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
) -> Result<Vec<Value>, String> {
    let entitled = entitlement.require().await?;
    list_agents_inner(link.inner(), &entitled).await
}

/// The AI apps connected right now (session 59): the keeper's list, read
/// from the vault folder (`vault_app::keeper::clients`). Empty when no keeper
/// is serving. Explicit allowlist serialisation (BRD §11.7.2): an app's
/// self-declared name and two times, nothing else. Reads a file, so it never
/// starts a keeper.
pub fn list_connected_apps_inner(link: &KeeperLink, _entitled: &Entitled) -> Vec<Value> {
    let Some(root) = link.vault_root() else {
        return Vec::new();
    };
    vault_app::keeper::clients::read_live(&root)
        .into_iter()
        .map(|a| {
            json!({
                "name": a.name,
                "since": a.since,
                "last_used": a.last_used,
            })
        })
        .collect()
}

#[tauri::command]
pub async fn list_connected_apps(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
) -> Result<Vec<Value>, String> {
    let entitled = entitlement.require().await?;
    Ok(list_connected_apps_inner(link.inner(), &entitled))
}

/// `revoke_agent`: `false` when no ACTIVE agent of that name existed.
/// Revocation takes effect on the agent's next request (ADR-SEC-001).
pub async fn revoke_agent_inner(
    link: &KeeperLink,
    _entitled: &Entitled,
    agent_name: String,
) -> Result<bool, String> {
    // Bounded here too, so a malformed call never reaches the keeper.
    if agent_name.is_empty() || agent_name.len() > MAX_AGENT_NAME_LEN {
        return Err("invalid agent name".to_string());
    }
    let text = link
        .call(
            "admin_agent_revoke",
            json!({ "agent_name": agent_name }),
            Kind::Write,
        )
        .await?;
    decoded(&text)
}

#[tauri::command]
pub async fn revoke_agent(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
    agent_name: String,
) -> Result<bool, String> {
    let entitled = entitlement.require().await?;
    revoke_agent_inner(link.inner(), &entitled, agent_name).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards the input bound: an empty or over-length name must be rejected
    /// by the command itself.
    fn name_is_within_bounds(name: &str) -> bool {
        !name.is_empty() && name.len() <= MAX_AGENT_NAME_LEN
    }

    #[test]
    fn agent_name_bounds_reject_empty_and_over_length() {
        assert!(!name_is_within_bounds(""), "empty name must be rejected");
        assert!(
            !name_is_within_bounds(&"a".repeat(MAX_AGENT_NAME_LEN + 1)),
            "over-length name must be rejected"
        );
    }

    #[test]
    fn agent_name_bounds_accept_realistic_names() {
        for good in [
            "claude",
            "cursor",
            "work-coder",
            &"a".repeat(MAX_AGENT_NAME_LEN),
        ] {
            assert!(
                name_is_within_bounds(good),
                "agent name {good:?} should be within bounds"
            );
        }
    }
}
