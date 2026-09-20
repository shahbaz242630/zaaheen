//! The persisted maintenance schedule and last-run outcome (ADR-SEC-016).
//!
//! # Why this lives in `vault-app` and not in the desktop app
//!
//! `maintenance.json` is read by the desktop app (to render the Maintenance
//! tab) and written by whatever actually *ran* a maintenance pass. Those are
//! two different processes: the app spawns a run when the user clicks "Run
//! now", and the OS scheduler spawns one nightly with the app closed.
//!
//! Before ADR-SEC-016 the type lived in `vault-tauri` and only the app ever
//! wrote it. The nightly run — the one that does the actual work — never
//! touched it. On 2026-08-27 the founder's first-ever scheduled run completed
//! successfully while the app's own Settings screen still reported the
//! previous day's failure, because nothing on the scheduled path could record
//! an outcome. A status that only updates when you press the button by hand is
//! not a status.
//!
//! So the type moves down to the crate both binaries already depend on, and
//! the rule becomes: **whoever runs maintenance records the outcome.**
//!
//! # The privacy constraint (ADR-SEC-007 / ADR-SEC-014, same rule)
//!
//! This file is **plaintext on disk**. It sits beside the encrypted vault and
//! is not itself encrypted, because the desktop app must read the schedule
//! before it has a key.
//!
//! The previous implementation recorded `truncate(child_stdout, 500)` as the
//! run summary. A consolidation run prints its `summary_markdown` and, when a
//! contradiction is surfaced for review, the model's `reasoning` about the
//! memories involved. Both are derived from memory content. That put
//! memory-derived text into a plaintext file — ADR-SEC-007's leak with a
//! different filename.
//!
//! [`RunSummary`] therefore carries **counters and durations only**, and the
//! human-readable string is built from those counters here rather than scraped
//! from anyone's output. The same rule as the application log: *record what
//! happened, never what was remembered.*

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Filename inside the app data directory.
pub const CONFIG_FILENAME: &str = "maintenance.json";

/// Recorded when a run could not start because another writer holds the vault
/// lock. Not a failure: maintenance runs at the next opportunity.
pub const OUTCOME_BUSY: &str = "maintenance_vault_busy";

/// Recorded when a run failed for any reason other than a held lock.
pub const OUTCOME_FAILED: &str = "maintenance_run_failed";

/// Recorded when a run did not start because the subscription is not active
/// (`SIGNIN-DESIGN.md` §8.26 §6.5: "paused until you subscribe"). Like
/// [`OUTCOME_BUSY`], nothing ran, so it is not a success — and nothing is
/// wrong, so it is not a failure either.
pub const OUTCOME_PAUSED: &str = "maintenance_paused_not_subscribed";

/// The persisted schedule choice plus the most recent outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceConfig {
    /// Whether automatic maintenance is turned on.
    pub enabled: bool,
    /// `"daily"` or `"weekly"`.
    pub frequency: String,
    /// Day of week for a weekly schedule, 0 = Sunday .. 6 = Saturday.
    pub weekday: u8,
    /// Hour of day (0-23).
    pub hour: u8,
    /// Minute of hour (0-59).
    pub minute: u8,
    /// Outcome of the most recent run, if any.
    #[serde(default)]
    pub last_run: Option<LastRun>,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            frequency: "daily".to_string(),
            weekday: 0,
            hour: 3,
            minute: 0,
            last_run: None,
        }
    }
}

/// The outcome of a maintenance run, as shown in the Maintenance tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRun {
    /// RFC-3339 timestamp of completion.
    pub finished_at: String,
    /// Whether it succeeded.
    pub ok: bool,
    /// A short outcome line. Built from counters (see [`RunSummary`]) or a
    /// stable error code — never free text from a model or a child process.
    pub summary: String,
}

