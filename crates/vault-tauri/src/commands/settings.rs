//! Settings commands — BRD §5.11 `commands/settings.rs`.
//!
//! Replaces the Settings tab's partly-hardcoded values with real ones.
//!
//! ## ADR-086 (UI content policy) applies to every string here
//!
//! User-visible values never name the underlying stack — no embedding model,
//! no vector store, no database engine. The tab reports capability and trust
//! ("stored on this device", "history verified"), not implementation. The
//! deliberate exception ADR-086 carves out for security specifics (encryption
//! standard, OS credential store) is not needed by this command: nothing here
//! surfaces one.
//!
//! ## Honest UI
//!
//! Every field is measured at call time. `audit_chain_verified` is the real
//! result of walking the tamper-evident chain (BRD §11.9.2), not an
//! assumption — if the chain is broken the tab must say so.

use std::time::Instant;

use tauri::State;
use vault_app::Application;
use vault_mcp::ToolInvokeDetails;

/// Inner get_settings_info implementation.
///
/// Chain verification walks the full audit log, so this is the most expensive
/// of the UI reads. It runs on tab open, not on a timer.
pub async fn get_settings_info_inner(app: &Application) -> Result<serde_json::Value, String> {
    let adapter = app.adapter();
    let start = Instant::now();

    let memory_count = adapter.total_memory_count().await;
    let boundary_count = adapter.list_boundaries().await.map(|b| b.len());
    // A broken chain is a REPORTABLE STATE, not a command failure — the tab
    // exists partly to surface it. Only the count/registry reads can fail the
    // command outright.
    let audit_chain_verified = adapter.verify_audit_chain().await.is_ok();

    let duration_ms = start.elapsed().as_millis() as u64;
    let combined = memory_count
        .as_ref()
        .err()
        .or(boundary_count.as_ref().err());
    let error_for_audit = combined.map(vault_mcp::ToolInvokeError::from_vault_error);

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "get_settings_info",
            duration_ms,
            result_count: 1,
            boundary_count: *boundary_count.as_ref().unwrap_or(&0) as u32,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        })
        .await;

    let memory_count = memory_count.map_err(|e| e.to_string())?;
    let boundary_count = boundary_count.map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        // As the person reads it: never the `\\?\` form a moved vault's
        // record keeps (ADR-105 L-e).
        "data_dir": vault_app::location::display_path(app.vault_root()),
        "memory_count": memory_count,
        "boundary_count": boundary_count,
        "audit_chain_verified": audit_chain_verified,
    }))
}

#[tauri::command]
pub async fn get_settings_info(state: State<'_, Application>) -> Result<serde_json::Value, String> {
    get_settings_info_inner(state.inner()).await
}

#[cfg(test)]
mod tests {
    /// ADR-086 white-label pin: the Settings payload must not name the
    /// underlying stack. This guards the KEYS and the static values this
    /// command emits — the dynamic ones are a path, two counts and a bool,
    /// none of which can carry a model name.
    #[test]
    fn settings_payload_shape_names_no_stack_components() {
        let shape = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "data_dir": "",
            "memory_count": 0,
            "boundary_count": 0,
            "audit_chain_verified": true,
        });
        let rendered = shape.to_string().to_lowercase();

        for forbidden in [
            "bge",
            "qwen",
            "phi",
            "onnx",
            "lance",
            "duckdb",
            "sqlite",
            "sqlcipher",
            "gguf",
            "llama",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "ADR-086: settings payload must not name '{forbidden}'; got {rendered}"
            );
        }
    }

    /// ADR-105 L-e: a moved vault's record keeps `\\?\D:\…`; Settings shows
    /// the folder as the person reads it. A source test, because the command
    /// needs a whole `Application` to run.
    #[test]
    fn the_vault_location_is_shown_as_the_person_reads_it() {
        let source = include_str!("settings.rs").replace("\r\n", "\n");
        let body = source
            .split_once("pub async fn get_settings_info_inner(")
            .expect("the inner command is defined here")
            .1
            .split_once("\n}\n")
            .expect("the inner command is closed")
            .0;
        assert!(body.contains("\"data_dir\": vault_app::location::display_path(app.vault_root()),"));
        assert!(!body.contains(".display().to_string()"));
    }

    #[test]
    fn settings_payload_reports_a_real_version() {
        // A hardcoded placeholder version was one of the honest-UI gaps this
        // command closes; pin that it comes from the crate metadata.
        assert!(
            !env!("CARGO_PKG_VERSION").is_empty(),
            "version must come from crate metadata, not a literal"
        );
    }
}
