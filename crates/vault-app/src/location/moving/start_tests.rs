//! ADR-105 L5 at a start: M1 (taking the vault, waiting for a window that is
//! closing), the dispatch, and the file-system edges a move must refuse.

use std::path::Path;
use std::time::Duration;

#[cfg(windows)]
use super::fs::database_in_use;
use super::tests::*;
use super::*;
use crate::keychain::test_helpers::test_location;
use crate::location::Homes;

fn put(path: &Path, content: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

// ── links and open files ─────────────────────────────────────────────────

#[test]
fn a_link_inside_the_vault_is_never_followed() {
    let w = world();
    let target = requested(&w);
    let outside = w.tmp.path().join("outside");
    put(&outside.join("secret.txt"), b"not the vault's");
    let link = w.source.join("lance").join("link");
    if !make_dir_link(&outside, &link) {
        eprintln!("skipped: this machine cannot make a folder link");
        return;
    }
    assert_eq!(
        move_holding_source(&w.homes, &w.key, &Scripted::new(&w), &TestEnv),
        MoveOutcome::Failed {
            reason: MoveFailure::CopyFailed,
            retrying: true
        }
    );
    assert!(!target.exists());
    assert!(outside.join("secret.txt").exists());
}

#[cfg(unix)]
fn make_dir_link(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

/// A junction needs no special rights on Windows (a symbolic link does).
#[cfg(windows)]
fn make_dir_link(target: &Path, link: &Path) -> bool {
    std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(windows)]
#[test]
fn an_open_database_is_seen() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(
        !database_in_use(tmp.path()).unwrap(),
        "no database: not in use"
    );
    put(&tmp.path().join("vault.db"), b"x");
    assert!(!database_in_use(tmp.path()).unwrap());
    let open = std::fs::File::open(tmp.path().join("vault.db")).unwrap();
    assert!(database_in_use(tmp.path()).unwrap(), "a window has it open");
    drop(open);
    assert!(!database_in_use(tmp.path()).unwrap());
}

// ── the start (M1 and the dispatch) ──────────────────────────────────────

struct NoKey;

impl MasterKeySource for NoKey {
    fn read(&self) -> Option<zeroize::Zeroizing<[u8; 32]>> {
        None
    }
}

pub(super) async fn start(w: &World, wait: Duration) -> MoveOutcome {
    start_reporting(w, wait, &Arc::new(MoveProgress::default())).await
}

/// A start whose move reports to `progress` (ADR-105 L-f).
pub(super) async fn start_reporting(
    w: &World,
    wait: Duration,
    progress: &Arc<MoveProgress>,
) -> MoveOutcome {
    run_pending_with(
        &w.homes,
        &w.key,
        &NoKey,
        TestEnv,
        wait,
        Duration::from_millis(100),
        progress,
    )
    .await
}

#[tokio::test]
async fn a_start_with_nothing_recorded_does_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let homes = Homes {
        local: tmp.path().join("Local"),
        roaming: tmp.path().join("Roaming"),
    };
    let key = test_location("move", tmp.path());
    let outcome = run_pending_with(
        &homes,
        &key,
        &NoKey,
        TestEnv,
        Duration::ZERO,
        Duration::ZERO,
        &Arc::new(MoveProgress::default()),
    )
    .await;
    assert_eq!(outcome, MoveOutcome::Nothing);
    assert!(!homes.local.exists(), "nothing is created");
}

#[tokio::test]
async fn a_start_moves_the_memories() {
    let w = world();
    let target = requested(&w);
    let outcome = start(&w, Duration::from_secs(5)).await;
    assert!(matches!(outcome, MoveOutcome::Moved { .. }), "{outcome:?}");
    assert_moved(&w, &target);
}

/// Another window holds the vault to itself (it is moving or erasing):
/// this one must not open the memories, and counts nothing.
#[tokio::test]
async fn a_second_window_is_told_busy() {
    let w = world();
    requested(&w);
    let _other = crate::keeper::intent::IntentGuard::acquire(&w.source, Duration::ZERO).unwrap();
    assert_eq!(
        start(&w, Duration::from_millis(300)).await,
        MoveOutcome::Busy
    );
    assert_eq!(record(&w).pending_move.unwrap().attempts, 0);
}

/// A connection that does not let go, or a maintenance run: the move waits
/// for another start, and the memories open where they are.
#[tokio::test]
async fn a_vault_that_cannot_be_had_defers_the_move() {
    let w = world();
    let target = requested(&w);
    let _held =
        crate::ConsolidatorLock::try_acquire_named(&w.source, crate::VAULT_LOCKFILE_NAME).unwrap();
    assert_eq!(
        start(&w, Duration::from_millis(300)).await,
        MoveOutcome::Deferred
    );
    assert_eq!(record(&w).pending_move.unwrap().attempts, 0);
    assert_stayed(&w, &target);
}

#[cfg(windows)]
#[tokio::test]
async fn the_move_waits_for_a_closing_window() {
    let w = world();
    let target = requested(&w);
    let open = std::fs::File::open(w.source.join("vault.db")).unwrap();
    let closing = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        drop(open);
    });
    let outcome = start(&w, Duration::from_secs(5)).await;
    closing.join().unwrap();
    assert!(matches!(outcome, MoveOutcome::Moved { .. }), "{outcome:?}");
    assert_moved(&w, &target);
}

#[cfg(windows)]
#[tokio::test]
async fn a_window_that_stays_open_defers_the_move() {
    let w = world();
    let target = requested(&w);
    let _open = std::fs::File::open(w.source.join("vault.db")).unwrap();
    assert_eq!(
        start(&w, Duration::from_millis(300)).await,
        MoveOutcome::Deferred
    );
    assert_stayed(&w, &target);
}