/// The counters a completed run reports.
///
/// Deliberately not a passthrough of `ConsolidationReport`: only the fields
/// that are safe to write to a plaintext file appear here, and every one of
/// them is a count or a duration. Adding a `String` field to this struct is
/// the change that would reintroduce the leak, so don't.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunSummary {
    /// Memories examined by the run.
    pub memories_processed: u64,
    /// Merges applied.
    pub memories_merged: u64,
    /// Memories removed as duplicates.
    pub memories_deduped: u64,
    /// Memories moved to cold archive.
    pub memories_archived: u64,
    /// Contradictions queued for user review.
    pub contradictions_resolved: u64,
    /// Whole seconds the run took.
    pub duration_secs: u64,
}

impl RunSummary {
    /// Render the counters as the one-line summary shown in the tab.
    ///
    /// Counts and a duration, in the app's own words. Nothing here can carry
    /// memory content, because nothing here is a string to begin with.
    pub fn to_line(self) -> String {
        format!(
            "processed {}, merged {}, deduped {}, archived {}, flagged {} in {}s",
            self.memories_processed,
            self.memories_merged,
            self.memories_deduped,
            self.memories_archived,
            self.contradictions_resolved,
            self.duration_secs,
        )
    }
}

/// What a finished run reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// The run completed. Carries the counters it reported.
    Completed(RunSummary),
    /// Another writer held the vault lock, so nothing ran.
    Busy,
    /// The subscription is not active, so nothing ran (§8.26 §6.5).
    Paused,
    /// The run failed. No detail is recorded here on purpose (§11.7.2) — the
    /// detail is in the application log.
    Failed,
}

impl RunOutcome {
    /// Whether this outcome counts as success in the UI.
    ///
    /// [`RunOutcome::Busy`] is deliberately **not** a success: nothing ran. It
    /// is also not a failure the user should act on, which is why it keeps its
    /// own stable code rather than being folded into `Failed`.
    fn ok(&self) -> bool {
        matches!(self, RunOutcome::Completed(_))
    }

    fn summary(&self) -> String {
        match self {
            RunOutcome::Completed(s) => s.to_line(),
            RunOutcome::Busy => OUTCOME_BUSY.to_string(),
            RunOutcome::Paused => OUTCOME_PAUSED.to_string(),
            RunOutcome::Failed => OUTCOME_FAILED.to_string(),
        }
    }
}

/// Classify a failed maintenance run from the output the child process
/// produced.
///
/// # Why this takes the child's output but never stores it
///
/// A held vault lock is not a failure the user should be alarmed by —
/// maintenance simply runs at the next opportunity — so it needs its own
/// outcome. The only way to tell it apart from a real failure is the message
/// the child printed.
///
/// That output is **read here and discarded**. It is a classification input,
/// never a value that reaches [`record_run`]: an error chain can quote
/// whatever it was working on, and `maintenance.json` is plaintext. The return
/// type is an enum with no string payload precisely so that this cannot drift.
///
/// Both the desktop app's "Run now" and the scheduled path call this, so the
/// two can never disagree about what "busy" looks like.
pub fn classify_failure(child_output: &str) -> RunOutcome {
    let haystack = child_output.to_lowercase();
    if haystack.contains("already in use") || haystack.contains("busy") {
        RunOutcome::Busy
    } else {
        RunOutcome::Failed
    }
}

/// Load the persisted config, falling back to the default when it is absent or
/// unreadable.
///
/// Never fails. A corrupt config must not brick the Maintenance tab or stop a
/// scheduled run from recording its result — losing a schedule preference is
/// recoverable, a vault that will not run maintenance is not.
pub fn load(path: &Path) -> MaintenanceConfig {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist `config` atomically.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] if the temporary file cannot be
/// written or the rename fails.
pub fn save(path: &Path, config: &MaintenanceConfig) -> io::Result<()> {
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(path, json.as_bytes())
}

