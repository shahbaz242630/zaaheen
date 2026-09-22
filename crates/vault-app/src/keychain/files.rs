//! The files the key's lifecycle looks at (ADR-SEC-029 D4, E0, C1): the data
//! sealed under the key, the "erased" marker, and erasure's file step.
//!
//! The lifecycle logic sees these only through [`VaultFiles`], so the tests
//! can script them; [`DiskFiles`] is the real implementation.

use std::io;
use std::path::{Component, Path, PathBuf};

use super::KeyedPaths;

/// What the erased marker says (E0, C1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Marker {
    /// No marker: no erasure is waiting to be finished.
    None,
    /// A marker naming the folder an erasure ran on, checked to be that
    /// folder (or, when that folder is gone entirely, its old path: there is
    /// nothing left to delete).
    Valid(PathBuf),
    /// A marker that cannot be trusted: unreadable text, a relative path, a
    /// `..`, or a folder other than the one erasure runs on. Never acted on.
    Invalid,
}

/// What erasure's file step removed and could not remove.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CleanReport {
    /// Entries removed.
    pub(crate) removed: usize,
    /// Entries still there (locked, denied, or not checkable).
    pub(crate) left: Vec<PathBuf>,
}

/// The files, as the lifecycle logic needs them.
pub(crate) trait VaultFiles {
    /// D4: does any data sealed under a key exist at this process's actual
    /// paths? An I/O error is returned, never read as "no".
    fn keyed_data_present(&self) -> io::Result<bool>;

    /// The erased marker, validated.
    fn marker(&self) -> io::Result<Marker>;

    /// E0: record that an erasure of `folder` destroyed the key.
    fn write_marker(&self, folder: &Path) -> io::Result<()>;

    /// Remove the marker. Removing one that is not there succeeds.
    fn remove_marker(&self) -> io::Result<()>;

    /// Erasure's file step over `folder`: the `VAULT_ENTRIES` names only.
    fn clean(&self, folder: &Path) -> CleanReport;

    /// Whether `folder` is this process's own vault folder (C1).
    fn is_own_folder(&self, folder: &Path) -> bool;
}

/// The real files.
pub(crate) struct DiskFiles<'a> {
    keyed: &'a KeyedPaths,
    marker_path: PathBuf,
    /// The folder erasure runs on; `None` when it cannot be resolved (then
    /// every marker is untrusted, ADR-SEC-029 amendment 5).
    erase_root: Option<PathBuf>,
}

impl<'a> DiskFiles<'a> {
    pub(crate) fn new(
        keyed: &'a KeyedPaths,
        marker_path: PathBuf,
        erase_root: Option<PathBuf>,
    ) -> Self {
        Self {
            keyed,
            marker_path,
            erase_root,
        }
    }
}

impl VaultFiles for DiskFiles<'_> {
    fn keyed_data_present(&self) -> io::Result<bool> {
        for path in self.keyed.keyed_entries() {
            if path.try_exists()? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn marker(&self) -> io::Result<Marker> {
        let bytes = match std::fs::read(&self.marker_path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Marker::None),
            Err(e) => return Err(e),
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return Ok(Marker::Invalid);
        };
        match &self.erase_root {
            Some(root) => validate_marker(Path::new(&text), root),
            // No erasure folder can be resolved (amendment 5): untrusted.
            None => Ok(Marker::Invalid),
        }
    }

    fn write_marker(&self, folder: &Path) -> io::Result<()> {
        let text = folder.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "the vault folder is not UTF-8")
        })?;
        if let Some(parent) = self.marker_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Written aside, then renamed, so a reader never sees half a path.
        let mut tmp = self.marker_path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, text.as_bytes())?;
        std::fs::rename(&tmp, &self.marker_path)
    }

    fn remove_marker(&self) -> io::Result<()> {
        match std::fs::remove_file(&self.marker_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn clean(&self, folder: &Path) -> CleanReport {
        let (removed, left) = crate::erasure::remove_vault_entries(folder);
        CleanReport { removed, left }
    }

    fn is_own_folder(&self, folder: &Path) -> bool {
        same_directory(folder, &self.keyed.vault_root())
    }
}

/// C1's check on a recorded folder: absolute, no `..`, and the same
/// directory on disk as the one erasure runs on (compared by resolved path,
/// not by text).
///
/// Amendment 4: a recorded folder that no longer exists is trusted only
/// when it IS the erasure folder and that folder is gone too (the whole
/// data folder was removed: there is nothing left to clean). Any other
/// missing folder — a drive that is not plugged in, say — is not trusted,
/// so the marker stays and no new key is made.
pub(crate) fn validate_marker(recorded: &Path, erase_root: &Path) -> io::Result<Marker> {
    let plain = recorded.is_absolute()
        && recorded
            .components()
            .all(|c| !matches!(c, Component::ParentDir | Component::CurDir));
    if !plain {
        return Ok(Marker::Invalid);
    }
    if !recorded.try_exists()? {
        let both_gone = !erase_root.try_exists()? && recorded == erase_root;
        return Ok(if both_gone {
            Marker::Valid(recorded.to_path_buf())
        } else {
            Marker::Invalid
        });
    }
    Ok(if same_directory(recorded, erase_root) {
        Marker::Valid(recorded.to_path_buf())
    } else {
        Marker::Invalid
    })
}

/// Whether two paths resolve to the same directory. Either failing to
/// resolve means "not known to be the same".
fn same_directory(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
