//! The file operations a move needs (ADR-105 L5), behind [`MoveFs`] so the
//! tests can make a process "die" after any single step and script failures;
//! [`DiskFs`] is the real implementation.
//!
//! Every step that changes something on disk is its own call, so a crash can
//! be placed between any two of them.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::progress::{MovePhase, MoveProgress};
use crate::location::pointer::{self, Pointer};

/// A copied file's size and BLAKE3 hash (M4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Digest {
    pub(crate) len: u64,
    pub(crate) hash: [u8; 32],
}

/// One entry of the vault being moved, relative to the vault folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    Dir(PathBuf),
    File { rel: PathBuf, len: u64 },
}

/// What a move asks of the machine.
pub(crate) trait MoveFs {
    /// The location record. `Ok(None)`: there is none.
    fn read_pointer(&self) -> io::Result<Option<Pointer>>;
    /// Replace the location record atomically. Mutating.
    fn write_pointer(&self, pointer: &Pointer) -> io::Result<()>;
    /// Whether an ADR-SEC-029 "erased" marker exists (any marker, trusted or
    /// not: a move never runs beside one).
    fn marker_exists(&self) -> io::Result<bool>;
    fn exists(&self, path: &Path) -> io::Result<bool>;
    /// A small text file's trimmed contents; `Ok(None)` if it is not there.
    fn read_text(&self, path: &Path) -> io::Result<Option<String>>;
    /// The names directly inside a folder.
    fn entry_names(&self, path: &Path) -> io::Result<Vec<String>>;
    /// Create one folder; its parent must exist and it must not. Mutating.
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    /// Restrict a folder to its owner (ADR-SEC-019) without writing the
    /// marker file, so a folder a crash left holds only what the move wrote.
    /// Mutating.
    fn restrict(&self, path: &Path) -> io::Result<()>;
    /// Write a small file, flushed to disk: first as
    /// `pointer::being_written(path)`, then renamed into place — two steps,
    /// and a crash between them leaves only the first. Mutating.
    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// The entries named `names` inside `root`, folders before what they
    /// hold. A link anywhere inside is an error: a move never follows one.
    fn list(&self, root: &Path, names: &[&str]) -> io::Result<Vec<Item>>;
    /// Copy `src` to a new file `dst` (never over an existing one), flushed
    /// to disk; returns the source's digest. Mutating.
    fn copy_file(&self, src: &Path, dst: &Path) -> io::Result<Digest>;
    fn digest(&self, path: &Path) -> io::Result<Digest>;
    /// Remove one entry — a folder with everything in it. Removing one that
    /// is not there succeeds. Mutating.
    fn remove_entry(&self, path: &Path) -> io::Result<()>;
    /// Remove an empty folder (refused when it is not empty). Mutating.
    fn remove_dir(&self, path: &Path) -> io::Result<()>;
    /// Whether two paths are the same folder on disk. Either failing to
    /// resolve means "not known to be the same".
    fn same_dir(&self, a: &Path, b: &Path) -> bool;
    /// A step of the move begins, handling `total` bytes (ADR-105 L-f): for
    /// a screen only, never a decision. Changes nothing on disk; the crash
    /// model keeps this default and reports nothing.
    fn report_phase(&self, _phase: MovePhase, _total: u64) {}
}

/// The real machine: the record at `pointer_path`, the key marker at
/// `marker_path`, and where to report progress, if anywhere.
#[derive(Clone, Debug)]
pub(crate) struct DiskFs {
    pointer_path: PathBuf,
    marker_path: PathBuf,
    progress: Option<Arc<MoveProgress>>,
}

impl DiskFs {
    pub(crate) fn new(pointer_path: PathBuf, marker_path: PathBuf) -> Self {
        Self {
            pointer_path,
            marker_path,
            progress: None,
        }
    }

    /// The same machine, reporting each chunk copied or read back to
    /// `progress`.
    pub(crate) fn reporting_to(mut self, progress: Arc<MoveProgress>) -> Self {
        self.progress = Some(progress);
        self
    }

    fn advanced(&self, bytes: usize) {
        if let Some(progress) = &self.progress {
            progress.advance(bytes as u64);
        }
    }
}

impl MoveFs for DiskFs {
    fn read_pointer(&self) -> io::Result<Option<Pointer>> {
        pointer::read(&self.pointer_path)
    }

    fn write_pointer(&self, pointer: &Pointer) -> io::Result<()> {
        pointer::write(&self.pointer_path, pointer)
    }

