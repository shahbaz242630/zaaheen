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

/// The entitlement guard: the token every gated command must hold, and the
/// one door that hands it out (`SIGNIN-DESIGN.md` §8.26 §6.4).
pub mod guard;

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
use vault_core::{VaultError, VaultKeyFailure};

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

/// The support address on every key-failure message (founder, session 51).
pub const SUPPORT_EMAIL: &str = "customerservice@zaaheen.com";

/// Title of the fatal dialog when the vault key cannot be opened.
pub const KEY_ERROR_DIALOG_TITLE: &str = "Zaaheen can't open your memories";

/// Shown when another Zaaheen window is moving or deleting the memories
/// (ADR-105 L5): this window must not open them meanwhile. Founder-approved
/// with the location screens (session 54).
pub const MSG_MEMORIES_BUSY: &str = "Another Zaaheen window is busy with your memories \
     right now.\n\nWait a minute, then open Zaaheen again.";

/// The missing-location dialog's button that offers ADR-105 L6, and the
/// one that closes the app instead.
pub const START_AGAIN_BUTTON: &str = "Start again";
pub const CLOSE_BUTTON: &str = "Close";

/// L6's confirmation, shown before anything changes. The locked text asks
/// the person to confirm "The memories on that drive can't be opened from
/// here".
pub const START_AGAIN_TITLE: &str = "Start again with no memories?";
pub const START_AGAIN_CONFIRMATION: &str = "The memories on that drive can't be opened from \
     here. Zaaheen will keep new memories in its usual folder on this computer.\n\n\
     If you find the drive later, keep it, and write to customerservice@zaaheen.com so we \
     can help.";
pub const CANCEL_BUTTON: &str = "Cancel";

/// Why the memories cannot be found where the record says (ADR-105 L1), as
/// the startup dialog says it. Only a folder that is not there at all is
/// offered "Start again" (L6); one that is there may hold the memories, so
/// it gets help instead. The folder is shown as the person reads it.
pub fn format_location_problem_dialog(problem: &vault_app::location::missing::Problem) -> String {
    use vault_app::location::display_path;
    use vault_app::location::missing::Problem;
    match problem {
        Problem::FolderAbsent(folder) => format!(
            "Zaaheen keeps your memories in:\n{}\n\n\
             That folder isn't there right now. If it's on a drive that isn't plugged in, \
             plug it in and open Zaaheen again. Please don't delete anything.\n\n\
             If that drive is lost for good, you can start again with no memories.",
            display_path(folder)
        ),
        Problem::FolderUnusable(folder) => format!(
            "Zaaheen keeps your memories in:\n{}\n\n\
             The folder there isn't the one Zaaheen expects. If you use more than one drive, \
             plug in the one you chose for Zaaheen and open it again. Please don't delete \
             anything.\n\n\
             For help, write to {SUPPORT_EMAIL}.",
            display_path(folder)
        ),
        Problem::RecordUnreadable => format!(
            "Zaaheen can't read where your memories are kept.\n\n\
             Please don't delete anything. Write to {SUPPORT_EMAIL} so we can help."
        ),
    }
}

/// Why "Start again" did not happen (nothing changed in any case).
pub fn format_start_again_refusal(
    refusal: vault_app::location::missing::StartAgainRefusal,
) -> String {
    use vault_app::location::missing::StartAgainRefusal;
    match refusal {
        StartAgainRefusal::NotOffered => {
            "The folder with your memories is back. Open Zaaheen again to use them.".to_owned()
        }
        StartAgainRefusal::DefaultFolderHoldsMemories => format!(
            "Zaaheen can't start again in its usual folder, because there are memories in it \
             already. Please don't delete anything. Write to {SUPPORT_EMAIL} so we can help."
        ),
        StartAgainRefusal::Busy => "Zaaheen is finishing another task with your memories, \
             such as deleting them. Wait a minute, then open Zaaheen again."
            .to_owned(),
        StartAgainRefusal::KeyUnchecked | StartAgainRefusal::WriteFailed => format!(
            "Zaaheen couldn't start again just now, and nothing was changed.\n\n\
             Restart your computer and open Zaaheen again. If this keeps happening, write to \
             {SUPPORT_EMAIL}."
        ),
    }
}

