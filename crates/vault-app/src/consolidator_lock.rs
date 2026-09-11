//! Cross-process vault locks (RAII): the vault-owner lock (`.vault.lock`) and
//! the consolidation-run lock (`.consolidator.lock`).
//!
//! Memory Vault's locked-next-arc Step 4 contract: the consolidator runs at
//! most once per vault at any moment — scheduled nightly OR manual via
//! `vault-cli consolidate run`. Two callers must NOT clobber each other's
//! state (Phase 3 merges are per-merge transactional, but K-means topic
//! discovery + REPORT writes are not, and an overlap would race the
//! atomic-rename REPORT artifact write at Commit 4). The vault-owner lock
//! (ADR-SEC-002) is the stronger guarantee: at most one process has the
//! stores open at all.
//!
//! ## Mechanism (ADR-SEC-020 — replaces the ADR-SEC-012 reclaim)
//!
//! Open-or-create a lockfile that is **never deleted**, then take an OS lock
//! on it with [`File::try_lock`] (`LockFileEx` on Windows, `flock` on POSIX).
//! The OS releases the lock when the handle closes — including when the owner
//! crashes or is killed — so a stale lock cannot exist and there is nothing to
//! reclaim.
//!
//! ### What this replaces, and why
//!
//! The previous mechanism created the lockfile with `create_new` and, when it
//! already existed, asked whether its owner was still alive; if not, it
//! deleted the file by name and created a new one (ADR-SEC-012). That
//! probe-then-delete-then-create sequence had no identity check, and the
//! owner's handle shared DELETE access, so two processes reclaiming at once
//! could BOTH end up owning the vault: A deletes the stale file and creates
//! its own, then B — whose probe ran before A's create — deletes A's LIVE
//! lockfile and creates another. `Drop` had the same window, because it
//! closed its handle before deleting the file.
//!
//! That race was not rare. Claude Desktop kills its MCP servers abruptly and
//! restarts two to four of them within milliseconds, so the reclaim path ran
//! in ordinary use: `reclaiming a lock whose owner is gone` appears three
//! times in the founder's Claude log between 2026-09-03 and 2026-09-10. Two
//! owners is exactly the concurrent-writer corruption ADR-SEC-002 exists to
//! prevent. Found by adversarial design review, 2026-09-10.
//!
//! ### Why the new mechanism cannot produce two owners
//!
//! Ownership is decided by the kernel's lock on one file, not by who created
//! or deleted a name. The file is opened without DELETE sharing on Windows, so
//! nobody can unlink it while it is held, and the lock itself is atomic.
//! `tests/vault_lock_multiprocess.rs` pins it across real processes with a
//! `create_new` sentinel that only an overlapping second owner could trip.
//!
//! ### Upgrade interplay
//!
//! A pre-ADR-SEC-020 owner holds the file without WRITE sharing, so our open
//! fails with a sharing violation. That is mapped to "busy", never to a hard
//! error. In the other direction, the old stale-lock probe opens for write with
//! no sharing, which fails against our handle, so an old binary sees a new
//! owner as alive. Both directions are pinned by tests below.
//!
//! ### Platform scope
//!
//! Identical on every platform, which closes the gap the old mechanism left
//! (it could only detect dead owners on Windows). `flock` is advisory on
//! POSIX, which is sufficient because every process that touches a vault takes
//! this lock first.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

use vault_core::{VaultError, VaultResult};

/// `FILE_SHARE_READ | FILE_SHARE_WRITE`, deliberately WITHOUT
/// `FILE_SHARE_DELETE`.
///
/// **READ | WRITE** so a second would-be owner can still OPEN the file and get
/// a clean "would block" from the lock itself, which is the normal busy path.
///
/// **Not DELETE**, so nobody can unlink a lockfile while it is held. Deleting a
/// live lockfile by name is precisely how the old mechanism produced two
/// owners.
#[cfg(windows)]
const LOCK_SHARE_MODE: u32 = 0x0000_0001 | 0x0000_0002;

/// How many times to retry an open that fails with a sharing violation before
/// reporting busy. Antivirus scanners open files briefly without sharing; a
/// short retry keeps a scan from being reported as a busy vault.
const OPEN_ATTEMPTS: u32 = 5;

/// Delay between those retries.
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(20);

/// Filename of the consolidation-run lock under the vault root.
///
/// Hidden (leading-dot) so it doesn't appear in casual directory listings
/// of the user's vault; consistent with other dotfile conventions like
/// `.git/`. Persists after release by design (see the module docs).
pub(crate) const LOCKFILE_NAME: &str = ".consolidator.lock";

