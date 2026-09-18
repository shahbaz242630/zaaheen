//! The account folder and its four files (§8.26 §4):
//! `%LOCALAPPDATA%\com.zaaheen.app\account\`.
//!
//! | File | Holds | Written |
//! |---|---|---|
//! | `lease` | the signed lease bytes | atomically (temp + rename), under the lock |
//! | `state.json` | [`LocalState`] | atomically, merged with what is on disk, under the lock |
//! | `signed-in` | the signed-in user's `sub` | when a refresh token is stored; removed on sign-out |
//! | `refresh.lock` | nothing; its OS lock | never written, **never deleted** |
//!
//! Only the keeper and the desktop write here, never a relay. The folder lies
//! outside the vault root, so erasure never walks it.
//!
//! # The folder must already exist
//!
//! [`AccountDir::open`] refuses a folder that is not there. The caller
//! (vault-app) creates it and applies the ADR-SEC-019 grant-then-strip ACL
//! **before** the first write (§8.26 §4); a crate that created the folder
//! itself would open a window in which files land under the inherited ACL.
//!
//! # The lock
//!
//! `refresh.lock` is locked with `File::try_lock` (`LockFileEx` on Windows,
//! `flock` on POSIX), the ADR-SEC-020 pattern: a lock on a file that is never
//! deleted, released by the OS if the holder dies. Waiting is bounded
//! (§8.26 §4: at most 2 s inside a call).
//!
//! Everything here is synchronous; async callers run it under
//! `spawn_blocking` (BRD §2.8).

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read};
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::entitlement::LocalState;
use crate::lease::MAX_LEASE_BYTES;

/// Share mode for `refresh.lock` on Windows: `FILE_SHARE_READ |
/// FILE_SHARE_WRITE`, without `FILE_SHARE_DELETE`, so the file cannot be
/// deleted while anyone has it open (the ADR-SEC-020 pattern).
#[cfg(windows)]
const LOCK_SHARE_MODE: u32 = 0x0000_0001 | 0x0000_0002;

/// Retries for opens and renames that an antivirus scanner briefly blocks.
const BLOCKED_ATTEMPTS: u32 = 5;
const BLOCKED_RETRY_DELAY: Duration = Duration::from_millis(20);

/// Longest `sub` accepted in the marker (as for userinfo and the lease).
const MAX_SUB_LEN: usize = 256;

/// `lease` file name.
pub const LEASE_FILE: &str = "lease";
/// `state.json` file name.
pub const STATE_FILE: &str = "state.json";
/// `signed-in` marker file name.
pub const MARKER_FILE: &str = "signed-in";
/// `refresh.lock` file name.
pub const LOCK_FILE: &str = "refresh.lock";

/// Largest `state.json` or marker read back; anything larger is damage.
const MAX_SMALL_FILE: u64 = 4096;

/// Poll interval while waiting for the lock.
const LOCK_POLL: Duration = Duration::from_millis(25);

/// The account folder.
#[derive(Clone, Debug)]
pub struct AccountDir {
    dir: PathBuf,
}

impl AccountDir {
    /// Use `dir`, which must already exist (see the module docs).
    ///
    /// # Errors
    ///
    /// `NotFound` if `dir` does not exist or is not a folder.
    pub fn open(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        if !std::fs::metadata(&dir)?.is_dir() {
            return Err(std::io::Error::new(
                ErrorKind::NotFound,
                "the account folder is not a folder",
            ));
        }
        Ok(Self { dir })
    }

