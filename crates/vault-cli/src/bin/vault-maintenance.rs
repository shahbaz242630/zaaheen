//! Windowless maintenance runner (ADR-SEC-015).
//!
//! # The bug this exists to fix
//!
//! On 2026-08-27 the founder turned on their laptop and a black console window
//! appeared unannounced. It was the nightly maintenance task, firing late
//! (`StartWhenAvailable` catches up a run missed while the machine was off) and
//! running to completion in full view.
//!
//! The cause is structural, not a missing setting. Windows creates a console
//! for a console-subsystem executable, and the scheduled task ran `vault-cli`
//! directly under `InteractiveToken` — as the logged-in user, on their desktop.
//! Task Scheduler's `<Hidden>` element does not help: it hides the *task* from
//! the Task Scheduler UI, not the window of the process it launches.
//!
//! At thirty beta testers this is thirty people watching an unexplained
//! terminal open on their machine. Some of them will assume malware, and they
//! will be behaving sensibly.
//!
//! # Why a second binary rather than changing `vault-cli`
//!
//! The console is allocated by the OS **before** `main` runs, so nothing
//! `vault-cli` does at startup can prevent it. `ShowWindow(SW_HIDE)` still
//! flashes. The subsystem is fixed at link time by a crate-level attribute, and
//! `vault-cli` must stay a console program — it is the operator CLI, and an
//! operator needs its output.
//!
//! So the scheduler points at this binary instead. It is linked for the
//! Windows subsystem, so it never gets a console of its own, and it spawns
//! `vault-cli` with `CREATE_NO_WINDOW` so the child never gets one either. No
//! window appears at any point.
//!
//! The alternative — scheduling the desktop app itself with a headless flag —
//! was rejected: it would boot the whole GUI runtime for a background job, and
//! if the app were already running the second instance would contend for the
//! vault lock. That is the failure ADR-SEC-012 just finished fixing.
//!
//! # What it is responsible for
//!
//! Deliberately almost nothing. It spawns the already-tested consolidation
//! path and records an outcome. It does not parse the child's report, hold the
//! vault lock, or touch the vault. Keeping it this thin is the point: the
//! process that runs unattended every night should have the least logic in the
//! system, not the most.
//!
//! Two things it does own, because nobody else can:
//!
//! - **Logging.** A windowless child has no stderr either, so it passes
//!   `VAULT_LOG_DIR` and the child's `tracing` output lands in the shared
//!   application log (ADR-SEC-014). Without this the nightly run would be
//!   silent, which is worse than the window.
//! - **Recording a run that never started.** `vault-cli` records its own
//!   outcome once it has a report (ADR-SEC-016). A run that dies before
//!   that — a held lock, a missing model, a crash — can only be recorded by
//!   its parent, which is this.

// Unconditional, unlike the desktop app's
// `cfg_attr(not(debug_assertions), ...)`. The app keeps a console in debug
// builds so a developer can see its output; this binary must not, because a
// debug build that shows a window would hide the very defect it exists to fix.
// Its output goes to the shared log file instead, which is where a background
// process's output belongs anyway.
#![cfg_attr(windows, windows_subsystem = "windows")]
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Parser;
use vault_app::logging;
use vault_app::maintenance_state::{self, RunOutcome};

/// Executable spawned to do the actual work, resolved as our own sibling.
#[cfg(windows)]
const VAULT_CLI_EXE: &str = "zaaheen.exe";
#[cfg(not(windows))]
const VAULT_CLI_EXE: &str = "zaaheen";

/// `CREATE_NO_WINDOW` — run a console child with no console window.
///
/// From the Win32 process-creation flags. Declared here rather than pulled
/// from a binding crate: it is one stable constant, and the alternative is a
/// new dependency for a single `u32`.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Run a maintenance pass without showing a console window.
#[derive(Parser, Debug)]
#[command(
    name = "zaaheen-maintenance",
    about = "Runs Zaaheen maintenance in the background, with no window.",
    long_about = None
)]
struct Args {
    /// Where to record the run's outcome (`<data>/maintenance.json`).
    #[arg(long, value_name = "PATH")]
    status_file: PathBuf,