/// Filename of the vault-owner lock (ADR-SEC-002). At most one process owns a
/// vault at a time — this replaces the implicit single-writer guard the DuckDB
/// exclusive file lock provided before the graph moved in-memory (ADR-SEC-002).
/// Distinct from [`LOCKFILE_NAME`], which serializes consolidation runs.
pub const VAULT_LOCKFILE_NAME: &str = ".vault.lock";

/// RAII guard for a cross-process vault lock.
///
/// Acquired by [`Self::try_acquire`] / [`Self::try_acquire_named`]; released on
/// drop. The lockfile itself stays on disk — only the OS lock on it is
/// released.
///
/// Cloning is intentionally not implemented — multiple guards for the same
/// lockfile would defeat the single-writer invariant.
#[derive(Debug)]
pub struct ConsolidatorLock {
    path: PathBuf,
    /// The locked handle. Its existence IS the lock: the OS releases it when
    /// this handle closes, on drop or on process death.
    file: File,
}

impl ConsolidatorLock {
    /// Attempt to acquire the consolidation-run lock at
    /// `<vault_root>/.consolidator.lock`.
    ///
    /// # Errors
    ///
    /// - [`VaultError::ConsolidatorBusy`] — another live process holds it.
    /// - [`VaultError::Io`] — any other I/O failure (permissions, disk full,
    ///   parent directory missing, etc.).
    pub fn try_acquire(vault_root: &Path) -> VaultResult<Self> {
        Self::try_acquire_named(vault_root, LOCKFILE_NAME)
    }

    /// Like [`Self::try_acquire`] but with a caller-chosen lockfile name. Used
    /// for the vault-owner lock ([`VAULT_LOCKFILE_NAME`], ADR-SEC-002) — at most
    /// one owning process per vault — distinct from the consolidator run lock.
    ///
    /// # Errors
    ///
    /// Same as [`Self::try_acquire`].
    pub fn try_acquire_named(vault_root: &Path, lockfile_name: &str) -> VaultResult<Self> {
        let path = vault_root.join(lockfile_name);
        let Some(file) = open_lockfile(&path).map_err(VaultError::Io)? else {
            return Err(busy(&path));
        };
        if try_lock_exclusive(&file).map_err(VaultError::Io)? {
            Ok(Self { path, file })
        } else {
            Err(busy(&path))
        }
    }

    /// Path of the lockfile this guard owns. Exposed for diagnostics + tests.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ConsolidatorLock {
    fn drop(&mut self) {
        // Closing the handle would release the lock anyway, but Windows
        // documents that the release then happens "depending upon available
        // system resources" and recommends unlocking explicitly. The next
        // owner is usually waiting, so release it now.
        //
        // The lockfile is deliberately NOT deleted: deleting a lockfile by name
        // is the race this module was rewritten to remove.
        if let Err(e) = unlock(&self.file) {
            tracing::warn!(
                path = %self.path.display(),
                error = %e,
                "vault lock unlock failed on drop; the OS releases it when the \
                 handle closes"
            );
        }
    }
}

/// The error every busy path returns. The text says what is happening and
/// that it resolves itself — and no longer tells anyone to delete the file,
/// which would not release a live lock and was the old race's trigger.
fn busy(path: &Path) -> VaultError {
    VaultError::ConsolidatorBusy(format!(
        "lockfile at {} is held by another running Zaaheen process (an AI \
         app's vault connection, a maintenance run, or the daemon). It is \
         released automatically the moment that process exits.",
        path.display()
    ))
}

/// Open the lockfile for locking, creating it if needed. Never truncates and
/// never deletes.
///
/// Returns `Ok(None)` when the file is held open by something whose sharing
/// mode refuses ours — a pre-ADR-SEC-020 owner, or a scanner that keeps
/// failing after [`OPEN_ATTEMPTS`] tries. Both mean "someone else has it",
/// which the caller reports as busy.
fn open_lockfile(path: &Path) -> std::io::Result<Option<File>> {
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    opts.share_mode(LOCK_SHARE_MODE);

    let mut attempt = 1;
    loop {
        match opts.open(path) {
            Ok(file) => return Ok(Some(file)),
            Err(e) if is_sharing_violation(&e) => {
                if attempt >= OPEN_ATTEMPTS {
                    return Ok(None);
                }
                attempt += 1;
                std::thread::sleep(OPEN_RETRY_DELAY);
            }
            Err(e) => return Err(e),
        }
    }
}