    /// The folder.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Take `refresh.lock`, waiting up to `wait`. `Ok(None)` means another
    /// holder kept it for the whole wait.
    ///
    /// # Errors
    ///
    /// I/O failures other than "held by someone else".
    pub fn lock(&self, wait: Duration) -> std::io::Result<Option<RefreshLock>> {
        let Some(file) = open_lock_file(&self.dir.join(LOCK_FILE))? else {
            return Ok(None);
        };
        let deadline = Instant::now() + wait;
        loop {
            if try_lock_exclusive(&file)? {
                return Ok(Some(RefreshLock { file }));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            std::thread::sleep(LOCK_POLL.min(deadline - now));
        }
    }

    /// The `lease` file's bytes, or `None` when there is none or it is larger
    /// than any lease could be.
    ///
    /// # Errors
    ///
    /// I/O failures other than "not found".
    pub fn read_lease(&self) -> std::io::Result<Option<Vec<u8>>> {
        read_capped(&self.dir.join(LEASE_FILE), MAX_LEASE_BYTES as u64)
    }

    /// Replace the `lease` file atomically. Call with the lock held.
    ///
    /// # Errors
    ///
    /// I/O failures.
    pub fn write_lease(&self, _lock: &RefreshLock, wire: &[u8]) -> std::io::Result<()> {
        self.write_atomic(LEASE_FILE, wire)
    }

    /// The local record. Missing, unreadable or damaged reads as the default
    /// ("absent"), which only lowers the floor to now (§8.26 §4).
    pub fn read_state(&self) -> LocalState {
        match read_capped(&self.dir.join(STATE_FILE), MAX_SMALL_FILE) {
            Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                tracing::warn!("account state.json is damaged; reading it as absent");
                LocalState::default()
            }),
            Ok(None) => LocalState::default(),
            Err(e) => {
                tracing::warn!(error = %e, "account state.json unreadable; reading it as absent");
                LocalState::default()
            }
        }
    }

    /// Merge `mine` with the record on disk (§8.26 §4 rules) and write the
    /// result atomically. Call with the lock held.
    ///
    /// # Errors
    ///
    /// I/O failures.
    pub fn write_state(&self, lock: &RefreshLock, mine: &LocalState) -> std::io::Result<()> {
        let merged = LocalState::merge(&self.read_state(), mine);
        self.reset_state(lock, &merged)
    }

    /// Replace the record outright (sign-in starts a fresh one). Call with
    /// the lock held.
    ///
    /// # Errors
    ///
    /// I/O failures.
    pub fn reset_state(&self, _lock: &RefreshLock, state: &LocalState) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(state).map_err(std::io::Error::other)?;
        self.write_atomic(STATE_FILE, &bytes)
    }

    /// The signed-in user's `sub`, or `None` when nobody is signed in on this
    /// computer (or the marker is damaged).
    ///
    /// # Errors
    ///
    /// I/O failures other than "not found".
    pub fn read_marker(&self) -> std::io::Result<Option<String>> {
        let Some(bytes) = read_capped(&self.dir.join(MARKER_FILE), MAX_SMALL_FILE)? else {
            return Ok(None);
        };
        let sub = String::from_utf8(bytes).ok().filter(|s| {
            !s.is_empty() && s.len() <= MAX_SUB_LEN && s.bytes().all(|b| b.is_ascii_graphic())
        });
        if sub.is_none() {
            tracing::warn!("account marker is damaged; reading it as signed out");
        }
        Ok(sub)
    }

    /// Write the marker. Call with the lock held.
    ///
    /// # Errors
    ///
    /// I/O failures.
    pub fn write_marker(&self, _lock: &RefreshLock, sub: &str) -> std::io::Result<()> {
        self.write_atomic(MARKER_FILE, sub.as_bytes())
    }

    /// Remove the marker, the lease and the record (sign-out). Missing files
    /// are fine. `refresh.lock` stays. Call with the lock held.
    ///
    /// The marker goes first: relays read only the marker, so from that
    /// moment every new AI call gets the sign-in message.
    ///
    /// # Errors
    ///
    /// I/O failures other than "not found".
    pub fn clear(&self, _lock: &RefreshLock) -> std::io::Result<()> {
        for name in [MARKER_FILE, LEASE_FILE, STATE_FILE] {
            remove_if_present(&self.dir.join(name))?;
        }
        Ok(())
    }

    /// Remove just the lease (a new user is signing in). Call with the lock
    /// held.
    ///
    /// # Errors
    ///
    /// I/O failures other than "not found".
    pub fn remove_lease(&self, _lock: &RefreshLock) -> std::io::Result<()> {
        remove_if_present(&self.dir.join(LEASE_FILE))
    }

    /// Write `bytes` to `name` through a temp file and a rename, so a reader
    /// sees the old file or the new one, never a torn write. Writers hold the
    /// lock, so one temp name per process is enough.
    fn write_atomic(&self, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        let target = self.dir.join(name);
        let tmp = self.dir.join(format!("{name}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        let mut attempt = 1;
        loop {
            match std::fs::rename(&tmp, &target) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == ErrorKind::PermissionDenied && attempt < BLOCKED_ATTEMPTS => {
                    attempt += 1;
                    std::thread::sleep(BLOCKED_RETRY_DELAY);
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e);
                }
            }
        }
    }
}

/// Holding `refresh.lock`. Released on drop; the file is never deleted.
#[derive(Debug)]
pub struct RefreshLock {
    file: File,
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        // Closing the handle would release the lock too, but Windows then
        // releases it "depending upon available system resources"; the next
        // refresher is usually waiting, so release it now.
        if let Err(e) = unlock(&self.file) {
            tracing::warn!(error = %e, "refresh.lock unlock failed; the OS releases it on close");
        }
    }
}

