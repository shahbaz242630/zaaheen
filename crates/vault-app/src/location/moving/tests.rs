//! ADR-105 L5 against real temporary folders, with [`Scripted`] standing in
//! for the file system where a failure or a crash has to be placed exactly.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use super::fs::{Digest, DiskFs, Item, MoveFs};
use super::*;
use crate::keychain::test_helpers::test_location;
use crate::location::check::Refusal;
use crate::location::pointer::{self, PendingCleanup, Pointer};
use crate::location::{self, Homes, MOVE_ID_FILE, VAULT_ID_FILE};

/// What a move must copy, written out here rather than taken from the code
/// under test.
const EXPECTED_MOVED: &[&str] = &[
    "vault.db",
    "vault.db-wal",
    "vault.db-shm",
    "lance",
    "graph.duckdb",
    "graph.sealed",
    "reports",
    "maintenance.json",
    ".vault-id",
];

/// A person's vault in the pre-ADR-105 home, recorded, and a folder `E`
/// they chose.
pub(super) struct World {
    pub(super) tmp: tempfile::TempDir,
    pub(super) homes: Homes,
    pub(super) key: KeyLocation,
    pub(super) source: PathBuf,
    pub(super) chosen: PathBuf,
    /// The memories as they were: relative path → bytes (`None`: a folder).
    pub(super) original: BTreeMap<PathBuf, Option<Vec<u8>>>,
}

fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn put(path: &Path, content: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

pub(super) fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let homes = Homes {
        local: tmp.path().join("Local").join("com.zaaheen.app"),
        roaming: tmp.path().join("Roaming").join("com.zaaheen.app"),
    };
    let source = homes.roaming.clone();
    put(&source.join("vault.db"), &bytes(5000, 1));
    put(&source.join("vault.db-wal"), &bytes(300, 2));
    put(&source.join("lance/data/0001.lance"), &bytes(2000, 3));
    put(&source.join("lance/_versions/1.manifest"), &bytes(100, 4));
    std::fs::create_dir_all(source.join("lance/_indices")).unwrap();
    put(&source.join("reports/r1.sealed"), &bytes(400, 5));
    put(&source.join("graph.sealed"), &bytes(700, 6));
    put(&source.join("maintenance.json"), br#"{"schedule":1}"#);
    // Never moved.
    put(&source.join(".acl-v1"), b"");
    put(&source.join(".vault-host.json"), b"{}");
    put(&source.join(".vault.lock"), b"");
    put(&source.join(".consolidator.lock"), b"");
    put(&source.join("models/bge/model.onnx"), &bytes(50, 7));
    put(&source.join("notes.txt"), b"the person's own file");
    let key = test_location("move", tmp.path());
    let dir = location::prepare(&homes, &key, &[]).unwrap();
    assert_eq!(dir.path(), source, "the existing install is adopted");
    let chosen = tmp.path().join("E");
    std::fs::create_dir_all(&chosen).unwrap();
    let original = snapshot(&source);
    World {
        tmp,
        homes,
        key,
        source,
        chosen,
        original,
    }
}

/// The moved names inside `dir`, read independently of the code under test.
pub(super) fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn add(root: &Path, rel: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let path = root.join(rel);
        if path.is_dir() {
            out.insert(rel.to_path_buf(), None);
            for entry in std::fs::read_dir(&path).unwrap() {
                add(root, &rel.join(entry.unwrap().file_name()), out);
            }
        } else if path.is_file() {
            out.insert(rel.to_path_buf(), Some(std::fs::read(&path).unwrap()));
        }
    }
    let mut out = BTreeMap::new();
    for name in EXPECTED_MOVED {
        add(dir, Path::new(name), &mut out);
    }
    out
}

/// Accepts every folder; the real file system for the rest.
pub(super) struct TestEnv;

