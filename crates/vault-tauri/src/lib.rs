//! `vault-tauri` library — testable utility functions consumed by the
//! Tauri shell binary at `src/main.rs`.
//!
//! ## ADR-003 lib→bin conversion (T0.1.11 Phase 3)
//!
//! Per ADR-003, vault-tauri shipped as a library skeleton at T0.1.1 and
//! converts to a binary at T0.1.11. Phase 3 interpretation: ADR-003's
//! "converts to binary" reads as "add binary target," not "remove library
//! target." Keeping the library alongside the binary lets us unit-test
//! OS dispatch + keychain-failure formatting WITHOUT launching the Tauri
//! runtime — standard Rust pattern for testable apps.
//!
//! ## What lives here vs in main.rs
//!
//! - **lib.rs (this file):** pure functions that take inputs and return
//!   outputs — testable in isolation. ADR-019 OS-aware dylib filename
//!   dispatch, ADR-020 integrity-failure dialog text formatting,
//!   ADR-040 keychain-error dialog text formatting (T0.2.0 Phase 1).
//! - **main.rs:** Tauri Builder orchestration — sources master_key from
//!   keychain via `vault_app::keychain`, derives SqlCipher / at-rest
//!   subkeys, builds AppConfig, launches Application. Thin glue on top
//!   of these utilities.
//!
//! ## T0.2.0 Phase 1 retirement (2026-05-09)
//!
//! Per ADR-040 + ADR-040 amendment, the V0.1 `parse_vault_key` /
//! `ConfigError::VaultKey*` surface retired alongside the VAULT_KEY env
//! var. The single remaining `ConfigError` variant (`UnsupportedPlatform`)
//! is still used by `dylib_filename_for_os` for OS-dispatch error
//! reporting. New `format_keychain_error_dialog` formats the
//! `VaultError::KeychainProvenance` variant for fatal-dialog surfacing.

#![forbid(unsafe_code)]

pub mod commands;

/// First-run model acquisition, re-exported from `vault-app`.
///
/// The module itself moved down to `vault-app` (ADR-100) so the CLI can
/// acquire its own models when it is launched directly by an MCP client
/// from an `.mcpb` bundle, with no desktop app in the picture. This
/// re-export keeps every existing `vault_tauri::model_fetch::*` path
/// working unchanged.
pub use vault_app::model_fetch;

use std::path::PathBuf;

use thiserror::Error;
use vault_core::VaultError;

/// Configuration errors surfaced before the Tauri runtime starts.
///
/// Pre-Phase-1 this enum carried `VaultKeyUnset` + `VaultKeyEmpty`
/// variants for the V0.1 VAULT_KEY env-var path; both retired at T0.2.0
/// Phase 1 alongside the env var (per ADR-040 + ADR-040 amendment —
/// keychain provenance replaces env-var provenance). The remaining
/// variant is still used by `dylib_filename_for_os` for OS-dispatch
/// error reporting at the libonnxruntime resolution site.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Host OS is not one of the three supported V0.1 platforms (Linux /
    /// macOS / Windows per ADR-029 BRD amendment to "Mac or Windows" +
    /// `[ubuntu-latest, windows-latest, macos-latest]` CI matrix landed
    /// at T0.1.11 Phase 1).
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
}

/// Resolve the platform-specific filename for the bundled libonnxruntime
/// dylib per ADR-019 `load-dynamic` strategy.
///
/// Returns the relative path under `BaseDirectory::Resource` (Phase 5
/// installer-mode) or under the `VAULT_ORT_LIB_PATH` env var override
/// (Phase 3 dev-mode boot — main.rs reads this env var if set,
/// otherwise calls `app.path().resolve(BaseDirectory::Resource)`).
pub fn dylib_filename_for_os(os: &str) -> Result<&'static str, ConfigError> {
    match os {
        "windows" => Ok("libs/onnxruntime.dll"),
        "macos" => Ok("libs/libonnxruntime.dylib"),
        "linux" => Ok("libs/libonnxruntime.so"),
        other => Err(ConfigError::UnsupportedPlatform(other.to_string())),
    }
}

