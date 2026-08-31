//! Automatic-maintenance commands — the scheduler + the Phi-4 download.
//!
//! "Automatic maintenance" is the user-facing (ADR-086 white-label) name for
//! the nightly sleep consolidator. This module exposes:
//!
//! - [`ensure_maintenance_engine`] — first-run download of Phi-4 (the model the
//!   consolidation runs on). It downloads for every user during onboarding but
//!   does NOT gate onboarding: recall never needs Phi-4, only maintenance does.
//! - [`get_maintenance_schedule`] / [`set_maintenance_schedule`] — read and
//!   change the schedule. `set` registers (or removes) a per-user OS task via
//!   `vault-scheduler` that runs the bundled `vault-maintenance` runner
//!   (ADR-093, ADR-SEC-015), and persists the user's choice.
//! - [`run_maintenance_now`] — run a consolidation immediately by spawning the
//!   same bundled runner (one code path with the schedule).
//!
//! ## Why the desktop app never loads Phi-4 (ADR-093)
//!
//! Both the scheduled run and "Run now" launch the consolidation in its own
//! short-lived process. The desktop app therefore never loads the 2.5 GB model
//! into its own address space, and there is exactly one consolidation code
//! path. The subprocess is pointed at the desktop app's exact vault paths so it
//! operates on the same vault.
//!
//! ## Why the runner and not `vault-cli` directly (ADR-SEC-015)
//!
//! `vault-cli` is a console program, so Windows gives it a console window. A
//! task registered under `InteractiveToken` therefore opened a black terminal
//! on the user's desktop — which is exactly what happened to the founder at
//! login on 2026-08-27. `vault-maintenance` is linked for the Windows subsystem
//! and spawns `vault-cli` with `CREATE_NO_WINDOW`, so neither process shows a
//! window. Both entry points here go through it.
//!
//! ## Who records the outcome (ADR-SEC-016)
//!
//! Whoever *ran* maintenance, not whoever started it. `vault-cli` writes its
//! own counters once it has a report; the runner records a run that never got
//! that far. This module reads `maintenance.json`, and no longer writes a
//! `last_run` of its own — a scheduled run with the app closed must update the
//! same status the tab shows.
//!
//! ## Single-writer safety
//!
//! `vault-cli consolidate` takes the vault-owner lock. If an MCP agent is
//! connected (it holds that lock), the run fails fast with a "busy" signal,
//! which [`run_maintenance_now`] surfaces as [`ERR_MAINTENANCE_BUSY`] rather
//! than an error — maintenance simply runs at the next opportunity.
//!
//! ## Error shape (§11.7.2) + white-label (ADR-086)
//!
//! Commands return short stable codes, never a message built from an
//! underlying error; details go to the local log. Nothing user-visible names a
//! model, a repository, or a runtime.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use tauri::{Emitter, State};
use vault_app::maintenance_state::{self, LastRun, MaintenanceConfig, RunOutcome};
use vault_app::Application;
use vault_mcp::ToolInvokeDetails;
use vault_scheduler::{platform_scheduler, Frequency, ScheduleSpec, SchedulerError, TaskId};

use crate::model_fetch;

/// Stable OS task id for the maintenance schedule (safe charset per
/// `vault_scheduler::TaskId`).
pub const MAINTENANCE_TASK_ID: &str = "com.zaaheen.maintenance";

/// White-label description the OS scheduler may show (ADR-086).
pub const MAINTENANCE_LABEL: &str = "Zaaheen automatic maintenance";

/// The LANCE memory-pool env var the run needs (ADR-038). The Windows installer
/// sets it per-user (ADR-091) and Task Scheduler inherits it; we still put it in
/// the spec so macOS/Linux, whose backends inject env directly, are covered.
const LANCE_MEM_POOL_ENV: (&str, &str) = ("LANCE_MEM_POOL_SIZE", "268435456");

/// Progress event channel for the Phi-4 (maintenance-engine) download.
pub const MAINTENANCE_PROGRESS_EVENT: &str = "maintenance-engine://progress";

