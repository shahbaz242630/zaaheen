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
//! vault lock — the contention class ADR-SEC-012 and then ADR-SEC-020 dealt
//! with.
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
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

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

/// Pause between attempts while the vault is busy (`--wait-if-busy-minutes`).
const BUSY_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Run a maintenance pass without showing a console window.
#[derive(Parser, Debug)]
#[command(
    name = "zaaheen-maintenance",
    about = "Runs Zaaheen maintenance in the background, with no window.",
    long_about = None
)]
struct Args {
    /// Where to record the run's outcome. Normally omitted: the runner finds
    /// `maintenance.json` inside the recorded vault folder itself (ADR-105
    /// L3), so a scheduled task never carries a path that a move would
    /// leave behind. Given explicitly by tests and development runs.
    #[arg(long, value_name = "PATH")]
    status_file: Option<PathBuf>,

    /// Directory for the application log.
    #[arg(long, value_name = "PATH")]
    log_dir: PathBuf,

    /// Start the vault keeper (ADR-102) instead of a maintenance run, and
    /// stay alive exactly as long as it does. Used by the keeper's on-demand
    /// Task Scheduler entry.
    #[arg(long, conflicts_with = "status_file")]
    keeper: bool,

    /// When the vault is busy, try again every five minutes for up to this
    /// many minutes before recording the night as skipped. The scheduled task
    /// passes it: an AI app left open overnight keeps the vault in use until
    /// its connection goes idle, and one refusal at 03:00 would otherwise cost
    /// the whole night's tidy-up. 0 (the default, and what "Run now" uses)
    /// means one attempt.
    #[arg(long, value_name = "MINUTES", default_value_t = 0)]
    wait_if_busy_minutes: u32,

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
                if let Some(status_file) = &args.status_file {
                    record(status_file, &RunOutcome::Failed);
                }
                return ExitCode::FAILURE;
            }
        },
    };

    if args.keeper {
        return match run_keeper(&vault_cli, &args.log_dir) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(e) => {
                tracing::error!(error = %e, "could not start the vault keeper");
                ExitCode::FAILURE
            }
        };
    }
    // ADR-105 L3: the status file lives in the recorded vault folder, found
    // here rather than passed in. A location that cannot be found is logged
    // and nothing runs: the child would find no vault either, and nothing may
    // be created in its place.
    let status_file = match args.status_file.clone() {
        Some(path) => path,
        None => match recorded_status_file() {
            Ok(path) => path,
            Err(e) => {
                tracing::error!(error = %e, "the vault's location could not be found; nothing was run");
                return ExitCode::FAILURE;
            }
        },
    };

    let budget = Duration::from_secs(u64::from(args.wait_if_busy_minutes) * 60);
    let started = Instant::now();
    loop {
        match run_child(&vault_cli, &args, &status_file) {
            Ok(None) => {
                // The child recorded its own counters on the way out
                // (ADR-SEC-016). Writing again here would overwrite a real
                // summary with a less informative one.
                tracing::info!("maintenance run completed");
                return ExitCode::SUCCESS;
            }
            Ok(Some(RunOutcome::Busy))
                if retry_fits(started.elapsed(), BUSY_RETRY_INTERVAL, budget) =>
            {
                // Not recorded: a skip is only recorded once the wait is
                // over, so the card never shows "skipped" for a night that
                // then ran.
                tracing::info!(
                    "the vault is in use (an AI app, or another run); trying again in five minutes"
                );
                std::thread::sleep(BUSY_RETRY_INTERVAL);
            }
            Ok(Some(outcome)) => {
                match outcome {
                    RunOutcome::Busy => {
                        tracing::warn!("maintenance skipped: the vault is in use by another writer")
                    }
                    _ => tracing::error!("maintenance run failed"),
                }
                record(&status_file, &outcome);
                return ExitCode::FAILURE;
            }
            Err(e) => {
                tracing::error!(error = %e, "could not start the maintenance run");
                record(&status_file, &RunOutcome::Failed);
                return ExitCode::FAILURE;
            }
        }
    }
}

/// `maintenance.json` in the recorded vault folder (ADR-105), setting the
/// location up first if this is the build's first run (the runner is one of
/// the processes allowed to, under the key lock).
fn recorded_status_file() -> Result<PathBuf, vault_core::VaultError> {
    let homes = vault_app::location::Homes::production()?;
    let key = vault_app::keychain::KeyLocation::production()?;
    let dir = vault_app::location::prepare(&homes, &key, &[])?;
    Ok(dir.path().join(maintenance_state::CONFIG_FILENAME))
}

/// Whether another attempt, `interval` from now, still starts inside the
/// `budget` measured from the first attempt.
fn retry_fits(elapsed: Duration, interval: Duration, budget: Duration) -> bool {
    elapsed.saturating_add(interval) <= budget
}

/// Keeper mode (ADR-102): start `zaaheen keeper` with no window and wait for
/// it.
///
/// - **Stays alive exactly as long as the keeper**, so Task Scheduler's
///   `IgnoreNew` sees a running instance and drops repeated start requests.
/// - **Holds the keeper's stdin open.** If this launcher is killed (by
///   `schtasks /End`, an installer, a logoff), the OS closes that pipe and the
///   keeper, started with `--exit-on-stdin-eof`, shuts down cleanly — a
///   parent-death signal with no platform-specific API.
/// - **Records nothing** in `maintenance.json`: a keeper is not a maintenance
///   run, and its exits must not appear on the Consolidation card.
/// - **No output buffering**: stdout and stderr go nowhere (the keeper logs to
///   the shared log file via `VAULT_LOG_DIR`), so a long-lived keeper never
///   accumulates output in this process's memory.
///
/// Returns whether the keeper exited successfully.
fn run_keeper(vault_cli: &Path, log_dir: &Path) -> std::io::Result<bool> {
    let mut command = Command::new(vault_cli);
    command
        .arg("keeper")
        .arg("--exit-on-stdin-eof")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(logging::LOG_DIR_ENV, log_dir);
    no_window(&mut command);

    let mut child = command.spawn()?;
    // Held, never written, until the keeper exits.
    let _keeper_stdin = child.stdin.take();
    let status = child.wait()?;
    if !status.success() {
        tracing::warn!(%status, "the vault keeper exited with an error");
    }
    Ok(status.success())
}

