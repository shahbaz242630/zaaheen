//! Diagnostic-log export (ADR-SEC-017).
//!
//! # Why this exists
//!
//! ADR-SEC-014 gave the app a log file. It lands in the OS log directory —
//! `%LOCALAPPDATA%\<bundle id>\logs` on Windows — which is a path a
//! non-technical beta tester will never find, and could not paste into an
//! email if they did.
//!
//! So "it broke" still yields nothing, which is the situation ADR-SEC-014 was
//! written to end. A log nobody can send is a log nobody reads. This command
//! is the last few feet: one button, one file, somewhere they can attach it
//! from.
//!
//! # What it exports, and what it does not
//!
//! **Both generations of the application log**, concatenated oldest-first into
//! one file. One attachment is easier to send than two, and the previous
//! generation is often the one holding the crash.
//!
//! It does **not** export the audit log. That is a different artifact with a
//! different purpose: it is the tamper-evident record of every operation on
//! the vault (BRD §11.9.2) and it names boundaries, resources and timestamps
//! for the user's own memories. Handing that to us to debug a startup failure
//! would be wildly disproportionate. BRD §11.9.3's "user can review, search and
//! export their logs" describes a user-facing right over that audit log; it is
//! a separate, unbuilt feature and this command does not satisfy it.
//!
//! It also does not export `maintenance.json`. It is small and would be
//! useful, but it is governed by a different rule set than the application log
//! and adding it here would mean two privacy contracts in one export path.
//!
//! # Why the export is safe to hand over
//!
//! The application log carries no memory content by construction: the rule is
//! *log what happened, never what was remembered*, and
//! `tests/log_privacy.rs` fails the build if a `tracing` call site records a
//! field carrying memory text. This command copies that file verbatim and adds
//! a header of its own — so the export is exactly as safe as the log, and the
//! enforcement already in place covers both.
//!
//! The header deliberately omits the log's own path. §11.7.2 rules file paths
//! out of anything user-facing, and a home directory contains a real name more
//! often than not.
//!
//! # Audit
//!
//! An export is a data operation under §11.9.1, so it writes an audit row like
//! every other command here.

use std::path::{Path, PathBuf};
use std::time::Instant;

use tauri::State;
use vault_app::logging::{LOG_FILENAME, LOG_FILENAME_PREVIOUS};
use vault_app::Application;
use vault_mcp::ToolInvokeDetails;

/// Opaque error codes (§11.7.2 — no internals cross the IPC boundary).
///
/// The export could not be written.
pub const ERR_LOG_EXPORT_FAILED: &str = "log_export_failed";
/// There is no log to export yet.
pub const ERR_LOG_EXPORT_EMPTY: &str = "log_export_empty";
/// The destination the UI supplied is not usable.
pub const ERR_LOG_EXPORT_BAD_DESTINATION: &str = "log_export_bad_destination";

/// Where the application log lives. Managed as Tauri state, resolved once in
/// `main.rs` setup from the same directory `logging::init` was given.
pub struct LogContext {
    /// The OS log directory for this app.
    pub log_dir: PathBuf,
}

/// Save a copy of the diagnostic log to `destination`.
///
/// Returns the number of bytes written, which the UI shows as a size so the
/// user can see that something real was produced.
///
/// # Errors
///
/// Returns [`ERR_LOG_EXPORT_BAD_DESTINATION`] when the path is unusable,
/// [`ERR_LOG_EXPORT_EMPTY`] when no log exists yet, and
/// [`ERR_LOG_EXPORT_FAILED`] when the copy could not be written.
#[tauri::command]
pub async fn export_logs(
    app: State<'_, Application>,
    ctx: State<'_, LogContext>,
    destination: String,
) -> Result<u64, String> {
    let start = Instant::now();
    let result = export_logs_inner(&ctx.log_dir, &destination);

    let duration_ms = start.elapsed().as_millis() as u64;
    let error_for_audit = result.as_ref().err().map(|_| {
        vault_mcp::ToolInvokeError::from_vault_error(&vault_core::VaultError::Storage(
            "log export failed".to_string(),
        ))
    });
    // §11.9.1: an export is a data operation and is recorded as one.
    let _ = app
        .adapter()
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "export_logs",
            duration_ms,
            result_count: u32::from(result.is_ok()),
            boundary_count: 0,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        })
        .await;

    result
}

