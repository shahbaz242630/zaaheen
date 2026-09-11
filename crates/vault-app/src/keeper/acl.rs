//! Restrict the vault folder to its owner (ADR-SEC-019 §2).
//!
//! **Why.** On the founder's machine `%APPDATA%` — and so the vault folder —
//! grants read access to a second local principal (`CodexSandboxUsers`, a
//! group another app created) by inheritance. The vault's data files are
//! encrypted, so that exposed ciphertext only; but read access is enough to
//! open `.vault.lock` and take the lock (`LockFileEx` needs only read access),
//! which would lock the owner out of their own vault, and enough to read
//! whatever else lands in the folder. Defence in depth: the folder should be
//! the user's alone.
//!
//! **Windows:** grant full control to the user's SID, SYSTEM and
//! Administrators FIRST, check it succeeded, and only THEN remove inherited
//! entries. The reverse order can leave the owner without access if the grant
//! fails. SIDs, never account names: names are localised (`Administratoren`)
//! and a bare user name can resolve to the wrong account. Exit codes only —
//! `icacls` output text is localised too.
//!
//! **POSIX:** `chmod 0700` on the vault root.
//!
//! Runs once per vault (a marker file records it) and BEFORE any lockfile is
//! opened, so an attacker cannot hold the lock to stop it from running.
//! Handles opened before hardening survive until the holder closes them (on
//! Windows, until logoff at the latest); this cannot be helped from user mode.

use std::path::Path;

/// Marker written after a successful hardening. Declared in `VAULT_ENTRIES`.
pub const ACL_MARKER: &str = ".acl-v1";

/// What happened.
#[derive(Debug, PartialEq, Eq)]
pub enum AclOutcome {
    AlreadyHardened,
    Hardened,
}

/// Why hardening did not complete. The vault still works; callers log this.
#[derive(Debug)]
pub enum AclError {
    Io(std::io::Error),
    /// A system tool exited non-zero or printed something unusable.
    Tool(&'static str),
}

impl std::fmt::Display for AclError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "vault folder permissions: {e}"),
            Self::Tool(what) => write!(f, "vault folder permissions: {what} failed"),
        }
    }
}

impl std::error::Error for AclError {}

impl From<std::io::Error> for AclError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Harden `vault_root` if not already done.
///
/// # Errors
///
/// [`AclError`] when a step fails. Nothing is removed unless the grants
/// succeeded first.
pub fn harden_vault_dir(vault_root: &Path) -> Result<AclOutcome, AclError> {
    let marker = vault_root.join(ACL_MARKER);
    if marker.exists() {
        return Ok(AclOutcome::AlreadyHardened);
    }
    restrict(vault_root)?;
    std::fs::write(&marker, b"")?;
    Ok(AclOutcome::Hardened)
}

#[cfg(windows)]
fn restrict(vault_root: &Path) -> Result<(), AclError> {
    const SYSTEM_SID: &str = "S-1-5-18";
    const ADMINISTRATORS_SID: &str = "S-1-5-32-544";

    let user_sid = current_user_sid()?;
    let root = vault_root.as_os_str();

    // 1. Grants first. `:r` replaces any existing explicit grant for these SIDs.
    let grant = run_quiet(
        "icacls.exe",
        &[
            root.to_os_string(),
            "/grant:r".into(),
            format!("*{user_sid}:(OI)(CI)F").into(),
            format!("*{SYSTEM_SID}:(OI)(CI)F").into(),
            format!("*{ADMINISTRATORS_SID}:(OI)(CI)F").into(),
        ],
    )?;
    if !grant.status.success() {
        return Err(AclError::Tool("icacls grant"));
    }

    // 2. Only now stop inheriting from %APPDATA% (which is where the extra
    //    principals come from).
    let strip = run_quiet(
        "icacls.exe",
        &[root.to_os_string(), "/inheritance:r".into()],
    )?;
    if !strip.status.success() {
        return Err(AclError::Tool("icacls inheritance"));
    }
    Ok(())
}

/// The current user's SID, from `whoami /user /fo csv /nh`, whose single line
/// is `"DOMAIN\user","S-1-5-21-..."`. Also names the per-user keeper task, so
/// two Windows users on one PC never share one.
///
/// # Errors
///
/// `whoami` failed or printed something that is not a SID.
#[cfg(windows)]
pub fn current_user_sid() -> Result<String, AclError> {
    let out = run_quiet(
        "whoami.exe",
        &["/user".into(), "/fo".into(), "csv".into(), "/nh".into()],
    )?;
    if !out.status.success() {
        return Err(AclError::Tool("whoami"));
    }
    parse_whoami_sid(&String::from_utf8_lossy(&out.stdout)).ok_or(AclError::Tool("whoami parse"))
}

/// Run a system tool with no window, no stdin, and output captured rather
/// than inherited. Inheriting stdout here would be a real bug: the relay's
/// stdout is the MCP channel to the agent.
#[cfg(windows)]
fn run_quiet(program: &str, args: &[std::ffi::OsString]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let system_dir = std::env::var_os("SystemRoot")
        .map(|root| Path::new(&root).join("System32"))
        .unwrap_or_else(|| Path::new(r"C:\Windows\System32").to_path_buf());
    std::process::Command::new(system_dir.join(program))
        .args(args)
        .stdin(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

#[cfg(not(windows))]
fn restrict(vault_root: &Path) -> Result<(), AclError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(vault_root, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Extract the SID from whoami's CSV line: the last quoted field. Only the
/// characters a SID can contain are accepted.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_whoami_sid(line: &str) -> Option<String> {
    let last = line.trim().rsplit(',').next()?.trim().trim_matches('"');
    let valid = last.starts_with("S-1-")
        && last.len() <= 184
        && last
            .bytes()
            .all(|c| c.is_ascii_digit() || c == b'-' || c == b'S');
    valid.then(|| last.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn the_sid_is_the_last_csv_field() {
        assert_eq!(
            parse_whoami_sid("\"shahbaz\\shahb\",\"S-1-5-21-1111-2222-3333-1001\"\r\n"),
            Some("S-1-5-21-1111-2222-3333-1001".to_string())
        );
    }

    #[test]
    fn anything_that_is_not_a_sid_is_refused() {
        for bad in [
            "",
            "\"user\",\"not-a-sid\"",
            "\"user\",\"S-1-5-21-12;rm\"",
            "\"user\",\"S-1-5-21-12 & calc\"",
        ] {
            assert_eq!(parse_whoami_sid(bad), None, "{bad:?}");
        }
    }

    /// Hardening a real directory succeeds, leaves it usable by its owner, and
    /// runs only once.
    #[test]
    fn hardening_keeps_the_owner_in_and_runs_once() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(harden_vault_dir(tmp.path()).unwrap(), AclOutcome::Hardened);

        let probe = tmp.path().join("probe.txt");
        std::fs::write(&probe, b"still mine").expect("owner can still write");
        assert_eq!(std::fs::read(&probe).unwrap(), b"still mine");

        assert_eq!(
            harden_vault_dir(tmp.path()).unwrap(),
            AclOutcome::AlreadyHardened
        );
    }

    #[cfg(unix)]
    #[test]
    fn posix_hardening_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        harden_vault_dir(tmp.path()).unwrap();
        let mode = std::fs::metadata(tmp.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
