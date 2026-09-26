//! Connecting an AI app to Zaaheen through the app's own install route
//! (ADR-106 + ADR-SEC-031, `CONNECT-APPS-DESIGN.md`).
//!
//! Zaaheen never writes another app's settings. It hands the app an install
//! request; the app asks the person, and writes its own settings in its own
//! way:
//! - **Cursor:** its documented install link (`external_link`).
//! - **Claude Desktop:** a desktop extension, "Zaaheen for Claude.mcpb",
//!   saved to the person's Downloads folder. It carries no program: its
//!   server is the Zaaheen already installed, by its full path (ADR-111;
//!   session 55 showed Claude runs either form). Opened
//!   straight away where Windows opens `.mcpb` files; otherwise the page
//!   says where to install it from in Claude's settings.

use std::fs::OpenOptions;
use std::io::{self, Cursor, Write};
use std::path::Path;

use crate::external_link::{ExternalLink, LinkError};
use crate::server_command::ServerCommand;

/// The Claude extension's file name, in the person's Downloads folder.
pub const CLAUDE_EXTENSION_FILE: &str = "Zaaheen for Claude.mcpb";

/// The one file the extension carries besides its manifest and icon: its
/// entry point, a note, because the program is the installed Zaaheen.
const CLAUDE_ENTRY_POINT: &str = "server/uses-the-installed-zaaheen.txt";
const CLAUDE_ENTRY_NOTE: &str = "This extension runs the Zaaheen app installed on this \
     computer. It carries no program of its own.\n";

/// What asking an app to install Zaaheen did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectOutcome {
    /// The app was handed the request and asks the person itself.
    Asked,
    /// The Claude extension is saved; the person installs it from Claude's
    /// settings (Windows does not open `.mcpb` files here).
    Saved,
    /// The app does not seem to be installed on this computer: nothing was
    /// opened or saved.
    AppNotFound,
    /// The Claude extension could not be saved.
    CouldNotSave,
    /// The app could not be started.
    CouldNotOpen,
}

impl ConnectOutcome {
    /// The stable code the desktop's page words.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            ConnectOutcome::Asked => "asked",
            ConnectOutcome::Saved => "saved",
            ConnectOutcome::AppNotFound => "app_not_found",
            ConnectOutcome::CouldNotSave => "could_not_save",
            ConnectOutcome::CouldNotOpen => "could_not_open",
        }
    }
}

// ── Cursor ────────────────────────────────────────────────────────────────

/// Ask Cursor to install Zaaheen. Blocking (it asks Windows first).
#[must_use]
pub fn connect_cursor() -> ConnectOutcome {
    connect_cursor_with(
        || scheme_is_registered("cursor"),
        &ServerCommand::installed(),
        ExternalLink::open,
    )
}

/// [`connect_cursor`] with the installed check, the command and the opener
/// given.
pub(crate) fn connect_cursor_with(
    installed: impl FnOnce() -> bool,
    command: &ServerCommand,
    open: impl FnOnce(&ExternalLink) -> Result<(), LinkError>,
) -> ConnectOutcome {
    // Without Cursor, Windows would offer to find an app for the link in
    // the Store: a confusing window, so nothing is opened at all.
    if !installed() {
        return ConnectOutcome::AppNotFound;
    }
    let Ok(link) = ExternalLink::install_in_cursor(command) else {
        return ConnectOutcome::CouldNotOpen;
    };
    match open(&link) {
        Ok(()) => ConnectOutcome::Asked,
        Err(_) => ConnectOutcome::CouldNotOpen,
    }
}

// ── Claude Desktop ────────────────────────────────────────────────────────

/// Save the Claude extension to `downloads` and, where Windows opens `.mcpb`
/// files, open it so Claude asks the person. `icon_png` is the app's icon.
/// Blocking.
#[must_use]
pub fn connect_claude(downloads: Option<&Path>, icon_png: &[u8]) -> ConnectOutcome {
    connect_claude_with(
        || scheme_is_registered("claude"),
        downloads,
        &ServerCommand::installed(),
        icon_png,
        || file_type_is_registered(".mcpb"),
        |file| {
            open::that(file).map_err(|e| {
                tracing::warn!(target: "vault_app::connect", error = %e, "could not open the Claude extension");
            })
        },
    )
}

