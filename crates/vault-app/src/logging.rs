//! On-disk application log (ADR-SEC-014; moved here by ADR-SEC-015).
//!
//! # Why this exists
//!
//! Until now the desktop app called `tracing_subscriber::fmt::init()`, which
//! writes to **stdout**. A GUI process launched from Explorer or the Start menu
//! has no console attached, so every `tracing` event this application has ever
//! emitted went nowhere.
//!
//! # Why it lives in `vault-app` rather than the desktop app
//!
//! ADR-SEC-015 made the nightly maintenance run **windowless**, which removed
//! the last place its output could have gone: a scheduled console process with
//! no console has no stderr either. The run that operates on the vault
//! unattended is precisely the one whose logs we cannot afford to lose, so the
//! two binaries that need file logging — the desktop app and the maintenance
//! path — now share one implementation in the crate they both depend on.
//!
//! Sharing it also keeps `tests/log_privacy.rs` meaningful: one writer, one set
//! of rules, one place to check them.
//!
//! That was survivable while the only user was the founder, sitting at the
//! machine, able to be asked questions directly. It stops being survivable at
//! beta: a tester reports "it broke", and there is no file to ask them for, no
//! error history, and no way to see what the app was doing. We would be
//! debugging blind on a machine we cannot inspect.
//!
//! It already cost us. On 2026-08-26 a queued vector-delete failed to drain in
//! the running app. `RetryWorker` emits a `tracing` event on every step and
//! every failure — none of it was recorded, so the root cause could not be
//! read off a log and had to be narrowed by filesystem forensics instead.
//!
//! # The privacy constraint, which is not negotiable
//!
//! This product's promise is that memories stay private and encrypted at rest.
//! A log file is, by construction, **plaintext on disk**. Writing memory
//! content into it would recreate ADR-SEC-007 — the leak we spent this same day
//! fixing — with a different filename.
//!
//! So the rule is: **log what happened, never what was remembered.** Ids,
//! counts, durations, error kinds, boundary names — yes. Memory text, query
//! text, fact content, alias lists — never.
//!
//! That rule is enforced, not merely stated: `tests/log_privacy.rs` scans the
//! workspace's own source and fails the build if a `tracing` call site records
//! a field that carries memory content. A rule nobody checks is a rule that
//! lasts until the next person in a hurry.
//!
//! # What is deliberately NOT here
//!
//! No upload, no telemetry, no network. The file sits in the user's own log
//! directory and goes nowhere unless they choose to send it. For a
//! local-first, zero-knowledge product, a log that phones home would
//! contradict the entire proposition.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::fmt::MakeWriter;

/// Log filename inside the log directory.
pub const LOG_FILENAME: &str = "zaaheen.log";

/// Previous log, kept across one restart so a crash's final lines survive the
/// next launch. One generation only — enough to diagnose "it broke, I
/// reopened it", without unbounded growth on someone's disk.
pub const LOG_FILENAME_PREVIOUS: &str = "zaaheen.log.1";

/// Rotate when the live log passes this size.
///
/// 5 MiB holds a long session at INFO and is small enough to attach to an
/// email, which is how a beta tester will actually send it.
pub const ROTATE_AT_BYTES: u64 = 5 * 1024 * 1024;

/// Default filter when `RUST_LOG` is unset.
///
/// INFO for our own crates, WARN for everything else — dependency chatter at
/// INFO (lance, datafusion, hyper) would bury the lines that matter and blow
/// the rotation budget in minutes.
///
/// `vault_embedding` was missing until ADR-103 D3: session 36's live test
/// could not tell when the keeper's ranking model finished loading, because
/// its load and warm-up lines were filtered out. They carry no memory text
/// (static messages; every span on that crate skips its text arguments).
pub const DEFAULT_FILTER: &str = "warn,vault_app=info,vault_tauri=info,vault_storage=info,\
                                  vault_retrieval=info,vault_consolidator=info,vault_mcp=info,\
                                  vault_scheduler=info,vault_cli=info,vault_maintenance=info,\
                                  vault_embedding=info";

/// Environment variable that redirects a console binary's logs into the shared
/// application log file (ADR-SEC-015).
///
/// The windowless maintenance path sets this on the `vault-cli` child it
/// spawns. Without it that child logs to stderr, which a process created with
/// `CREATE_NO_WINDOW` has nowhere to show — the nightly run would be silent
/// exactly when something went wrong overnight.
pub const LOG_DIR_ENV: &str = "VAULT_LOG_DIR";

/// A log file behind a mutex, usable as a `tracing` writer.
///
/// `tracing` events arrive from arbitrary threads, so writes must be
/// serialised; without the mutex two concurrent events interleave mid-line and
/// the log becomes unreadable exactly when it is needed most.
struct LogFile(Mutex<File>);

/// Write guard handed to one `tracing` event.
struct LogFileGuard<'a>(std::sync::MutexGuard<'a, File>);

