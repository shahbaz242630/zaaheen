//! The command an AI app runs to reach Zaaheen (ADR-111 + ADR-SEC-031
//! amendment 1, `CONNECT-APPS-DESIGN.md`).
//!
//! The installer puts its folder on the **user** `PATH`, but a program reads
//! `PATH` only when it starts: an AI app that was open before the install, or
//! one the freshly installed app opened for the person, cannot find a bare
//! `zaaheen` (session 64: Cursor, *"'zaaheen' is not recognized"*). So every
//! route (Cursor's link, the Claude extension, the copy-paste steps) names
//! `zaaheen.exe` by its full path, found beside the running program. The
//! person may choose the install folder, so the path is never assumed.
//!
//! A [`ServerCommand`] is either the short name or an absolute path whose
//! file name is the shipped program; nothing else can be made. It comes from
//! the operating system (`current_exe`), never from the page.

use std::fmt;
use std::path::{Path, PathBuf};

/// The short name the installer puts on `PATH`: what a developer build, or
/// an install whose own folder cannot be found, falls back to.
pub const SHORT_NAME: &str = "zaaheen";

/// The program's file name beside the desktop app.
#[cfg(windows)]
pub const PROGRAM_FILE: &str = "zaaheen.exe";
#[cfg(not(windows))]
pub const PROGRAM_FILE: &str = "zaaheen";

/// What an AI app is told to run, before its two fixed arguments
/// (`mcp serve`).
#[derive(Clone, PartialEq, Eq)]
pub struct ServerCommand(String);

impl ServerCommand {
    /// The installed program beside this executable when it is there,
    /// otherwise the short name.
    #[must_use]
    pub fn installed() -> Self {
        Self::beside(crate::install_paths::resource_dir().as_deref())
    }

    /// [`Self::installed`] for a given folder.
    #[must_use]
    pub fn beside(folder: Option<&Path>) -> Self {
        folder
            .map(|dir| dir.join(PROGRAM_FILE))
            .filter(|program| program.is_file())
            .and_then(|program| program.to_str().and_then(Self::parse))
            .unwrap_or_else(Self::short_name)
    }

    /// The short name, `zaaheen`.
    #[must_use]
    pub fn short_name() -> Self {
        Self(SHORT_NAME.to_owned())
    }

    /// The short name, or an absolute path to the shipped program with no
    /// control characters. `None` for anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text == SHORT_NAME {
            return Some(Self::short_name());
        }
        // No network share (`\\server\share`) and no verbatim `\\?\` form:
        // an installed program is on a local drive (independent review,
        // session 64: defence in depth, as the source is `current_exe`).
        if text.is_empty() || text.chars().any(char::is_control) || text.starts_with(r"\\") {
            return None;
        }
        let path = PathBuf::from(text);
        let names_the_program = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(PROGRAM_FILE));
        (path.is_absolute() && names_the_program).then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The server an MCP settings file gives: `{"command": .., "args":
    /// ["mcp", "serve"]}`.
    #[must_use]
    pub fn server_json(&self) -> serde_json::Value {
        serde_json::json!({ "command": self.0, "args": ["mcp", "serve"] })
    }
}

/// The path names the person's install folder, which may carry their user
/// name: shown only as "short name" or "full path" in a log.
impl fmt::Debug for ServerCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self.0 == SHORT_NAME {
            "short name"
        } else {
            "full path"
        };
        f.debug_tuple("ServerCommand").field(&kind).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn absolute_program() -> String {
        if cfg!(windows) {
            r"C:\Program Files\Zaaheen\zaaheen.exe".to_owned()
        } else {
            "/opt/zaaheen/zaaheen".to_owned()
        }
    }

    #[test]
    fn the_short_name_is_accepted() {
        assert_eq!(ServerCommand::parse("zaaheen").unwrap().as_str(), "zaaheen");
    }

    #[test]
    fn an_absolute_path_to_the_program_is_accepted() {
        let text = absolute_program();
        assert_eq!(ServerCommand::parse(&text).unwrap().as_str(), text);
    }

    #[test]
    fn anything_else_is_refused() {
        let other_program = if cfg!(windows) {
            r"C:\Windows\System32\cmd.exe"
        } else {
            "/bin/sh"
        };
        let lookalike = if cfg!(windows) {
            r"C:\Program Files\Zaaheen\zaaheen.exe.bat"
        } else {
            "/opt/zaaheen/zaaheen.sh"
        };
        let with_newline = format!("{}\n", absolute_program());
        for refused in [
            "",
            "zaaheen.exe",
            r"Zaaheen\zaaheen.exe",
            "./zaaheen",
            other_program,
            lookalike,
            &with_newline,
            "zaaheen mcp serve",
            r"\\server\share\zaaheen.exe",
            r"\\?\C:\Program Files\Zaaheen\zaaheen.exe",
        ] {
            assert!(
                ServerCommand::parse(refused).is_none(),
                "accepted {refused:?}"
            );
        }
    }

    #[test]
    fn with_no_program_beside_it_the_short_name_is_used() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            ServerCommand::beside(Some(empty.path())),
            ServerCommand::short_name()
        );
        assert_eq!(ServerCommand::beside(None), ServerCommand::short_name());
    }

    #[test]
    fn with_the_program_beside_it_its_full_path_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join(PROGRAM_FILE);
        std::fs::write(&program, b"").unwrap();
        let command = ServerCommand::beside(Some(dir.path()));
        assert_eq!(command.as_str(), program.to_str().unwrap());
    }

    #[test]
    fn a_folder_named_like_the_program_is_not_the_program() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(PROGRAM_FILE)).unwrap();
        assert_eq!(
            ServerCommand::beside(Some(dir.path())),
            ServerCommand::short_name()
        );
    }

    #[test]
    fn the_server_is_the_command_and_its_two_arguments() {
        let command = ServerCommand::parse(&absolute_program()).unwrap();
        assert_eq!(
            command.server_json(),
            serde_json::json!({ "command": absolute_program(), "args": ["mcp", "serve"] })
        );
    }

    #[test]
    fn debug_never_prints_the_path() {
        let shown = format!("{:?}", ServerCommand::parse(&absolute_program()).unwrap());
        assert!(
            !shown.contains("Zaaheen") && !shown.contains("opt"),
            "{shown}"
        );
        assert!(shown.contains("full path"));
    }
}