/// Opaque error codes (§11.7.2 — no internals cross the IPC boundary).
pub const ERR_MAINTENANCE_ENGINE_FAILED: &str = "maintenance_engine_unavailable";
/// Registering/removing the schedule failed.
pub const ERR_SCHEDULE_FAILED: &str = "maintenance_schedule_failed";
/// The vault was in use (an agent held the lock) — maintenance was skipped.
pub const ERR_MAINTENANCE_BUSY: &str = "maintenance_vault_busy";
/// A manual maintenance run failed for a reason other than busy.
pub const ERR_MAINTENANCE_FAILED: &str = "maintenance_run_failed";

/// Resolved paths the maintenance commands need to build the `vault-cli`
/// invocation. Constructed once in `main.rs` setup and managed as Tauri state.
pub struct MaintenanceContext {
    /// Absolute path to the bundled `vault-cli` executable.
    pub vault_cli: PathBuf,
    /// Absolute path to the bundled `vault-maintenance` executable — the
    /// windowless runner both the schedule and "Run now" go through
    /// (ADR-SEC-015).
    pub vault_maintenance: PathBuf,
    /// Directory the application log lives in, handed to the runner so a
    /// background run's diagnostics reach the same file as the app's.
    pub log_dir: PathBuf,
    /// The desktop app's SQLCipher metadata DB (`<data>/vault.db`).
    pub vault_db: PathBuf,
    /// The desktop app's LanceDB dir (`<data>/lance`).
    pub vector_dir: PathBuf,
    /// The desktop app's DuckDB graph file (`<data>/graph.duckdb`).
    pub graph_db: PathBuf,
    /// BGE embedder model.
    pub bge_model: PathBuf,
    /// BGE tokenizer.
    pub bge_tokenizer: PathBuf,
    /// ONNX Runtime dylib.
    pub ort_lib: PathBuf,
    /// Phi-4 GGUF (may not exist yet on a first run).
    pub phi4_model: PathBuf,
    /// Where the persisted schedule config lives (`<data>/maintenance.json`).
    pub config_path: PathBuf,
}

/// Build the full `vault-maintenance` argument vector (after the program) for
/// a consolidation run against the desktop app's own vault.
///
/// The runner's own two flags come first; everything after them is forwarded
/// to `vault-cli` verbatim (ADR-SEC-015). The runner appends `--record-status`
/// itself, so the status file is named exactly once, here.
fn consolidate_args(ctx: &MaintenanceContext) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    vec![
        "--status-file".into(),
        s(&ctx.config_path),
        "--log-dir".into(),
        s(&ctx.log_dir),
        "--vault-db".into(),
        s(&ctx.vault_db),
        "--vector-dir".into(),
        s(&ctx.vector_dir),
        "--graph-db".into(),
        s(&ctx.graph_db),
        "consolidate".into(),
        "--bge-model".into(),
        s(&ctx.bge_model),
        "--bge-tokenizer".into(),
        s(&ctx.bge_tokenizer),
        "--ort-lib".into(),
        s(&ctx.ort_lib),
        "--phi4-model".into(),
        s(&ctx.phi4_model),
        "run".into(),
    ]
}

/// Managed state for the first-run Phi-4 download. Mirrors
/// `engine::RecallEngineFetch`: a `OnceCell` dedupes concurrent callers onto one
/// transfer and leaves the cell cold on failure so a retry is possible.
pub struct MaintenanceEngineFetch {
    models_dir: PathBuf,
    once: tokio::sync::OnceCell<()>,
}

impl MaintenanceEngineFetch {
    /// Bind acquisition to `models_dir` (`<data>/models`).
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            once: tokio::sync::OnceCell::new(),
        }
    }

    /// Whether the Phi-4 acquisition has completed successfully this session.
    pub fn is_complete(&self) -> bool {
        self.once.initialized()
    }
}