/// `ERROR_SHARING_VIOLATION` (32) or `ERROR_LOCK_VIOLATION` (33): the file is
/// open elsewhere with a sharing mode that refuses ours.
#[cfg(windows)]
fn is_sharing_violation(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32 | 33))
}

/// POSIX has no sharing modes; an open never fails this way.
#[cfg(not(windows))]
fn is_sharing_violation(_e: &std::io::Error) -> bool {
    false
}

/// Take an exclusive, non-blocking OS lock. `Ok(false)` means another handle
/// holds it.
///
/// `File::try_lock` is stable since Rust 1.89. The toolchain is pinned to
/// 1.92 (`rust-toolchain.toml`), so it is always available; the workspace's
/// declared `rust-version` (1.81) is stale, and raising it is tracked as tech
/// debt because doing so re-arms MSRV-gated clippy lints across every crate.
#[allow(clippy::incompatible_msrv)]
fn try_lock_exclusive(file: &File) -> std::io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// Release the OS lock. Same MSRV note as [`try_lock_exclusive`].
#[allow(clippy::incompatible_msrv)]
fn unlock(file: &File) -> std::io::Result<()> {
    file.unlock()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn acquire_creates_the_lockfile() {
        let tmp = TempDir::new().unwrap();
        let guard = ConsolidatorLock::try_acquire(tmp.path()).unwrap();
        assert!(
            tmp.path().join(LOCKFILE_NAME).exists(),
            "lockfile MUST exist after a successful acquire"
        );
        assert_eq!(guard.path(), tmp.path().join(LOCKFILE_NAME));
    }

    /// A second handle — even in the same process — cannot take a held lock.
    /// `LockFileEx` locks are per handle and `flock` locks are per open file
    /// description, so this holds on every platform.
    #[test]
    fn a_second_acquire_is_busy_while_the_first_is_held() {
        let tmp = TempDir::new().unwrap();
        let _first = ConsolidatorLock::try_acquire(tmp.path()).unwrap();

        match ConsolidatorLock::try_acquire(tmp.path()) {
            Err(VaultError::ConsolidatorBusy(msg)) => assert!(
                msg.contains(LOCKFILE_NAME),
                "busy message MUST name the lockfile; got: {msg}"
            ),
            other => panic!("expected ConsolidatorBusy, got: {other:?}"),
        }
    }

    /// Release frees the lock but keeps the file. Deleting the file is what
    /// the old mechanism did, and it is how two owners happened.
    #[test]
    fn release_keeps_the_file_and_frees_the_lock() {
        let tmp = TempDir::new().unwrap();
        {
            let _first = ConsolidatorLock::try_acquire(tmp.path()).unwrap();
        }
        assert!(
            tmp.path().join(LOCKFILE_NAME).exists(),
            "the lockfile MUST survive release; it is never deleted"
        );
        let _second =
            ConsolidatorLock::try_acquire(tmp.path()).expect("acquire after release MUST succeed");
    }

    #[test]
    fn release_after_panic_unwind_frees_the_lock() {
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();

        let result = std::panic::catch_unwind(|| {
            let _guard = ConsolidatorLock::try_acquire(&tmp_path).unwrap();
            panic!("simulated inner failure mid-consolidation");
        });
        assert!(
            result.is_err(),
            "inner panic should propagate to catch_unwind"
        );

        let _retry = ConsolidatorLock::try_acquire(&tmp_path)
            .expect("acquire after panic-unwind drop MUST succeed");
    }

    /// What a crash leaves behind — a lockfile nobody has open, here with the
    /// old mechanism's payload in it — must not block anyone. No reclaim step
    /// exists or is needed: with no open handle there is no lock.
    #[test]
    fn a_leftover_lockfile_with_no_live_owner_does_not_block() {
        let tmp = TempDir::new().unwrap();
        for name in [LOCKFILE_NAME, VAULT_LOCKFILE_NAME] {
            std::fs::write(
                tmp.path().join(name),
                "pid=31092 acquired_at=2026-07-26T05:46:21.568342100+00:00\n",
            )
            .unwrap();
            let _guard = ConsolidatorLock::try_acquire_named(tmp.path(), name)
                .expect("a lockfile with no live owner MUST be acquirable");
        }
    }

    /// The consolidation lock is taken INSIDE a process that already owns the
    /// vault (`consolidate run`, and the nightly scheduler in `mcp serve`), so
    /// the two locks must never block each other.
    #[test]
    fn the_vault_lock_and_the_consolidator_lock_are_independent() {
        let tmp = TempDir::new().unwrap();
        let _owner = ConsolidatorLock::try_acquire_named(tmp.path(), VAULT_LOCKFILE_NAME)
            .expect("vault lock");
        let _run = ConsolidatorLock::try_acquire(tmp.path())
            .expect("holding the vault lock MUST NOT block the consolidator lock");
    }

    #[test]
    fn a_missing_parent_directory_is_an_io_error_not_busy() {
        let bogus_root = std::path::PathBuf::from("/this/path/definitely/does/not/exist/vault");
        match ConsolidatorLock::try_acquire(&bogus_root) {
            Err(VaultError::Io(_)) => {}
            other => panic!("expected VaultError::Io, got: {other:?}"),
        }
    }

    /// The busy text must not send a user off to delete the lockfile: that
    /// would not release a live lock, and deleting live lockfiles by name was
    /// the old race.
    #[test]
    fn the_busy_error_says_it_clears_itself_and_never_says_delete() {
        let tmp = TempDir::new().unwrap();
        let _held = ConsolidatorLock::try_acquire(tmp.path()).unwrap();

        match ConsolidatorLock::try_acquire(tmp.path()) {
            Err(VaultError::ConsolidatorBusy(msg)) => {
                assert!(
                    msg.contains("released automatically"),
                    "busy error must say the lock clears itself, got: {msg}"
                );
                assert!(
                    !msg.to_lowercase().contains("remov") && !msg.to_lowercase().contains("delet"),
                    "busy error must never advise removing the lockfile, got: {msg}"
                );
            }
            other => panic!("expected ConsolidatorBusy, got: {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    //   Windows sharing semantics and upgrade interplay
    // ------------------------------------------------------------------

    /// A held lockfile cannot be unlinked. This is the property whose absence
    /// let the old reclaim delete a live owner's lockfile.
    #[cfg(windows)]
    #[test]
    fn a_held_lockfile_cannot_be_deleted() {
        let tmp = TempDir::new().unwrap();
        let _held = ConsolidatorLock::try_acquire_named(tmp.path(), VAULT_LOCKFILE_NAME).unwrap();
        assert!(
            std::fs::remove_file(tmp.path().join(VAULT_LOCKFILE_NAME)).is_err(),
            "a held lockfile MUST refuse deletion"
        );
    }

    /// Something holding the file open with no sharing at all — a scanner, or
    /// anything else — reads as busy, not as a hard I/O error.
    #[cfg(windows)]
    #[test]
    fn an_exclusive_open_elsewhere_reads_as_busy() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(VAULT_LOCKFILE_NAME);
        let _exclusive = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(&path)
            .unwrap();

        match ConsolidatorLock::try_acquire_named(tmp.path(), VAULT_LOCKFILE_NAME) {
            Err(VaultError::ConsolidatorBusy(_)) => {}
            other => panic!("an incompatible open MUST read as busy, got: {other:?}"),
        }
    }

    /// A pre-ADR-SEC-020 owner (still running during an upgrade) opened the
    /// file with `create_new`, write access and READ|DELETE sharing. A new
    /// binary must see that as busy.
    #[cfg(windows)]
    #[test]
    fn an_old_scheme_owner_reads_as_busy() {
        use std::io::Write;

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(VAULT_LOCKFILE_NAME);
        let mut old_owner = OpenOptions::new()
            .create_new(true)
            .write(true)
            .share_mode(0x0000_0001 | 0x0000_0004)
            .open(&path)
            .unwrap();
        old_owner
            .write_all(b"pid=4242 acquired_at=2026-09-10T12:39:57Z\n")
            .unwrap();

        match ConsolidatorLock::try_acquire_named(tmp.path(), VAULT_LOCKFILE_NAME) {
            Err(VaultError::ConsolidatorBusy(_)) => {}
            other => panic!("an old-scheme owner MUST read as busy, got: {other:?}"),
        }
    }

    /// The other direction: an old binary's stale-lock probe (open for write,
    /// no sharing) must FAIL against a new owner, so the old binary treats the
    /// new owner as alive and reports busy instead of reclaiming.
    #[cfg(windows)]
    #[test]
    fn the_old_reclaim_probe_sees_a_new_owner_as_alive() {
        let tmp = TempDir::new().unwrap();
        let _held = ConsolidatorLock::try_acquire_named(tmp.path(), VAULT_LOCKFILE_NAME).unwrap();

        let probe = OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(tmp.path().join(VAULT_LOCKFILE_NAME));
        assert!(
            probe.is_err(),
            "the old holder_is_gone probe MUST fail against a live new owner"
        );
    }
}
