//! `zaaheen keeper` and the relay mode of `zaaheen mcp serve` (ADR-102).
//!
//! **Why.** AI apps start one `zaaheen mcp serve` per connection — Claude
//! Desktop starts several at once — and the vault admits one owner. Before
//! ADR-102 every copy tried to own it and all but one died, which is how a
//! Claude Desktop chat ended up with "Server disconnected". Now every copy is
//! a relay: it answers the handshake itself and forwards tool calls to ONE
//! keeper process, started on demand, that owns the vault.
//!
//! On Windows the keeper is started by Task Scheduler (an on-demand, per-user
//! task), not spawned by the relay. A relay's stdin/stdout are the AI app's
//! pipe handles, and on stable Rust a spawned child inherits every inheritable
//! handle — a keeper spawned directly would keep the AI app's pipe open after
//! the relay exited. Task Scheduler creates the keeper fresh, outside the AI
//! app's process tree, inheriting nothing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use rmcp::ServiceExt;
use vault_app::install_paths;
use vault_app::keeper::acl::harden_vault_dir;
use vault_app::keeper::discovery::{self, Discovery};
use vault_app::keeper::handshake::{HandshakeKeys, WIRE};
use vault_app::keeper::intent;
use vault_app::keeper::relay::{KeeperPool, KeeperStarter, KeychainKeySource, RelaySettings};
use vault_app::keeper::runtime::{self, KeeperSettings};
use vault_app::keychain::{read_existing_master_key, PRODUCTION_NAMESPACE, VAULT_ID};
use vault_app::model_fetch;
use vault_app::{ConsolidatorLock, VAULT_LOCKFILE_NAME};
use vault_core::{Boundary, VaultError};
use vault_mcp::{RelayServer, Upstream};

/// Version marker for the keeper's Task Scheduler entry. Changing the task's
/// shape means bumping this, which makes every relay replace the old entry.
#[cfg(windows)]
const KEEPER_TASK_LABEL: &str = "zaaheen-keeper-task-v1";

/// The Windows-subsystem launcher the task runs (ADR-SEC-015), so no console
/// window ever appears.
#[cfg(windows)]
const LAUNCHER_EXE: &str = "zaaheen-maintenance.exe";

/// Everything `zaaheen keeper` needs to open the vault.
pub struct KeeperPaths {
    pub vault_db: PathBuf,
    pub vector_dir: PathBuf,
    pub graph_db: PathBuf,
    pub bge_model: PathBuf,
    pub bge_tokenizer: PathBuf,
    pub ort_lib: PathBuf,
}