/// The export itself, free of Tauri state so it can be tested directly.
fn export_logs_inner(log_dir: &Path, destination: &str) -> Result<u64, String> {
    let dest = validate_destination(destination)?;

    let previous = read_if_present(&log_dir.join(LOG_FILENAME_PREVIOUS));
    let current = read_if_present(&log_dir.join(LOG_FILENAME));

    if previous.is_none() && current.is_none() {
        // Not a failure — there is genuinely nothing yet, and the UI says so
        // in those words rather than reporting an error the user cannot act on.
        return Err(ERR_LOG_EXPORT_EMPTY.to_string());
    }

    let mut out = String::with_capacity(
        previous.as_ref().map_or(0, String::len) + current.as_ref().map_or(0, String::len) + 512,
    );
    out.push_str(&header());
    // Oldest first, so the file reads forwards in time.
    if let Some(text) = previous {
        out.push_str("\n===== earlier session =====\n");
        out.push_str(&text);
    }
    if let Some(text) = current {
        out.push_str("\n===== current session =====\n");
        out.push_str(&text);
    }

    std::fs::write(&dest, out.as_bytes()).map_err(|e| {
        tracing::error!(error = %e, "could not write the log export");
        ERR_LOG_EXPORT_FAILED.to_string()
    })?;

    let written = out.len() as u64;
    tracing::info!(bytes = written, "diagnostic log exported");
    Ok(written)
}

/// Validate the destination the UI supplied (§11.7.1 — every input is
/// adversarial, including one that came from our own save dialog).
fn validate_destination(destination: &str) -> Result<PathBuf, String> {
    if destination.trim().is_empty() || destination.contains('\0') {
        return Err(ERR_LOG_EXPORT_BAD_DESTINATION.to_string());
    }
    let path = PathBuf::from(destination);
    // Absolute only. A relative path would resolve against whatever directory
    // the app happens to have been launched from, and the user would never
    // find the file they just asked us to save.
    if !path.is_absolute() {
        return Err(ERR_LOG_EXPORT_BAD_DESTINATION.to_string());
    }
    match path.parent() {
        Some(dir) if dir.is_dir() => Ok(path),
        _ => Err(ERR_LOG_EXPORT_BAD_DESTINATION.to_string()),
    }
}