/// The fatal-dialog body shown by `main.rs::show_fatal_dialog_and_exit` when
/// the vault key cannot be opened, before `Application::new` is reached.
///
/// **ADR-SEC-029 U1 (`VAULT-KEY-AND-LOCATION.md`).** One message per cause,
/// each saying only what the person can do:
/// - [`VaultKeyFailure::Missing`] — the key is gone but the memories are
///   not: don't delete them; restart, or go back to the computer and Windows
///   account used before;
/// - [`VaultError::KeychainProvenance`] — Windows' password store is failing:
///   restart;
/// - [`VaultKeyFailure::FolderUnavailable`] — the folder can't be reached or
///   cleared: plug the drive in, close the program using it;
/// - [`VaultKeyFailure::Unusable`] — a stored key this app could not have
///   written: delete nothing, write to support;
/// - [`VaultKeyFailure::Busy`] — another task (a deletion) holds the key:
///   wait a minute.
///
/// **It never tells anyone to delete the key.** The text this replaces did
/// ("delete it and relaunch Zaaheen (a new master_key will be generated on
/// first run)"), and following it would have made every memory permanently
/// unreadable. It also shows no internal names (ADR-100) and no error
/// detail (BRD §11.7.2): the caller logs the detail.
///
/// Other variants fall through to [`format_startup_failure_dialog`].
pub fn format_keychain_error_dialog(err: &VaultError) -> String {
    match err {
        VaultError::VaultKey(VaultKeyFailure::Missing) => format!(
            "Zaaheen can't open your memories. The key that unlocks them is missing \
             from this computer's Windows account.\n\n\
             Your memories are still on this computer. Please don't delete them.\n\n\
             If you haven't changed computer or Windows account, restart your computer \
             and open Zaaheen again. If you have, go back to the computer and Windows \
             account you used before.\n\n\
             For help, write to {SUPPORT_EMAIL}."
        ),
        VaultError::KeychainProvenance(_) => format!(
            "Zaaheen can't reach Windows' secure password store right now, so it can't \
             unlock your memories.\n\n\
             Restart your computer and open Zaaheen again. Your memories are still on \
             this computer. Please don't delete them.\n\n\
             If this keeps happening, write to {SUPPORT_EMAIL}."
        ),
        VaultError::VaultKey(VaultKeyFailure::FolderUnavailable) => format!(
            "Zaaheen can't reach the folder where your memories are kept.\n\n\
             If they're on a drive that isn't plugged in, plug it in and open Zaaheen \
             again. If another program is using the folder, close it and try again.\n\n\
             For help, write to {SUPPORT_EMAIL}."
        ),
        VaultError::VaultKey(VaultKeyFailure::Unusable) => format!(
            "Zaaheen found the key that unlocks your memories, but it can't use it.\n\n\
             Please don't delete anything: your memories are still on this computer. \
             Write to {SUPPORT_EMAIL} so we can help."
        ),
        VaultError::VaultKey(VaultKeyFailure::Busy) => {
            "Zaaheen is finishing another task with your memories, such as \
             deleting them. Wait a minute, then open Zaaheen again."
                .to_owned()
        }
        // ADR-105 L1: the recorded folder is missing or is not this vault,
        // when the reason could not be told (`main.rs` tells the reasons
        // apart with `format_location_problem_dialog`, and offers "Start
        // again" only there).
        VaultError::VaultLocation(_) => format!(
            "Zaaheen can't find your memories where they're kept.\n\n\
             If they're on a drive that isn't plugged in, plug it in and open \
             Zaaheen again. Please don't delete anything.\n\n\
             For help, write to {SUPPORT_EMAIL}."
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

    /// Every key failure, with the text that must be in its message.
    fn key_failures() -> Vec<(VaultError, &'static str)> {
        vec![
            (
                VaultError::VaultKey(VaultKeyFailure::Missing),
                "The key that unlocks them is missing",
            ),
            (
                VaultError::KeychainProvenance(
                    "Store::new failed: simulated keychain unavailable".into(),
                ),
                "secure password store",
            ),
            (
                VaultError::VaultKey(VaultKeyFailure::FolderUnavailable),
                "can't reach the folder",
            ),
            (
                VaultError::VaultKey(VaultKeyFailure::Unusable),
                "can't use it",
            ),
            (VaultError::VaultKey(VaultKeyFailure::Busy), "Wait a minute"),
            (
                VaultError::VaultLocation(vault_core::VaultLocationFailure::Missing),
                "can't find your memories where they're kept",
            ),
        ]
    }

    /// ADR-SEC-029 U1: each cause gets its own message, routed by the error.
    #[test]
    fn each_key_failure_gets_its_own_message() {
        let mut seen = std::collections::HashSet::new();
        for (err, says) in key_failures() {
            let dialog = format_keychain_error_dialog(&err);
            assert!(dialog.contains(says), "{err}: {dialog}");
            assert!(seen.insert(dialog), "{err} shares another cause's message");
        }
    }

    /// ADR-SEC-029 U1: the old text told people to delete the key, which
    /// would have destroyed every memory. No message may ever again.
    #[test]
    fn no_key_failure_message_advises_deleting_the_key() {
        for (err, _) in key_failures() {
            let dialog = format_keychain_error_dialog(&err).to_lowercase();
            for bad in [
                "delete it",
                "delete the key",
                "delete the entry",
                "delete your key",
                "remove the key",
                "new master_key",
                "will be generated",
                "credential manager",
                "reinstall",
            ] {
                assert!(!dialog.contains(bad), "{err}: says {bad:?}: {dialog}");
            }
        }
    }

    /// No internal names, no error detail, the support address where help
    /// is offered, and "don't delete" wherever memories are at stake.
    #[test]
    fn key_failure_messages_show_no_internals() {
        for (err, _) in key_failures() {
            let dialog = format_keychain_error_dialog(&err);
            for internal in [
                "ADR-",
                "com.zaaheen",
                "master_key",
                "keychain",
                "Store::new",
                "simulated keychain unavailable",
                "VAULT_KEY",
            ] {
                assert!(
                    !dialog.contains(internal),
                    "{err}: shows {internal:?}: {dialog}"
                );
            }
            if !matches!(err, VaultError::VaultKey(VaultKeyFailure::Busy)) {
                assert!(dialog.contains(SUPPORT_EMAIL), "{err}: {dialog}");
            }
        }
        for err in [
            VaultError::VaultKey(VaultKeyFailure::Missing),
            VaultError::KeychainProvenance("x".into()),
        ] {
            assert!(
                format_keychain_error_dialog(&err).contains("Please don't delete them"),
                "{err}"
            );
        }
        assert!(
            format_keychain_error_dialog(&VaultError::VaultKey(VaultKeyFailure::Unusable))
                .contains("Please don't delete anything")
        );
        assert_eq!(KEY_ERROR_DIALOG_TITLE, "Zaaheen can't open your memories");
    }

    /// ADR-105 L5: a window started while another moves or deletes the
    /// memories is told only what to do.
    #[test]
    fn the_busy_message_says_what_to_do_and_shows_no_internals() {
        assert!(MSG_MEMORIES_BUSY.contains("Wait a minute"));
        let lower = MSG_MEMORIES_BUSY.to_lowercase();
        for internal in ["adr-", "com.zaaheen", "vault", "lock", "keeper", "intent"] {
            assert!(!lower.contains(internal), "shows {internal:?}");
        }
    }

    // -----------------------------------------------------------------
    // ADR-105 L1/L6 — the memories are not where the record says
    // -----------------------------------------------------------------

    fn problems() -> Vec<vault_app::location::missing::Problem> {
        use vault_app::location::missing::Problem;
        vec![
            Problem::FolderAbsent(PathBuf::from(r"\\?\E:\Zaaheen Memories")),
            Problem::FolderUnusable(PathBuf::from(r"\\?\E:\Zaaheen Memories")),
            Problem::RecordUnreadable,
        ]
    }

    /// Each reason its own words; the folder as the person reads it.
    #[test]
    fn each_location_problem_gets_its_own_message() {
        let mut seen = std::collections::HashSet::new();
        for problem in problems() {
            let dialog = format_location_problem_dialog(&problem);
            assert!(seen.insert(dialog.clone()), "{problem:?} shares a message");
            assert!(!dialog.contains(r"\\?\"), "{problem:?}: {dialog}");
            if !matches!(
                problem,
                vault_app::location::missing::Problem::RecordUnreadable
            ) {
                assert!(dialog.contains(r"E:\Zaaheen Memories"), "{dialog}");
            }
            assert!(dialog.contains("Please don't delete anything"), "{dialog}");
        }
    }

    /// L6 is offered only when the folder itself is not there: one that is
    /// there may hold the memories.
    #[test]
    fn only_a_folder_that_is_not_there_is_offered_a_fresh_start() {
        for problem in problems() {
            let offers = format_location_problem_dialog(&problem)
                .to_lowercase()
                .contains("start again");
            let absent = matches!(
                problem,
                vault_app::location::missing::Problem::FolderAbsent(_)
            );
            assert_eq!(offers, absent, "{problem:?}");
        }
        assert!(START_AGAIN_CONFIRMATION
            .contains("The memories on that drive can't be opened from here"));
        assert!(START_AGAIN_CONFIRMATION.contains(SUPPORT_EMAIL));
    }

    /// Same rules as the key messages: no internals, no advice to delete,
    /// and help where help is offered.
    #[test]
    fn location_messages_show_no_internals_and_never_advise_deleting() {
        use vault_app::location::missing::StartAgainRefusal;
        let mut texts: Vec<String> = problems()
            .iter()
            .map(format_location_problem_dialog)
            .collect();
        texts.extend(
            [
                StartAgainRefusal::NotOffered,
                StartAgainRefusal::DefaultFolderHoldsMemories,
                StartAgainRefusal::KeyUnchecked,
                StartAgainRefusal::Busy,
                StartAgainRefusal::WriteFailed,
            ]
            .map(format_start_again_refusal),
        );
        texts.push(START_AGAIN_CONFIRMATION.to_owned());
        texts.push(MSG_MEMORIES_BUSY.to_owned());
        for text in &texts {
            let lower = text.to_lowercase();
            for internal in [
                "adr-",
                "com.zaaheen",
                "vault-id",
                ".json",
                "pointer",
                "marker",
                "keychain",
                "credential",
                "details:",
            ] {
                assert!(!lower.contains(internal), "shows {internal:?}: {text}");
            }
            for bad in [
                "delete it",
                "delete the folder",
                "delete the key",
                "reinstall",
            ] {
                assert!(!lower.contains(bad), "says {bad:?}: {text}");
            }
        }
    }

    /// The startup words, founder-approved 2026-09-22 (session 54: *"yes all
    /// good"*). Pinned so an edit is a decision, not a drift.
    #[test]
    fn the_location_startup_words_are_the_founders() {
        use vault_app::location::missing::Problem;
        let absent = format_location_problem_dialog(&Problem::FolderAbsent(PathBuf::from(
            r"\\?\E:\Zaaheen Memories",
        )));
        assert_eq!(
            absent,
            "Zaaheen keeps your memories in:\nE:\\Zaaheen Memories\n\n\
             That folder isn't there right now. If it's on a drive that isn't plugged in, \
             plug it in and open Zaaheen again. Please don't delete anything.\n\n\
             If that drive is lost for good, you can start again with no memories."
        );
        assert_eq!(START_AGAIN_BUTTON, "Start again");
        assert_eq!(CLOSE_BUTTON, "Close");
        assert_eq!(START_AGAIN_TITLE, "Start again with no memories?");
        assert_eq!(
            START_AGAIN_CONFIRMATION,
            "The memories on that drive can't be opened from here. Zaaheen will keep new \
             memories in its usual folder on this computer.\n\n\
             If you find the drive later, keep it, and write to customerservice@zaaheen.com \
             so we can help."
        );
        assert_eq!(
            MSG_MEMORIES_BUSY,
            "Another Zaaheen window is busy with your memories right now.\n\n\
             Wait a minute, then open Zaaheen again."
        );
    }

    /// ADR-105 L6 in the desktop: offered only for a folder that is not
    /// there, only after the person confirms, and before the key opens.
    #[test]
    fn the_desktop_starts_again_only_when_offered_and_confirmed() {
        let main = include_str!("main.rs").replace("\r\n", "\n");
        let body = main
            .split_once("fn when_the_memories_are_missing(")
            .expect("main.rs handles missing memories in one place")
            .1
            .split_once("\n}\n")
            .expect("that function is closed")
            .0;
        let at = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("when_the_memories_are_missing has no {needle}"))
        };
        let absent = at("Problem::FolderAbsent(");
        let offered = at("START_AGAIN_BUTTON");
        let declined = at("if !wants_to {\n        std::process::exit(EXIT_STARTUP_FAILURE);");
        let confirmed = at("START_AGAIN_CONFIRMATION");
        let cancelled = at("if !confirmed {\n        std::process::exit(EXIT_STARTUP_FAILURE);");
        let started = at("missing::start_again(");
        assert!(
            absent < offered
                && offered < declined
                && declined < confirmed
                && confirmed < cancelled
                && cancelled < started,
            "offered, then asked, and Close or Cancel leaves before anything changes"
        );
        assert_eq!(body.matches("start_again(").count(), 1);

        let prepared = main.find("location::prepare(").expect("prepare");
        let handled = main
            .find("when_the_memories_are_missing(app,")
            .expect("prepare's failure goes through it");
        let key = main
            .find("bridge_or_init_master_key(&key_location")
            .expect("the key opens");
        assert!(prepared < handled && handled < key);
    }

    /// ADR-105 L5 and amendment 1 (L-f): a waiting move is finished before
    /// anything opens the vault or the key, on both paths through the start
    /// (the setup thread, and the thread beside the window); a window told
    /// "busy" reaches neither; and the page is told the start is ready only
    /// after every piece of state is managed.
    #[test]
    fn the_desktop_finishes_a_move_before_it_opens_the_vault() {
        let main = include_str!("main.rs").replace("\r\n", "\n");
        let body_of = |name: &str| -> String {
            main.split_once(&format!("\nfn {name}("))
                .unwrap_or_else(|| panic!("main.rs has no fn {name}"))
                .1
                .split_once("\n}\n")
                .unwrap_or_else(|| panic!("fn {name} is not closed"))
                .0
                .to_owned()
        };
        let find = |text: &str, needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("no {needle} where it belongs"))
        };

        // The move, and "busy", before anything else.
        let finish = body_of("finish_the_move");
        let moved = find(&finish, "location::moving::run_pending_reporting(");
        let busy = find(&finish, "MoveOutcome::Busy");
        assert!(moved < busy);
        assert!(finish[busy..].contains("show_fatal_dialog_and_exit("));
        assert!(finish[busy..].contains("MSG_MEMORIES_BUSY"));

        // Everything after the move, in order, with "ready" last.
        let open = body_of("open_the_vault");
        let prepared = find(&open, "location::prepare(");
        let key = find(&open, "bridge_or_init_master_key(&key_location");
        let opened = find(&open, "Application::new(&config)");
        let guarded = find(&open, "guard::build()");
        let ready = find(&open, "startup.ready();");
        assert!(prepared < key && key < opened && opened < guarded && guarded < ready);
        assert!(
            !open[ready..].contains(".manage("),
            "every piece of state is managed before the page is told ready"
        );
        for once in [
            "Application::new(",
            "location::prepare(",
            "bridge_or_init_master_key(",
        ] {
            assert_eq!(main.matches(once).count(), 1, "{once} in one place only");
        }

        // The thread beside the window: the move, then the rest.
        let then_open = body_of("move_then_open");
        let m = find(&then_open, "finish_the_move(");
        let f = find(&then_open, "startup.move_finished();");
        let o = find(&then_open, "open_the_vault(");
        assert!(m < f && f < o);

        // The setup: a move waiting goes to that thread and returns; the
        // other path makes the move (or cleans an old copy), then opens.
        let setup = main
            .split_once(".setup(|app| {")
            .expect("main.rs has a setup hook")
            .1
            .split_once("\n        });\n")
            .expect("the setup hook closes")
            .0;
        let waiting = find(setup, "waiting_move(");
        let managed = find(setup, "app.manage(startup.clone());");
        let spawned = find(setup, ".name(MOVE_THREAD.to_owned())");
        let on_thread = find(
            setup,
            "move_then_open(&handle, on_thread, &reporting, &told);",
        );
        let returned = find(setup, "return Ok(());");
        assert!(spawned < on_thread && on_thread < returned);
        assert!(
            setup[spawned..returned].contains("std::panic::catch_unwind(run).is_err()"),
            "a panic on the thread ends the app rather than leaving the page waiting"
        );
        let normal_move = find(&setup[returned..], "finish_the_move(") + returned;
        let normal_open = find(&setup[returned..], "open_the_vault(") + returned;
        assert!(waiting < managed && managed < spawned && spawned < returned);
        assert!(normal_move < normal_open);
        assert_eq!(
            main.matches("open_the_vault(").count(),
            3,
            "defined once, and called once on each path"
        );
    }

    /// L-f's review: a startup dialog shown from the move's thread is set in
    /// front of the window, which is drawn by then; from the setup thread it
    /// never is, because there the window's own thread is the one waiting for
    /// the answer. Every startup dialog is built in that one place.
    #[test]
    fn startup_dialogs_sit_in_front_only_from_the_moves_thread() {
        let main = include_str!("main.rs").replace("\r\n", "\n");
        assert_eq!(
            main.matches(".dialog()").count(),
            1,
            "one place builds a startup dialog"
        );
        let body = main
            .split_once("\nfn startup_dialog(")
            .expect("main.rs builds its dialogs in startup_dialog")
            .1
            .split_once("\n}\n")
            .expect("startup_dialog is closed")
            .0;
        assert!(body.contains("std::thread::current().name() == Some(MOVE_THREAD)"));
        assert!(body.contains("Some(window) if on_move_thread => dialog.parent(&window),"));
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
