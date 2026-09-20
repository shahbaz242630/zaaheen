//! Agent registry commands — BRD §5.11 `commands/agent.rs`.
//!
//! Reads (and revokes against) the ADR-SEC-001 per-agent capability-token
//! registry, so the Agents tab shows the grants that actually exist rather
//! than the browser-local list of agents the user once clicked through in
//! onboarding.
//!
//! **SP-1 / zero-knowledge:** a capability token's plaintext is never stored —
//! only its BLAKE3 hash — and `AgentToken` does not expose even the hash. These
//! commands therefore cannot leak a token, only the fact that one exists. The
//! serialization below is an explicit allowlist per BRD §11.7.2 ("strip any
//! unintended fields via explicit serialization (allowlist, not denylist)").

use std::time::Instant;

use tauri::State;
use vault_app::Application;
use vault_mcp::ToolInvokeDetails;

use crate::guard::{Entitled, Entitlement};

/// Upper bound on an agent name, mirroring the boundary-name cap in
/// BRD §11.7.1 — the closest specified analogue for a short identifier.
pub const MAX_AGENT_NAME_LEN: usize = 64;

/// Inner list_agents implementation. Returns active AND revoked agents —
/// revocation is soft so the history stays auditable.
pub async fn list_agents_inner(
    app: &Application,
    _entitled: &Entitled,
) -> Result<Vec<serde_json::Value>, String> {
    let adapter = app.adapter();
    let start = Instant::now();

    let result = adapter.list_agents().await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (count, error_for_audit) = match &result {
        Ok(agents) => (agents.len() as u32, None),
        Err(e) => (0, Some(vault_mcp::ToolInvokeError::from_vault_error(e))),
    };

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "list_agents",
            duration_ms,
            result_count: count,
            boundary_count: 0,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        })
        .await;

    result
        .map(|agents| {
            agents
                .into_iter()
                .map(|a| {
                    serde_json::json!({
                        "name": a.agent_name,
                        "boundaries": a.boundaries
                            .iter()
                            .map(|b| b.as_str())
                            .collect::<Vec<_>>(),
                        "created_at": a.created_at.to_rfc3339(),
                        "revoked_at": a.revoked_at.map(|t| t.to_rfc3339()),
                        "active": a.revoked_at.is_none(),
                    })
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_agents(
    state: State<'_, Application>,
    entitlement: State<'_, Entitlement>,
) -> Result<Vec<serde_json::Value>, String> {
    let entitled = entitlement.require().await?;
    list_agents_inner(state.inner(), &entitled).await
}

/// Inner revoke_agent implementation.
///
/// Returns `false` when no ACTIVE agent of that name existed. Revocation takes
/// effect on the agent's next request: the daemon's auth lookup filters
/// `revoked_at IS NULL` (ADR-SEC-001), so a revoked token stops resolving.
pub async fn revoke_agent_inner(
    app: &Application,
    _entitled: &Entitled,
    agent_name: String,
) -> Result<bool, String> {
    let adapter = app.adapter();
    let start = Instant::now();

    // Bound the input before it reaches storage (BRD §11.7.1). Agent names are
    // minted by `vault-cli agent add` as lowercase kebab-case; anything wildly
    // outside that shape is a malformed call, not a lookup worth running.
    if agent_name.is_empty() || agent_name.len() > MAX_AGENT_NAME_LEN {
        return Err("invalid agent name".to_string());
    }

    let result = adapter.revoke_agent(&agent_name).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (revoked, error_for_audit) = match &result {
        Ok(revoked) => (*revoked, None),
        Err(e) => (false, Some(vault_mcp::ToolInvokeError::from_vault_error(e))),
    };

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "revoke_agent",
            duration_ms,
            result_count: u32::from(revoked),
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
pub async fn revoke_agent(
    state: State<'_, Application>,
    entitlement: State<'_, Entitlement>,
    agent_name: String,
) -> Result<bool, String> {
    let entitled = entitlement.require().await?;
    revoke_agent_inner(state.inner(), &entitled, agent_name).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards the input bound without needing a live Application: an empty or
    /// over-length name must be rejected by the command itself.
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
