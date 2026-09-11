//! "Someone needs the vault to themselves" — the exclusive-access intent
//! (ADR-102 amendment, session 35).
//!
//! **Why.** "Delete everything" must not run while a keeper is serving: the
//! keeper holds the vault's keys in memory, so an AI app could keep reading
//! (and writing) memories the user has just deleted until the keeper happened
//! to exit. The eraser therefore asks the keeper to hand the vault over — but
//! AI apps keep asking Windows to start a keeper, and a fresh one could take
//! the vault in the gap. This file closes that gap: while an eraser holds it,
//! no new keeper starts.
//!
//! **Mechanism.** The same never-deleted lockfile and OS lock as
//! `.vault.lock` (ADR-SEC-020). The eraser holds it EXCLUSIVELY
//! ([`IntentGuard`]). A keeper only PROBES it with a shared lock
//! ([`is_held`]): shared probes never block each other, so two keepers
//! starting at once cannot mistake each other for an eraser. A keeper probes
//! again after taking `.vault.lock`, which closes the race with an eraser that
//! arrives between the first probe and the lock.

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

use vault_core::{VaultError, VaultResult};

use crate::ConsolidatorLock;

/// File name under the vault root. Declared in `erasure::VAULT_LOCK_FILES`.
pub const INTENT_FILE: &str = ".vault.intent";

/// `FILE_SHARE_READ | FILE_SHARE_WRITE`: compatible with the holder's handle
/// (see `consolidator_lock`), so a probe can always open the file.
#[cfg(windows)]
const PROBE_SHARE_MODE: u32 = 0x0000_0001 | 0x0000_0002;

/// Held for as long as exclusive access is wanted. Dropping it lets keepers
/// start again.
#[derive(Debug)]
pub struct IntentGuard {
    _lock: ConsolidatorLock,
}

impl IntentGuard {
    /// Declare the intent, waiting up to `wait` for keepers' brief probes to
    /// clear.
    ///
    /// # Errors
    ///
    /// [`VaultError::ConsolidatorBusy`] when another eraser holds it for the
    /// whole wait; [`VaultError::Io`] for anything else.
    pub fn acquire(vault_root: &Path, wait: Duration) -> VaultResult<Self> {
        let deadline = Instant::now() + wait;
        loop {
            match ConsolidatorLock::try_acquire_named(vault_root, INTENT_FILE) {
                Ok(lock) => return Ok(Self { _lock: lock }),
                Err(VaultError::ConsolidatorBusy(_)) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// `true` while someone holds the intent. A missing file, or any failure to
/// probe, reads as "not held": the keeper then starts as it always has, and
/// the eraser's own lock wait still protects the erasure.
pub fn is_held(vault_root: &Path) -> bool {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    opts.share_mode(PROBE_SHARE_MODE);
    match opts.open(vault_root.join(INTENT_FILE)) {
        Ok(file) => probe_is_blocked(&file),
        Err(_) => false,
    }
}

/// Take and immediately release a SHARED lock. Blocked means an exclusive
/// holder exists. `File::try_lock_shared` is stable since 1.89; see the MSRV
/// note in `consolidator_lock`.
#[allow(clippy::incompatible_msrv)]
fn probe_is_blocked(file: &File) -> bool {
    match file.try_lock_shared() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn no_file_means_not_held() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_held(tmp.path()));
    }

    #[test]
    fn held_while_the_guard_lives_and_free_after() {
        let tmp = TempDir::new().unwrap();
        {
            let _guard = IntentGuard::acquire(tmp.path(), Duration::ZERO).unwrap();
            assert!(is_held(tmp.path()));
        }
        assert!(
            !is_held(tmp.path()),
            "dropping the guard releases the intent"
        );
    }

    /// Keepers probing at the same moment must not read each other as an
    /// eraser — that would make every keeper in a start burst exit.
    #[test]
    fn concurrent_probes_do_not_block_each_other() {
        let tmp = TempDir::new().unwrap();
        drop(IntentGuard::acquire(tmp.path(), Duration::ZERO).unwrap());
        let mut opts = OpenOptions::new();
        opts.read(true);
        #[cfg(windows)]
        opts.share_mode(PROBE_SHARE_MODE);
        let other = opts.open(tmp.path().join(INTENT_FILE)).unwrap();
        #[allow(clippy::incompatible_msrv)]
        other.try_lock_shared().unwrap();
        assert!(
            !is_held(tmp.path()),
            "a shared probe in flight is not an eraser"
        );
    }

    /// A second eraser waits, then reports busy rather than sharing.
    #[test]
    fn a_second_guard_is_busy() {
        let tmp = TempDir::new().unwrap();
        let _first = IntentGuard::acquire(tmp.path(), Duration::ZERO).unwrap();
        assert!(matches!(
            IntentGuard::acquire(tmp.path(), Duration::from_millis(50)),
            Err(VaultError::ConsolidatorBusy(_))
        ));
    }
}
