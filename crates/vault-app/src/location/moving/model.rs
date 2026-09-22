//! An in-memory disk for the crash enumerations (ADR-SEC-029's method: its
//! `keychain::tests::double`). The move's own code runs unchanged over it;
//! only the file system is modelled, so every crash point — and every pair
//! of them — can be checked in well under the time a test may take. The
//! real disk's behaviour is checked by `tests` and `start_tests`, and at
//! chosen crash points by `crash_tests::the_real_disk_recovers_at_chosen_crash_points`.
//!
//! What is modelled is what the move relies on: a folder is created only
//! inside an existing one and never over an existing entry; a copy never
//! overwrites; an empty-folder removal refuses a folder that holds anything;
//! a record write is atomic; a small file is written beside its place and
//! then renamed in, two steps, as `DiskFs` does.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use super::fs::{Digest, Item, MoveFs};
use super::*;
use crate::keychain::test_helpers::test_location;
use crate::location::pointer::{self, Pointer};
use crate::location::{Homes, MOVE_ID_FILE, VAULT_ID_FILE};

/// One entry on the model disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Node {
    Dir,
    File(Vec<u8>),
}

/// The model disk: every entry by its full path, the location record, and
/// whether an erasure marker exists.
#[derive(Clone, Debug, Default)]
pub(super) struct Disk {
    pub(super) nodes: BTreeMap<PathBuf, Node>,
    pub(super) pointer: Option<Pointer>,
    pub(super) marker: bool,
}

impl Disk {
    fn is_dir(&self, p: &Path) -> bool {
        self.nodes.get(p) == Some(&Node::Dir)
    }

    fn children(&self, p: &Path) -> Vec<PathBuf> {
        self.nodes
            .keys()
            .filter(|k| k.parent() == Some(p))
            .cloned()
            .collect()
    }

    /// Anything at or under `p`.
    pub(super) fn has_anything_at(&self, p: &Path) -> bool {
        self.nodes.keys().any(|k| k.starts_with(p))
    }

    /// The resolver's rules (`location::resolve`) over the model.
    pub(super) fn resolve(&self) -> Option<PathBuf> {
        let p = self.pointer.as_ref()?;
        if !self.is_dir(&p.vault_dir) {
            return None;
        }
        match self.nodes.get(&p.vault_dir.join(VAULT_ID_FILE)) {
            Some(Node::File(id)) if String::from_utf8_lossy(id).trim() == p.vault_id => {
                Some(p.vault_dir.clone())
            }
            _ => None,
        }
    }

    /// The moved names under `dir`, relative to it.
    pub(super) fn snapshot(&self, dir: &Path) -> BTreeMap<PathBuf, Node> {
        self.nodes
            .iter()
            .filter_map(|(k, v)| {
                let rel = k.strip_prefix(dir).ok()?;
                let first = rel.components().next()?.as_os_str().to_str()?;
                moved_names()
                    .contains(&first)
                    .then(|| (rel.to_path_buf(), v.clone()))
            })
            .collect()
    }
}

fn digest_of(bytes: &[u8]) -> Digest {
    Digest {
        len: bytes.len() as u64,
        hash: *blake3::hash(bytes).as_bytes(),
    }
}

fn not_found(p: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, p.display().to_string())
}

/// One process over the model disk, dying after a budget of changing steps
/// (as `tests::Scripted` does over the real one).
pub(super) struct ModelFs<'a> {
    disk: &'a RefCell<Disk>,
    budget: Cell<Option<usize>>,
    dead: Cell<bool>,
}

impl<'a> ModelFs<'a> {
    pub(super) fn new(disk: &'a RefCell<Disk>) -> Self {
        Self {
            disk,
            budget: Cell::new(None),
            dead: Cell::new(false),
        }
    }

    pub(super) fn crashing_after(disk: &'a RefCell<Disk>, steps: usize) -> Self {
        let fs = Self::new(disk);
        fs.budget.set(Some(steps));
        fs
    }

    pub(super) fn died(&self) -> bool {
        self.dead.get()
    }