    /// Directory for the application log.
    #[arg(long, value_name = "PATH")]
    log_dir: PathBuf,

    /// Override the `vault-cli` executable. Defaults to our own sibling, which
    /// is what the installer lays down; the override exists for tests and for
    /// a development tree where the two binaries sit elsewhere.
    #[arg(long, value_name = "PATH")]
    vault_cli: Option<PathBuf>,

    /// Arguments passed through to `vault-cli` verbatim.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    child_args: Vec<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();

    // Logging first, so a failure in any later step is recorded somewhere. A
    // log we could not open is never a reason to skip maintenance.
    if let Err(e) = logging::init(&args.log_dir) {
        eprintln!("zaaheen: could not start file logging: {e}");
    }

    let vault_cli = match args.vault_cli.clone() {
        Some(path) => path,
        None => match sibling_vault_cli() {
            Ok(path) => path,
            Err(e) => {
                tracing::error!(error = %e, "could not locate the maintenance executable");
                record(&args.status_file, &RunOutcome::Failed);
                return ExitCode::FAILURE;
            }
        },
    };

    match run_child(&vault_cli, &args) {
        Ok(true) => {
            // The child recorded its own counters on the way out
            // (ADR-SEC-016). Writing again here would overwrite a real summary
            // with a less informative one.
            tracing::info!("maintenance run completed");
            ExitCode::SUCCESS
        }
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            tracing::error!(error = %e, "could not start the maintenance run");
            record(&args.status_file, &RunOutcome::Failed);
            ExitCode::FAILURE
        }
    }
}

/// Spawn `vault-cli` and record the outcome. Returns whether it succeeded.
fn run_child(vault_cli: &Path, args: &Args) -> std::io::Result<bool> {
    let mut command = Command::new(vault_cli);
    command
        .args(&args.child_args)
        // The child records its own counters, so it needs to know where.
        // Appended here rather than asked of the caller so there is exactly
        // one place that decides which file a run reports into.
        .arg("--record-status")
        .arg(&args.status_file)
        .env(logging::LOG_DIR_ENV, &args.log_dir);
    no_window(&mut command);

    let output = command.output()?;
    if output.status.success() {
        return Ok(true);
    }

    // Classify from the child's output, then discard it. It is never stored:
    // see `maintenance_state::classify_failure`.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let outcome = maintenance_state::classify_failure(&combined);
    match outcome {
        RunOutcome::Busy => {
            tracing::warn!("maintenance skipped: the vault is in use by another writer")
        }
        _ => tracing::error!(status = %output.status, "maintenance run failed"),
    }
    record(&args.status_file, &outcome);
    Ok(false)
}

/// Suppress the child's console window on Windows.
///
/// A no-op elsewhere: POSIX has no console to create, and a scheduled job on
/// macOS or Linux inherits no terminal in the first place.
#[cfg(windows)]
fn no_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn no_window(_command: &mut Command) {}

/// Resolve `vault-cli` as a sibling of this executable.
///
/// The installer places both in the same directory, so this holds for every
/// shipped build and needs no configuration. Resolving by path rather than by
/// `PATH` lookup also means we cannot be tricked into running some other
/// `vault-cli` that happens to be earlier in the user's `PATH`.
fn sibling_vault_cli() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "executable has no parent directory",
        )
    })?;
    Ok(dir.join(VAULT_CLI_EXE))
}