/// Read at most `max` bytes of `path`. `None` when the file is missing or
/// larger than `max` (damage, or not ours).
fn read_capped(path: &Path, max: u64) -> std::io::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Ok(None);
    }
    Ok(Some(bytes))
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Open `refresh.lock`, creating it if needed; never truncates or deletes.
/// `None` when a scanner (or anything opening it without sharing) keeps it
/// blocked through the retries: someone else has it, so the caller skips.
fn open_lock_file(path: &Path) -> std::io::Result<Option<File>> {
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    opts.share_mode(LOCK_SHARE_MODE);
    let mut attempt = 1;
    loop {
        match opts.open(path) {
            Ok(file) => return Ok(Some(file)),
            Err(e) if is_sharing_violation(&e) => {
                if attempt >= BLOCKED_ATTEMPTS {
                    return Ok(None);
                }
                attempt += 1;
                std::thread::sleep(BLOCKED_RETRY_DELAY);
            }
            Err(e) => return Err(e),
        }
    }
}

/// `ERROR_SHARING_VIOLATION` (32) or `ERROR_LOCK_VIOLATION` (33).
#[cfg(windows)]
fn is_sharing_violation(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32 | 33))
}

/// POSIX has no sharing modes; an open never fails this way.
#[cfg(not(windows))]
fn is_sharing_violation(_e: &std::io::Error) -> bool {
    false
}