impl Write for LogFileGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<'a> MakeWriter<'a> for LogFile {
    type Writer = LogFileGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        // A poisoned mutex means some other thread panicked mid-write. The
        // log is diagnostic, not transactional: recovering the guard and
        // carrying on is strictly better than losing all logging from the
        // first panic onward -- which is precisely when the log matters.
        LogFileGuard(
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

/// Rotate `live` to `previous` when it has grown past [`ROTATE_AT_BYTES`].
///
/// Best-effort and deliberately non-fatal: if rotation fails the app keeps
/// logging to the existing file. A failed rotation must never be the reason an
/// app will not start.
fn rotate_if_needed(live: &Path, previous: &Path) {
    let Ok(meta) = std::fs::metadata(live) else {
        return; // no log yet -- nothing to rotate
    };
    if meta.len() < ROTATE_AT_BYTES {
        return;
    }
    // `rename` replaces an existing destination on both Windows and POSIX, so
    // the previous generation is dropped rather than accumulating.
    if let Err(e) = std::fs::rename(live, previous) {
        eprintln!("zaaheen: log rotation failed ({e}); continuing on the existing file");
    }
}

/// Initialise file logging and return the path being written to.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] if the log directory cannot be created
/// or the log file cannot be opened. Callers should treat this as
/// non-fatal — an app that will not start because it could not open a log file
/// has made a diagnostic aid into an outage.
pub fn init(log_dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(log_dir)?;

    let live = log_dir.join(LOG_FILENAME);
    let previous = log_dir.join(LOG_FILENAME_PREVIOUS);
    rotate_if_needed(&live, &previous);

    let file = OpenOptions::new().create(true).append(true).open(&live)?;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_FILTER));

    // `with_ansi(false)`: colour escapes are noise in a file and make it
    // unreadable in Notepad, which is what a beta tester will open it with.
    let subscriber = tracing_subscriber::fmt()
        .with_writer(LogFile(Mutex::new(file)))
        .with_ansi(false)
        .with_target(true)
        .with_level(true)
        .with_env_filter(filter)
        .finish();

    // `try_init` rather than `init`: a second call must not panic the app.
    // Losing logging is bad; crashing on startup because logging was already
    // configured is worse.
    if let Err(e) = tracing::subscriber::set_global_default(subscriber) {
        eprintln!("zaaheen: tracing already initialised ({e}); file logging not installed");
    }

    Ok(live)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_creates_the_log_file_and_returns_its_path() {
        let tmp = TempDir::new().expect("temp dir");
        let dir = tmp.path().join("logs");
        let path = init(&dir).expect("init logging");

        assert_eq!(path, dir.join(LOG_FILENAME));
        assert!(path.exists(), "log file must exist after init");
    }

    #[test]
    fn init_creates_a_missing_log_directory() {
        // First run on a fresh machine: the directory does not exist yet.
        let tmp = TempDir::new().expect("temp dir");
        let nested = tmp.path().join("a").join("b").join("logs");
        init(&nested).expect("init must create the directory tree");
        assert!(nested.is_dir());
    }

    #[test]
    fn a_small_log_is_not_rotated() {
        let tmp = TempDir::new().expect("temp dir");
        let live = tmp.path().join(LOG_FILENAME);
        let previous = tmp.path().join(LOG_FILENAME_PREVIOUS);
        std::fs::write(&live, b"small").expect("write");

        rotate_if_needed(&live, &previous);

        assert!(live.exists(), "a small log stays put");
        assert!(!previous.exists(), "nothing should have been rotated");
    }

    #[test]
    fn an_oversized_log_rotates_to_the_previous_generation() {
        let tmp = TempDir::new().expect("temp dir");
        let live = tmp.path().join(LOG_FILENAME);
        let previous = tmp.path().join(LOG_FILENAME_PREVIOUS);
        std::fs::write(&live, vec![b'x'; (ROTATE_AT_BYTES + 1) as usize]).expect("write");

        rotate_if_needed(&live, &previous);

        assert!(!live.exists(), "the oversized log moved aside");
        assert!(previous.exists(), "it became the previous generation");
    }

    #[test]
    fn rotation_keeps_only_one_previous_generation() {
        // Two rotations must not leave three files behind. Unbounded log
        // growth on a user's disk is its own bug.
        let tmp = TempDir::new().expect("temp dir");
        let live = tmp.path().join(LOG_FILENAME);
        let previous = tmp.path().join(LOG_FILENAME_PREVIOUS);

        for marker in ["first", "second"] {
            let mut bytes = vec![b'x'; ROTATE_AT_BYTES as usize];
            bytes.extend_from_slice(marker.as_bytes());
            std::fs::write(&live, bytes).expect("write");
            rotate_if_needed(&live, &previous);
        }

        let count = std::fs::read_dir(tmp.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .count();
        assert_eq!(count, 1, "only the previous generation is kept");

        let kept = std::fs::read(&previous).expect("read previous");
        assert!(
            kept.ends_with(b"second"),
            "the newest rotation must win, not the oldest"
        );
    }

    #[test]
    fn default_filter_silences_dependencies_but_keeps_our_crates() {
        // A log drowning in lance/datafusion INFO chatter blows the rotation
        // budget in minutes and buries the lines that matter.
        assert!(DEFAULT_FILTER.starts_with("warn,"), "deps default to warn");
        for crate_name in ["vault_app", "vault_storage", "vault_tauri"] {
            assert!(
                DEFAULT_FILTER.contains(&format!("{crate_name}=info")),
                "{crate_name} must log at info"
            );
        }
    }

    #[test]
    fn the_filter_covers_the_windowless_maintenance_path() {
        // ADR-SEC-015: the scheduled run has no console, so this file is its
        // only output. If these two are filtered out, a nightly failure leaves
        // no trace anywhere -- which is the situation ADR-SEC-014 existed to
        // end.
        for crate_name in ["vault_cli", "vault_maintenance"] {
            assert!(
                DEFAULT_FILTER.contains(&format!("{crate_name}=info")),
                "{crate_name} must log at info -- it is the unattended path"
            );
        }
    }

    #[test]
    fn the_filter_records_when_the_ranking_model_loads() {
        // ADR-103 D3: a slow first read after a keeper start is only
        // diagnosable if the log says when the model load began and ended.
        assert!(
            DEFAULT_FILTER.contains("vault_embedding=info"),
            "vault_embedding must log at info -- it is where model loading is reported"
        );
    }
}