/// Record an outcome, logging rather than failing if the file cannot be
/// written. A status we could not save is a stale badge in the UI; refusing to
/// exit over it helps nobody.
fn record(status_file: &Path, outcome: &RunOutcome) {
    if let Err(e) =
        maintenance_state::record_run(status_file, outcome, chrono::Utc::now().to_rfc3339())
    {
        tracing::error!(error = %e, "could not record the maintenance outcome");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Args {
        Args::try_parse_from(argv).expect("args should parse")
    }

    #[test]
    fn the_launcher_forwards_every_child_argument_verbatim() {
        // The scheduler builds a long, exact argument vector pointing at the
        // desktop app's vault. Dropping or reordering any of it would silently
        // consolidate the wrong vault, or none.
        let args = parse(&[
            "zaaheen-maintenance",
            "--status-file",
            "/data/maintenance.json",
            "--log-dir",
            "/logs",
            "--vault-db",
            "/data/vault.db",
            "consolidate",
            "run",
        ]);
        assert_eq!(
            args.child_args,
            vec!["--vault-db", "/data/vault.db", "consolidate", "run"]
        );
        assert_eq!(args.status_file, PathBuf::from("/data/maintenance.json"));
        assert_eq!(args.log_dir, PathBuf::from("/logs"));
    }

    #[test]
    fn hyphenated_child_arguments_are_not_claimed_by_the_launcher() {
        // `allow_hyphen_values` is what makes this work; without it clap
        // rejects the child's own flags as unknown arguments to us.
        let args = parse(&[
            "zaaheen-maintenance",
            "--status-file",
            "/s.json",
            "--log-dir",
            "/l",
            "--phi4-model",
            "/models/phi4.gguf",
        ]);
        assert_eq!(args.child_args, vec!["--phi4-model", "/models/phi4.gguf"]);
    }

    #[test]
    fn the_status_file_and_log_dir_are_required() {
        // Both are how a run reports for duty. A launcher that can start
        // without them would run nightly and report nothing, which is the
        // state ADR-SEC-016 exists to end.
        assert!(Args::try_parse_from(["zaaheen-maintenance", "--log-dir", "/l"]).is_err());
        assert!(Args::try_parse_from(["zaaheen-maintenance", "--status-file", "/s.json"]).is_err());
    }

    #[test]
    fn the_child_executable_defaults_to_our_own_sibling() {
        let args = parse(&[
            "zaaheen-maintenance",
            "--status-file",
            "/s.json",
            "--log-dir",
            "/l",
        ]);
        assert!(args.vault_cli.is_none(), "resolved at run time, not parsed");

        let resolved = sibling_vault_cli().expect("current_exe must resolve under test");
        assert_eq!(resolved.file_name().expect("file name"), VAULT_CLI_EXE);
        let own = std::env::current_exe().expect("current exe");
        assert_eq!(resolved.parent(), own.parent(), "must be a sibling");
    }

    #[test]
    fn a_spawn_failure_records_a_failed_run() {
        // The case only the parent can report: the child never started, so it
        // could not record anything itself.
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let status = tmp.path().join("maintenance.json");
        let args = Args {
            status_file: status.clone(),
            log_dir: tmp.path().join("logs"),
            vault_cli: Some(tmp.path().join("definitely-not-here")),
            child_args: vec![],
        };

        let result = run_child(&args.vault_cli.clone().expect("set above"), &args);
        assert!(result.is_err(), "spawning a missing executable must fail");

        // `main` is what records on this path; do the same here so the
        // assertion covers the behaviour rather than the call site.
        record(&status, &RunOutcome::Failed);
        let last = maintenance_state::load(&status)
            .last_run
            .expect("an outcome must be recorded");
        assert!(!last.ok);
        assert_eq!(last.summary, maintenance_state::OUTCOME_FAILED);
    }

    #[cfg(windows)]
    #[test]
    fn the_no_window_flag_is_the_documented_win32_constant() {
        // 0x08000000 is CREATE_NO_WINDOW. A wrong value here compiles, runs,
        // and silently reintroduces the window this binary exists to remove --
        // so pin the number itself.
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }
}