/// Progress payload for [`MAINTENANCE_PROGRESS_EVENT`].
#[derive(Debug, Clone, Copy, Serialize)]
pub struct MaintenanceEngineProgress {
    /// Bytes transferred so far.
    pub downloaded_bytes: u64,
    /// Total bytes the acquisition covers.
    pub total_bytes: u64,
    /// Whole-number percent, pre-computed so the UI cannot divide by zero.
    pub percent: u8,
}

impl MaintenanceEngineProgress {
    fn new(downloaded_bytes: u64, total_bytes: u64) -> Self {
        let percent = if total_bytes == 0 {
            100
        } else {
            (downloaded_bytes.saturating_mul(100) / total_bytes).min(100) as u8
        };
        Self {
            downloaded_bytes,
            total_bytes,
            percent,
        }
    }
}

/// What the Maintenance tab renders.
#[derive(Debug, Clone, Serialize)]
pub struct ScheduleView {
    /// Whether automatic maintenance is turned on.
    pub enabled: bool,
    /// `"daily"` or `"weekly"`.
    pub frequency: String,
    /// Day of week for a weekly schedule (0-6).
    pub weekday: u8,
    /// Hour of day (0-23).
    pub hour: u8,
    /// Minute of hour (0-59).
    pub minute: u8,
    /// Whether the OS currently has the task registered.
    pub registered: bool,
    /// Whether the maintenance engine (Phi-4) is present and ready.
    pub engine_ready: bool,
    /// The most recent run's outcome, if any.
    pub last_run: Option<LastRun>,
}

/// Load the persisted config, falling back to the default when absent or
/// unreadable (a corrupt file must not brick the tab).
fn load_config(path: &Path) -> MaintenanceConfig {
    maintenance_state::load(path)
}

/// Persist the config atomically, mapping any failure to the opaque code the
/// IPC boundary allows (§11.7.2).
fn save_config(path: &Path, config: &MaintenanceConfig) -> Result<(), String> {
    maintenance_state::save(path, config).map_err(|e| {
        tracing::error!(error = %e, "could not persist the maintenance schedule");
        ERR_SCHEDULE_FAILED.to_string()
    })
}

/// Map a 0-6 index (Sunday-based) to a [`chrono::Weekday`].
fn weekday_from_index(index: u8) -> Result<chrono::Weekday, String> {
    use chrono::Weekday::*;
    Ok(match index {
        0 => Sun,
        1 => Mon,
        2 => Tue,
        3 => Wed,
        4 => Thu,
        5 => Fri,
        6 => Sat,
        _ => return Err(ERR_SCHEDULE_FAILED.to_string()),
    })
}

/// Parse a wire frequency + weekday index into a [`Frequency`].
fn parse_frequency(frequency: &str, weekday: u8) -> Result<Frequency, String> {
    match frequency {
        "daily" => Ok(Frequency::Daily),
        "weekly" => Ok(Frequency::Weekly {
            day: weekday_from_index(weekday)?,
        }),
        _ => Err(ERR_SCHEDULE_FAILED.to_string()),
    }
}

/// Build the `ScheduleSpec` for the current config. Validates the frequency and
/// time of day.
fn build_spec(
    ctx: &MaintenanceContext,
    config: &MaintenanceConfig,
) -> Result<ScheduleSpec, String> {
    let task_id = TaskId::new(MAINTENANCE_TASK_ID).map_err(|_| ERR_SCHEDULE_FAILED.to_string())?;
    let frequency = parse_frequency(&config.frequency, config.weekday)?;
    let time_of_day = chrono::NaiveTime::from_hms_opt(config.hour as u32, config.minute as u32, 0)
        .ok_or_else(|| ERR_SCHEDULE_FAILED.to_string())?;
    Ok(ScheduleSpec {
        task_id,
        label: MAINTENANCE_LABEL.to_string(),
        frequency,
        time_of_day,
        // The windowless runner, never `vault-cli` directly (ADR-SEC-015).
        // Pointing this at a console binary is what put an unexplained black
        // terminal on the founder's screen at login on 2026-08-27.
        program: ctx.vault_maintenance.clone(),
        args: consolidate_args(ctx),
        env: vec![(
            LANCE_MEM_POOL_ENV.0.to_string(),
            LANCE_MEM_POOL_ENV.1.to_string(),
        )],
    })
}

