//! Per-OS scheduler backends and the platform factory (ADR-092).
//!
//! Each backend module is compiled on **its own target OS, or in any test
//! build** (`cfg(any(<os>, test))`):
//!
//! - In a real build, only the native backend is present, so its pure builder
//!   is genuinely used by its shell-out `imp` and nothing is dead code.
//! - In a test build on any platform, all backend modules compile, so the pure
//!   builders (plist / XML / unit-file construction, the injection-safety-
//!   critical part) are unit-tested on every CI leg — including the ones that
//!   cannot exercise the live OS call. Only the `imp` struct that shells out is
//!   further gated to its exact target and never runs off it.
//!
//! [`platform_scheduler`] returns the backend for the current target, or
//! [`crate::SchedulerError::Unsupported`] where no backend exists yet.

use crate::error::SchedulerResult;
use crate::Scheduler;

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(any(target_os = "macos", test))]
mod macos;
#[cfg(any(windows, test))]
mod windows;

/// Construct the [`Scheduler`] backend for the current platform.
pub fn platform_scheduler() -> SchedulerResult<Box<dyn Scheduler>> {
    #[cfg(windows)]
    {
        Ok(Box::new(windows::WindowsScheduler))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::MacScheduler))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::LinuxScheduler))
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Err(crate::error::SchedulerError::Unsupported)
    }
}

/// Make sure the on-demand `task` exists with this exact definition, then ask
/// the OS to start it (ADR-102).
///
/// Idempotent and safe to repeat: an up-to-date task is not re-registered,
/// and a start request while an instance runs is dropped by the OS
/// (`IgnoreNew`). Windows only in V0.2; elsewhere the caller starts the
/// program itself.
///
/// # Errors
///
/// [`crate::SchedulerError::InvalidSpec`] from validation,
/// [`crate::SchedulerError::Unsupported`] off Windows, or the OS tool's
/// failure.
pub fn start_on_demand(task: &crate::OnDemandTask) -> SchedulerResult<()> {
    task.validate()?;
    #[cfg(windows)]
    {
        windows::start_on_demand(task)
    }
    #[cfg(not(windows))]
    {
        Err(crate::error::SchedulerError::Unsupported)
    }
}

/// Escape the five XML predefined entities, shared by the backends that emit
/// XML (Windows Task Scheduler definitions and macOS launchd plists).
///
/// `&` is replaced first so the ampersands introduced by the other four
/// replacements are not themselves re-escaped. Compiled on the two XML targets
/// or in test builds, matching its only callers.
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::xml_escape;

    #[test]
    fn xml_escape_encodes_all_five_entities() {
        assert_eq!(
            xml_escape(r#"a & b < c > d " e ' f"#),
            "a &amp; b &lt; c &gt; d &quot; e &apos; f"
        );
    }
}