    fn step(&self, changes: bool) -> io::Result<()> {
        if self.dead.get() {
            return Err(io::Error::other("the process has died"));
        }
        if changes {
            if let Some(left) = self.budget.get() {
                if left == 0 {
                    self.dead.set(true);
                    return Err(io::Error::other("the process died here"));
                }
                self.budget.set(Some(left - 1));
            }
        }
        Ok(())
    }

    fn walk(&self, root: &Path, rel: &Path, out: &mut Vec<Item>) {
        let (item, children) = {
            let disk = self.disk.borrow();
            match disk.nodes.get(&root.join(rel)) {
                None => return,
                Some(Node::File(b)) => (
                    Item::File {
                        rel: rel.to_path_buf(),
                        len: b.len() as u64,
                    },
                    Vec::new(),
                ),
                Some(Node::Dir) => (Item::Dir(rel.to_path_buf()), disk.children(&root.join(rel))),
            }
        };
        out.push(item);
        for child in children {
            if let Some(name) = child.file_name() {
                self.walk(root, &rel.join(name), out);
            }
        }
    }
}

impl MoveFs for ModelFs<'_> {
    fn read_pointer(&self) -> io::Result<Option<Pointer>> {
        self.step(false)?;
        Ok(self.disk.borrow().pointer.clone())
    }
    fn write_pointer(&self, p: &Pointer) -> io::Result<()> {
        self.step(true)?;
        if !p.is_well_formed() {
            return Err(io::Error::other("not a record this build may write"));
        }
        self.disk.borrow_mut().pointer = Some(p.clone());
        Ok(())
    }
    fn marker_exists(&self) -> io::Result<bool> {
        self.step(false)?;
        Ok(self.disk.borrow().marker)
    }
    fn exists(&self, path: &Path) -> io::Result<bool> {
        self.step(false)?;
        Ok(self.disk.borrow().nodes.contains_key(path))
    }
    fn read_text(&self, path: &Path) -> io::Result<Option<String>> {
        self.step(false)?;
        match self.disk.borrow().nodes.get(path) {
            None => Ok(None),
            Some(Node::File(b)) => Ok(Some(String::from_utf8_lossy(b).trim().to_owned())),
            Some(Node::Dir) => Err(io::Error::other("a folder")),
        }
    }
    fn entry_names(&self, path: &Path) -> io::Result<Vec<String>> {
        self.step(false)?;
        let disk = self.disk.borrow();
        if !disk.is_dir(path) {
            return Err(not_found(path));
        }
        Ok(disk
            .children(path)
            .iter()
            .filter_map(|c| c.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect())
    }
    fn create_dir(&self, path: &Path) -> io::Result<()> {
        self.step(true)?;
        let mut disk = self.disk.borrow_mut();
        if disk.nodes.contains_key(path) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "exists"));
        }
        if !path.parent().is_some_and(|p| disk.is_dir(p)) {
            return Err(not_found(path));
        }
        disk.nodes.insert(path.to_path_buf(), Node::Dir);
        Ok(())
    }
    fn restrict(&self, path: &Path) -> io::Result<()> {
        self.step(true)?;
        if self.disk.borrow().is_dir(path) {
            Ok(())
        } else {
            Err(not_found(path))
        }
    }
    /// As `DiskFs` does it: the file beside its place first, then renamed —
    /// two steps, so a crash can fall between them.
    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let writing = pointer::being_written(path);
        self.step(true)?;
        {
            let mut disk = self.disk.borrow_mut();
            if !path.parent().is_some_and(|p| disk.is_dir(p)) || disk.is_dir(path) {
                return Err(not_found(path));
            }
            disk.nodes
                .insert(writing.clone(), Node::File(bytes.to_vec()));
        }
        self.step(true)?;
        let mut disk = self.disk.borrow_mut();
        let written = disk
            .nodes
            .remove(&writing)
            .ok_or_else(|| not_found(&writing))?;
        disk.nodes.insert(path.to_path_buf(), written);
        Ok(())
    }
    fn list(&self, root: &Path, names: &[&str]) -> io::Result<Vec<Item>> {
        self.step(false)?;
        let mut out = Vec::new();
        for name in names {
            self.walk(root, Path::new(name), &mut out);
        }
        Ok(out)
    }
    fn copy_file(&self, src: &Path, dst: &Path) -> io::Result<Digest> {
        self.step(true)?;
        let mut disk = self.disk.borrow_mut();
        let Some(Node::File(bytes)) = disk.nodes.get(src).cloned() else {
            return Err(not_found(src));
        };
        if disk.nodes.contains_key(dst) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "exists"));
        }
        if !dst.parent().is_some_and(|p| disk.is_dir(p)) {
            return Err(not_found(dst));
        }
        let digest = digest_of(&bytes);
        disk.nodes.insert(dst.to_path_buf(), Node::File(bytes));
        Ok(digest)
    }
    fn digest(&self, path: &Path) -> io::Result<Digest> {
        self.step(false)?;
        match self.disk.borrow().nodes.get(path) {
            Some(Node::File(b)) => Ok(digest_of(b)),
            _ => Err(not_found(path)),
        }
    }
    fn remove_entry(&self, path: &Path) -> io::Result<()> {
        self.step(true)?;
        self.disk
            .borrow_mut()
            .nodes
            .retain(|k, _| !k.starts_with(path));
        Ok(())
    }
    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        self.step(true)?;
        let mut disk = self.disk.borrow_mut();
        if !disk.is_dir(path) {
            return Err(not_found(path));
        }
        if !disk.children(path).is_empty() {
            return Err(io::Error::other("the folder is not empty"));
        }
        disk.nodes.remove(path);
        Ok(())
    }
    fn same_dir(&self, a: &Path, b: &Path) -> bool {
        a == b && self.disk.borrow().is_dir(a)
    }
}