/// Spawn `vault-cli` once. `Ok(None)` when it succeeded (it has recorded its
/// own outcome); otherwise the classified failure, which the caller records —
/// or retries, when it is `Busy` and time remains.
fn run_child(
    vault_cli: &Path,
    args: &Args,
    status_file: &Path,
) -> std::io::Result<Option<RunOutcome>> {
    let mut command = Command::new(vault_cli);
    command
        .args(&args.child_args)
        // The child records its own counters, so it needs to know where.
        // Appended here rather than asked of the caller so there is exactly
        // one place that decides which file a run reports into.
        .arg("--record-status")
        .arg(status_file)
        .env(logging::LOG_DIR_ENV, &args.log_dir);
    no_window(&mut command);

    let output = command.output()?;
    if output.status.success() {
        return Ok(None);
    }

    // Classify from the child's output, then discard it. It is never stored:
    // see `maintenance_state::classify_failure`.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    tracing::debug!(status = %output.status, "maintenance child exited unsuccessfully");
    Ok(Some(maintenance_state::classify_failure(&combined)))
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
        assert_eq!(
            args.status_file,
            Some(PathBuf::from("/data/maintenance.json"))
        );
        assert_eq!(args.log_dir, PathBuf::from("/logs"));
        assert!(!args.keeper, "a maintenance run is not keeper mode");
    }

    #[test]
    fn keeper_mode_needs_only_the_log_dir() {
        // The keeper's Task Scheduler entry passes exactly this.
        let args = parse(&["zaaheen-maintenance", "--keeper", "--log-dir", "/logs"]);
        assert!(args.keeper);
        assert!(args.status_file.is_none());
        assert!(args.child_args.is_empty());
    }

    #[test]
    fn keeper_mode_and_a_status_file_cannot_be_combined() {
        // A keeper is not a maintenance run; letting it record into
        // maintenance.json would put keeper exits on the Consolidation card.
        assert!(Args::try_parse_from([
            "zaaheen-maintenance",
            "--keeper",
            "--status-file",
            "/s.json",
            "--log-dir",
            "/l",
        ])
        .is_err());
    }

    #[test]
    fn a_keeper_that_cannot_start_is_a_failure_not_a_hang() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let result = run_keeper(&tmp.path().join("definitely-not-here"), tmp.path());
        assert!(result.is_err(), "spawning a missing executable must fail");
        assert!(
            !tmp.path().join("maintenance.json").exists(),
            "keeper mode records nothing"
        );
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
    fn the_log_dir_is_required_and_the_status_file_is_found_not_passed() {
        // The log dir is how a run reports for duty (ADR-SEC-016). The
        // status file is no longer passed by the scheduled task (ADR-105 L3):
        // the runner finds it in the recorded vault folder, so a move never
        // leaves the task pointing at the old one.
        assert!(Args::try_parse_from(["zaaheen-maintenance", "--status-file", "/s.json"]).is_err());
        let args = parse(&[
            "zaaheen-maintenance",
            "--log-dir",
            "/l",
            "consolidate",
            "run",
        ]);
        assert!(args.status_file.is_none());
        assert_eq!(args.child_args, vec!["consolidate", "run"]);
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
            status_file: Some(status.clone()),
            log_dir: tmp.path().join("logs"),
            keeper: false,
            wait_if_busy_minutes: 0,
            vault_cli: Some(tmp.path().join("definitely-not-here")),
            child_args: vec![],
        };

        let result = run_child(&args.vault_cli.clone().expect("set above"), &args, &status);
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

    #[test]
    fn the_busy_wait_is_opt_in_and_parsed_before_the_child_arguments() {
        let args = parse(&[
            "zaaheen-maintenance",
            "--status-file",
            "/s.json",
            "--log-dir",
            "/l",
            "--wait-if-busy-minutes",
            "75",
            "consolidate",
            "run",
        ]);
        assert_eq!(args.wait_if_busy_minutes, 75);
        assert_eq!(args.child_args, vec!["consolidate", "run"]);

        let default = parse(&[
            "zaaheen-maintenance",
            "--status-file",
            "/s",
            "--log-dir",
            "/l",
        ]);
        assert_eq!(default.wait_if_busy_minutes, 0, "\"Run now\" tries once");
    }

    /// A 75-minute wait with 5-minute pauses: attempts at 0, 5, ..., 75 —
    /// never one that would start after the budget.
    #[test]
    fn retries_stop_once_the_next_attempt_would_start_after_the_budget() {
        let every = Duration::from_secs(5 * 60);
        let budget = Duration::from_secs(75 * 60);
        assert!(retry_fits(Duration::ZERO, every, budget));
        assert!(retry_fits(Duration::from_secs(70 * 60), every, budget));
        assert!(!retry_fits(Duration::from_secs(71 * 60), every, budget));
        assert!(
            !retry_fits(Duration::ZERO, every, Duration::ZERO),
            "no budget, no retry"
        );
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
