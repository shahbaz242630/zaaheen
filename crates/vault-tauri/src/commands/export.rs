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

use serde_json::json;
use tauri::State;
use vault_core::{Memory, VaultError, VaultKeyFailure};

use crate::link::{decoded, KeeperLink, KeyState, Kind};

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

/// Upper bound on one export: pages of [`PAGE`] up to this many memories.
///
/// Not a security bound — it is the person's own vault — but an honest one,
/// and a guard against a keeper that never ends its pages. A hundred
/// thousand memories is far beyond V0.2 scale (BRD §5: hundreds to low
/// thousands) while still being a real ceiling rather than a pretend one.
const MAX_EXPORTED: usize = 100_000;

/// One page from the keeper (ADR-108 D2: never the whole vault in one
/// message).
const PAGE: usize = 2_000;

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

/// Inner implementation, over the keeper link so it is testable without a
/// Tauri runtime (the pattern the other command modules already use).
///
/// Returns how many memories were written, so the UI can say "412 memories
/// saved" rather than a bare tick.
///
/// # Errors
///
/// One of this module's stable codes. The underlying reason goes to the log,
/// never to the caller (BRD §11.7.2).
pub async fn export_memories_inner(link: &KeeperLink, destination: &str) -> Result<usize, String> {
    let started = Instant::now();
    let dest = validate_destination(destination)?;

    // Since ADR-108 the memories come from the keeper, page by page. A
    // computer with no key yet has nothing to export and needs no keeper
    // (D4); one whose key is missing while memories are on disk says so in
    // the startup message's own words, never "nothing to export" (review
    // A R2-2).
    let result = match link.key_state() {
        KeyState::Present => run_export(link, &dest).await,
        KeyState::Absent { keyed_data: false } => {
            return write_all(&[], &dest);
        }
        KeyState::Absent { keyed_data: true } => {
            return Err(crate::format_keychain_error_dialog(&VaultError::VaultKey(
                VaultKeyFailure::Missing,
            )));
        }
        KeyState::Unreadable => {
            return Err(crate::format_keychain_error_dialog(
                &VaultError::KeychainProvenance("the credential store could not be read".into()),
            ));
        }
    };

    // BRD §11.9.1 lists "export" among the data operations that must be
    // logged (step 4c's independent review). Written AFTER the file, with
    // the final outcome, by the keeper that holds the vault (ADR-108 D2,
    // reviews A-S2 / B-M6). Best effort: the export itself has happened.
    let _ = link
        .call(
            "admin_audit_event",
            json!({
                "event": "export_memories",
                "duration_ms": started.elapsed().as_millis() as u64,
                "result_count": result.as_ref().copied().unwrap_or(0) as u32,
                "failed": result.is_err(),
            }),
            Kind::Write,
        )
        .await;

    if let Ok(count) = &result {
        tracing::info!(count, "memories exported");
    }
    result
}

/// Every memory, page by page from the keeper, then the file.
async fn run_export(link: &KeeperLink, dest: &Path) -> Result<usize, String> {
    let mut memories: Vec<Memory> = Vec::new();
    let mut args = json!({ "limit": PAGE });
    while memories.len() < MAX_EXPORTED {
        let text = link
            .call("admin_export_page", args.clone(), Kind::Read)
            .await
            .map_err(|code| {
                tracing::error!(code, "could not read memories for the export");
                ERR_EXPORT_READ_FAILED.to_string()
            })?;
        let page: Vec<Memory> = decoded(&text).map_err(|_| ERR_EXPORT_READ_FAILED.to_string())?;
        let Some(last) = page.last() else {
            break;
        };
        args = json!({
            "limit": PAGE,
            "after_created_at": last.created_at.to_rfc3339(),
            "after_id": last.id.to_string(),
        });
        memories.extend(page);
    }
    write_all(&memories, dest)
}

/// Format and write, returning how many memories the file holds.
fn write_all(memories: &[Memory], dest: &Path) -> Result<usize, String> {
    let text = vault_app::export::to_markdown(memories, chrono::Utc::now());
    write_file(dest, &text)?;
    Ok(memories.len())
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
    link: State<'_, KeeperLink>,
    destination: String,
) -> Result<usize, String> {
    export_memories_inner(link.inner(), &destination).await
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