/// The folder checks over the model disk; everything else accepted.
pub(super) struct ModelEnv<'a>(pub(super) &'a RefCell<Disk>);

impl CheckEnv for ModelEnv<'_> {
    fn folder_var(&self, _: &str) -> Option<PathBuf> {
        None
    }
    fn sync_roots(&self) -> Option<Vec<PathBuf>> {
        Some(Vec::new())
    }
    fn dropbox_roots(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn canonical(&self, p: &Path) -> io::Result<PathBuf> {
        if self.0.borrow().nodes.contains_key(p) {
            Ok(p.to_path_buf())
        } else {
            Err(not_found(p))
        }
    }
    fn exists(&self, p: &Path) -> io::Result<bool> {
        Ok(self.0.borrow().nodes.contains_key(p))
    }
    fn probe_writable(&self, _: &Path) -> io::Result<()> {
        Ok(())
    }
    fn available_space(&self, _: &Path) -> io::Result<u64> {
        Ok(u64::MAX)
    }
    fn install_dir(&self) -> Option<PathBuf> {
        None
    }
}

/// A person's vault on the model disk with a move to `E` asked for; the key
/// lock K1 is real (it is a file lock), in `keys`.
pub(super) struct Model {
    pub(super) disk: RefCell<Disk>,
    pub(super) homes: Homes,
    pub(super) key: KeyLocation,
    pub(super) source: PathBuf,
    pub(super) target: PathBuf,
    pub(super) original: BTreeMap<PathBuf, Node>,
}

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\m" } else { "/m" })
}

