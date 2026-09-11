#![forbid(unsafe_code)]
//! `vault-scheduler` — cross-platform OS-level task scheduling.
//!
//! Registers a **per-user** scheduled task with the operating system's own
//! scheduler so the vault can run its nightly maintenance
//! (`zaaheen consolidate run`, ADR-093) even when the desktop app is closed:
//!
//! - **Windows** — Task Scheduler (`schtasks`), current-user task, no
//!   elevation.
//! - **macOS** — a launchd LaunchAgent plist in `~/Library/LaunchAgents`.
//! - **Linux** — a systemd `--user` timer (cron fallback where systemd is
//!   absent).
//!
//! # Design (ADR-092)
//!
//! One [`Scheduler`] trait, one backend chosen at compile time per target OS.
//! The crate is a **leaf**: it depends only on `vault-core` for the shared
//! error catalogue and builds a self-contained [`ScheduleSpec`] into the
//! correct OS artefact. It never runs the maintenance itself — it only asks
//! the OS to run a given command on a schedule. Per-user tasks are deliberate:
//! registration needs no administrator/UAC elevation, so it can happen quietly
//! during onboarding.
//!
//! # Why the trait is synchronous
//!
//! Registering, querying, or removing a task is a short, bounded subprocess
//! call (`schtasks` / `launchctl` / `systemctl --user`) or a small file write
//! — not long-running I/O and not CPU-heavy. The methods are therefore
//! synchronous, and the async caller (the Tauri command layer) invokes them
//! inside `tokio::task::spawn_blocking`. That keeps this crate free of a tokio
//! dependency and matches BRD §2's rule that blocking work is sync, dispatched
//! via `spawn_blocking`, rather than pretending to be async.
//!
//! # Security (ADR-SEC-005)
//!
//! Every backend MUST call [`ScheduleSpec::validate`] before serialising a
//! spec, and MUST pass `program`/`args` to the OS as an argument vector rather
//! than a shell string. [`TaskId::new`] and [`ScheduleSpec::validate`] form
//! the injection-safety gate; per-backend escaping is defence in depth on top
//! of it. No secret is ever placed in a spec — the scheduled `zaaheen` reads
//! the master key from the OS keychain at run time.

mod backends;
mod error;
mod spec;

pub use backends::{platform_scheduler, start_on_demand};
pub use error::{SchedulerError, SchedulerResult};
pub use spec::{Frequency, OnDemandTask, ScheduleSpec, ScheduleStatus, TaskId};

/// Registers, queries, and removes a per-user OS-level scheduled task.
///
/// Backends are constructed per target OS (added in the platform-backend
/// phases). All methods are synchronous — see the crate-level docs for why —
/// and callers on an async runtime should invoke them via `spawn_blocking`.
pub trait Scheduler: Send + Sync {
    /// Register the scheduled task described by `spec`, replacing any existing
    /// task with the same [`TaskId`].
    ///
    /// Implementations MUST call [`ScheduleSpec::validate`] first and return
    /// its error unchanged on failure, so a spec that could break the OS
    /// serialisation is rejected before any OS call is made.
    fn register(&self, spec: &ScheduleSpec) -> SchedulerResult<()>;

    /// Remove the scheduled task with `task_id`.
    ///
    /// Idempotent: removing a task that is not registered is a success, so a
    /// caller can always "make sure it is gone" without first checking. This
    /// is what the uninstaller and the disable-maintenance path rely on.
    fn unregister(&self, task_id: &TaskId) -> SchedulerResult<()>;

    /// Report whether the OS currently has `task_id` registered.
    fn status(&self, task_id: &TaskId) -> SchedulerResult<ScheduleStatus>;
}
