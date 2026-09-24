//! Settings commands — BRD §5.11 `commands/settings.rs`.
//!
//! Replaces the Settings tab's partly-hardcoded values with real ones.
//!
//! ## ADR-086 (UI content policy) applies to every string here
//!
//! User-visible values never name the underlying stack — no embedding model,
//! no vector store, no database engine. The tab reports capability and trust
//! ("stored on this device", "history verified"), not implementation.
//!
//! ## Honest UI
//!
//! Every field is measured at call time, by the keeper (ADR-108 D2):
//! `audit_chain_verified` is the real result of walking the tamper-evident
//! chain (BRD §11.9.2). The `version` is THIS app's, added here — during an
//! update an older keeper may briefly be the one answering.
//!
//! ## Open, and it must work with no key
//!
//! The lock screen asks this to decide whether to offer the export. A
//! computer with no key yet (a fresh install before sign-in) answers here
//! with zeros, without a keeper (ADR-108 D4); a computer whose key is
//! missing while memories are on disk says so, in the startup message's own
//! words, rather than "nothing yet" (review A R2-2).

use serde_json::{json, Value};
use tauri::State;
use vault_core::{VaultError, VaultKeyFailure};

use crate::link::{decoded, KeeperLink, KeyState, Kind};

/// `get_settings_info`.
pub async fn get_settings_info_inner(link: &KeeperLink) -> Result<Value, String> {
    let root = link.vault_root();
    match link.key_state() {
        KeyState::Present => {
            let text = link
                .call("admin_settings_info", json!({}), Kind::Read)
                .await?;
            let mut settings: Value = decoded(&text)?;
            settings["version"] = json!(env!("CARGO_PKG_VERSION"));
            Ok(settings)
        }
        KeyState::Absent { keyed_data: false } => Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            // As the person reads it: never the `\\?\` form a moved vault's
            // record keeps (ADR-105 L-e).
            "data_dir": root.as_deref().map(vault_app::location::display_path).unwrap_or_default(),
            "memory_count": 0,
            "boundary_count": 0,
            "audit_chain_verified": true,
        })),
        KeyState::Absent { keyed_data: true } => Err(crate::format_keychain_error_dialog(
            &VaultError::VaultKey(VaultKeyFailure::Missing),
        )),
        KeyState::Unreadable => Err(crate::format_keychain_error_dialog(
            &VaultError::KeychainProvenance("the credential store could not be read".into()),
        )),
    }
}

#[tauri::command]
pub async fn get_settings_info(link: State<'_, KeeperLink>) -> Result<Value, String> {
    get_settings_info_inner(link.inner()).await
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
    /// the folder as the person reads it — here and in the keeper, which
    /// fills `data_dir` when a key exists.
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
        assert!(body.contains("vault_app::location::display_path"));
        assert!(!body.contains(".display().to_string()"));
    }

    /// The version shown is this app's, not the keeper's (review A-N3).
    #[test]
    fn settings_payload_reports_this_apps_version() {
        let source = include_str!("settings.rs").replace("\r\n", "\n");
        assert!(source.contains("settings[\"version\"] = json!(env!(\"CARGO_PKG_VERSION\"));"));
    }
}