/// A fresh model; `keys` holds the real key lock and must outlive it.
pub(super) fn model(keys: &Path) -> Model {
    let root = root();
    let source = root.join("vault");
    let chosen = root.join("E");
    let target = chosen.join(check::VAULT_SUBFOLDER);
    let vault_id = pointer::new_id().unwrap();
    let mut nodes = BTreeMap::new();
    for dir in [
        root.clone(),
        source.clone(),
        chosen.clone(),
        source.join("lance"),
        source.join("lance").join("data"),
        source.join("lance").join("_indices"),
        source.join("reports"),
        source.join("models"),
    ] {
        nodes.insert(dir, Node::Dir);
    }
    for (rel, bytes) in [
        ("vault.db", b"db".to_vec()),
        ("vault.db-wal", b"wal".to_vec()),
        ("lance/data/0001.lance", b"vectors".to_vec()),
        ("reports/r1.sealed", b"report".to_vec()),
        ("graph.sealed", b"graph".to_vec()),
        ("maintenance.json", b"{}".to_vec()),
        (VAULT_ID_FILE, vault_id.clone().into_bytes()),
        // Never moved.
        (".acl-v1", Vec::new()),
        (".vault-host.json", b"{}".to_vec()),
        (".vault.lock", Vec::new()),
        ("notes.txt", b"theirs".to_vec()),
        ("models/m.onnx", b"model".to_vec()),
    ] {
        nodes.insert(source.join(rel), Node::File(bytes));
    }
    let mut record = Pointer::new(source.clone(), vault_id);
    record.pending_move = Some(pointer::PendingMove {
        to: target.clone(),
        move_id: pointer::new_id().unwrap(),
        attempts: 0,
    });
    let disk = Disk {
        nodes,
        pointer: Some(record),
        marker: false,
    };
    let original = disk.snapshot(&source);
    Model {
        disk: RefCell::new(disk),
        homes: Homes {
            local: root.join("Local"),
            roaming: root.join("Roaming"),
        },
        key: test_location("model", keys),
        source,
        target,
        original,
    }
}

impl Model {
    /// One start's work with the vault held, as `run_pending` dispatches it.
    pub(super) fn start(&self, fs: &ModelFs<'_>) -> MoveOutcome {
        let Ok(Some(p)) = fs.read_pointer() else {
            return MoveOutcome::Nothing;
        };
        if p.pending_cleanup.is_some() {
            let cleaned = clean_holding_old(&self.key, fs);
            if cleaned != MoveOutcome::OldCopyRemoved {
                return cleaned;
            }
        }
        move_holding_source(&self.homes, &self.key, fs, &ModelEnv(&self.disk))
    }

    /// The record names a folder holding every memory, now.
    pub(super) fn one_complete_vault(&self, at: &str) -> PathBuf {
        let disk = self.disk.borrow();
        let live = disk
            .resolve()
            .unwrap_or_else(|| panic!("{at}: the memories cannot be found: {:?}", disk.pointer));
        assert_eq!(
            disk.snapshot(&live),
            self.original,
            "{at}: the vault named by the record is not complete"
        );
        live
    }

    /// Starts with nothing going wrong until nothing is left to do; then the
    /// memories are in exactly one place, and nothing else was touched.
    pub(super) fn settle(&self, at: &str) {
        for _ in 0..4 {
            if self.start(&ModelFs::new(&self.disk)) == MoveOutcome::Nothing {
                break;
            }
        }
        let live = self.one_complete_vault(at);
        let disk = self.disk.borrow();
        let p = disk.pointer.as_ref().unwrap();
        assert!(
            p.pending_move.is_none() && p.pending_cleanup.is_none(),
            "{at}: work left waiting: {p:?}"
        );
        let other = if live == self.source {
            &self.target
        } else {
            assert_eq!(live, self.target, "{at}");
            &self.source
        };
        if other == &self.target {
            assert!(
                !disk.has_anything_at(&self.target),
                "{at}: a half-made copy was left"
            );
        } else {
            assert!(
                disk.snapshot(&self.source).is_empty(),
                "{at}: the old copy was left"
            );
            for kept in [".vault.lock", "notes.txt", "models/m.onnx"] {
                assert!(
                    disk.nodes.contains_key(&self.source.join(kept)),
                    "{at}: {kept} is not the vault's to remove"
                );
            }
            for never in [
                MOVE_ID_FILE,
                ".acl-v1",
                ".vault-host.json",
                ".vault.lock",
                "notes.txt",
            ] {
                assert!(
                    !disk.nodes.contains_key(&self.target.join(never)),
                    "{at}: {never} does not belong in the new folder"
                );
            }
        }
    }
}