impl CheckEnv for TestEnv {
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
        std::fs::canonicalize(p)
    }
    fn exists(&self, p: &Path) -> io::Result<bool> {
        p.try_exists()
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

pub(super) fn real_fs(w: &World) -> DiskFs {
    DiskFs::new(w.homes.pointer_path(), w.key.marker_path())
}

pub(super) fn record(w: &World) -> Pointer {
    pointer::read(&w.homes.pointer_path()).unwrap().unwrap()
}

/// Ask for the move to `E`; its target.
pub(super) fn requested(w: &World) -> PathBuf {
    let checked = request_move(&w.homes, &w.key, &w.chosen, &TestEnv).unwrap();
    let pending = record(w).pending_move.expect("the move is recorded");
    assert_eq!(pending.to, checked.target);
    pending.to
}

/// One start's work, as `run_pending` dispatches it, with the vault held.
pub(super) fn next_start(w: &World, fs: &dyn MoveFs) -> MoveOutcome {
    let Ok(Some(p)) = fs.read_pointer() else {
        return MoveOutcome::Nothing;
    };
    if p.pending_cleanup.is_some() {
        let cleaned = clean_holding_old(&w.key, fs);
        if cleaned != MoveOutcome::OldCopyRemoved {
            return cleaned;
        }
    }
    move_holding_source(&w.homes, &w.key, fs, &TestEnv)
}

/// The memories live in `to`, complete, and nowhere else.
pub(super) fn assert_moved(w: &World, to: &Path) {
    assert_eq!(location::resolve(&w.homes).unwrap().path(), to);
    assert_eq!(
        snapshot(to),
        w.original,
        "the new folder holds every memory"
    );
    let p = record(w);
    assert!(p.pending_move.is_none() && p.pending_cleanup.is_none());
    for name in EXPECTED_MOVED {
        assert!(
            !w.source.join(name).exists(),
            "{name} was left in the old folder"
        );
    }
    for kept in ["models/bge/model.onnx", "notes.txt", ".vault.lock"] {
        assert!(
            w.source.join(kept).exists(),
            "{kept} is not the vault's to remove"
        );
    }
    for never in [
        MOVE_ID_FILE,
        ".acl-v1",
        ".vault-host.json",
        ".vault.lock",
        "notes.txt",
        "models",
    ] {
        assert!(
            !to.join(never).exists(),
            "{never} does not belong in the new folder"
        );
    }
}

/// The memories are where they were, complete, and the target is gone.
pub(super) fn assert_stayed(w: &World, target: &Path) {
    assert_eq!(location::resolve(&w.homes).unwrap().path(), w.source);
    assert_eq!(
        snapshot(&w.source),
        w.original,
        "every memory is still there"
    );
    assert!(!target.exists(), "the half-made copy was removed");
}

// ── a stand-in file system: failures and crashes placed exactly ──────────

/// [`DiskFs`] with a budget of changing steps (a process that "dies" after
/// it: every later call fails, reads included) and scripted failures.
pub(super) struct Scripted {
    inner: DiskFs,
    budget: Cell<Option<usize>>,
    dead: Cell<bool>,
    pub(super) fail_copy: Option<&'static str>,
    pub(super) corrupt_copy: Option<&'static str>,
    pub(super) fail_remove: Option<&'static str>,
    pub(super) fail_restrict: bool,
    /// The process dies as it starts copying this file.
    pub(super) die_copying: Option<&'static str>,
    /// The process dies right after the record switches to the new folder.
    pub(super) die_after_commit: bool,
    pub(super) log: RefCell<Vec<String>>,
}

impl Scripted {
    pub(super) fn new(w: &World) -> Self {
        Self {
            inner: real_fs(w),
            budget: Cell::new(None),
            dead: Cell::new(false),
            fail_copy: None,
            corrupt_copy: None,
            fail_remove: None,
            fail_restrict: false,
            die_copying: None,
            die_after_commit: false,
            log: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn crashing_after(w: &World, steps: usize) -> Self {
        let s = Self::new(w);
        s.budget.set(Some(steps));
        s
    }

    pub(super) fn died(&self) -> bool {
        self.dead.get()
    }

    fn step(&self, what: String, changes: bool) -> io::Result<()> {
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
        self.log.borrow_mut().push(what);
        Ok(())
    }
}

fn named(path: &Path, name: Option<&str>) -> bool {
    name.is_some_and(|n| path.file_name().is_some_and(|f| f == n))
}

impl MoveFs for Scripted {
    fn read_pointer(&self) -> io::Result<Option<Pointer>> {
        self.step("read_pointer".into(), false)?;
        self.inner.read_pointer()
    }
    fn write_pointer(&self, p: &Pointer) -> io::Result<()> {
        let what = if p.pending_cleanup.is_some() && p.pending_move.is_none() {
            "commit"
        } else {
            "write_pointer"
        };
        self.step(what.into(), true)?;
        self.inner.write_pointer(p)?;
        if what == "commit" && self.die_after_commit {
            self.dead.set(true);
        }
        Ok(())
    }
    fn marker_exists(&self) -> io::Result<bool> {
        self.step("marker_exists".into(), false)?;
        self.inner.marker_exists()
    }
    fn exists(&self, path: &Path) -> io::Result<bool> {
        self.step("exists".into(), false)?;
        self.inner.exists(path)
    }
    fn read_text(&self, path: &Path) -> io::Result<Option<String>> {
        self.step("read_text".into(), false)?;
        self.inner.read_text(path)
    }
    fn entry_names(&self, path: &Path) -> io::Result<Vec<String>> {
        self.step("entry_names".into(), false)?;
        self.inner.entry_names(path)
    }
    fn create_dir(&self, path: &Path) -> io::Result<()> {
        self.step(format!("create_dir {}", path.display()), true)?;
        self.inner.create_dir(path)
    }
    fn restrict(&self, _path: &Path) -> io::Result<()> {
        // Stands in for icacls (two processes per call: too slow to run at
        // every crash point); the real one runs in `run_pending`'s test.
        self.step("restrict".into(), true)?;
        if self.fail_restrict {
            return Err(io::Error::other("this drive has no permissions"));
        }
        Ok(())
    }
    fn write_file(&self, path: &Path, b: &[u8]) -> io::Result<()> {
        self.step(format!("write_file {}", path.display()), true)?;
        self.inner.write_file(path, b)
    }
    fn list(&self, root: &Path, names: &[&str]) -> io::Result<Vec<Item>> {
        self.step("list".into(), false)?;
        self.inner.list(root, names)
    }
    fn copy_file(&self, src: &Path, dst: &Path) -> io::Result<Digest> {
        if named(src, self.die_copying) {
            self.dead.set(true);
        }
        self.step(format!("copy {}", dst.display()), true)?;
        if named(src, self.fail_copy) {
            return Err(io::Error::other("scripted: the copy failed"));
        }
        let digest = self.inner.copy_file(src, dst)?;
        if named(src, self.corrupt_copy) {
            let mut b = std::fs::read(dst)?;
            b[0] ^= 0xFF;
            std::fs::write(dst, b)?;
        }
        Ok(digest)
    }
    fn digest(&self, path: &Path) -> io::Result<Digest> {
        self.step(format!("digest {}", path.display()), false)?;
        self.inner.digest(path)
    }
    fn remove_entry(&self, path: &Path) -> io::Result<()> {
        self.step(format!("remove {}", path.display()), true)?;
        if named(path, self.fail_remove) {
            return Err(io::Error::other("scripted: the file is in use"));
        }
        self.inner.remove_entry(path)
    }
    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        self.step(format!("remove_dir {}", path.display()), true)?;
        self.inner.remove_dir(path)
    }
    fn same_dir(&self, a: &Path, b: &Path) -> bool {
        self.inner.same_dir(a, b)
    }
}

// ── asking for a move ─────────────────────────────────────────────────────

#[test]
fn asking_for_a_move_records_it_and_changes_nothing_else() {
    let w = world();
    let target = requested(&w);
    assert_eq!(target.file_name().unwrap(), "Zaaheen Memories");
    assert!(!target.exists(), "nothing is created until the next start");
    let p = record(&w);
    assert_eq!(p.vault_dir, w.source, "the memories stay where they are");
    let pending = p.pending_move.unwrap();
    assert_eq!(pending.attempts, 0);
    assert!(pointer::is_id(&pending.move_id));
    assert_eq!(snapshot(&w.source), w.original);
}

#[test]
fn a_move_is_refused_while_another_is_waiting() {
    let w = world();
    requested(&w);
    assert_eq!(
        request_move(&w.homes, &w.key, &w.chosen, &TestEnv),
        Err(RequestRefusal::MoveWaiting)
    );
}

#[test]
fn a_move_is_refused_while_an_old_copy_is_still_being_removed() {
    let w = world();
    let mut p = record(&w);
    p.pending_cleanup = Some(PendingCleanup {
        from: w.tmp.path().join("Old"),
        move_id: pointer::new_id().unwrap(),
    });
    pointer::write(&w.homes.pointer_path(), &p).unwrap();
    assert_eq!(
        request_move(&w.homes, &w.key, &w.chosen, &TestEnv),
        Err(RequestRefusal::OldCopyWaiting)
    );
}

#[test]
fn a_move_is_refused_after_an_unfinished_erasure() {
    let w = world();
    put(&w.key.marker_path(), b"C:\\somewhere");
    assert_eq!(
        request_move(&w.homes, &w.key, &w.chosen, &TestEnv),
        Err(RequestRefusal::ErasureUnfinished)
    );
    assert!(record(&w).pending_move.is_none());
}

#[test]
fn a_refused_folder_is_never_recorded() {
    let w = world();
    let inside = w.source.join("reports");
    assert_eq!(
        request_move(&w.homes, &w.key, &inside, &TestEnv),
        Err(RequestRefusal::Folder(Refusal::InsideAppFolders))
    );
    assert!(record(&w).pending_move.is_none());
}

#[test]
fn with_nothing_recorded_there_is_nothing_to_move() {
    let tmp = tempfile::tempdir().unwrap();
    let homes = Homes {
        local: tmp.path().join("Local"),
        roaming: tmp.path().join("Roaming"),
    };
    let key = test_location("move", tmp.path());
    let chosen = tmp.path().join("E");
    std::fs::create_dir_all(&chosen).unwrap();
    assert_eq!(
        request_move(&homes, &key, &chosen, &TestEnv),
        Err(RequestRefusal::VaultUnavailable)
    );
    assert!(!homes.pointer_path().exists());
}

// ── moving ────────────────────────────────────────────────────────────────

#[test]
fn a_move_copies_the_memories_checks_them_then_switches() {
    let w = world();
    let target = requested(&w);
    let outcome = move_holding_source(&w.homes, &w.key, &real_fs(&w), &TestEnv);
    assert_eq!(
        outcome,
        MoveOutcome::Moved {
            to: target.clone(),
            not_restricted: false,
            old_copy_waiting: false
        }
    );
    assert_moved(&w, &target);
    assert_eq!(next_start(&w, &real_fs(&w)), MoveOutcome::Nothing);
}

#[test]
fn only_the_memories_move() {
    let names = moved_names();
    let mut expected: Vec<&str> = EXPECTED_MOVED.to_vec();
    let mut got = names.clone();
    expected.sort_unstable();
    got.sort_unstable();
    assert_eq!(got, expected);
    let keyed = crate::keychain::KeyedPaths::in_folder(Path::new("v")).keyed_entries();
    for entry in keyed {
        let name = entry.file_name().unwrap().to_str().unwrap().to_owned();
        assert!(
            names.contains(&name.as_str()),
            "{name} holds memories and must move"
        );
    }
    for name in &names {
        assert!(
            crate::erasure::VAULT_ENTRIES.contains(name)
                || crate::erasure::VAULT_KEEP_FILES.contains(name),
            "{name} is not a declared vault file"
        );
    }
}

/// M2: the folder is restricted before anything is written into it; M4
/// before M5: the record changes only once every copy has been checked.
#[test]
fn the_steps_happen_in_the_locked_order() {
    let w = world();
    requested(&w);
    let fs = Scripted::new(&w);
    assert!(matches!(
        move_holding_source(&w.homes, &w.key, &fs, &TestEnv),
        MoveOutcome::Moved { .. }
    ));
    let log = fs.log.borrow();
    let at = |prefix: &str| {
        log.iter()
            .position(|s| s.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix} in {log:?}"))
    };
    let last = |prefix: &str| log.iter().rposition(|s| s.starts_with(prefix)).unwrap();
    assert!(
        at("write_pointer") < at("create_dir"),
        "the attempt is counted first"
    );
    assert!(at("create_dir") < at("restrict"));
    assert!(
        at("restrict") < at("write_file"),
        "restricted before .move-id"
    );
    assert!(log[at("write_file")].ends_with(MOVE_ID_FILE));
    assert!(at("write_file") < at("copy"));
    assert!(last("copy") < at("digest"));
    assert!(
        last("digest") < at("commit"),
        "every copy checked before the switch"
    );
    assert!(
        at("commit") < at("remove"),
        "the old copy goes only after the switch"
    );
}

#[test]
fn a_copy_that_does_not_match_is_never_used() {
    let w = world();
    let target = requested(&w);
    let mut fs = Scripted::new(&w);
    fs.corrupt_copy = Some("0001.lance");
    assert_eq!(
        move_holding_source(&w.homes, &w.key, &fs, &TestEnv),
        MoveOutcome::Failed {
            reason: MoveFailure::CopyDidNotMatch,
            retrying: true
        }
    );
    assert_stayed(&w, &target);
    assert_eq!(record(&w).pending_move.unwrap().attempts, 1);
}

#[test]
fn a_failed_copy_is_removed_and_the_memories_stay() {
    let w = world();
    let target = requested(&w);
    let mut fs = Scripted::new(&w);
    fs.fail_copy = Some("graph.sealed");
    assert_eq!(
        move_holding_source(&w.homes, &w.key, &fs, &TestEnv),
        MoveOutcome::Failed {
            reason: MoveFailure::CopyFailed,
            retrying: true
        }
    );
    assert_stayed(&w, &target);
}

#[test]
fn a_retry_after_a_failure_moves_the_memories() {
    let w = world();
    let target = requested(&w);
    let mut fs = Scripted::new(&w);
    fs.fail_copy = Some("vault.db");
    assert!(matches!(
        move_holding_source(&w.homes, &w.key, &fs, &TestEnv),
        MoveOutcome::Failed { retrying: true, .. }
    ));
    assert!(matches!(
        move_holding_source(&w.homes, &w.key, &Scripted::new(&w), &TestEnv),
        MoveOutcome::Moved { .. }
    ));
    assert_moved(&w, &target);
}

#[test]
fn after_two_failed_attempts_the_move_is_given_up() {
    let w = world();
    let target = requested(&w);
    let mut fs = Scripted::new(&w);
    fs.fail_copy = Some("vault.db");
    let first = move_holding_source(&w.homes, &w.key, &fs, &TestEnv);
    let second = move_holding_source(&w.homes, &w.key, &fs, &TestEnv);
    assert_eq!(
        (first, second),
        (
            MoveOutcome::Failed {
                reason: MoveFailure::CopyFailed,
                retrying: true
            },
            MoveOutcome::Failed {
                reason: MoveFailure::CopyFailed,
                retrying: false
            }
        )
    );
    assert!(record(&w).pending_move.is_none(), "given up: nothing waits");
    assert_stayed(&w, &target);
    assert_eq!(next_start(&w, &real_fs(&w)), MoveOutcome::Nothing);
}

#[test]
fn a_move_never_runs_beside_an_unfinished_erasure() {
    let w = world();
    let target = requested(&w);
    put(&w.key.marker_path(), b"C:\\somewhere");
    assert_eq!(
        move_holding_source(&w.homes, &w.key, &Scripted::new(&w), &TestEnv),
        MoveOutcome::Failed {
            reason: MoveFailure::MemoriesErased,
            retrying: false
        }
    );
    assert!(record(&w).pending_move.is_none());
    assert_stayed(&w, &target);
}

#[test]
fn a_folder_that_cannot_be_restricted_still_gets_the_memories() {
    let w = world();
    let target = requested(&w);
    let mut fs = Scripted::new(&w);
    fs.fail_restrict = true;
    assert_eq!(
        move_holding_source(&w.homes, &w.key, &fs, &TestEnv),
        MoveOutcome::Moved {
            to: target.clone(),
            not_restricted: true,
            old_copy_waiting: false
        }
    );
    assert_moved(&w, &target);
}

// ── recovery removes only what the move made ────────────────────────────

#[test]
fn a_folder_without_the_move_id_is_never_touched() {
    let w = world();
    let target = requested(&w);
    put(&target.join("holiday.jpg"), b"theirs");
    put(&target.join("vault.db"), b"not ours either");
    assert_eq!(
        move_holding_source(&w.homes, &w.key, &Scripted::new(&w), &TestEnv),
        MoveOutcome::Failed {
            reason: MoveFailure::FolderNotUsable,
            retrying: true
        }
    );
    assert_eq!(
        std::fs::read(target.join("holiday.jpg")).unwrap(),
        b"theirs"
    );
    assert!(target.join("vault.db").exists());
    assert_eq!(snapshot(&w.source), w.original);
}

#[test]
fn a_folder_with_another_moves_id_is_never_touched() {
    let w = world();
    let target = requested(&w);
    put(
        &target.join(MOVE_ID_FILE),
        pointer::new_id().unwrap().as_bytes(),
    );
    put(&target.join("vault.db"), b"another move's");
    assert!(matches!(
        move_holding_source(&w.homes, &w.key, &Scripted::new(&w), &TestEnv),
        MoveOutcome::Failed {
            reason: MoveFailure::FolderNotUsable,
            ..
        }
    ));
    assert!(target.join("vault.db").exists());
}

#[test]
fn an_empty_folder_left_at_the_target_is_removed_and_the_move_goes_on() {
    let w = world();
    let target = requested(&w);
    std::fs::create_dir_all(&target).unwrap();
    assert!(matches!(
        move_holding_source(&w.homes, &w.key, &Scripted::new(&w), &TestEnv),
        MoveOutcome::Moved { .. }
    ));
    assert_moved(&w, &target);
}

/// A crash while `.move-id` was being written: the folder holds only that
/// file, not yet renamed into place. It is this move's, and goes.
#[test]
fn a_folder_left_with_the_move_id_half_written_is_removed_and_the_move_goes_on() {
    let w = world();
    let target = requested(&w);
    let id = record(&w).pending_move.unwrap().move_id;
    put(
        &pointer::being_written(&target.join(MOVE_ID_FILE)),
        &id.as_bytes()[..10],
    );
    assert!(matches!(
        move_holding_source(&w.homes, &w.key, &Scripted::new(&w), &TestEnv),
        MoveOutcome::Moved { .. }
    ));
    assert_moved(&w, &target);
}

/// …but a file of that name that does not hold (the start of) this move's
/// ID is not this move's, and nothing there is touched.
#[test]
fn a_half_written_id_that_is_not_this_moves_is_never_touched() {
    let w = world();
    let target = requested(&w);
    let writing = pointer::being_written(&target.join(MOVE_ID_FILE));
    put(&writing, b"not-this-move");
    assert!(matches!(
        move_holding_source(&w.homes, &w.key, &Scripted::new(&w), &TestEnv),
        MoveOutcome::Failed {
            reason: MoveFailure::FolderNotUsable,
            ..
        }
    ));
    assert_eq!(std::fs::read(&writing).unwrap(), b"not-this-move");
}

#[test]
fn a_half_copy_is_removed_but_a_file_that_is_not_the_vaults_stays() {
    let w = world();
    let target = requested(&w);
    let id = record(&w).pending_move.unwrap().move_id;
    put(&target.join(MOVE_ID_FILE), id.as_bytes());
    put(&target.join("vault.db"), b"half");
    put(&target.join("lance/x"), b"half");
    put(&target.join("holiday.jpg"), b"theirs");
    assert!(matches!(
        move_holding_source(&w.homes, &w.key, &Scripted::new(&w), &TestEnv),
        MoveOutcome::Failed {
            reason: MoveFailure::FolderNotUsable,
            ..
        }
    ));
    assert!(!target.join("vault.db").exists() && !target.join("lance").exists());
    assert!(
        target.join("holiday.jpg").exists(),
        "never the person's own file"
    );
    assert_eq!(snapshot(&w.source), w.original);
}

// ── the old copy ──────────────────────────────────────────────────────────

#[test]
fn an_old_copy_that_cannot_be_removed_yet_is_removed_at_the_next_start() {
    let w = world();
    let target = requested(&w);
    let mut fs = Scripted::new(&w);
    fs.fail_remove = Some("graph.sealed");
    assert_eq!(
        move_holding_source(&w.homes, &w.key, &fs, &TestEnv),
        MoveOutcome::Moved {
            to: target.clone(),
            not_restricted: false,
            old_copy_waiting: true
        }
    );
    assert_eq!(location::resolve(&w.homes).unwrap().path(), target);
    assert!(record(&w).pending_cleanup.is_some());
    assert!(
        w.source.join(VAULT_ID_FILE).exists(),
        "kept until the rest is gone"
    );
    assert_eq!(
        clean_holding_old(&w.key, &real_fs(&w)),
        MoveOutcome::OldCopyRemoved
    );
    assert_moved(&w, &target);
}

#[test]
fn the_folder_in_use_is_never_cleaned_as_an_old_copy() {
    let w = world();
    let mut p = record(&w);
    p.pending_cleanup = Some(PendingCleanup {
        from: w.source.clone(),
        move_id: pointer::new_id().unwrap(),
    });
    pointer::write(&w.homes.pointer_path(), &p).unwrap();
    assert_eq!(
        clean_holding_old(&w.key, &real_fs(&w)),
        MoveOutcome::OldCopyRemoved
    );
    assert_eq!(
        snapshot(&w.source),
        w.original,
        "the live vault is untouched"
    );
    assert!(record(&w).pending_cleanup.is_none());
}

#[test]
fn delete_everything_also_removes_a_waiting_old_copy() {
    let w = world();
    let target = requested(&w);
    let mut fs = Scripted::new(&w);
    fs.fail_remove = Some("vault.db");
    move_holding_source(&w.homes, &w.key, &fs, &TestEnv);
    assert!(record(&w).pending_cleanup.is_some());
    assert_eq!(
        clean_old_copy_now(&w.homes, &w.key),
        MoveOutcome::OldCopyRemoved
    );
    assert_moved(&w, &target);
}