/// [`connect_claude`] with the checks and the opener given.
pub(crate) fn connect_claude_with(
    installed: impl FnOnce() -> bool,
    downloads: Option<&Path>,
    command: &ServerCommand,
    icon_png: &[u8],
    opens_mcpb: impl FnOnce() -> bool,
    open: impl FnOnce(&Path) -> Result<(), ()>,
) -> ConnectOutcome {
    if !installed() {
        return ConnectOutcome::AppNotFound;
    }
    let Some(downloads) = downloads else {
        tracing::warn!(target: "vault_app::connect", "no Downloads folder to save the Claude extension in");
        return ConnectOutcome::CouldNotSave;
    };
    let file = downloads.join(CLAUDE_EXTENSION_FILE);
    let saved = claude_extension(icon_png, command).and_then(|bytes| save(&file, &bytes));
    if let Err(e) = saved {
        tracing::warn!(target: "vault_app::connect", error = %e, "could not save the Claude extension");
        return ConnectOutcome::CouldNotSave;
    }
    if !opens_mcpb() {
        return ConnectOutcome::Saved;
    }
    // Saved either way: if it will not open, the page says where to install
    // it from.
    match open(&file) {
        Ok(()) => ConnectOutcome::Asked,
        Err(()) => ConnectOutcome::Saved,
    }
}

/// Open the folder the Claude extension was saved in ("Show the file").
/// `false` when Explorer could not be started; the reason goes to the log.
#[must_use]
pub fn show_downloads(downloads: &Path) -> bool {
    match open::that(downloads) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(target: "vault_app::connect", error = %e, "could not show the Downloads folder");
            false
        }
    }
}

/// "Zaaheen for Claude.mcpb": a desktop-extension bundle (MCPB manifest
/// 0.3) whose server is `command` with `mcp serve`, the manual
/// snippet's server. Stored, not compressed, with a fixed date, so the same
/// icon always makes the same bytes.
///
/// # Errors
///
/// Only if the zip cannot be written in memory.
pub fn claude_extension(icon_png: &[u8], command: &ServerCommand) -> io::Result<Vec<u8>> {
    let manifest = serde_json::json!({
        "manifest_version": "0.3",
        "name": "zaaheen",
        "display_name": "Zaaheen",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "One memory for all your AI apps, kept safe on your computer. \
                        Uses the Zaaheen app installed on this computer.",
        "author": { "name": "Zaaheen" },
        "icon": "icon.png",
        "server": {
            "type": "binary",
            "entry_point": CLAUDE_ENTRY_POINT,
            "mcp_config": command.server_json()
        },
        "compatibility": { "platforms": ["win32"] }
    });
    let manifest = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default());
    let mut bundle = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in [
        ("manifest.json", manifest.as_slice()),
        ("icon.png", icon_png),
        (CLAUDE_ENTRY_POINT, CLAUDE_ENTRY_NOTE.as_bytes()),
    ] {
        bundle.start_file(name, options).map_err(io::Error::other)?;
        bundle.write_all(bytes)?;
    }
    Ok(bundle.finish().map_err(io::Error::other)?.into_inner())
}

/// Write `bytes` to `file` whole or not at all: to `<file>.part` first,
/// then renamed over any earlier copy (an older extension of ours).
fn save(file: &Path, bytes: &[u8]) -> io::Result<()> {
    let part = file.with_extension("mcpb.part");
    match std::fs::remove_file(&part) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let result = write_new(&part, bytes).and_then(|()| std::fs::rename(&part, file));
    if result.is_err() {
        // Best effort: a stray part-file is ours and harmless.
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// A new file (never over an existing one), flushed to disk.
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut out = OpenOptions::new().write(true).create_new(true).open(path)?;
    out.write_all(bytes)?;
    out.sync_all()
}

// ── what Windows knows ────────────────────────────────────────────────────

/// Whether a link scheme is registered (`HKCR\<scheme>` with "URL
/// Protocol"): Cursor's installer registers `cursor:`, Claude's (Store and
/// direct) `claude:` (measured on the founder's machine, session 55).
/// `scheme` is always a fixed name from this module.
#[cfg(windows)]
fn scheme_is_registered(scheme: &str) -> bool {
    registry_has(&format!(r"HKCR\{scheme}"), Some("URL Protocol"))
}

/// Whether Windows opens a file type with some program (`HKCR\<.ext>`).
#[cfg(windows)]
fn file_type_is_registered(extension: &str) -> bool {
    registry_has(&format!(r"HKCR\{extension}"), None)
}

/// `reg.exe query` of a fixed key (and value): no window, no admin, and
/// nothing but its exit status is used.
#[cfg(windows)]
fn registry_has(key: &str, value: Option<&str>) -> bool {
    let mut args: Vec<std::ffi::OsString> = vec!["query".into(), key.into()];
    match value {
        Some(value) => args.extend(["/v".into(), value.into()]),
        None => args.push("/ve".into()),
    }
    crate::keeper::acl::run_quiet("reg.exe", &args)
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// V0.2's desktop is Windows-only: elsewhere no app is found.
#[cfg(not(windows))]
fn scheme_is_registered(_scheme: &str) -> bool {
    false
}

#[cfg(not(windows))]
fn file_type_is_registered(_extension: &str) -> bool {
    false
}

#[cfg(test)]
mod tests;
