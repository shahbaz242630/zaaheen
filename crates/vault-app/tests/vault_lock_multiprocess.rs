//! ADR-SEC-020 — the vault lock admits at most one owner across real
//! PROCESSES, and a killed owner's lock frees itself.
//!
//! The in-crate unit tests cover one process. The bug this lock was rewritten
//! to remove only exists between processes: Claude Desktop starts two to four
//! `zaaheen mcp serve` processes within milliseconds and kills them abruptly,
//! and the old create-then-reclaim sequence could let two of them own the
//! vault at once. So these tests re-run this test binary as child processes
//! and make them fight.
//!
//! **How a double owner is detected.** Inside its critical section each owner
//! creates a sentinel file with `create_new` and deletes it before unlocking.
//! `create_new` is atomic, so it can only fail with `AlreadyExists` if a second
//! owner is inside the critical section at the same moment. A shared counter
//! file would not work: two overlapping owners doing read-modify-write can
//! lose exactly the update that would reveal them.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use vault_app::{ConsolidatorLock, VAULT_LOCKFILE_NAME};
use vault_core::VaultError;

/// Selects what a child process does. Unset in a normal test run, so
/// [`child_worker`] is a no-op there.
const CHILD_ENV: &str = "VAULT_LOCK_TEST_CHILD";
/// The shared vault root the children fight over.
const ROOT_ENV: &str = "VAULT_LOCK_TEST_ROOT";

const CONTENDERS: usize = 5;
const CONTEND_WINDOW: Duration = Duration::from_millis(1000);
const SENTINEL: &str = "owner.sentinel";
const GO_FILE: &str = "go";

/// Entry point for child processes. Does nothing unless [`CHILD_ENV`] is set.
#[test]
fn child_worker() {
    let Ok(mode) = std::env::var(CHILD_ENV) else {
        return;
    };
    let root = PathBuf::from(std::env::var(ROOT_ENV).expect("child needs ROOT_ENV"));
    match mode.as_str() {
        "contend" => contend(&root),
        "hold" => hold(&root),
        other => panic!("unknown child mode {other:?}"),
    }
}

/// Acquire, prove exclusivity with the sentinel, release — repeatedly, for a
/// fixed window. Exit code 2 and a `VIOLATION` line mean two owners overlapped.
fn contend(root: &Path) {
    // Start barrier: the parent creates GO_FILE once every child is spawned,
    // so the windows overlap instead of running one after another.
    let barrier_deadline = Instant::now() + Duration::from_secs(3);
    while !root.join(GO_FILE).exists() {
        assert!(
            Instant::now() < barrier_deadline,
            "start barrier never opened"
        );
        std::thread::sleep(Duration::from_millis(1));
    }

    let pid = std::process::id();
    let deadline = Instant::now() + CONTEND_WINDOW;
    let mut acquired: u32 = 0;
    while Instant::now() < deadline {
        match ConsolidatorLock::try_acquire_named(root, VAULT_LOCKFILE_NAME) {
            Ok(guard) => {
                let sentinel = root.join(SENTINEL);
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&sentinel)
                {
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        println!("VIOLATION pid={pid}");
                        std::process::exit(2);
                    }
                    Err(e) => panic!("sentinel create failed: {e}"),
                }
                // Hold for 0-2 ms, varied per process and iteration, so
                // releases and acquires interleave in many different orders.
                let hold_us = u64::from((pid.wrapping_add(acquired.wrapping_mul(7919))) % 2000);
                std::thread::sleep(Duration::from_micros(hold_us));
                std::fs::remove_file(&sentinel).expect("remove sentinel");
                drop(guard);
                acquired += 1;
                // Give the others a turn rather than re-acquiring instantly.
                std::thread::sleep(Duration::from_micros(200));
            }
            Err(VaultError::ConsolidatorBusy(_)) => std::thread::yield_now(),
            Err(other) => panic!("unexpected lock error: {other:?}"),
        }
    }
    println!("ACQUIRED={acquired}");
}

/// Acquire, announce it, then wait to be killed.
fn hold(root: &Path) {
    let _guard = ConsolidatorLock::try_acquire_named(root, VAULT_LOCKFILE_NAME)
        .expect("hold child acquires the lock");
    println!("LOCKED");
    std::thread::sleep(Duration::from_secs(20));
}