    fn marker_exists(&self) -> io::Result<bool> {
        self.marker_path.try_exists()
    }

    fn exists(&self, path: &Path) -> io::Result<bool> {
        path.try_exists()
    }

    fn read_text(&self, path: &Path) -> io::Result<Option<String>> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).trim().to_owned())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn entry_names(&self, path: &Path) -> io::Result<Vec<String>> {
        std::fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect()
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir(path)
    }

    fn restrict(&self, path: &Path) -> io::Result<()> {
        crate::keeper::acl::restrict_folder(path).map_err(|e| io::Error::other(e.to_string()))
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        pointer::write_atomic(path, bytes)
    }

    fn list(&self, root: &Path, names: &[&str]) -> io::Result<Vec<Item>> {
        let mut items = Vec::new();
        for name in names {
            walk(root, Path::new(name), &mut items)?;
        }
        Ok(items)
    }

    fn copy_file(&self, src: &Path, dst: &Path) -> io::Result<Digest> {
        let mut input = File::open(src)?;
        let mut output = OpenOptions::new().write(true).create_new(true).open(dst)?;
        let digest = stream(&mut input, |chunk| {
            output.write_all(chunk)?;
            self.advanced(chunk.len());
            Ok(())
        })?;
        output.sync_all()?;
        Ok(digest)
    }

    fn digest(&self, path: &Path) -> io::Result<Digest> {
        stream(&mut File::open(path)?, |chunk| {
            self.advanced(chunk.len());
            Ok(())
        })
    }

    fn remove_entry(&self, path: &Path) -> io::Result<()> {
        crate::erasure::remove_entry(path).map(|_| ())
    }

    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir(path)
    }

    fn same_dir(&self, a: &Path, b: &Path) -> bool {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }

    fn report_phase(&self, phase: MovePhase, total: u64) {
        if let Some(progress) = &self.progress {
            progress.begin(phase, total);
        }
    }
}

/// Add `rel` (inside `root`) to `items`, and everything under it when it is
/// a folder. Not there: nothing. A link: an error.
fn walk(root: &Path, rel: &Path, items: &mut Vec<Item>) -> io::Result<()> {
    let path = root.join(rel);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let kind = meta.file_type();
    if kind.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a link inside the vault folder: {}", rel.display()),
        ));
    }
    if kind.is_file() {
        items.push(Item::File {
            rel: rel.to_path_buf(),
            len: meta.len(),
        });
        return Ok(());
    }
    if !kind.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("not a file or a folder: {}", rel.display()),
        ));
    }
    items.push(Item::Dir(rel.to_path_buf()));
    let mut children: Vec<PathBuf> = std::fs::read_dir(&path)?
        .map(|entry| entry.map(|e| rel.join(e.file_name())))
        .collect::<io::Result<_>>()?;
    children.sort();
    for child in children {
        walk(root, &child, items)?;
    }
    Ok(())
}

/// Read `input` to its end, handing each chunk to `sink`; the digest of what
/// was read.
fn stream(input: &mut File, mut sink: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<Digest> {
    let mut hasher = blake3::Hasher::new();
    let mut len = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).unwrap_or_default();
        hasher.update(chunk);
        sink(chunk)?;
        len = len.saturating_add(n as u64);
    }
    Ok(Digest {
        len,
        hash: *hasher.finalize().as_bytes(),
    })
}

/// Whether a process has the vault's database open (M1). Every process that
/// opens the vault keeps its database open for as long as it runs, and the
/// desktop takes no vault lock, so this is how a move knows that a window
/// which has not finished closing — or a second window — is not about to
/// write a memory the copy would miss.
///
/// Windows: an open that shares nothing is refused while any other handle
/// is open. Elsewhere there is no such check and this answers "no"; V0.2
/// runs the desktop on Windows only.
///
/// # Errors
///
/// When the check itself cannot be made (never read as "not in use").
#[cfg(windows)]
pub(crate) fn database_in_use(vault_root: &Path) -> io::Result<bool> {
    use std::os::windows::fs::OpenOptionsExt;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    match OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(vault_root.join("vault.db"))
    {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => Ok(true),
        Err(e) => Err(e),
    }
}

/// See the Windows version.
#[cfg(not(windows))]
pub(crate) fn database_in_use(_vault_root: &Path) -> io::Result<bool> {
    Ok(false)
}
