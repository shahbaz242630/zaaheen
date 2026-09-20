//! "Download my memories" (S4, S3 step 4c) — the command behind the lock
//! screen's second button.
//!
//! # Why this one is ungated, and why that needs its own argument
//!
//! It fills the **export** slot of §8.26 §6.4's allowlist, and it has to: the
//! whole reason the export exists is that it works when nothing else does.
//! BRD §1.6 amendment 1: *"`vault.db` is SQLCipher-encrypted, so the files on
//! disk are unreadable without the app. Without the export, a locked app
//! would cut people off from their own memories, and we could not honestly
//! say 'your memories are always yours'."*
//!
//! The five account commands are safe to leave ungated because they are never
//! given the vault. **This one is different** — it reads every memory, so it
//! cannot borrow that argument. What makes it acceptable is narrower and
//! worth stating:
//!
//! * it **only reads**, and writes nothing back to the vault;
//! * it writes to a path the person chose in their own save dialog, nowhere
//!   else, and the path is validated the same way `export_logs` validates its
//!   own (absolute, real parent directory, no NUL);
//! * it sends nothing anywhere — there is no network call on this path at
//!   all;
//! * and the data it writes is the person's own, handed to the person
//!   themselves, which is the entire point.
//!
//! It is, in other words, the one command whose *job* is to give somebody
//! their data when they are locked out. Gating it would defeat the promise it
//! exists to keep.

use std::path::{Path, PathBuf};
use std::time::Instant;

use tauri::State;
use vault_app::Application;
use vault_mcp::ToolInvokeDetails;

/// Opaque error code: the destination was not a usable file path.
pub const ERR_EXPORT_BAD_DESTINATION: &str = "export_bad_destination";
/// Opaque error code: the memories could not be read.
pub const ERR_EXPORT_READ_FAILED: &str = "export_read_failed";
/// Opaque error code: the file could not be written.
pub const ERR_EXPORT_WRITE_FAILED: &str = "export_write_failed";

/// Every code this module can return, for the test pinning each to a
/// plain-English line in the desktop bundle.
pub const ALL_CODES: &[&str] = &[
    ERR_EXPORT_BAD_DESTINATION,
    ERR_EXPORT_READ_FAILED,
    ERR_EXPORT_WRITE_FAILED,
];

/// Upper bound on one export.
///
/// Not a security bound — it is the person's own vault — but an honest one:
/// `list_recent_memories` takes a limit, and a number has to be chosen. A
/// hundred thousand memories is far beyond V0.2 scale (BRD §5: hundreds to
/// low thousands) while still being a real ceiling rather than a pretend one.
const MAX_EXPORTED: usize = 100_000;

/// Same rules as the log export: absolute, real parent, no NUL. A relative
/// path would resolve against whatever directory the app was launched from,
/// and the person would never find the file they just asked us to save.
fn validate_destination(destination: &str) -> Result<PathBuf, String> {
    if destination.trim().is_empty() || destination.contains('\0') {
        return Err(ERR_EXPORT_BAD_DESTINATION.to_string());
    }
    // A UNC path (`\host\share\...`) is "absolute" and its parent can be a
    // real directory, so the checks below would pass and the whole vault
    // would be written over SMB to another machine. This module's own docs
    // promise it "sends nothing anywhere"; refusing UNC is what makes that
    // sentence true rather than nearly true. Found by step 4c's independent
    // review.
    //
    // Note the shared gap: `export_logs` validates the same way and does not
    // refuse UNC. Logged as tech debt rather than changed here, because a
    // log export going to a share is a different (and much smaller) fact than
    // a memory export doing so.
    if destination.starts_with("\\\\") || destination.starts_with("//") {
        return Err(ERR_EXPORT_BAD_DESTINATION.to_string());
    }
    let path = PathBuf::from(destination);
    if !path.is_absolute() {
        return Err(ERR_EXPORT_BAD_DESTINATION.to_string());
    }
    match path.parent() {
        Some(dir) if dir.is_dir() => Ok(path),
        _ => Err(ERR_EXPORT_BAD_DESTINATION.to_string()),
    }
}