/// Kills and reaps a child if the test ends early. Without it, a failed
/// assertion leaves a child running — still holding the lock, and still
/// holding whatever handles it inherited (in the gate script, the log file,
/// which then made every later gate step fail to open it).
struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("child present until consumed")
    }

    fn wait_with_output(mut self) -> std::process::Output {
        self.0
            .take()
            .expect("child present until consumed")
            .wait_with_output()
            .expect("wait for child")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_child(mode: &str, root: &Path, stderr: Stdio) -> ChildGuard {
    let child = Command::new(std::env::current_exe().expect("current_exe"))
        .args(["--exact", "child_worker", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, mode)
        .env(ROOT_ENV, root)
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .expect("spawn child test process");
    ChildGuard(Some(child))
}

/// The number after `ACQUIRED=`, wherever it appears. The test harness prints
/// `test child_worker ... ` and leaves the line open while the test runs, so a
/// child's markers land at the END of that line, never at its start.
fn acquired_count(stdout: &str) -> Option<u32> {
    let rest = stdout.split("ACQUIRED=").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[test]
fn the_marker_parser_finds_markers_mid_line() {
    assert_eq!(
        acquired_count("\nrunning 1 test\ntest child_worker ... ACQUIRED=42\nok\n"),
        Some(42)
    );
    assert_eq!(acquired_count("no marker"), None);
}

#[test]
fn at_most_one_process_owns_the_vault_at_any_moment() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let children: Vec<ChildGuard> = (0..CONTENDERS)
        .map(|_| spawn_child("contend", tmp.path(), Stdio::piped()))
        .collect();
    std::fs::write(tmp.path().join(GO_FILE), b"").expect("open the start barrier");

    let mut total: u32 = 0;
    let mut processes_that_acquired = 0;
    for child in children {
        let output = child.wait_with_output();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stdout.contains("VIOLATION"),
            "TWO PROCESSES OWNED THE VAULT AT ONCE:\n{stdout}"
        );
        assert!(
            output.status.success(),
            "contender exited with {:?}:\n{stdout}\n{stderr}",
            output.status
        );
        let acquired = acquired_count(&stdout).unwrap_or_else(|| {
            panic!("child did not report its acquisition count:\n{stdout}\n{stderr}")
        });
        total += acquired;
        if acquired > 0 {
            processes_that_acquired += 1;
        }
    }

    // Non-vacuity: a lock that nobody ever took, or that one process kept,
    // would pass the checks above without proving anything.
    assert!(
        total >= 50,
        "only {total} acquisitions across {CONTENDERS} processes; the test \
         did not exercise real contention"
    );
    assert!(
        processes_that_acquired >= 2,
        "only {processes_that_acquired} process(es) ever acquired; ownership \
         never moved between processes"
    );
}

#[test]
fn a_killed_owner_releases_the_lock_and_the_file_survives() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let mut child = spawn_child("hold", tmp.path(), Stdio::null());

    let stdout = child.child().stdout.take().expect("child stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            // Mid-line, after the harness's `test child_worker ... `.
            if line.contains("LOCKED") {
                let _ = tx.send(());
                return;
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(3))
        .expect("child never reported holding the lock");

    match ConsolidatorLock::try_acquire_named(tmp.path(), VAULT_LOCKFILE_NAME) {
        Err(VaultError::ConsolidatorBusy(_)) => {}
        other => panic!("a lock held by another LIVE process must be busy, got: {other:?}"),
    }

    // A hard kill — what Claude Desktop does to its servers. No Drop runs.
    child.child().kill().expect("kill child");
    child.child().wait().expect("reap child");

    // TerminateProcess returns before every handle is closed, and Windows
    // documents that lock release timing depends on system resources, so
    // allow a short grace period rather than asserting instantly.
    let deadline = Instant::now() + Duration::from_millis(1500);
    let guard = loop {
        match ConsolidatorLock::try_acquire_named(tmp.path(), VAULT_LOCKFILE_NAME) {
            Ok(guard) => break guard,
            Err(VaultError::ConsolidatorBusy(_)) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            other => panic!("a killed owner's lock must free itself, got: {other:?}"),
        }
    };
    assert!(
        tmp.path().join(VAULT_LOCKFILE_NAME).exists(),
        "the lockfile is never deleted, not even by a crash"
    );
    drop(guard);
}