/// Exclusive, non-blocking OS lock; `Ok(false)` when another handle holds it.
///
/// `File::try_lock` is stable since Rust 1.89 and the toolchain is pinned to
/// 1.92; the workspace's declared `rust-version` (1.81) is stale, as noted at
/// the same allow in vault-app's `consolidator_lock.rs`.
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
    use tempfile::TempDir;

    use super::*;

    const FAST: Duration = Duration::from_millis(200);

    fn dir() -> (TempDir, AccountDir) {
        let tmp = TempDir::new().unwrap();
        let account = AccountDir::open(tmp.path()).unwrap();
        (tmp, account)
    }

    fn files_in(tmp: &TempDir) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    // ---- the folder ------------------------------------------------------------------

    #[test]
    fn a_missing_folder_is_refused_not_created() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("account");
        assert!(AccountDir::open(&missing).is_err());
        assert!(
            !missing.exists(),
            "the folder must be prepared (ACL) by the caller"
        );
        let file = tmp.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        assert!(AccountDir::open(&file).is_err());
    }

    // ---- the lock --------------------------------------------------------------------

    #[test]
    fn the_lock_is_exclusive_and_waits_are_bounded() {
        let (_tmp, account) = dir();
        let held = account.lock(FAST).unwrap().expect("free lock");
        let started = Instant::now();
        assert!(
            account.lock(FAST).unwrap().is_none(),
            "second holder refused"
        );
        let waited = started.elapsed();
        assert!(waited >= FAST, "waited the full bound: {waited:?}");
        assert!(waited < FAST * 5, "but not much longer: {waited:?}");
        drop(held);
        assert!(
            account.lock(FAST).unwrap().is_some(),
            "free again after drop"
        );
    }

    #[test]
    fn a_waiter_gets_the_lock_when_it_is_released() {
        let (_tmp, account) = dir();
        let held = account.lock(FAST).unwrap().unwrap();
        let other = account.clone();
        let waiter = std::thread::spawn(move || other.lock(Duration::from_secs(3)).unwrap());
        std::thread::sleep(Duration::from_millis(150));
        drop(held);
        assert!(waiter.join().unwrap().is_some());
    }

    #[test]
    fn the_lock_file_is_never_deleted() {
        let (tmp, account) = dir();
        let lock = account.lock(FAST).unwrap().unwrap();
        account.clear(&lock).unwrap();
        drop(lock);
        assert!(tmp.path().join(LOCK_FILE).exists());
    }

    // ---- the lease file --------------------------------------------------------------

    #[test]
    fn the_lease_round_trips_and_leaves_no_temp_files() {
        let (tmp, account) = dir();
        assert_eq!(account.read_lease().unwrap(), None);
        let lock = account.lock(FAST).unwrap().unwrap();
        account.write_lease(&lock, b"payload.signature").unwrap();
        account.write_lease(&lock, b"newer.lease").unwrap();
        assert_eq!(
            account.read_lease().unwrap().as_deref(),
            Some(&b"newer.lease"[..])
        );
        assert_eq!(files_in(&tmp), vec![LEASE_FILE, LOCK_FILE]);
    }

    #[test]
    fn an_oversized_lease_file_reads_as_none() {
        let (tmp, account) = dir();
        std::fs::write(tmp.path().join(LEASE_FILE), vec![b'a'; MAX_LEASE_BYTES + 1]).unwrap();
        assert_eq!(account.read_lease().unwrap(), None);
    }

    // ---- state.json ------------------------------------------------------------------

    fn record(lease_issued_at: i64, floor: i64, active: i64, attempt: i64) -> LocalState {
        LocalState {
            lease_issued_at,
            floor,
            last_active_anchor: active,
            last_refresh_attempt: attempt,
        }
    }

    #[test]
    fn a_missing_or_damaged_record_reads_as_absent() {
        let (tmp, account) = dir();
        assert_eq!(account.read_state(), LocalState::default());
        std::fs::write(tmp.path().join(STATE_FILE), b"{not json").unwrap();
        assert_eq!(account.read_state(), LocalState::default());
        std::fs::write(tmp.path().join(STATE_FILE), vec![b' '; 10_000]).unwrap();
        assert_eq!(account.read_state(), LocalState::default());
    }

    #[test]
    fn writes_merge_with_what_another_writer_left() {
        let (_tmp, account) = dir();
        let lock = account.lock(FAST).unwrap().unwrap();
        account
            .write_state(&lock, &record(100, 150, 90, 400))
            .unwrap();
        // A second writer holding the same lease, with a lower floor and a
        // later activity: max() on every field.
        account
            .write_state(&lock, &record(100, 120, 95, 300))
            .unwrap();
        assert_eq!(account.read_state(), record(100, 150, 95, 400));
        // A writer holding an older lease never changes the floor.
        account
            .write_state(&lock, &record(50, 999, 10, 10))
            .unwrap();
        assert_eq!(account.read_state(), record(100, 150, 95, 400));
    }

    #[test]
    fn reset_replaces_the_record_outright() {
        let (_tmp, account) = dir();
        let lock = account.lock(FAST).unwrap().unwrap();
        account
            .write_state(&lock, &record(100, 150, 90, 400))
            .unwrap();
        account
            .reset_state(&lock, &record(200, 200, 200, 0))
            .unwrap();
        assert_eq!(account.read_state(), record(200, 200, 200, 0));
    }

    // ---- the marker ------------------------------------------------------------------

    #[test]
    fn the_marker_round_trips_the_sub() {
        let (_tmp, account) = dir();
        assert_eq!(account.read_marker().unwrap(), None);
        let lock = account.lock(FAST).unwrap().unwrap();
        account.write_marker(&lock, "user_2abc").unwrap();
        assert_eq!(account.read_marker().unwrap().as_deref(), Some("user_2abc"));
    }

    #[test]
    fn a_damaged_marker_reads_as_signed_out() {
        let (tmp, account) = dir();
        for bad in [
            &b""[..],
            b"user 2",
            b"user\n2",
            &[0xff, 0xfe][..],
            &[b'u'; 300][..],
        ] {
            std::fs::write(tmp.path().join(MARKER_FILE), bad).unwrap();
            assert_eq!(account.read_marker().unwrap(), None, "{bad:?}");
        }
    }

    // ---- sign-out ----------------------------------------------------------------------

    #[test]
    fn clear_removes_everything_but_the_lock_and_is_idempotent() {
        let (tmp, account) = dir();
        let lock = account.lock(FAST).unwrap().unwrap();
        account.write_marker(&lock, "user_2abc").unwrap();
        account.write_lease(&lock, b"a.b").unwrap();
        account.write_state(&lock, &record(1, 2, 3, 4)).unwrap();
        account.clear(&lock).unwrap();
        account.clear(&lock).unwrap();
        assert_eq!(files_in(&tmp), vec![LOCK_FILE]);
        assert_eq!(account.read_marker().unwrap(), None);
        assert_eq!(account.read_lease().unwrap(), None);
    }

    #[test]
    fn remove_lease_touches_nothing_else() {
        let (tmp, account) = dir();
        let lock = account.lock(FAST).unwrap().unwrap();
        account.write_marker(&lock, "user_2abc").unwrap();
        account.write_lease(&lock, b"a.b").unwrap();
        account.remove_lease(&lock).unwrap();
        account.remove_lease(&lock).unwrap();
        assert_eq!(files_in(&tmp), vec![LOCK_FILE, MARKER_FILE]);
    }
}