/// Re-register the maintenance task so an already-registered one points at the
/// current runner (ADR-SEC-015 amendment 1).
///
/// # The upgrade hole this closes
///
/// The task is registered in exactly one place — [`set_maintenance_schedule`],
/// which only runs when the user changes their schedule. Nothing re-registers
/// it on startup.
///
/// So an installer that replaces the files on disk does **not** change what
/// Windows has recorded. A user who had automatic maintenance on before
/// ADR-SEC-015 keeps a task pointing at `vault-cli.exe`, and keeps getting a
/// console window at login, forever — the fix would reach only brand-new
/// installs and anyone who happened to toggle their schedule off and on.
///
/// That is not a hypothetical: it is the founder's own machine, and it would
/// have been every beta tester who upgraded.
///
/// # Why unconditional re-registration rather than comparing first
///
/// Registration is idempotent (`schtasks /Create /F` replaces), and the
/// alternative needs the backend to report a registered task's program so we
/// can diff it — more surface, for a check that would end in the same call.
///
/// It also heals drift this comparison would not have thought to look for: the
/// stale task found in session 27 pointed into `target\debug`, inside a build
/// tree that gets wiped routinely.
///
/// The persisted config is the source of truth for the schedule, so rebuilding
/// the task from it is the correct resolution of any disagreement.
///
/// # Never fatal
///
/// Returns nothing and logs on failure. A task we could not refresh is a
/// console window at worst; an app that will not start because of one is an
/// outage.
pub fn heal_registered_task(ctx: &MaintenanceContext) {
    let config = load_config(&ctx.config_path);
    if !config.enabled {
        // Maintenance is off. Registering a task the user turned off would be
        // the app overriding an explicit choice.
        return;
    }

    let spec = match build_spec(ctx, &config) {
        Ok(spec) => spec,
        Err(_) => {
            tracing::warn!("could not rebuild the maintenance schedule; leaving the task as it is");
            return;
        }
    };

    match platform_scheduler().and_then(|s| s.register(&spec)) {
        Ok(()) => tracing::info!(
            program = %spec.program.display(),
            "maintenance task refreshed to the current runner"
        ),
        Err(e) => tracing::warn!(error = %e, "could not refresh the maintenance task"),
    }
}

/// Read back the summary the run just recorded, for the UI to display.
///
/// Reading the file rather than the child's stdout means the UI and the
/// Maintenance tab always show the same line, and that line is the
/// counters-only one built in `vault_app::maintenance_state` — never report
/// prose. Falls back to the generic success wording if the file could not be
/// read, because the run itself did succeed and saying otherwise would be a
/// lie about the vault's state.
fn recorded_summary(config_path: &Path) -> String {
    load_config(config_path)
        .last_run
        .map(|r| r.summary)
        .unwrap_or_else(|| "maintenance complete".to_string())
}

/// Download Phi-4 (the maintenance engine) if absent, reporting progress.
///
/// Non-gating: the frontend fires this during onboarding but never blocks
/// completion on it. Idempotent and deduped via the managed `OnceCell`.
#[tauri::command]
pub async fn ensure_maintenance_engine(
    app: tauri::AppHandle,
    state: State<'_, MaintenanceEngineFetch>,
) -> Result<(), String> {
    let models_dir = state.models_dir.clone();
    let result = state
        .once
        .get_or_try_init(|| async {
            tracing::info!("maintenance engine acquisition starting");
            model_fetch::ensure_phi4_with_progress(&models_dir, |p| {
                if let Err(e) = app.emit(
                    MAINTENANCE_PROGRESS_EVENT,
                    MaintenanceEngineProgress::new(p.downloaded_bytes, p.total_bytes),
                ) {
                    tracing::debug!(error = %e, "maintenance progress emit failed (no listener?)");
                }
            })
            .await
            .map(|_| ())
        })
        .await;

    match result {
        Ok(()) => {
            tracing::info!("maintenance engine ready");
            Ok(())
        }
        Err(e) => {
            tracing::error!(error = %e, "maintenance engine acquisition failed");
            Err(ERR_MAINTENANCE_ENGINE_FAILED.to_string())
        }
    }
}