/// Run the keeper: own the vault and serve every relay until idle, then EXIT
/// THE PROCESS.
///
/// # Why this never returns normally
///
/// Once serving ends, the process exits on the spot — while still holding
/// `.vault.lock` — instead of returning and letting the runtime shut down.
/// Two independent reasons, both found by adversarial review:
///
/// - **The lock must outlive every writer.** rmcp runs each request in a
///   detached task, the retry worker's handle is not joinable, and SQLite
///   calls finish on blocking threads even through runtime shutdown. If the
///   lock were released first, another process could open the vault while one
///   of those was still writing. Exiting with the lock held means the OS
///   releases it only after every thread of this process is gone.
/// - **Runtime shutdown would never finish.** A stdin read cannot be
///   cancelled (tokio's own `stdin` documentation), so waiting for the runtime
///   to wind down would wait forever on the launcher's pipe — while the
///   launcher waits for us. Task Scheduler would then see a running instance
///   and drop every start request until logoff.
///
/// A killed keeper and an exiting one therefore look the same to the stores,
/// which is the case they are already built to survive (durable queue, WAL,
/// atomic Lance commits).
///
/// # Errors
///
/// Only failures BEFORE serving starts are returned (vault could not be
/// opened); the caller reports them as usual.
pub async fn dispatch_keeper(paths: KeeperPaths, exit_on_stdin_eof: bool) -> Result<()> {
    let vault_root = paths
        .vault_db
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("vault path has no parent directory"))?;
    std::fs::create_dir_all(&vault_root).context("create vault directory")?;

    // BEFORE any lockfile exists: a principal that can read the folder could
    // otherwise hold the lock and stop this from ever running (ADR-SEC-019).
    if let Err(e) = harden_vault_dir(&vault_root) {
        tracing::warn!(error = %e, "vault folder permissions were not tightened");
    }

    // Someone is about to take the vault exclusively (erasure): do not start.
    if intent::is_held(&vault_root) {
        tracing::info!("exclusive access to the vault is pending; this keeper exits");
        return Ok(());
    }

    let vault_lock = match ConsolidatorLock::try_acquire_named(&vault_root, VAULT_LOCKFILE_NAME) {
        Ok(lock) => lock,
        // Another keeper (or maintenance) owns it. Not an error: start
        // requests are repeated on purpose and all but one land here.
        Err(VaultError::ConsolidatorBusy(_)) => {
            tracing::info!("the vault already has an owner; this keeper exits");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    // Again, now that we hold the lock: an eraser that declared its intent
    // between the first check and the lock would otherwise watch us take the
    // vault from under it.
    if intent::is_held(&vault_root) {
        tracing::info!("exclusive access to the vault is pending; this keeper exits");
        drop(vault_lock);
        return Ok(());
    }

    // Relays that look now wait for us instead of requesting more starts.
    let started_at = chrono::Utc::now().to_rfc3339();
    let _ = discovery::write(
        &vault_root,
        &Discovery::starting(
            std::process::id(),
            WIRE,
            env!("CARGO_PKG_VERSION"),
            &started_at,
        ),
    );
    // Any failure from here on is published, so relays report it instead of
    // re-requesting a start in a loop.
    let publish_failure = || {
        let at = chrono::Utc::now().to_rfc3339();
        let failed = Discovery::failed(std::process::id(), WIRE, env!("CARGO_PKG_VERSION"), &at);
        let _ = discovery::write(&vault_root, &failed);
    };

    let models_dir = install_paths::models_dir_in(&vault_root);
    let reranker = model_fetch::reranker_paths_in(&models_dir);
    let app = match crate::build_application(
        &paths.vault_db,
        &paths.vector_dir,
        &paths.graph_db,
        paths.bge_model,
        paths.bge_tokenizer,
        paths.ort_lib,
        None,
        Some(reranker.model),
        Some(reranker.tokenizer),
    )
    .await
    {
        Ok(app) => app,
        Err(e) => {
            publish_failure();
            return Err(e);
        }
    };
    let _worker_shutdown = app.spawn_retry_worker();

    // `build_application` has just read (or, on a fresh install, created) the
    // key, so this only reads it. Derive the handshake keys and drop the
    // master key at once.
    let keys = match read_existing_master_key(PRODUCTION_NAMESPACE, VAULT_ID) {
        Ok(Some(master_key)) => HandshakeKeys::derive(&master_key),
        Ok(None) | Err(_) => {
            publish_failure();
            return Err(anyhow!("authentication failed"));
        }
    };

    let adapter: Arc<dyn vault_mcp::Adapter> = app.adapter().clone();
    let settings = KeeperSettings::production(vault_root.clone(), env!("CARGO_PKG_VERSION"));

    // ADR-100: fetch the reranker concurrently with serving; reads use the
    // cosine gate until it lands. Raced rather than joined: when serving ends
    // the acquisition is abandoned, so a stopping keeper never holds the vault
    // for the ~20 s re-hash (or a first-run download) of a model it will not
    // use again.
    let serve = runtime::serve(adapter, keys, settings, shutdown_signal(exit_on_stdin_eof));
    tokio::pin!(serve);
    let acquire = async {
        match model_fetch::ensure_reranker(&models_dir).await {
            Ok(_) => {
                let _ = app.spawn_reranker_warmup();
            }
            Err(e) => {
                tracing::warn!(error = %e, "reranker acquisition failed; reads use the cosine gate")
            }
        }
    };
    tokio::pin!(acquire);
    let exit = tokio::select! {
        exit = &mut serve => exit,
        () = &mut acquire => serve.await,
    };

    let code = match exit {
        Ok(exit) => {
            tracing::info!(exit = ?exit, "keeper stopped");
            0
        }
        Err(e) => {
            tracing::error!(error = %e, "keeper stopped on an error");
            // Serving never started or lost its endpoint; the discovery file
            // may still say "starting".
            publish_failure();
            1
        }
    };
    // See the function docs: exit with `vault_lock` still held (it is never
    // dropped; the OS releases it with the process).
    std::process::exit(code)
}

/// Resolves when the keeper should stop: Ctrl-C, or — when launched by the
/// windowless launcher — EOF on stdin, which is how the keeper learns its
/// launcher was killed (by `schtasks /End` or an installer) without any
/// platform-specific process API.
///
/// Stdin is watched on a plain OS thread, NOT with `tokio::io::stdin()`:
/// tokio documents that its stdin read runs on a blocking thread that cannot
/// be cancelled, which would make runtime shutdown hang on the launcher's
/// pipe. A detached thread is simply ended with the process.
fn shutdown_signal(exit_on_stdin_eof: bool) -> impl std::future::Future<Output = ()> + Send {
    let stdin_eof = exit_on_stdin_eof.then(stdin_eof_watcher);
    async move {
        let ctrl_c = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        match stdin_eof {
            Some(eof) => {
                tokio::select! {
                    () = ctrl_c => {}
                    _ = eof => {}
                }
            }
            None => ctrl_c.await,
        }
    }
}

/// Start a thread that reads stdin to end-of-file and then fires the returned
/// channel.
fn stdin_eof_watcher() -> tokio::sync::oneshot::Receiver<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 64];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = tx.send(());
    });
    rx
}

