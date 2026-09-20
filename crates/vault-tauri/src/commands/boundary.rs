//! Boundary commands — BRD §5.11 `commands/boundary.rs`.
//!
//! Shared context (audit posture, ADR-SEC-003 boundary posture, testability
//! pattern) is documented on the parent module.
//!
//! Registering a boundary here grants nothing on its own. What a caller may
//! read is decided by the authorized-boundary slice passed to retrieval, never
//! by the presence of a registry row (BRD §11.4.3 rules 3 and 4).

use std::time::Instant;

use tauri::State;
use vault_app::Application;
use vault_core::Boundary;
use vault_mcp::ToolInvokeDetails;

use crate::guard::{Entitled, Entitlement};

/// Inner list_boundaries implementation.
pub async fn list_boundaries_inner(
    app: &Application,
    _entitled: &Entitled,
) -> Result<Vec<serde_json::Value>, String> {
    let adapter = app.adapter();
    let start = Instant::now();

    let result = adapter.list_boundaries().await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (count, error_for_audit) = match &result {
        Ok(boundaries) => (boundaries.len() as u32, None),
        Err(e) => (0, Some(vault_mcp::ToolInvokeError::from_vault_error(e))),
    };

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "list_boundaries",
            duration_ms,
            result_count: count,
            boundary_count: count,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        })
        .await;

    result
        .map(|boundaries| {
            boundaries
                .into_iter()
                .map(|b| {
                    serde_json::json!({
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

#[tauri::command]
pub async fn list_boundaries(
    state: State<'_, Application>,
    entitlement: State<'_, Entitlement>,
) -> Result<Vec<serde_json::Value>, String> {
    let entitled = entitlement.require().await?;
    list_boundaries_inner(state.inner(), &entitled).await
}

/// Inner create_boundary implementation.
///
/// Returns `true` when a new boundary was registered, `false` when one of that
/// name already existed — an idempotent no-op rather than an error, since the
/// caller's desired end state already holds.
pub async fn create_boundary_inner(
    app: &Application,
    _entitled: &Entitled,
    name: String,
    description: Option<String>,
) -> Result<bool, String> {
    let adapter = app.adapter();
    let start = Instant::now();

    // Validate BEFORE touching the vault (BRD §11.7.1 — validate at the
    // boundary). `Boundary::new` enforces the 64-byte cap and the
    // ASCII-identifier charset that keeps names safe to interpolate into the
    // LanceDB `only_if` filter downstream.
    let parsed = Boundary::new(&name).map_err(|e| format!("invalid boundary name: {e}"))?;

    let result = adapter
        .create_boundary(&parsed, description.as_deref())
        .await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (created, error_for_audit) = match &result {
        Ok(created) => (*created, None),
        Err(e) => (false, Some(vault_mcp::ToolInvokeError::from_vault_error(e))),
    };

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "create_boundary",
            duration_ms,
            result_count: u32::from(created),
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
pub async fn create_boundary(
    state: State<'_, Application>,
    entitlement: State<'_, Entitlement>,
    name: String,
    description: Option<String>,
) -> Result<bool, String> {
    let entitled = entitlement.require().await?;
    create_boundary_inner(state.inner(), &entitled, name, description).await
}

#[cfg(test)]
mod tests {
    use vault_core::Boundary;

    /// The command layer's first act is `Boundary::new`, so every name the
    /// type rejects is rejected before any vault I/O happens. These pin the
    /// cases that matter for a UI text field wired straight to this command.
    #[test]
    fn boundary_name_validation_rejects_injection_shaped_input() {
        for bad in [
            "",                 // empty
            "has space",        // whitespace
            "quote'breakout",   // SQL quote breakout (LanceDB only_if)
            "semi;colon",       // statement separator
            "nul\0byte",        // null byte
            "new\nline",        // control character
            "unicode·dot",      // non-ASCII
            "../../etc/passwd", // path traversal shape
        ] {
            assert!(
                Boundary::new(bad).is_err(),
                "boundary name {bad:?} must be rejected before reaching storage"
            );
        }
    }

    #[test]
    fn boundary_name_validation_rejects_over_length_names() {
        // BRD §11.7.1: boundary names cap at 64 bytes.
        let too_long = "a".repeat(65);
        assert!(
            Boundary::new(&too_long).is_err(),
            "a 65-byte boundary name must be rejected"
        );
        let at_limit = "a".repeat(64);
        assert!(
            Boundary::new(&at_limit).is_ok(),
            "a 64-byte boundary name is within the documented limit"
        );
    }

    #[test]
    fn boundary_name_validation_accepts_realistic_names() {
        for good in [
            "default",
            "work",
            "personal",
            "side-project",
            "client_acme",
            "q3",
        ] {
            assert!(
                Boundary::new(good).is_ok(),
                "boundary name {good:?} should be accepted"
            );
        }
    }
}