/// Report the current schedule + engine readiness for the Maintenance tab.
#[tauri::command]
pub async fn get_maintenance_schedule(
    ctx: State<'_, MaintenanceContext>,
    engine: State<'_, MaintenanceEngineFetch>,
) -> Result<ScheduleView, String> {
    let config = load_config(&ctx.config_path);

    let task_id = TaskId::new(MAINTENANCE_TASK_ID).map_err(|_| ERR_SCHEDULE_FAILED.to_string())?;
    // The scheduler trait is synchronous — query the OS off the async runtime.
    let registered = tokio::task::spawn_blocking(move || {
        platform_scheduler()
            .and_then(|s| s.status(&task_id))
            .map(|s| s.registered)
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false);

    let engine_ready = engine.is_complete() || ctx.phi4_model.exists();

    Ok(ScheduleView {
        enabled: config.enabled,
        frequency: config.frequency,
        weekday: config.weekday,
        hour: config.hour,
        minute: config.minute,
        registered,
        engine_ready,
        last_run: config.last_run,
    })
}

/// Turn automatic maintenance on/off and set its schedule.
///
/// Registers (or removes) the per-user OS task and persists the choice. A
/// settings change per §11.9.1, so it writes an audit row.
#[tauri::command]
pub async fn set_maintenance_schedule(
    app: State<'_, Application>,
    ctx: State<'_, MaintenanceContext>,
    enabled: bool,
    frequency: String,
    weekday: u8,
    hour: u8,
    minute: u8,
) -> Result<(), String> {
    let start = Instant::now();

    let mut config = load_config(&ctx.config_path);
    config.enabled = enabled;
    config.frequency = frequency;
    config.weekday = weekday;
    config.hour = hour;
    config.minute = minute;

    let spec = build_spec(&ctx, &config)?;
    let task_id = spec.task_id.clone();

    // Register/unregister off the async runtime (the trait is synchronous).
    let join = tokio::task::spawn_blocking(move || -> Result<(), SchedulerError> {
        let scheduler = platform_scheduler()?;
        if enabled {
            scheduler.register(&spec)
        } else {
            scheduler.unregister(&task_id)
        }
    })
    .await;

    let scheduler_result: Result<(), String> = match join {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            tracing::error!(error = %e, "scheduler operation failed");
            Err(ERR_SCHEDULE_FAILED.to_string())
        }
        Err(e) => {
            tracing::error!(error = %e, "scheduler task panicked");
            Err(ERR_SCHEDULE_FAILED.to_string())
        }
    };

    // Persist only when the OS op succeeded, so the tab never claims a state the
    // OS does not actually hold.
    if scheduler_result.is_ok() {
        save_config(&ctx.config_path, &config)?;
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    let error_for_audit = scheduler_result.as_ref().err().map(|_| {
        vault_mcp::ToolInvokeError::from_vault_error(&vault_core::VaultError::Scheduler(
            "schedule change failed".to_string(),
        ))
    });
    let _ = app
        .adapter()
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "set_maintenance_schedule",
            duration_ms,
            result_count: u32::from(enabled),
            boundary_count: 0,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        })
        .await;

    scheduler_result
}