/// Record the outcome of a run, preserving whatever schedule is on disk.
///
/// # Why this re-reads immediately before writing
///
/// A consolidation run takes minutes. If this read the config when the run
/// started and wrote it back at the end, a user who changed their schedule
/// mid-run would have that change silently reverted by the run that was
/// already in flight. Re-reading here shrinks the clobber window to the
/// microseconds between read and rename.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] if the config cannot be written.
pub fn record_run(path: &Path, outcome: &RunOutcome, finished_at: String) -> io::Result<()> {
    let mut config = load(path);
    config.last_run = Some(LastRun {
        finished_at,
        ok: outcome.ok(),
        summary: outcome.summary(),
    });
    save(path, &config)
}

/// Write `bytes` to `path` via a sibling temporary file and a rename.
///
/// Two processes can write this file: the desktop app (schedule changes) and
/// whichever process ran maintenance. A plain truncate-then-write leaves a
/// window in which a reader sees a half-written file and falls back to
/// defaults — which would read to the user as "my schedule reset itself".
/// `rename` replaces the destination atomically on both Windows and POSIX.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = temp_sibling(path);
    // Scope the handle: Windows refuses to rename a file that is still open.
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Don't leave litter beside the user's vault if the rename failed.
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// The temporary path used by [`write_atomic`], as a sibling of `path` so the
/// rename stays on one volume (a cross-volume rename is a copy, not an atomic
/// swap).
///
/// Includes the current process id so the app and a concurrently running
/// maintenance process cannot pick the same temporary file and corrupt each
/// other's write.
fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| CONFIG_FILENAME.into());
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn summary() -> RunSummary {
        RunSummary {
            memories_processed: 41,
            memories_merged: 3,
            memories_deduped: 2,
            memories_archived: 1,
            contradictions_resolved: 0,
            duration_secs: 82,
        }
    }

    #[test]
    fn load_returns_defaults_when_the_file_is_absent() {
        let tmp = TempDir::new().expect("temp dir");
        let cfg = load(&tmp.path().join(CONFIG_FILENAME));
        assert_eq!(cfg, MaintenanceConfig::default());
    }

    #[test]
    fn load_returns_defaults_when_the_file_is_corrupt() {
        // A corrupt config must not stop a scheduled run from recording.
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join(CONFIG_FILENAME);
        std::fs::write(&path, b"{ this is not json").expect("write");
        assert_eq!(load(&path), MaintenanceConfig::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join(CONFIG_FILENAME);
        let cfg = MaintenanceConfig {
            enabled: true,
            frequency: "weekly".into(),
            weekday: 3,
            hour: 22,
            minute: 15,
            last_run: None,
        };
        save(&path, &cfg).expect("save");
        assert_eq!(load(&path), cfg);
    }

    #[test]
    fn record_run_preserves_the_schedule_the_user_chose() {
        // The regression this whole module exists to prevent: a run recording
        // its outcome must not reset the user's schedule to defaults.
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join(CONFIG_FILENAME);
        let cfg = MaintenanceConfig {
            enabled: true,
            frequency: "weekly".into(),
            weekday: 5,
            hour: 22,
            minute: 30,
            last_run: None,
        };
        save(&path, &cfg).expect("save");

        record_run(
            &path,
            &RunOutcome::Completed(summary()),
            "2026-08-27T03:00:00Z".into(),
        )
        .expect("record");

        let after = load(&path);
        assert!(after.enabled);
        assert_eq!(after.frequency, "weekly");
        assert_eq!(after.weekday, 5);
        assert_eq!(after.hour, 22);
        assert_eq!(after.minute, 30);
        assert!(after.last_run.is_some());
    }

    #[test]
    fn record_run_creates_the_file_when_no_config_exists_yet() {
        // The scheduled run can be the first thing to ever write this file if
        // the task was registered by an installer path that never saved one.
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join(CONFIG_FILENAME);

        record_run(
            &path,
            &RunOutcome::Completed(summary()),
            "2026-08-27T03:00:00Z".into(),
        )
        .expect("record");

        let after = load(&path);
        assert_eq!(after.hour, MaintenanceConfig::default().hour);
        assert!(after.last_run.expect("last run").ok);
    }

    #[test]
    fn a_completed_run_records_counters_and_a_success_flag() {
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join(CONFIG_FILENAME);
        record_run(
            &path,
            &RunOutcome::Completed(summary()),
            "2026-08-27T03:00:00Z".into(),
        )
        .expect("record");

        let last = load(&path).last_run.expect("last run");
        assert!(last.ok);
        assert_eq!(last.finished_at, "2026-08-27T03:00:00Z");
        assert!(last.summary.contains("processed 41"));
        assert!(last.summary.contains("merged 3"));
        assert!(last.summary.contains("82s"));
    }

    #[test]
    fn a_busy_run_is_not_success_and_keeps_its_own_code() {
        // "Busy" must stay distinguishable from a real failure: the UI tells
        // the user it will run at the next opportunity rather than alarming
        // them, and that copy is selected on this exact code.
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join(CONFIG_FILENAME);
        record_run(&path, &RunOutcome::Busy, "2026-08-27T03:00:00Z".into()).expect("record");

        let last = load(&path).last_run.expect("last run");
        assert!(!last.ok, "nothing ran, so this is not a success");
        assert_eq!(last.summary, OUTCOME_BUSY);
    }

    #[test]
    fn a_failed_run_records_a_stable_code_and_no_detail() {
        // §11.7.2: no internals cross into a file the user may send us.
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join(CONFIG_FILENAME);
        record_run(&path, &RunOutcome::Failed, "2026-08-27T03:00:00Z".into()).expect("record");

        let last = load(&path).last_run.expect("last run");
        assert!(!last.ok);
        assert_eq!(last.summary, OUTCOME_FAILED);
    }

    #[test]
    fn the_recorded_summary_is_built_only_from_counters() {
        // ADR-SEC-016's privacy pin. `maintenance.json` is plaintext beside the
        // encrypted vault. The previous implementation wrote 500 characters of
        // the consolidation run's stdout, which carries `summary_markdown` and
        // the model's contradiction `reasoning` -- both derived from memory
        // content. If this assertion ever needs relaxing, the leak is back.
        let line = summary().to_line();
        for ch in line.chars() {
            assert!(
                ch.is_ascii_alphanumeric() || ch == ' ' || ch == ',',
                "summary line may only contain counters and their labels; found {ch:?}"
            );
        }
        assert!(line.starts_with("processed "));
    }

    #[test]
    fn a_later_run_replaces_the_previous_outcome() {
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join(CONFIG_FILENAME);
        record_run(&path, &RunOutcome::Failed, "2026-08-26T03:00:00Z".into()).expect("first");
        record_run(
            &path,
            &RunOutcome::Completed(summary()),
            "2026-08-27T03:00:00Z".into(),
        )
        .expect("second");

        let last = load(&path).last_run.expect("last run");
        assert!(last.ok, "the newest run wins");
        assert_eq!(last.finished_at, "2026-08-27T03:00:00Z");
    }

    #[test]
    fn writing_leaves_no_temporary_file_behind() {
        // Litter beside the user's vault is its own small bug, and a stray
        // `.tmp` next to `maintenance.json` looks like corruption to anyone
        // who opens the folder.
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join(CONFIG_FILENAME);
        record_run(
            &path,
            &RunOutcome::Completed(summary()),
            "2026-08-27T03:00:00Z".into(),
        )
        .expect("record");

        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "only the config should remain: {entries:?}"
        );
        assert_eq!(entries[0], CONFIG_FILENAME);
    }

    #[test]
    fn a_held_vault_lock_classifies_as_busy_not_failure() {
        // The real message `ConsolidatorLock` produces. If this stops being
        // recognised, a user with an agent connected sees "maintenance failed"
        // every night for a situation that is entirely normal.
        let out = "error: vault is already in use by another vault-cli process \
                   (daemon/serve/consolidate)";
        assert_eq!(classify_failure(out), RunOutcome::Busy);
    }

    #[test]
    fn the_consolidator_busy_message_also_classifies_as_busy() {
        assert_eq!(classify_failure("consolidator busy"), RunOutcome::Busy);
    }

    #[test]
    fn an_unrecognised_error_classifies_as_a_real_failure() {
        // Defaulting to `Failed` is the safe direction: a genuine problem
        // reported as "will retry tonight" is how a broken vault goes
        // unnoticed for a month.
        assert_eq!(
            classify_failure("error: could not open the embedding model"),
            RunOutcome::Failed
        );
    }

    #[test]
    fn classification_is_case_insensitive() {
        assert_eq!(
            classify_failure("VAULT IS ALREADY IN USE"),
            RunOutcome::Busy
        );
    }

    #[test]
    fn classifying_never_puts_child_output_into_the_recorded_summary() {
        // The privacy pin for `classify_failure`: an error chain can quote the
        // content it was processing, and this file is plaintext. The outcome
        // enum carries no string, so the only way this could regress is by
        // changing the type -- which this asserts against.
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join(CONFIG_FILENAME);
        let leaky = "error: failed on memory 'the user moved to Porto' -- already in use";

        let outcome = classify_failure(leaky);
        record_run(&path, &outcome, "2026-08-27T03:00:00Z".into()).expect("record");

        let last = load(&path).last_run.expect("last run");
        assert_eq!(last.summary, OUTCOME_BUSY);
        assert!(
            !last.summary.contains("Porto"),
            "child output reached the plaintext status file"
        );
    }

    #[test]
    fn the_temporary_path_is_a_sibling_so_the_rename_stays_on_one_volume() {
        let path = Path::new("/data/vault/maintenance.json");
        let tmp = temp_sibling(path);
        assert_eq!(tmp.parent(), path.parent());
        assert_ne!(tmp.file_name(), path.file_name());
    }

    // -----------------------------------------------------------------------
    // Maintenance while the subscription is not active (§8.26 §6.5, §8.35)
    // -----------------------------------------------------------------------

    /// Nothing ran, so it is not a success; nothing is wrong, so it is not a
    /// failure. The same distinction `Busy` already draws, and it keeps its
    /// own code so the UI can say something true about it.
    #[test]
    fn a_paused_run_is_neither_a_success_nor_a_failure() {
        assert!(!RunOutcome::Paused.ok(), "nothing ran");
        assert_eq!(RunOutcome::Paused.summary(), OUTCOME_PAUSED);
        assert_ne!(
            RunOutcome::Paused.summary(),
            OUTCOME_FAILED,
            "a paused run is not something the user has to fix"
        );
        assert_ne!(RunOutcome::Paused.summary(), OUTCOME_BUSY);
    }

    /// What is written is a code, never free text (§11.7.2): this file sits in
    /// plaintext beside the encrypted vault.
    #[test]
    fn a_paused_run_records_its_code_and_nothing_else() {
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join(CONFIG_FILENAME);
        record_run(&path, &RunOutcome::Paused, "2026-09-20T10:00:00Z".into()).expect("record");

        let last = load(&path).last_run.expect("an outcome was recorded");
        assert!(!last.ok);
        assert_eq!(last.summary, OUTCOME_PAUSED);
    }

    /// A code with no arm in `friendlyMaintError` falls through to showing the
    /// user the raw code, so the plain-English line ships with the code.
    ///
    /// This lives here, beside the codes themselves, rather than in
    /// `vault-tauri`: the codes are defined in this file, `vault-account`
    /// already sets the precedent for pinning an artefact from elsewhere in
    /// the tree with `include_str!` (`lease/worker_vectors.rs`), and it adds
    /// no new gate step.
    #[test]
    fn every_outcome_code_has_a_plain_english_line_in_the_app() {
        const APP_JS: &str = include_str!("../../vault-tauri/dist/app.js");
        for code in [OUTCOME_BUSY, OUTCOME_FAILED, OUTCOME_PAUSED] {
            assert!(
                APP_JS.contains(&format!("case \"{code}\":")),
                "{code} has no plain-English line in friendlyMaintError, so \
                 the user would be shown the raw code"
            );
        }
    }
}