/// Resolve a dev-mode env-var override to a `PathBuf` if set + non-empty.
///
/// Used by the resource-resolution functions in `main.rs` so the founder
/// running `cargo run -p vault-tauri` can point at the test-fixture
/// dylib / model / tokenizer without needing an installer. Production
/// builds set no env var and fall through to `app.path().resolve(...,
/// BaseDirectory::Resource)`.
pub fn env_override_for(env_var_name: &str) -> Option<PathBuf> {
    match std::env::var(env_var_name) {
        Ok(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => None,
    }
}

/// Format a [`VaultError::KeychainProvenance`] as the fatal-dialog body
/// shown by `main.rs::show_fatal_dialog_and_exit` when keychain access
/// fails before Application::new is reached.
///
/// Per ADR-040 + ADR-040 amendment (T0.2.0 Phase 1, 2026-05-09):
/// keychain provenance replaces the V0.1 VAULT_KEY env-var provenance.
/// The dialog body explains the failure category + suggests the standard
/// recovery path (Credential Manager inspection on Windows, reinstall on
/// non-Windows where keychain is not yet supported in V0.2 Phase 1).
///
/// **The dialog text deliberately does NOT name ADR-040, and a test must not
/// require it to** (ADR-100). The reference belongs here, where a developer
/// tracing the behaviour will look, not in front of a user whose app has just
/// failed to start — "Per ADR-040 (T0.2.0 Phase 1)" tells them nothing they can
/// act on and reads as leaked internals at the worst possible moment. What the
/// dialog owes the user is the credential namespace to inspect, which
/// `format_keychain_error_dialog_for_keychain_provenance_variant` pins.
///
/// **Defensive fallback for non-`KeychainProvenance` variants:** the
/// function accepts any `&VaultError` so the call site can pass through
/// without prior pattern-matching, but only `KeychainProvenance` variants
/// produce a tailored message; other variants render via the generic
/// `format_startup_failure_dialog` path.
pub fn format_keychain_error_dialog(err: &VaultError) -> String {
    match err {
        VaultError::KeychainProvenance(msg) => format!(
            "Zaaheen cannot start: keychain access failed.\n\n\
             Details: {msg}\n\n\
             Zaaheen sources its master \
             encryption key from the OS keychain (Windows Credential Manager).\n\n\
             Recovery options:\n\
             1. On Windows, open Control Panel → User Accounts → Credential \
                Manager → Windows Credentials and inspect entries under \
                'com.zaaheen.v0.2'. If an entry exists with a corrupted \
                or unexpected secret, delete it and relaunch Zaaheen \
                (a new master_key will be generated on first run).\n\
             2. On macOS or Linux, note that Zaaheen currently stores its key \
                in the Windows Credential Manager only. Support for the macOS \
                and Linux keychains is coming.\n\
             3. Reinstall Zaaheen if the failure persists."
        ),
        other => format_startup_failure_dialog(other),
    }
}

/// Format a `VaultError` as a user-facing fatal-dialog message body for
/// ADR-020 integrity-failure surfacing.
///
/// Special-cases [`VaultError::ModelIntegrityFailed`] with reinstall
/// guidance per ADR-020's "Reinstall to recover" specification (HANDOFF
/// .md line 875 forward-pointer to T0.1.11). All other startup failures
/// get a generic "details: {err}" body with reinstall guidance — the
/// specific recovery procedure for non-integrity failures is
/// out-of-scope for V0.1 founder-only alpha (revisit at V0.2 alpha
/// cohort task when external-user error UX matters).
pub fn format_startup_failure_dialog(err: &VaultError) -> String {
    match err {
        VaultError::ModelIntegrityFailed {
            file,
            expected,
            actual,
        } => format!(
            "Zaaheen cannot start: model integrity check failed.\n\n\
             File: {file}\n\
             Expected SHA-256: {expected}\n\
             Actual SHA-256:   {actual}\n\n\
             Reinstall to recover."
        ),
        other => format!(
            "Zaaheen cannot start.\n\n\
             Details: {other}\n\n\
             Reinstall to recover."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // ADR-019 dylib path resolution (v4 floor item — OS dispatch test)
    // -----------------------------------------------------------------

    /// ADR-019 OS dispatch: each supported V0.1 platform per ADR-029
    /// BRD amendment maps to its canonical libonnxruntime filename.
    /// Unsupported platforms surface as ConfigError::UnsupportedPlatform.
    #[test]
    fn dylib_filename_dispatches_correctly_per_os() {
        // Three V0.1 first-class platforms per ADR-029 + Phase 1 CI
        // matrix [ubuntu-latest, windows-latest, macos-latest].
        assert_eq!(
            dylib_filename_for_os("windows").unwrap(),
            "libs/onnxruntime.dll",
            "Windows dylib filename must match scripts/setup-dev-env.ps1's \
             extracted onnxruntime.dll"
        );
        assert_eq!(
            dylib_filename_for_os("macos").unwrap(),
            "libs/libonnxruntime.dylib",
            "macOS dylib filename must match scripts/setup-dev-env.sh's \
             Darwin branch ORT_LIB_NAME"
        );
        assert_eq!(
            dylib_filename_for_os("linux").unwrap(),
            "libs/libonnxruntime.so",
            "Linux dylib filename must match scripts/setup-dev-env.sh's \
             Linux branch ORT_LIB_NAME"
        );

        // Unsupported platform → typed error, not panic.
        let err = dylib_filename_for_os("freebsd").unwrap_err();
        assert!(
            matches!(err, ConfigError::UnsupportedPlatform(ref s) if s == "freebsd"),
            "Unsupported OS MUST surface as ConfigError::UnsupportedPlatform \
             with the OS name preserved for diagnostics; got {err:?}"
        );
    }

    // -----------------------------------------------------------------
    // ADR-020 integrity-fatal-dialog wiring (v4 floor item)
    // -----------------------------------------------------------------

    /// ADR-020: ModelIntegrityFailed produces a dialog body that
    /// includes the file path + expected/actual SHA-256 + reinstall
    /// guidance. Pinning the format here prevents drift between the
    /// dialog text and what HANDOFF.md ADR-020 specified
    /// ("Reinstall to recover").
    #[test]
    fn format_startup_failure_dialog_includes_integrity_details() {
        let err = VaultError::ModelIntegrityFailed {
            file: "model.onnx".to_string(),
            expected: "abc123".to_string(),
            actual: "def456".to_string(),
        };
        let dialog = format_startup_failure_dialog(&err);

        assert!(
            dialog.contains("model integrity check failed"),
            "ADR-020 dialog body must announce integrity-check failure; got: {dialog}"
        );
        assert!(
            dialog.contains("model.onnx"),
            "ADR-020 dialog body must include the failing file path for \
             diagnostics; got: {dialog}"
        );
        assert!(
            dialog.contains("abc123") && dialog.contains("def456"),
            "ADR-020 dialog body must include both expected and actual \
             SHA-256 for verifying tampering vs. corruption; got: {dialog}"
        );
        assert!(
            dialog.contains("Reinstall"),
            "ADR-020 dialog body must include 'Reinstall' recovery \
             guidance per HANDOFF.md ADR-020 line 875 specification; \
             got: {dialog}"
        );
    }

    // -----------------------------------------------------------------
    // T0.2.0 Phase 1 — ADR-040 keychain-failure-dialog formatting
    // -----------------------------------------------------------------

    /// ADR-040: KeychainProvenance variant produces a dialog body that
    /// names the failure cause + cites ADR-040 + lists recovery steps
    /// (Credential Manager inspection on Windows; reinstall fallback).
    /// Pinning the body here prevents drift between the dialog text and
    /// what HANDOFF.md ADR-040 specified.
    #[test]
    fn format_keychain_error_dialog_for_keychain_provenance_variant() {
        let err = VaultError::KeychainProvenance(
            "Store::new failed: simulated keychain unavailable".to_string(),
        );
        let dialog = format_keychain_error_dialog(&err);

        assert!(
            dialog.contains("keychain access failed"),
            "KeychainProvenance dialog must announce the failure category; got: {dialog}"
        );
        assert!(
            dialog.contains("simulated keychain unavailable"),
            "KeychainProvenance dialog must propagate the underlying error \
             detail for diagnostics; got: {dialog}"
        );
        // ADR-100 REMOVED the old `dialog.contains("ADR-040")` assertion here.
        // It required an internal document number to appear in text a user
        // reads when their app will not start. The source-of-truth reference it
        // was protecting now lives in this function's doc comment, which is
        // where a developer tracing the behaviour actually looks. What the user
        // needs is asserted immediately below: the credential namespace.
        assert!(
            !dialog.contains("ADR-"),
            "user-facing dialog MUST NOT quote internal ADR numbers; got: {dialog}"
        );
        assert!(
            dialog.contains("Credential Manager"),
            "KeychainProvenance dialog must point Windows users at Credential \
             Manager for recovery; got: {dialog}"
        );
        assert!(
            dialog.contains("com.zaaheen.v0.2"),
            "KeychainProvenance dialog must include the production namespace \
             so users can find the entry in Credential Manager; got: {dialog}"
        );
        assert!(
            dialog.contains("Reinstall"),
            "KeychainProvenance dialog must include reinstall fallback for \
             unrecoverable failures; got: {dialog}"
        );
    }

    /// `format_keychain_error_dialog` falls through to
    /// `format_startup_failure_dialog` for non-`KeychainProvenance`
    /// variants. Pin the fall-through behaviour so future contributors
    /// don't accidentally narrow the surface.
    #[test]
    fn format_keychain_error_dialog_falls_through_for_non_keychain_variants() {
        let err = VaultError::ModelIntegrityFailed {
            file: "tokenizer.json".to_string(),
            expected: "expected123".to_string(),
            actual: "actual456".to_string(),
        };
        let dialog = format_keychain_error_dialog(&err);
        assert!(
            dialog.contains("model integrity check failed"),
            "Non-KeychainProvenance variants must fall through to \
             format_startup_failure_dialog; got: {dialog}"
        );
    }

    // -----------------------------------------------------------------
    // env_override_for tests (ADR-019 / Phase 4b extracted utility)
    // -----------------------------------------------------------------

    /// `env_override_for` returns Some(path) when env var is set + non-empty,
    /// None when unset OR set-but-empty. Tests use real env-var get since
    /// the function delegates to `std::env::var`; tests are structured to
    /// avoid the env-var-race issue (each test uses a unique var name).
    #[test]
    fn env_override_for_returns_some_when_var_set_and_none_when_unset_or_empty() {
        // Use unique env-var names per test to avoid races with parallel
        // test execution. These names are scoped to this test and not
        // used by production code.
        let unique_set = "VAULT_TAURI_TEST_ENV_OVERRIDE_SET_42";
        let unique_unset = "VAULT_TAURI_TEST_ENV_OVERRIDE_UNSET_42";
        let unique_empty = "VAULT_TAURI_TEST_ENV_OVERRIDE_EMPTY_42";

        // Ensure clean state.
        std::env::remove_var(unique_set);
        std::env::remove_var(unique_unset);
        std::env::remove_var(unique_empty);

        // Unset → None.
        assert_eq!(
            env_override_for(unique_unset),
            None,
            "env_override_for MUST return None when env var is unset"
        );

        // Empty → None (production path: empty value treated as no override
        // so we fall through to BaseDirectory::Resource).
        std::env::set_var(unique_empty, "");
        assert_eq!(
            env_override_for(unique_empty),
            None,
            "env_override_for MUST return None when env var is set-but-empty \
             (treats empty as no override)"
        );
        std::env::remove_var(unique_empty);

        // Set → Some(path).
        std::env::set_var(unique_set, "C:/test/path/libonnxruntime.dll");
        assert_eq!(
            env_override_for(unique_set),
            Some(PathBuf::from("C:/test/path/libonnxruntime.dll")),
            "env_override_for MUST return Some(PathBuf) when env var is set + non-empty"
        );
        std::env::remove_var(unique_set);
    }

    // -----------------------------------------------------------------
    // Phase 4b — ADR-030 negative regression test (source-grep)
    // -----------------------------------------------------------------

    /// **ADR-030 outcome shape (a) regression check.** vault-tauri MUST
    /// NOT expose a Tauri command that takes user-controlled input and
    /// passes it into `StdioServerParameters` or spawns external MCP
    /// servers. Adding such a command requires ADR-030 amendment first
    /// (V1.0 connectors task per ADR-026 forward-pointer).
    ///
    /// Mechanism per Shahbaz Phase 4b v2 review clarification 2:
    /// **Rust-side source-grep against main.rs via `include_str!`** —
    /// deterministic, no Tauri runtime needed, no false negatives from
    /// macro-generation paths (in V0.1 Tauri commands are listed
    /// directly in `main.rs`'s `.invoke_handler(tauri::generate_handler![
    /// commands::add_memory, ...])`; if a future contributor adds
    /// `commands::spawn_external_mcp_server` to that list, the source
    /// text contains the forbidden substring and this test catches it).
    #[test]
    fn main_rs_does_not_register_external_mcp_spawn_command_per_adr_030() {
        let main_rs = include_str!("main.rs");

        // Forbidden patterns are COMMAND-NAME-style identifiers — what
        // would appear in a `#[tauri::command] fn <name>` declaration
        // OR a `tauri::generate_handler![commands::<name>]` registration
        // line. The API type `StdioServerParameters` is deliberately
        // NOT in this list because main.rs's own doc comment references
        // the term in negative form (per ADR-030 outcome (a) cross-link).
        // Substring match against the API type name would false-positive
        // on the legitimate doc reference.
        let forbidden_substrings = [
            "spawn_external_mcp",
            "configure_mcp_server",
            "add_mcp_server",
            "external_mcp_server",
            "configure_external",
            "stdio_server_params",
        ];

        for substring in forbidden_substrings {
            assert!(
                !main_rs.contains(substring),
                "ADR-030 outcome (a) regression: main.rs contains '{}' which \
                 suggests external-MCP-server-spawn UI surface. Per ADR-030 \
                 amendment 2026-05-05 (T0.1.11 Phase 2): vault-tauri must \
                 spawn ONLY our own vault-mcp child via in-process stdio; \
                 user-controlled input MUST NOT flow into StdioServerParameters. \
                 Adding such a surface requires V1.0 connectors task per \
                 ADR-026 forward-pointer.",
                substring
            );
        }
    }
}
