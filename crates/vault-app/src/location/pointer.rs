//! The pointer file (ADR-105 L1): `vault-location.json` in the never-roaming
//! local data folder, naming the vault folder and its random ID.
//!
//! Written atomically (temp file, then rename), so a reader sees either the
//! old record or the new one, never half of either.

use std::io;
use std::path::{Component, Path, PathBuf, Prefix};

use serde::{Deserialize, Serialize};

/// File name of the pointer, in the local data folder.
pub const POINTER_FILE: &str = "vault-location.json";

/// The only format this build reads or writes.
const POINTER_VERSION: u32 = 1;

/// Length, in hex characters, of a vault or move ID (16 random bytes).
pub(crate) const ID_HEX_LEN: usize = 32;

/// A move recorded but not yet committed (ADR-105 L5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingMove {
    /// The new vault folder (`<chosen>\Zaaheen Memories`).
    pub to: PathBuf,
    /// Written as `.move-id` inside `to`; recovery deletes only a folder
    /// carrying it.
    pub move_id: String,
    /// Failed attempts so far; after two the move is given up.
    #[serde(default)]
    pub attempts: u32,
}

/// The old folder of a committed move, still to be cleaned (ADR-105 L5 M6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingCleanup {
    pub from: PathBuf,
    pub move_id: String,
}

/// The pointer's contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pointer {
    pub version: u32,
    pub vault_dir: PathBuf,
    pub vault_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_move: Option<PendingMove>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_cleanup: Option<PendingCleanup>,
}

impl Pointer {
    /// A pointer to `vault_dir` with `vault_id`, nothing pending.
    pub fn new(vault_dir: PathBuf, vault_id: String) -> Self {
        Self {
            version: POINTER_VERSION,
            vault_dir,
            vault_id,
            pending_move: None,
            pending_cleanup: None,
        }
    }

    /// Whether this pointer is one this build may act on: the right version,
    /// a well-formed ID, and plain local paths throughout.
    pub(crate) fn is_well_formed(&self) -> bool {
        self.version == POINTER_VERSION
            && is_id(&self.vault_id)
            && is_plain_local(&self.vault_dir)
            && self
                .pending_move
                .as_ref()
                .map_or(true, |m| is_plain_local(&m.to) && is_id(&m.move_id))
            && self
                .pending_cleanup
                .as_ref()
                .map_or(true, |c| is_plain_local(&c.from) && is_id(&c.move_id))
    }
}

/// 32 lowercase hex characters.
pub(crate) fn is_id(s: &str) -> bool {
    s.len() == ID_HEX_LEN
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A fresh random ID.
pub(crate) fn new_id() -> io::Result<String> {
    let mut bytes = [0u8; ID_HEX_LEN / 2];
    getrandom::getrandom(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(hex::encode(bytes))
}

/// Absolute, no `..`, and not a network or device path (`\\server\share`,
/// `\\?\UNC\…`, `\\.\…`). A drive-letter path in verbatim form (`\\?\C:\…`)
/// is accepted: it is what `canonicalize` returns on Windows.
pub(crate) fn is_plain_local(p: &Path) -> bool {
    if !p.is_absolute() {
        return false;
    }
    let mut components = p.components();
    if let Some(Component::Prefix(prefix)) = components.next() {
        if !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) {
            return false;
        }
    }
    p.components().all(|c| !matches!(c, Component::ParentDir))
}

/// Read the pointer. `Ok(None)`: there is none. A file that is not a
/// well-formed pointer is an error, never "none" (that would run the
/// first-run setup over a real record).
pub(crate) fn read(path: &Path) -> io::Result<Option<Pointer>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let pointer: Pointer = serde_json::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    if !pointer.is_well_formed() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the vault location record is not one this version can use",
        ));
    }
    Ok(Some(pointer))
}

/// Write the pointer atomically.
pub(crate) fn write(path: &Path, pointer: &Pointer) -> io::Result<()> {
    if !pointer.is_well_formed() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to record a vault location that is not a plain local path",
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(pointer).map_err(|e| io::Error::other(e.to_string()))?;
    write_atomic(path, &json)
}

/// Where [`write_atomic`] writes `path` before renaming it into place: a
/// crash in between leaves this file, never a half-written `path`.
pub(crate) fn being_written(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

/// Write `bytes` beside `path`, flush them to the disk, then rename over it:
/// a crash or a power cut leaves the old contents or the new, never an empty
/// or half-written file (a move's commit, ADR-105 L5 M5, depends on it).
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = being_written(path);
    {
        let mut file = std::fs::File::create(&tmp)?;
        io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}