/// Run a consolidation immediately by spawning the bundled `vault-cli`.
///
/// Returns the run's summary on success, or a stable code — notably
/// [`ERR_MAINTENANCE_BUSY`] when the vault is in use (an agent holds the lock),
/// which the UI presents as "will run at the next opportunity" rather than a
/// failure. Records the outcome as the last run and writes an audit row (a
/// consolidation run per §11.9.1).
#[tauri::command]
pub async fn run_maintenance_now(
    app: State<'_, Application>,
    ctx: State<'_, MaintenanceContext>,
) -> Result<String, String> {
    let start = Instant::now();
    let args = consolidate_args(&ctx);

    // Through the windowless runner, exactly as the schedule does
    // (ADR-SEC-015): one code path, and no console window flashes over the app
    // when the user presses the button.
    let output = tokio::process::Command::new(&ctx.vault_maintenance)
        .args(&args)
        .env(LANCE_MEM_POOL_ENV.0, LANCE_MEM_POOL_ENV.1)
        .output()
        .await;

    // The run records its own outcome into `maintenance.json` (ADR-SEC-016) —
    // this command no longer writes it. That keeps one writer for the status
    // regardless of who started the run, and it is what stops the child's
    // stdout (which carries `summary_markdown` and contradiction reasoning,
    // both derived from memory content) reaching a plaintext file.
    let (result, result_count): (Result<String, String>, u32) = match output {
        Ok(out) if out.status.success() => (Ok(recorded_summary(&ctx.config_path)), 1),
        Ok(out) => {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stderr),
                String::from_utf8_lossy(&out.stdout)
            );
            match maintenance_state::classify_failure(&combined) {
                RunOutcome::Busy => {
                    tracing::warn!("maintenance run skipped: vault in use by another writer");
                    (Err(ERR_MAINTENANCE_BUSY.to_string()), 0)
                }
                _ => {
                    tracing::error!(status = %out.status, "maintenance run failed");
                    (Err(ERR_MAINTENANCE_FAILED.to_string()), 0)
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to spawn the maintenance runner");
            // The runner never started, so nothing recorded an outcome. Do it
            // here or the tab keeps showing the previous run forever.
            let _ = maintenance_state::record_run(
                &ctx.config_path,
                &RunOutcome::Failed,
                chrono::Utc::now().to_rfc3339(),
            );
            (Err(ERR_MAINTENANCE_FAILED.to_string()), 0)
        }
    };

    let duration_ms = start.elapsed().as_millis() as u64;
    let error_for_audit = result.as_ref().err().map(|_| {
        vault_mcp::ToolInvokeError::from_vault_error(&vault_core::VaultError::Consolidation(
            "maintenance run failed".to_string(),
        ))
    });
    let _ = app
        .adapter()
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "run_maintenance_now",
            duration_ms,
            result_count,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> MaintenanceContext {
        MaintenanceContext {
            // The real installed layout post-ADR-SEC-018: directory "Zaaheen",
            // binaries zaaheen.exe / zaaheen-maintenance.exe. A fixture that
            // names paths no build produces teaches the next reader the wrong
            // shape, even though nothing here touches the filesystem.
            vault_cli: PathBuf::from(r"C:\Program Files\Zaaheen\zaaheen.exe"),
            vault_maintenance: PathBuf::from(r"C:\Program Files\Zaaheen\zaaheen-maintenance.exe"),
            log_dir: PathBuf::from(r"C:\logs"),
            vault_db: PathBuf::from(r"C:\data\vault.db"),
            vector_dir: PathBuf::from(r"C:\data\lance"),
            graph_db: PathBuf::from(r"C:\data\graph.duckdb"),
            bge_model: PathBuf::from(r"C:\data\models\model.onnx"),
            bge_tokenizer: PathBuf::from(r"C:\data\models\tokenizer.json"),
            ort_lib: PathBuf::from(r"C:\data\onnxruntime.dll"),
            phi4_model: PathBuf::from(r"C:\data\models\phi4.gguf"),
            config_path: PathBuf::from(r"C:\data\maintenance.json"),
        }
    }

    #[test]
    fn consolidate_args_target_the_apps_own_vault_and_end_in_run() {
        let args = consolidate_args(&ctx());
        // The vault paths come first so the run hits the desktop app's vault.
        let joined = args.join(" ");
        assert!(joined.contains("--vault-db C:\\data\\vault.db"));
        assert!(joined.contains("--vector-dir C:\\data\\lance"));
        assert!(joined.contains("--graph-db C:\\data\\graph.duckdb"));
        assert!(joined.contains("consolidate"));
        assert!(joined.contains("--phi4-model C:\\data\\models\\phi4.gguf"));
        // The action is the last token.
        assert_eq!(args.last().map(String::as_str), Some("run"));
    }

    #[test]
    fn parse_frequency_maps_daily_and_weekly_and_rejects_junk() {
        assert_eq!(parse_frequency("daily", 0).unwrap(), Frequency::Daily);
        assert_eq!(
            parse_frequency("weekly", 0).unwrap(),
            Frequency::Weekly {
                day: chrono::Weekday::Sun
            }
        );
        assert_eq!(
            parse_frequency("weekly", 3).unwrap(),
            Frequency::Weekly {
                day: chrono::Weekday::Wed
            }
        );
        assert!(parse_frequency("hourly", 0).is_err());
        assert!(parse_frequency("weekly", 9).is_err());
    }

    #[test]
    fn build_spec_produces_a_valid_injection_safe_spec() {
        let config = MaintenanceConfig {
            hour: 3,
            minute: 30,
            ..Default::default()
        };
        let spec = build_spec(&ctx(), &config).unwrap();
        // The spec must pass the scheduler's own injection-safety gate.
        spec.validate().unwrap();
        assert_eq!(spec.task_id.as_str(), MAINTENANCE_TASK_ID);
        assert_eq!(
            spec.time_of_day,
            chrono::NaiveTime::from_hms_opt(3, 30, 0).unwrap()
        );
    }

    #[test]
    fn build_spec_rejects_an_impossible_time() {
        let config = MaintenanceConfig {
            hour: 25,
            ..Default::default()
        };
        assert!(build_spec(&ctx(), &config).is_err());
    }

    #[test]
    fn config_round_trips_through_json_and_defaults_are_sane() {
        let config = MaintenanceConfig {
            enabled: true,
            frequency: "weekly".to_string(),
            weekday: 2,
            hour: 4,
            minute: 15,
            last_run: Some(LastRun {
                finished_at: "2026-07-24T03:00:00Z".to_string(),
                ok: true,
                summary: "merged 3 duplicates".to_string(),
            }),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(
            serde_json::from_str::<MaintenanceConfig>(&json).unwrap(),
            config
        );

        // Default is off, daily, 3 AM — the safe pre-opt-in state.
        let default = MaintenanceConfig::default();
        assert!(!default.enabled);
        assert_eq!(default.frequency, "daily");
        assert_eq!(default.hour, 3);
    }

    #[test]
    fn missing_config_file_yields_the_default_not_a_panic() {
        let cfg = load_config(Path::new("/definitely/not/here/maintenance.json"));
        assert_eq!(cfg, MaintenanceConfig::default());
    }

    #[test]
    fn progress_percent_is_bounded() {
        assert_eq!(MaintenanceEngineProgress::new(0, 0).percent, 100);
        assert_eq!(MaintenanceEngineProgress::new(1, 4).percent, 25);
        assert_eq!(MaintenanceEngineProgress::new(999, 100).percent, 100);
    }

    #[test]
    fn error_codes_and_event_leak_no_stack_names() {
        for s in [
            ERR_MAINTENANCE_ENGINE_FAILED,
            ERR_SCHEDULE_FAILED,
            ERR_MAINTENANCE_BUSY,
            ERR_MAINTENANCE_FAILED,
            MAINTENANCE_PROGRESS_EVENT,
        ] {
            let lower = s.to_lowercase();
            for forbidden in [
                "phi", "gguf", "qwen", "llama", "cron", "schtasks", "launchd",
            ] {
                assert!(
                    !lower.contains(forbidden),
                    "'{s}' must not leak '{forbidden}'"
                );
            }
        }
    }

    /// ADR-SEC-015: the schedule must point at the windowless runner. If this
    /// ever goes back to `vault_cli`, every user gets an unexplained black
    /// console window at login — the defect the founder hit on 2026-08-27.
    #[test]
    fn the_registered_task_runs_the_windowless_runner() {
        let ctx = ctx();
        let config = MaintenanceConfig {
            enabled: true,
            ..MaintenanceConfig::default()
        };
        let spec = build_spec(&ctx, &config).expect("spec should build");
        assert_eq!(spec.program, ctx.vault_maintenance);
        assert_ne!(
            spec.program, ctx.vault_cli,
            "scheduling the console binary is what shows a window"
        );
    }

    /// The runner's own flags must lead, and the child's arguments follow
    /// untouched — that ordering is what `trailing_var_arg` on the runner
    /// depends on.
    #[test]
    fn the_runner_receives_its_own_flags_before_the_forwarded_arguments() {
        let ctx = ctx();
        let args = consolidate_args(&ctx);

        assert_eq!(args[0], "--status-file");
        assert_eq!(args[1], ctx.config_path.to_string_lossy());
        assert_eq!(args[2], "--log-dir");
        assert_eq!(args[3], ctx.log_dir.to_string_lossy());
        assert_eq!(args[4], "--vault-db", "the child's arguments follow");
        assert!(
            args.iter().any(|a| a == "run"),
            "the consolidation subcommand must still be forwarded"
        );
    }

    /// ADR-SEC-015 amendment 1: maintenance OFF means we touch nothing.
    ///
    /// Registering a task for a user who turned maintenance off would be the
    /// app overriding an explicit choice — a worse bug than the one the heal
    /// exists to fix.
    #[test]
    fn healing_does_nothing_when_maintenance_is_disabled() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mut ctx = ctx();
        ctx.config_path = tmp.path().join("maintenance.json");

        let disabled = MaintenanceConfig {
            enabled: false,
            ..MaintenanceConfig::default()
        };
        maintenance_state::save(&ctx.config_path, &disabled).expect("save");

        // No OS call is made, so this is safe to run anywhere including CI.
        heal_registered_task(&ctx);

        // The config is left exactly as the user set it.
        assert_eq!(load_config(&ctx.config_path), disabled);
    }

    /// The heal must rebuild the task from the user's persisted schedule, not
    /// from defaults — otherwise refreshing the program path would silently
    /// move a 22:30 weekly run to 03:00 daily.
    ///
    /// ⚠️ Exercises `build_spec`, **not** `heal_registered_task`, on purpose.
    /// The heal's next step is a real `schtasks /Create`: calling it with an
    /// enabled, valid config would register an actual scheduled task on the
    /// developer's machine and on every CI runner. Do not "improve" this test
    /// by calling the outer function.
    #[test]
    fn healing_rebuilds_the_spec_from_the_persisted_schedule() {
        let ctx = ctx();
        let config = MaintenanceConfig {
            enabled: true,
            frequency: "weekly".into(),
            weekday: 3,
            hour: 22,
            minute: 30,
            last_run: None,
        };

        let spec = build_spec(&ctx, &config).expect("spec should build");
        assert_eq!(spec.program, ctx.vault_maintenance, "the windowless runner");
        assert_eq!(
            spec.time_of_day,
            chrono::NaiveTime::from_hms_opt(22, 30, 0).expect("valid time")
        );
        assert_eq!(
            spec.frequency,
            Frequency::Weekly {
                day: chrono::Weekday::Wed
            }
        );
    }

    /// A corrupt or unbuildable config must not panic startup.
    #[test]
    fn healing_survives_a_config_it_cannot_turn_into_a_schedule() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mut ctx = ctx();
        ctx.config_path = tmp.path().join("maintenance.json");

        // `enabled` with an hour no clock has. `build_spec` rejects it; the
        // heal must log and move on rather than unwrapping.
        let impossible = MaintenanceConfig {
            enabled: true,
            hour: 99,
            ..MaintenanceConfig::default()
        };
        maintenance_state::save(&ctx.config_path, &impossible).expect("save");

        heal_registered_task(&ctx);
        // Reaching here without a panic is the assertion.
    }

    /// The status file is named once, by the app. The runner appends
    /// `--record-status` itself, so naming it here too would pass it twice.
    #[test]
    fn the_app_does_not_pass_record_status_itself() {
        let ctx = ctx();
        let args = consolidate_args(&ctx);
        assert!(
            !args.iter().any(|a| a == "--record-status"),
            "the runner owns this flag"
        );
    }
}