/// Read a log generation, treating "not there" as "nothing to include".
///
/// A missing previous generation is the normal case on a fresh install, not an
/// error. Lossy decoding rather than a hard failure: a log truncated mid-write
/// by a crash is exactly the log worth sending.
fn read_if_present(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The export's own header.
///
/// Version and platform, because "which build was this?" is the first question
/// any report raises. Deliberately no file paths (§11.7.2) and nothing about
/// the vault's contents.
fn header() -> String {
    format!(
        "Zaaheen diagnostic log\nExported: {}\nApp version: {}\nPlatform: {} {}\n",
        chrono::Utc::now().to_rfc3339(),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_log(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).expect("log dir");
        std::fs::write(dir.join(name), body).expect("write log");
    }

    #[test]
    fn both_generations_are_exported_oldest_first() {
        // The previous generation usually holds the crash; the current one
        // holds what happened after the restart. Reading them out of order
        // makes a timeline impossible to follow.
        let tmp = TempDir::new().expect("temp dir");
        let logs = tmp.path().join("logs");
        write_log(&logs, LOG_FILENAME_PREVIOUS, "OLDER LINE\n");
        write_log(&logs, LOG_FILENAME, "NEWER LINE\n");
        let dest = tmp.path().join("export.txt");

        let written =
            export_logs_inner(&logs, &dest.to_string_lossy()).expect("export should succeed");
        assert!(written > 0);

        let out = std::fs::read_to_string(&dest).expect("read export");
        let older = out
            .find("OLDER LINE")
            .expect("previous generation included");
        let newer = out.find("NEWER LINE").expect("current generation included");
        assert!(older < newer, "the earlier session must come first");
    }

    #[test]
    fn a_missing_previous_generation_is_normal_not_an_error() {
        // The common case: a fresh install that has never rotated.
        let tmp = TempDir::new().expect("temp dir");
        let logs = tmp.path().join("logs");
        write_log(&logs, LOG_FILENAME, "ONLY LINE\n");
        let dest = tmp.path().join("export.txt");

        export_logs_inner(&logs, &dest.to_string_lossy()).expect("export should succeed");
        let out = std::fs::read_to_string(&dest).expect("read export");
        assert!(out.contains("ONLY LINE"));
        assert!(!out.contains("earlier session"));
    }

    #[test]
    fn no_log_at_all_reports_the_empty_code() {
        let tmp = TempDir::new().expect("temp dir");
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).expect("log dir");
        let dest = tmp.path().join("export.txt");

        let err = export_logs_inner(&logs, &dest.to_string_lossy())
            .expect_err("nothing to export must not silently write an empty file");
        assert_eq!(err, ERR_LOG_EXPORT_EMPTY);
        assert!(!dest.exists(), "no file should be produced");
    }

    #[test]
    fn an_empty_log_file_counts_as_nothing_to_export() {
        // `logging::init` creates the file on startup, so a user who exports
        // before anything is logged would otherwise get a header and no
        // content -- which reads as "the log is broken".
        let tmp = TempDir::new().expect("temp dir");
        let logs = tmp.path().join("logs");
        write_log(&logs, LOG_FILENAME, "");
        let dest = tmp.path().join("export.txt");

        assert_eq!(
            export_logs_inner(&logs, &dest.to_string_lossy()).expect_err("empty"),
            ERR_LOG_EXPORT_EMPTY
        );
    }

    #[test]
    fn the_export_carries_a_version_and_platform_header() {
        let tmp = TempDir::new().expect("temp dir");
        let logs = tmp.path().join("logs");
        write_log(&logs, LOG_FILENAME, "a line\n");
        let dest = tmp.path().join("export.txt");

        export_logs_inner(&logs, &dest.to_string_lossy()).expect("export");
        let out = std::fs::read_to_string(&dest).expect("read export");
        assert!(out.starts_with("Zaaheen diagnostic log"));
        assert!(out.contains(env!("CARGO_PKG_VERSION")));
        assert!(out.contains(std::env::consts::OS));
    }

    #[test]
    fn the_header_names_no_file_paths() {
        // §11.7.2: a home directory usually contains the user's real name, and
        // this file is about to be attached to an email.
        let head = header();
        assert!(!head.contains('/'), "no unix paths: {head}");
        assert!(!head.contains('\\'), "no windows paths: {head}");
    }

    #[test]
    fn a_relative_destination_is_rejected() {
        // It would resolve against the app's working directory and the user
        // would never find the file.
        assert_eq!(
            validate_destination("logs.txt").expect_err("must reject"),
            ERR_LOG_EXPORT_BAD_DESTINATION
        );
    }

    #[test]
    fn an_empty_or_null_bearing_destination_is_rejected() {
        assert!(validate_destination("").is_err());
        assert!(validate_destination("   ").is_err());
        assert!(validate_destination("/tmp/a\0b.txt").is_err());
    }

    #[test]
    fn a_destination_whose_directory_does_not_exist_is_rejected() {
        // Failing here gives a clear code; failing at `write` would surface as
        // the generic export failure and tell the user nothing.
        let tmp = TempDir::new().expect("temp dir");
        let missing = tmp.path().join("nope").join("export.txt");
        assert_eq!(
            validate_destination(&missing.to_string_lossy()).expect_err("must reject"),
            ERR_LOG_EXPORT_BAD_DESTINATION
        );
    }

    #[test]
    fn a_valid_absolute_destination_is_accepted() {
        let tmp = TempDir::new().expect("temp dir");
        let dest = tmp.path().join("export.txt");
        assert_eq!(
            validate_destination(&dest.to_string_lossy()).expect("must accept"),
            dest
        );
    }

    #[test]
    fn the_export_is_a_verbatim_copy_of_the_log_body() {
        // The privacy argument for this command rests entirely on the log
        // being clean (enforced by tests/log_privacy.rs). That argument only
        // holds if the export neither rewrites nor enriches the body -- if it
        // ever started pulling in context from elsewhere, the enforcement
        // would no longer cover what gets sent.
        let tmp = TempDir::new().expect("temp dir");
        let logs = tmp.path().join("logs");
        let body = "INFO vault_app: drained 3 queued deletes\nWARN vault_cli: busy\n";
        write_log(&logs, LOG_FILENAME, body);
        let dest = tmp.path().join("export.txt");

        export_logs_inner(&logs, &dest.to_string_lossy()).expect("export");
        let out = std::fs::read_to_string(&dest).expect("read export");
        assert!(out.contains(body), "the log body must survive unchanged");
        assert_eq!(
            out,
            format!(
                "{}\n===== current session =====\n{body}",
                header_prefix(&out)
            ),
            "nothing may be added beyond the header and the section marker"
        );
    }

    /// The header as it actually appeared in `out` (it carries a timestamp, so
    /// it cannot be regenerated and compared byte-for-byte).
    fn header_prefix(out: &str) -> String {
        let marker = "\n===== current session =====\n";
        out.split(marker)
            .next()
            .expect("header precedes the marker")
            .to_string()
    }
}