/// `zaaheen mcp serve` in relay mode: serve MCP on stdio, forwarding tool
/// calls to the keeper.
///
/// # Errors
///
/// The stdio transport could not be bound.
pub async fn run_relay(vault_root: PathBuf, boundaries: Vec<Boundary>) -> Result<()> {
    // Nothing that can fail runs before stdio is served: a relay that dies
    // before answering `initialize` is exactly the "Server disconnected" this
    // arc exists to remove. Folder hardening is the keeper's job (it runs it
    // before taking the lock), and the start mechanism is resolved on the
    // first start request — a failure there only makes that request fail.
    let starter: Arc<dyn KeeperStarter> = Arc::new(LazyStarter::default());
    let pool = KeeperPool::new(
        RelaySettings::production(vault_root, boundaries),
        starter,
        Arc::new(KeychainKeySource),
    );
    let _reaper = pool.spawn_idle_reaper();

    let upstream: Arc<dyn Upstream> = pool;
    let running = RelayServer::new(upstream)
        .serve(rmcp::transport::stdio())
        .await
        .context("MCP transport bind failed")?;
    eprintln!("zaaheen mcp serve: ready (sharing the vault through the keeper)");
    running
        .waiting()
        .await
        .context("MCP serve task exited with an error")?;
    eprintln!("zaaheen mcp serve: clean shutdown");
    Ok(())
}

/// Resolves the platform start mechanism on first use and caches it. Keeps
/// `whoami`, `current_exe` and log-directory lookups off the path to serving
/// stdio (see [`run_relay`]).
#[derive(Default)]
struct LazyStarter {
    inner: std::sync::OnceLock<PlatformStarter>,
}

impl KeeperStarter for LazyStarter {
    fn request_start(&self) -> std::io::Result<()> {
        let starter = match self.inner.get() {
            Some(starter) => starter,
            None => {
                let built = platform_starter().map_err(|e| std::io::Error::other(e.to_string()))?;
                self.inner.get_or_init(|| built)
            }
        };
        starter.request_start()
    }
}

#[cfg(windows)]
type PlatformStarter = TaskSchedulerStarter;
#[cfg(not(windows))]
type PlatformStarter = DirectSpawnStarter;

/// Starts the keeper through Task Scheduler (see the module docs for why not
/// by spawning it).
#[cfg(windows)]
struct TaskSchedulerStarter {
    task: vault_scheduler::OnDemandTask,
}

