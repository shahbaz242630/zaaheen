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

use anyhow::{anyhow, bail, Context, Result};
use rmcp::ServiceExt;
use vault_app::admin::{AdminGate, AdminHost, EngineCell, FullHost, LockedHost};
use vault_app::entitlement::{ModeCheck, UnreadableAccount};
use vault_app::install_paths;
use vault_app::keeper::acl::harden_vault_dir;
use vault_app::keeper::discovery::{self, Discovery, Role};
use vault_app::keeper::handshake::{HandshakeKeys, WIRE};
use vault_app::keeper::intent;
use vault_app::keeper::relay::{KeeperPool, KeeperStarter, KeychainKeySource, RelaySettings};
use vault_app::keeper::runtime::{self, AdminSide, KeeperSettings, Subscription};
use vault_app::keeper::start_failure::{self, StartFailureCode};
use vault_app::keychain::{
    derive_sqlcipher_passphrase, read_existing_master_key, PRODUCTION_NAMESPACE, VAULT_ID,
};
use vault_app::model_fetch;
use vault_app::{ConsolidatorLock, VAULT_LOCKFILE_NAME};
use vault_core::{Boundary, VaultError};
use vault_mcp::{EntitlementCheck, InFlight, NoVaultAdapter, RelayServer, Upstream, Verdict};

