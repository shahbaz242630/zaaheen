//! Checking a folder the person chose (ADR-105 L4): the onboarding's
//! "Change…" and a move's target.
//!
//! Everything this needs from the machine goes through [`CheckEnv`], so the
//! tests script every refusal; [`SystemEnv`] is the real one. No PowerShell:
//! it is often blocked by policy, slow, and flagged by antivirus (review
//! round 1). The cloud-sync check is best effort and the screen says so.

use std::io;
use std::path::{Component, Path, PathBuf};

use super::{pointer::is_plain_local, Homes};

/// The folder the vault goes into, inside the chosen one (L4.6): never mixed
/// with the person's own files, never a drive root.
pub const VAULT_SUBFOLDER: &str = "Zaaheen Memories";

/// Head-room over the vault's size when checking free space (L4.5).
const SPACE_MARGIN_PERCENT: u64 = 10;

/// Why a folder was refused. Each has its own plain message on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not an absolute local path (typed `\\server`, `\\?\`, a relative path).
    NotLocal,
    /// The folder is on another computer (a mapped network drive, a SUBST or
    /// junction to a share).
    Network,
    /// The folder cannot be reached (not there, or not readable).
    Unreachable,
    /// A drive's top level.
    DriveRoot,
    /// Windows' or a program's own folder.
    SystemFolder,
    /// Inside Zaaheen's own folders or the current vault.
    InsideAppFolders,
    /// Inside a folder a cloud app keeps in sync.
    CloudSynced,
    /// A "Zaaheen Memories" folder is already there.
    AlreadyExists,
    /// Zaaheen cannot write there.
    NotWritable,
    /// Not enough free space for the memories.
    NotEnoughSpace,
}

/// Something the person should be told about an accepted folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    /// Not on the Windows drive: only this computer, only while connected.
    OtherDrive,
    /// The cloud-sync check could not run; the person confirms instead.
    CloudUnchecked,
}

/// An accepted folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// Where the vault will go: `<chosen>\Zaaheen Memories`.
    pub target: PathBuf,
    pub notes: Vec<Note>,
}

/// What the check needs to know about this person's setup.
#[derive(Clone, Debug)]
pub struct CheckContext<'a> {
    pub homes: &'a Homes,
    /// The vault folder in use now, if any.
    pub current_vault: Option<&'a Path>,
    pub models_dir: &'a Path,
    /// Bytes the memories take (0 for a new, empty vault).
    pub vault_size: u64,
}

/// What the machine is asked.
pub trait CheckEnv {
    /// An environment variable naming a folder.
    fn folder_var(&self, name: &str) -> Option<PathBuf>;
    /// Every folder a sync app has registered with Windows; `None` when that
    /// list cannot be read (the check then notes it could not run).
    fn sync_roots(&self) -> Option<Vec<PathBuf>>;
    /// Folders Dropbox says it syncs.
    fn dropbox_roots(&self) -> Vec<PathBuf>;
    /// The path's final form on disk (drive letters, junctions, SUBST and
    /// mapped drives resolved).
    fn canonical(&self, p: &Path) -> io::Result<PathBuf>;
    fn exists(&self, p: &Path) -> io::Result<bool>;
    /// Create and remove a probe file in `dir`.
    fn probe_writable(&self, dir: &Path) -> io::Result<()>;
    fn available_space(&self, p: &Path) -> io::Result<u64>;
    /// This program's own folder.
    fn install_dir(&self) -> Option<PathBuf>;
}

/// Check `chosen` (L4). Returns where the vault would go and what to tell
/// the person, or why the folder cannot be used.
///
/// # Errors
///
/// The [`Refusal`] to show.
pub fn check_folder(
    chosen: &Path,
    ctx: &CheckContext<'_>,
    env: &dyn CheckEnv,
) -> Result<Checked, Refusal> {
    // 1. Absolute and local as typed.
    if !is_plain_local(chosen) {
        return Err(Refusal::NotLocal);
    }
    // 2. Its final form on disk; a share behind a letter shows up here.
    let canonical = env.canonical(chosen).map_err(|_| Refusal::Unreachable)?;
    if !is_plain_local(&canonical) {
        return Err(Refusal::Network);
    }
    // 3. Not a drive's top level.
    if canonical.parent().is_none() {
        return Err(Refusal::DriveRoot);
    }
    let inside = |root: &Path| match env.canonical(root) {
        Ok(root) => canonical.starts_with(root),
        Err(_) => false,
    };
    // 4. Not Windows' or a program's folder.
    let system_vars = [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
    ];
    let mut system: Vec<PathBuf> = system_vars
        .iter()
        .filter_map(|v| env.folder_var(v))
        .collect();
    system.extend(env.install_dir());
    if system.iter().any(|s| inside(s)) {
        return Err(Refusal::SystemFolder);
    }
    // 5. Not inside Zaaheen's own folders or the current vault.
    let mut own = vec![
        ctx.homes.local.clone(),
        ctx.homes.roaming.clone(),
        ctx.models_dir.to_path_buf(),
    ];
    own.extend(ctx.current_vault.map(Path::to_path_buf));
    if own.iter().any(|o| inside(o)) {
        return Err(Refusal::InsideAppFolders);
    }
    // 6. Not synced by a cloud app (best effort).
    let mut notes = Vec::new();
    let mut cloud: Vec<PathBuf> = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .filter_map(|v| env.folder_var(v))
        .collect();
    match env.sync_roots() {
        Some(roots) => cloud.extend(roots),
        None => notes.push(Note::CloudUnchecked),
    }
    cloud.extend(env.dropbox_roots());
    if let Some(profile) = env.folder_var("USERPROFILE") {
        cloud.extend(
            ["iCloudDrive", "Google Drive", "Box"]
                .iter()
                .map(|n| profile.join(n)),
        );
    }
    if let Some(root) = drive_root(&canonical) {
        // Google Drive for desktop's virtual drive holds "My Drive" at its
        // top (name from Google's documentation, not verified here).
        cloud.push(root.join("My Drive"));
    }
    if cloud.iter().any(|c| inside(c)) {
        return Err(Refusal::CloudSynced);
    }
    // 7. A new subfolder, not one already there.
    let target = canonical.join(VAULT_SUBFOLDER);
    if env.exists(&target).map_err(|_| Refusal::Unreachable)? {
        return Err(Refusal::AlreadyExists);
    }
    // 8. Writable.
    env.probe_writable(&canonical)
        .map_err(|_| Refusal::NotWritable)?;
    // 9. Room for the memories.
    let needed = ctx
        .vault_size
        .saturating_add(ctx.vault_size / 100 * SPACE_MARGIN_PERCENT);
    let free = env
        .available_space(&canonical)
        .map_err(|_| Refusal::Unreachable)?;
    if free < needed {
        return Err(Refusal::NotEnoughSpace);
    }
    // The USB note: anything not on the Windows drive. `SystemDrive` is
    // "C:", which on its own names the current folder on C:, so its root is
    // what is compared.
    let system_root = env.folder_var("SystemDrive").and_then(|d| {
        let mut root = d.into_os_string();
        root.push(std::path::MAIN_SEPARATOR_STR);
        env.canonical(Path::new(&root)).ok()
    });
    if drive_root(&canonical) != system_root.as_deref().and_then(drive_root) {
        notes.push(Note::OtherDrive);
    }
    Ok(Checked { target, notes })
}

