//! How the desktop starts a keeper (ADR-108 D4).
//!
//! On Windows: the same per-user, on-demand Task Scheduler entry the AI apps'
//! relays use, built from the same shared pieces
//! (`vault_app::keeper::keeper_task_args` and friends), so the two can never
//! register different tasks. If Task Scheduler cannot start it, the desktop
//! starts the windowless launcher itself: unlike a relay, a GUI holds none of
//! an AI app's pipe handles, so a direct child inherits nothing that matters
//! (the session-35 reviewers' must-fix 4). The launcher then starts the
//! keeper and lives as long as it. Elsewhere there is no task: the launcher
//! is started directly.

use std::path::{Path, PathBuf};

use vault_app::keeper::relay::KeeperStarter;

/// The desktop's starter.
pub struct DesktopStarter;

impl KeeperStarter for DesktopStarter {
    fn request_start(&self) -> std::io::Result<()> {
        let program = launcher()?;
        let log_dir = vault_app::install_paths::log_dir()
            .ok_or_else(|| std::io::Error::other("the log folder could not be found"))?;
        start(&program, &vault_app::keeper::keeper_task_args(&log_dir))
    }
}

/// The windowless launcher, beside this executable.
fn launcher() -> std::io::Result<PathBuf> {
    vault_app::install_paths::resource_dir()
        .map(|dir| dir.join(vault_app::keeper::LAUNCHER_EXE))
        .ok_or_else(|| std::io::Error::other("the install folder could not be found"))
}

/// Task Scheduler first, a direct start second.
#[cfg(windows)]
fn start(program: &Path, args: &[String]) -> std::io::Result<()> {
    let sid = vault_app::keeper::acl::current_user_sid()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let task_id =
        vault_scheduler::TaskId::new(format!("{}{sid}", vault_app::keeper::KEEPER_TASK_ID_PREFIX))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    let task = vault_scheduler::OnDemandTask {
        task_id,
        label: vault_app::keeper::KEEPER_TASK_LABEL.to_string(),
        program: program.to_path_buf(),
        args: args.to_vec(),
    };
    match vault_scheduler::start_on_demand(&task) {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::warn!(target: "vault_tauri::keeper_start", error = %e, "the keeper task could not start it; starting the launcher directly");
            start_directly(program, args)
        }
    }
}

#[cfg(not(windows))]
fn start(program: &Path, args: &[String]) -> std::io::Result<()> {
    start_directly(program, args)
}

/// Start the launcher with no window and no inherited stdio.
#[cfg(windows)]
fn start_directly(program: &Path, args: &[String]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let spawn = |flags: u32| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    // Out of the desktop's job if the job allows it, so closing the window
    // does not end the keeper; a job that forbids breaking away refuses with
    // "access denied", and then it starts inside the job (review A R2-6).
    let child = match spawn(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => spawn(CREATE_NO_WINDOW),
        other => other,
    }?;
    // Not waited for: the launcher outlives this window by design.
    drop(child);
    Ok(())
}

#[cfg(not(windows))]
fn start_directly(program: &Path, args: &[String]) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