/// Inner implementation, `Application`-only so it is testable without a Tauri
/// runtime (the pattern the other command modules already use).
///
/// Returns how many memories were written, so the UI can say "412 memories
/// saved" rather than a bare tick.
///
/// # Errors
///
/// One of this module's stable codes. The underlying reason goes to the log,
/// never to the caller (BRD §11.7.2).
pub async fn export_memories_inner(app: &Application, destination: &str) -> Result<usize, String> {
    let started = Instant::now();
    let dest = validate_destination(destination)?;

    let result = run_export(app, &dest).await;

    // BRD §11.9.1 lists "export" among the data operations that must be
    // logged, and this crate's own module docs say read-only commands audit
    // too. Missed on the first pass and found by step 4c's independent
    // review: a bulk read of the **entire vault** was leaving no trace at
    // all, while its sibling `export_logs` -- which reads far less -- did
    // record one.
    let error_for_audit = result.as_ref().err().map(|_| {
        vault_mcp::ToolInvokeError::from_vault_error(&vault_core::VaultError::Storage(
            "memory export failed".to_string(),
        ))
    });
    let _ = app
        .adapter()
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "export_memories",
            duration_ms: started.elapsed().as_millis() as u64,
            // How many memories left the vault, which is the fact an audit
            // reader cares about.
            result_count: result.as_ref().copied().unwrap_or(0) as u32,
            boundary_count: 0,
            max_results: Some(MAX_EXPORTED as u32),
            score_threshold: None,
            include_archived: Some(false),
            query_length: None,
            error: error_for_audit,
        })
        .await;

    if let Ok(count) = &result {
        tracing::info!(count, "memories exported");
    }
    result
}

/// The read and the write, split out so the audit row above records the
/// outcome of the whole operation however it ended.
async fn run_export(app: &Application, dest: &Path) -> Result<usize, String> {
    let memories = app
        .adapter()
        .list_recent_memories(MAX_EXPORTED)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "could not read memories for the export");
            ERR_EXPORT_READ_FAILED.to_string()
        })?;

    let count = memories.len();
    let text = vault_app::export::to_markdown(&memories, chrono::Utc::now());
    write_file(dest, &text)?;
    Ok(count)
}

/// The write itself, off the async runtime: a large vault is a real file
/// write and blocking the runtime would freeze the window mid-save.
fn write_file(dest: &Path, text: &str) -> Result<(), String> {
    std::fs::write(dest, text.as_bytes()).map_err(|e| {
        tracing::error!(error = %e, "could not write the memory export");
        ERR_EXPORT_WRITE_FAILED.to_string()
    })
}

/// Write every memory to a readable file the person chose.
///
/// Ungated by design — see this module's docs. It reads, it writes one local
/// file, and it sends nothing anywhere.
#[tauri::command]
pub async fn export_memories(
    app: State<'_, Application>,
    destination: String,
) -> Result<usize, String> {
    export_memories_inner(app.inner(), &destination).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_destination_that_is_not_a_real_place_is_refused() {
        for bad in ["", "   ", "memories.md", "./memories.md", "..\\memories.md"] {
            assert!(
                validate_destination(bad).is_err(),
                "{bad:?} should not be accepted as a destination"
            );
        }
    }

    /// The whole vault over SMB is exactly what "it sends nothing anywhere"
    /// is supposed to rule out.
    #[test]
    fn a_network_share_destination_is_refused() {
        for unc in [
            r"\\attacker-host\share\memories.md",
            "//attacker-host/share/memories.md",
            r"\\?\UNC\host\share\memories.md",
        ] {
            assert!(
                validate_destination(unc).is_err(),
                "{unc:?} would write the vault to another machine"
            );
        }
    }

    #[test]
    fn a_destination_containing_a_nul_is_refused() {
        assert!(validate_destination("C:\\Users\\me\\mem\0.md").is_err());
    }

    #[test]
    fn a_destination_in_a_directory_that_does_not_exist_is_refused() {
        let missing = if cfg!(windows) {
            "C:\\this\\directory\\does\\not\\exist\\memories.md"
        } else {
            "/this/directory/does/not/exist/memories.md"
        };
        assert!(validate_destination(missing).is_err());
    }

    #[test]
    fn a_real_absolute_destination_is_accepted() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let dest = dir.path().join("my memories.md");
        let ok = validate_destination(&dest.to_string_lossy()).expect("a real path is accepted");
        assert_eq!(ok, dest);
    }

    /// The file is written verbatim: what the formatter produced is what the
    /// person opens.
    #[test]
    fn the_file_holds_exactly_what_was_formatted() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let dest = dir.path().join("memories.md");
        let text = "# My Zaaheen memories\n\nsomething\n";

        write_file(&dest, text).expect("the write succeeds");
        let back = std::fs::read_to_string(&dest).expect("the file reads back");
        assert_eq!(back, text);
    }

    /// A code with no arm in the frontend shows the person the raw code.
    #[test]
    fn every_export_code_has_a_plain_english_line_in_the_app() {
        const APP_JS: &str = include_str!("../../dist/app.js");
        for code in ALL_CODES {
            assert!(
                APP_JS.contains(&format!("case \"{code}\":")),
                "{code} has no plain-English line in the desktop bundle"
            );
        }
    }
}
