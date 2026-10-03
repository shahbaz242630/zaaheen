//! The erasure, run in the program that created the key (ADR-SEC-040).
//!
//! On a Mac the vault key lives in the login keychain, and only the program
//! that created an item may delete it: the bundled `zaaheen` (the keeper)
//! creates the key, and the desktop's own delete fails with "Invalid attempt
//! to change the owner of this item" (session 82, the GitHub Mac runner,
//! signed as shipped). So the desktop runs `zaaheen erase-vault` while it
//! holds the vault, and believes only the one report line that program
//! prints on success.
//!
//! **No time limit.** The steps inside are bounded (the key lock's wait, the
//! file removal); a keychain question, if macOS ever asked one, waits for the
//! person on screen. Killing the eraser part-way would leave the desktop
//! unable to say truthfully whether the key is gone, and the screen's failure
//! line promises "nothing was deleted". The screen shows "Still working"
//! after a minute instead.

use std::path::Path;
use std::process::Stdio;

use vault_app::ErasureReport;

/// The arguments `zaaheen` takes for the erasure.
pub(super) const ERASE_ARGS: &[&str] = &["erase-vault", "--yes-delete-everything"];

/// Run the eraser `program` and return its report. Its log lines go to the
/// shared log file in `log_dir`, as the keeper's do.
pub(super) async fn erase_in(program: &Path, log_dir: Option<&Path>) -> Result<ErasureReport, String> {
    let mut command = tokio::process::Command::new(program);
    command.args(ERASE_ARGS);
    if let Some(dir) = log_dir {
        command.env(vault_app::logging::LOG_DIR_ENV, dir);
    }
    report_of(command).await
}

/// Anything but a clean exit with exactly one report on stdout is a failed
/// erasure, with a reason for the log (never shown to the person, never
/// containing their data).
async fn report_of(mut command: tokio::process::Command) -> Result<ErasureReport, String> {
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("the eraser could not start: {e}"))?;
    if !output.status.success() {
        return Err(format!("the eraser failed ({})", output.status));
    }
    ErasureReport::from_output(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| "the eraser exited without a report".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell that prints `stdout` and exits with `code` (passed through
    /// the environment: cmd.exe and the argument quoting disagree on `"`).
    async fn run(stdout: &str, code: i32) -> Result<ErasureReport, String> {
        let mut command = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", "echo %ZR%& exit /b %ZC%"]);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", "printf '%s\\n' \"$ZR\"; exit \"$ZC\""]);
            c
        };
        command.env("ZR", stdout).env("ZC", code.to_string());
        report_of(command).await
    }

    const REPORT: &str = r#"{"key_destroyed":true,"entries_removed":4,"undeletable_count":1}"#;

    #[tokio::test]
    async fn a_clean_exit_with_one_report_is_the_erasure() {
        assert_eq!(
            run(REPORT, 0).await,
            Ok(ErasureReport {
                key_destroyed: true,
                entries_removed: 4,
                undeletable_count: 1,
            })
        );
    }

    /// A report is believed only with a clean exit: a failing eraser that
    /// printed one anyway did not erase.
    #[tokio::test]
    async fn a_failed_exit_is_a_failed_erasure_even_with_a_report() {
        assert!(run(REPORT, 1).await.is_err());
        assert!(run("x", 3).await.is_err());
    }

    #[tokio::test]
    async fn a_clean_exit_without_a_report_is_a_failed_erasure() {
        assert!(run("erased", 0).await.is_err());
        assert!(run("{}", 0).await.is_err());
    }

    #[tokio::test]
    async fn an_eraser_that_cannot_start_is_a_failed_erasure() {
        let missing = std::env::temp_dir().join("zaaheen-no-such-eraser-s83");
        let err = erase_in(&missing, None).await.unwrap_err();
        assert!(err.contains("could not start"), "{err}");
    }

    /// The arguments match what `zaaheen` accepts (vault-cli's parse test
    /// pins the other side).
    #[test]
    fn the_eraser_is_asked_with_its_confirmation_flag() {
        assert_eq!(ERASE_ARGS, ["erase-vault", "--yes-delete-everything"]);
    }
}