#[cfg(windows)]
impl KeeperStarter for TaskSchedulerStarter {
    fn request_start(&self) -> std::io::Result<()> {
        vault_scheduler::start_on_demand(&self.task)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
}

#[cfg(windows)]
fn platform_starter() -> Result<TaskSchedulerStarter> {
    let sid = vault_app::keeper::acl::current_user_sid()
        .map_err(|e| anyhow!("could not identify the current user: {e}"))?;
    let exe_dir = install_paths::resource_dir()
        .ok_or_else(|| anyhow!("could not locate the directory holding this executable"))?;
    let log_dir =
        install_paths::log_dir().ok_or_else(|| anyhow!("could not determine the log directory"))?;
    Ok(TaskSchedulerStarter {
        task: keeper_task(&sid, &exe_dir.join(LAUNCHER_EXE), &log_dir)?,
    })
}

/// The keeper's on-demand task definition. Named with the user's SID so two
/// Windows users on one PC never share (or overwrite) one.
#[cfg(windows)]
fn keeper_task(
    sid: &str,
    launcher: &Path,
    log_dir: &Path,
) -> Result<vault_scheduler::OnDemandTask> {
    let task_id =
        vault_scheduler::TaskId::new(format!("{}{sid}", vault_app::keeper::KEEPER_TASK_ID_PREFIX))
            .map_err(|e| anyhow!("invalid keeper task id: {e}"))?;
    Ok(vault_scheduler::OnDemandTask {
        task_id,
        label: KEEPER_TASK_LABEL.to_string(),
        program: launcher.to_path_buf(),
        args: vec![
            "--keeper".to_string(),
            "--log-dir".to_string(),
            log_dir.to_string_lossy().into_owned(),
        ],
    })
}

/// Off Windows (compile + unit tested only in V0.2): spawn the keeper
/// directly, detached into its own process group. Rust marks every file
/// descriptor it opens close-on-exec and replaces 0-2 with /dev/null here, so
/// none of the AI app's pipes leak into the keeper.
#[cfg(not(windows))]
struct DirectSpawnStarter {
    program: PathBuf,
}

#[cfg(not(windows))]
impl KeeperStarter for DirectSpawnStarter {
    fn request_start(&self) -> std::io::Result<()> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let mut child = Command::new(&self.program)
            .arg("keeper")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        // Reap it whenever it exits, so a keeper that finds the vault owned
        // does not linger as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

#[cfg(not(windows))]
fn platform_starter() -> Result<DirectSpawnStarter> {
    let program = std::env::current_exe().context("locate this executable")?;
    Ok(DirectSpawnStarter { program })
}

/// The shutdown path the launcher relies on, exercised in real child
/// processes. Before this existed the keeper watched stdin with
/// `tokio::io::stdin()`, whose read cannot be cancelled: once serving ended,
/// runtime shutdown waited forever on the launcher's pipe while the launcher
/// waited for the keeper — and no test noticed, because no test ran it.
#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    const CHILD_ENV: &str = "ZAAHEEN_SHUTDOWN_TEST_CHILD";

    /// Child entry point; does nothing in a normal test run.
    #[test]
    fn shutdown_child() {
        let Ok(mode) = std::env::var(CHILD_ENV) else {
            return;
        };
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        match mode.as_str() {
            // Serving ends while the launcher still holds our stdin open. The
            // runtime must still shut down — with a tokio stdin read in
            // flight this `drop(rt)` hung forever.
            "stdin-open" => {
                rt.block_on(async {
                    let signal = shutdown_signal(true);
                    tokio::select! {
                        () = signal => {}
                        () = tokio::time::sleep(Duration::from_millis(200)) => {}
                    }
                });
                drop(rt);
            }
            // The launcher's pipe closes: that alone must stop us.
            "wait-eof" => {
                let signalled = rt.block_on(async {
                    tokio::time::timeout(Duration::from_secs(3), shutdown_signal(true))
                        .await
                        .is_ok()
                });
                assert!(signalled, "EOF on stdin did not signal shutdown");
            }
            other => panic!("unknown mode {other:?}"),
        }
    }

    fn spawn(mode: &str) -> Child {
        Command::new(std::env::current_exe().expect("current_exe"))
            .args([
                "--exact",
                "keeper::shutdown_tests::shutdown_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn child")
    }

    /// Wait up to `limit` for the child; kill it if it is still running.
    fn exited_within(child: &mut Child, limit: Duration) -> Option<bool> {
        let deadline = Instant::now() + limit;
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Some(status.success());
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_keeper_whose_serving_ended_exits_even_with_stdin_held_open() {
        let mut child = spawn("stdin-open");
        // Held open for the whole test, exactly as the launcher does.
        let _launcher_pipe = child.stdin.take();
        assert_eq!(
            exited_within(&mut child, Duration::from_secs(5)),
            Some(true),
            "the process must exit while its stdin is still open"
        );
    }

    #[test]
    fn closing_the_launchers_pipe_stops_the_keeper() {
        let mut child = spawn("wait-eof");
        std::thread::sleep(Duration::from_millis(300));
        drop(child.stdin.take());
        assert_eq!(
            exited_within(&mut child, Duration::from_secs(5)),
            Some(true),
            "EOF on stdin must stop the keeper"
        );
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn the_keeper_task_is_per_user_and_runs_the_windowless_launcher() {
        let task = keeper_task(
            "S-1-5-21-1111-2222-3333-1001",
            Path::new(r"C:\Program Files\Zaaheen\zaaheen-maintenance.exe"),
            Path::new(r"C:\Users\sam\AppData\Local\com.zaaheen.app\logs"),
        )
        .unwrap();
        assert_eq!(
            task.task_id.as_str(),
            "com.zaaheen.keeper.S-1-5-21-1111-2222-3333-1001"
        );
        assert!(task.program.ends_with(LAUNCHER_EXE));
        assert_eq!(task.args[0], "--keeper");
        assert!(task.validate().is_ok());
    }
}