/// The drive a path is on: its prefix and root (`C:\`), if it has one.
fn drive_root(p: &Path) -> Option<PathBuf> {
    let mut components = p.components();
    match (components.next(), components.next()) {
        (Some(prefix @ Component::Prefix(_)), Some(Component::RootDir)) => {
            let mut root = PathBuf::from(prefix.as_os_str());
            root.push(std::path::MAIN_SEPARATOR_STR);
            Some(root)
        }
        (Some(Component::RootDir), _) => Some(PathBuf::from("/")),
        _ => None,
    }
}

/// The real machine.
pub struct SystemEnv;

impl CheckEnv for SystemEnv {
    fn folder_var(&self, name: &str) -> Option<PathBuf> {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }

    fn sync_roots(&self) -> Option<Vec<PathBuf>> {
        sync_roots_from_registry()
    }

    fn dropbox_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        for var in ["LOCALAPPDATA", "APPDATA"] {
            let Some(base) = self.folder_var(var) else {
                continue;
            };
            if let Ok(text) = std::fs::read_to_string(base.join("Dropbox").join("info.json")) {
                roots.extend(dropbox_paths(&text));
            }
        }
        roots
    }

    fn canonical(&self, p: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(p)
    }

    fn exists(&self, p: &Path) -> io::Result<bool> {
        p.try_exists()
    }

    fn probe_writable(&self, dir: &Path) -> io::Result<()> {
        let probe = dir.join(format!(".zaaheen-probe-{}", std::process::id()));
        std::fs::write(&probe, b"")?;
        std::fs::remove_file(&probe)
    }

    fn available_space(&self, p: &Path) -> io::Result<u64> {
        fs4::available_space(p)
    }

    fn install_dir(&self) -> Option<PathBuf> {
        crate::install_paths::resource_dir()
    }
}

/// Every `UserSyncRoots` folder Windows' sync-root registry lists, for any
/// user — stricter than the current user's only (ADR-105 implementation
/// note). Read with `reg.exe` (no admin; the `icacls`/`schtasks`
/// precedent); `None` if it cannot run.
#[cfg(windows)]
fn sync_roots_from_registry() -> Option<Vec<PathBuf>> {
    const KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\SyncRootManager";
    let out = crate::keeper::acl::run_quiet("reg.exe", &["query".into(), KEY.into(), "/s".into()])
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_sync_roots(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(not(windows))]
fn sync_roots_from_registry() -> Option<Vec<PathBuf>> {
    Some(Vec::new())
}

/// The data of every `REG_SZ` value under a `…\UserSyncRoots` key in
/// `reg query /s` output.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn parse_sync_roots(output: &str) -> Vec<PathBuf> {
    let mut in_user_roots = false;
    let mut roots = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("HKEY_") {
            in_user_roots = trimmed.ends_with(r"\UserSyncRoots");
            continue;
        }
        if !in_user_roots {
            continue;
        }
        if let Some((_, data)) = trimmed.split_once("REG_SZ") {
            let data = data.trim();
            if !data.is_empty() {
                roots.push(PathBuf::from(data));
            }
        }
    }
    roots
}

/// The `"path"` values in Dropbox's `info.json`.
pub(crate) fn dropbox_paths(json: &str) -> Vec<PathBuf> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    value
        .as_object()
        .into_iter()
        .flat_map(|accounts| accounts.values())
        .filter_map(|account| account.get("path").and_then(|p| p.as_str()))
        .map(PathBuf::from)
        .collect()
}