// The keeper task's label, launcher and arguments are shared with the
// desktop (ADR-108 D4), so both register the very same task.
#[cfg(windows)]
use vault_app::keeper::{keeper_task_args, KEEPER_TASK_LABEL, LAUNCHER_EXE};

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
    // ADR-105 L3: the keeper never creates the vault folder. The first-run
    // setup (`location::prepare`, run while resolving the paths) or a move
    // made it; a folder that is not there is an error, never a new vault.
    if !vault_root.is_dir() {
        bail!("the vault folder is not there");
    }

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

    // The subscription (ADR-104). A build with no account settings has no
    // check at all and serves as it always did. A build whose settings are
    // present but cannot be read serves LOCK MODE as "cannot confirm"
    // (ADR-108's amendment to ADR-104): still fail-secure — nothing is served
    // ungated — but the export stays reachable, which a refusal to start
    // would take away now that the export lives in the keeper.
    let home = account_home()?;
    let check = match vault_app::account::build_check(&home) {
        Ok(check) => check,
        Err(e) => {
            tracing::warn!(error = %e, "the account could not be read; serving lock mode as 'cannot confirm'");
            let rebuild_home = home.clone();
            let unreadable = Arc::new(UnreadableAccount::new(Box::new(move || {
                match vault_app::account::build_check(&rebuild_home) {
                    Ok(Some(check)) => Ok(Some(check as Arc<dyn ModeCheck>)),
                    Ok(None) => Ok(None),
                    Err(_) => Err(()),
                }
            })));
            match serve_locked(&paths, &vault_root, unreadable, exit_on_stdin_eof).await {}
        }
    };

    // §8.26 §6.2: entitlement is decided BEFORE the vault is opened and the
    // models load, because a locked computer is served by lock mode instead —
    // the same MCP surface with no `Application` behind it.
    let locked = match &check {
        Some(check) => match check.check().await {
            Verdict::Entitled => None,
            Verdict::Locked(reason) => Some(reason),
        },
        None => None,
    };

    if let (Some(check), Some(reason)) = (&check, locked) {
        tracing::info!(
            reason = ?reason,
            "the vault is locked; serving lock mode without loading any models"
        );
        match serve_locked(&paths, &vault_root, Arc::clone(check), exit_on_stdin_eof).await {}
    }

    // ADR-105 L3: the models never follow the vault. Resolved BEFORE the
    // heartbeat starts, and a failure published, so it can never leave a
    // "starting" record refreshed by a keeper that has given up (security
    // review N2).
    let homes = match vault_app::location::Homes::production() {
        Ok(homes) => homes,
        Err(e) => {
            publish_failure();
            return Err(anyhow!("{e}"));
        }
    };
    let models_dir = vault_app::location::models_dir(&homes);

    // A keeper that is building the vault keeps its "starting" record fresh,
    // so relays and the desktop wait for it instead of asking for another
    // start once the record's 60 s are up (review B R2-S1).
    let heartbeat = tokio::spawn({
        let vault_root = vault_root.clone();
        async move {
            let mut every = tokio::time::interval(STARTING_HEARTBEAT);
            every.tick().await;
            loop {
                every.tick().await;
                let at = chrono::Utc::now().to_rfc3339();
                let _ = discovery::write(
                    &vault_root,
                    &Discovery::starting(std::process::id(), WIRE, env!("CARGO_PKG_VERSION"), &at),
                );
            }
        }
    });

    // §8.26 §4 and SIGNIN-DESIGN.md §8.40: at keeper start, refresh a stale
    // lease, then once a day while serving — never from tool activity, and
    // recording no use. Its own task, started before the models load, so
    // neither the start nor any call waits on the network. Lock mode (above)
    // has no need: its start already refreshed on the denial.
    if let Some(check) = &check {
        tokio::spawn(Arc::clone(check).refresh_at_start_then_daily());
    }

    let reranker = model_fetch::reranker_paths_in(&models_dir);
    let built = crate::build_application_keyed(
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
    .await;
    // Stopped, and waited for, before anything else is published: a last
    // "starting" write must never land on top of the keeper's own record.
    heartbeat.abort();
    let _ = heartbeat.await;
    let (app, master_key) = match built {
        Ok(built) => built,
        Err(e) => {
            // ADR-108 D7: say why, for the desktop, then publish Failed.
            let code = e
                .downcast_ref::<crate::OpenFailure>()
                .map_or(StartFailureCode::VaultOpenFailed, |f| f.code);
            start_failure::write(&vault_root, std::process::id(), code);
            publish_failure();
            return Err(e);
        }
    };
    start_failure::clear(&vault_root);
    // ADR-SEC-029 R2: the handshake keys come from the key the build just
    // opened (and may just have moved to Local persistence), never from a
    // second read of it. The master key is dropped, and wiped, at once.
    let keys = HandshakeKeys::derive(&master_key);
    drop(master_key);
    let app = Arc::new(app);
    let _worker_shutdown = app.spawn_retry_worker();

    let adapter: Arc<dyn vault_mcp::Adapter> = app.adapter().clone();
    let settings = KeeperSettings::production(vault_root.clone(), env!("CARGO_PKG_VERSION"));

    // ADR-100 / ADR-108 D10: fetch the reranker in the background while
    // serving; reads use the cosine gate until it lands. One acquisition per
    // keeper, shared with the desktop's "get it now". It runs on its own task,
    // so a stopping keeper does not wait for it: the process exits with it.
    let full = FullHost::new(Arc::clone(&app), EngineCell::new(models_dir));
    full.fetch_engine();

    // Entitled (or a build with no sign-in at all): the full keeper, with the
    // one check both serving calls and answering each tick, and the desktop's
    // admin side reading that same check from disk.
    let admin_gate = match &check {
        Some(check) => AdminGate::Full(Arc::clone(check) as Arc<dyn ModeCheck>),
        None => AdminGate::Open,
    };
    let subscription = check.map(|check| Subscription::full(check, InFlight::new()));
    let admin = AdminSide {
        host: Arc::new(AdminHost::Full(full)),
        gate: Arc::new(admin_gate),
    };

    let exit = runtime::serve(
        adapter,
        Some(admin),
        keys,
        settings,
        subscription,
        shutdown_signal(exit_on_stdin_eof),
    )
    .await;

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

/// How often a keeper that is still opening the vault refreshes its
/// "starting" record: well inside the 60 s relays and the desktop trust one.
const STARTING_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(20);

/// Lock mode (§8.26 §6.2): the same MCP surface with no `Application`
/// behind it, and the desktop's admin side with only what the lock screen
/// needs (ADR-108). Never returns: like the full keeper, it exits the process
/// with `.vault.lock` still held.
async fn serve_locked<C>(
    paths: &KeeperPaths,
    vault_root: &Path,
    check: Arc<C>,
    exit_on_stdin_eof: bool,
) -> std::convert::Infallible
where
    C: EntitlementCheck + ModeCheck,
{
    let pid = std::process::id();
    let publish_failure = || {
        let at = chrono::Utc::now().to_rfc3339();
        let failed = Discovery::failed(pid, WIRE, env!("CARGO_PKG_VERSION"), &at);
        let _ = discovery::write(vault_root, &failed);
    };
    // The handshake is authenticated with keys derived from the master key,
    // and only a full keeper may create one.
    let master_key = match read_existing_master_key(PRODUCTION_NAMESPACE, VAULT_ID) {
        Ok(Some(master_key)) => master_key,
        // A locked computer with no key yet (a fresh install before sign-in):
        // nothing to serve and nothing wrong, so no Failed record and no
        // note — just leave (review A R2-1 / B R2-M1). The desktop shows its
        // lock screen without a keeper; relays already say "signed out".
        Ok(None) => {
            tracing::info!("locked, and no key yet: nothing to serve; this keeper exits");
            let _ = discovery::remove_if_written_by(vault_root, Role::Starting, pid);
            std::process::exit(0)
        }
        Err(e) => {
            tracing::warn!(error = %e, "the key could not be read for lock mode");
            start_failure::write(vault_root, pid, StartFailureCode::CredentialStore);
            publish_failure();
            std::process::exit(1)
        }
    };
    let keys = HandshakeKeys::derive(&master_key);
    // For the lock screen's numbers and the export only, opened on first use
    // and never created (`LockedHost`).
    let database_key = derive_sqlcipher_passphrase(&master_key);
    drop(master_key);
    start_failure::clear(vault_root);

    let subscription = Subscription::lock(check, InFlight::new());
    let admin = AdminSide {
        host: Arc::new(AdminHost::Locked(LockedHost::new(
            paths.vault_db.clone(),
            database_key,
        ))),
        gate: Arc::new(AdminGate::Locked {
            check: subscription.check(),
            flip: subscription.flip().clone(),
        }),
    };
    let adapter: Arc<dyn vault_mcp::Adapter> = Arc::new(NoVaultAdapter);
    let exit = runtime::serve(
        adapter,
        Some(admin),
        keys,
        KeeperSettings::production(vault_root.to_path_buf(), env!("CARGO_PKG_VERSION")),
        Some(subscription),
        shutdown_signal(exit_on_stdin_eof),
    )
    .await;
    let code = match exit {
        Ok(exit) => {
            tracing::info!(exit = ?exit, "locked keeper stopped");
            0
        }
        Err(e) => {
            tracing::error!(error = %e, "locked keeper stopped on an error");
            publish_failure();
            1
        }
    };
    std::process::exit(code)
}

/// Where the account folder lives: `%LOCALAPPDATA%\com.zaaheen.app`, the same
/// place the logs go (§8.26 §4).
///
/// # Errors
///
/// The local app data directory could not be determined.
pub(crate) fn account_home() -> Result<PathBuf> {
    install_paths::local_data_dir()
        .ok_or_else(|| anyhow!("could not determine the local application data directory"))
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
pub async fn run_relay(homes: vault_app::location::Homes, boundaries: Vec<Boundary>) -> Result<()> {
    // Nothing that can fail runs before stdio is served: a relay that dies
    // before answering `initialize` is exactly the "Server disconnected" this
    // arc exists to remove. Folder hardening is the keeper's job (it runs it
    // before taking the lock), and the start mechanism is resolved on the
    // first start request — a failure there only makes that request fail.
    let starter: Arc<dyn KeeperStarter> = Arc::new(LazyStarter::default());
    // §8.26 §6.3: a build that carries a sign-in reads the marker, so a
    // signed-out computer is answered here instead of starting a keeper that
    // could only say the same thing. `option_env!` is resolved at compile
    // time, so this costs nothing at startup and never touches the folder.
    let account_dir = vault_app::account::has_account_settings()
        .then(account_home)
        .transpose()?
        .map(|home| vault_app::account::account_dir_path(&home));
    let pool = KeeperPool::new(
        // ADR-105 L3: the recorded location, re-resolved on every round.
        RelaySettings::recorded(homes, boundaries).with_account_dir(account_dir),
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
        args: keeper_task_args(log_dir),
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

/// §8.26 §4 and `SIGNIN-DESIGN.md` §8.40: the full keeper starts the routine
/// refresh — at start when the lease is stale, then daily — in its own task,
/// before the models load, and lock mode does not (its start already refreshed
/// on the denial, and every call it answers asks again).
///
/// A source test, because the keeper's start opens the real vault and the
/// credential store and cannot run inside a test. It reads only
/// `dispatch_keeper`, with comments stripped (§8.37: a source test must read
/// code, never prose), and was planted to prove it sees a missing or a
/// misplaced start.
#[cfg(test)]
mod routine_refresh_wiring {
    #[test]
    fn the_full_keeper_starts_the_routine_refresh_and_lock_mode_does_not() {
        let source = include_str!("keeper.rs").replace("\r\n", "\n");
        let code: String = source
            .lines()
            .map(|line| line.split("//").next().unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");
        // One function's body: from its signature to the first closing brace
        // at column 0 (the file goes on to this very test, whose own string
        // literals must not count as code).
        let body_of = |signature: &str| -> String {
            let from = code.split_once(signature).expect(signature).1;
            from.split_once("\n}\n")
                .map_or(from, |(body, _)| body)
                .to_string()
        };
        // Since ADR-108 lock mode is its own function, `serve_locked`.
        let lock_mode = body_of("async fn serve_locked<");
        let full = body_of("pub async fn dispatch_keeper(");
        let (before_models, after_build) = full
            .split_once("crate::build_application_keyed(")
            .expect("the full keeper builds the application");

        // Review A R2-1 / B R2-M1: a locked computer with no key yet is not a
        // failure; its keeper leaves without a Failed record.
        let (_, no_key) = lock_mode
            .split_once("Ok(None) =>")
            .expect("lock mode handles 'no key yet'");
        let no_key = no_key.split_once("Err(e) =>").map_or(no_key, |(b, _)| b);
        assert!(
            no_key.contains("remove_if_written_by(") && !no_key.contains("publish_failure"),
            "no key yet: clear the starting record, publish nothing"
        );

        // ADR-SEC-029 R2: the full keeper derives its handshake keys from the
        // key the build opened; a second read straight after a move to Local
        // persistence is the library's documented flaky case.
        assert!(
            !after_build.contains("read_existing_master_key"),
            "the full keeper must not read the key a second time after the build"
        );
        assert!(
            after_build.contains("HandshakeKeys::derive(&master_key);"),
            "the full keeper derives its handshake keys from the built key"
        );

        assert!(
            !lock_mode.contains("refresh_at_start_then_daily"),
            "lock mode must not start the routine refresh"
        );
        // One connected call, not two substrings: a `tokio::spawn(` somewhere
        // else in the window must not stand in for this one (the independent
        // review's note, §8.40).
        assert!(
            before_models
                .contains("tokio::spawn(Arc::clone(check).refresh_at_start_then_daily());"),
            "the full keeper must spawn the routine refresh before the models load"
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
