//! The keeper's serve loop (ADR-102): the one process that owns the vault,
//! serving every relay over authenticated local IPC.
//!
//! The caller has already hardened the folder, taken `.vault.lock` and built
//! the `Application`; this module publishes the discovery file, accepts
//! connections, authenticates each one (`handshake`), and serves it with the
//! SAME `StdioServer` the stdio path uses — so the tool surface, audit and
//! boundary enforcement are reused unchanged, scoped per connection to the
//! boundaries that connection proved.
//!
//! Exit paths, each removing the discovery file FIRST so relays stop dialling
//! a keeper that is leaving:
//! - **Idle:** no authenticated connection for `idle_exit`. This is what frees
//!   the vault for nightly maintenance once the user's AI apps go quiet.
//! - **Yield:** a relay of a NEWER build asked this keeper to step aside.
//! - **Handover:** a key holder needs the vault to itself ("Delete
//!   everything").
//! - **Shutdown:** the caller's shutdown future resolved (launcher death).
//! - **Listener lost:** no fresh endpoint could be bound or published. The
//!   same cleanup runs, so relays are never left dialling a dead endpoint.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::{RoleServer, Service, ServiceExt};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::Instant;
use vault_core::{VaultError, VaultResult};
use vault_mcp::{Adapter, EntitlementCheck, Gate, InFlight, StdioServer, Verdict};

use crate::entitlement::{Flip, LockModeCheck, ModeCheck};

use super::discovery::{self, Discovery};
use super::handshake::{
    keeper_handshake, HandshakeError, HandshakeKeys, KeeperAccept, KeeperIdentity, CHALLENGE_LEN,
    NONCE_LEN, WIRE,
};
use super::transport::{Listener, ServerStream};

/// How many fresh endpoints to try before giving up on listening at all.
const MAX_ROTATIONS: u32 = 3;

/// At most one security WARN per this interval for failed handshakes, so a
/// hostile local process cannot flood the log.
const WARN_INTERVAL: Duration = Duration::from_secs(10);

/// Tunables. [`KeeperSettings::production`] holds the shipped values; tests
/// shorten them.
#[derive(Clone, Debug)]
pub struct KeeperSettings {
    pub vault_root: PathBuf,
    pub version: String,
    pub idle_exit: Duration,
    pub idle_check: Duration,
    pub frame1_deadline: Duration,
    pub handshake_deadline: Duration,
    pub max_pending_handshakes: usize,
}

impl KeeperSettings {
    /// Shipped values (design v3 §5).
    pub fn production(vault_root: PathBuf, version: &str) -> Self {
        Self {
            vault_root,
            version: version.to_string(),
            idle_exit: Duration::from_secs(5 * 60),
            idle_check: Duration::from_secs(5),
            frame1_deadline: Duration::from_secs(1),
            handshake_deadline: Duration::from_secs(5),
            max_pending_handshakes: 8,
        }
    }
}

/// Why the keeper stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum KeeperExit {
    Idle,
    Yielded,
    HandedOver,
    Shutdown,
    /// Entitlement flipped, so this process is the wrong mode to serve it
    /// (§8.26 §6.2). Full to lock frees the models instead of holding them
    /// while relays stay connected; lock to full lets the next relay start a
    /// keeper that can actually serve.
    ModeChanged,
}

/// Which mode this keeper is running in (§8.26 §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeeperMode {
    /// The vault is open and calls are served.
    Full,
    /// The user is locked: the same MCP surface over `NoVaultAdapter`, with no
    /// `Application` and no models behind it.
    Lock,
}

/// The subscription this keeper serves behind, and what it re-evaluates on
/// each tick (§8.35).
///
/// One value rather than a gate and a mode side by side: two `Option`s that
/// must agree is a defect waiting to happen.
pub struct Subscription {
    /// Gates every session. The real check in full mode; [`LockModeCheck`] in
    /// lock mode, which never serves a call.
    gate: Gate,
    mode: KeeperMode,
    /// Asked on each tick, and never the always-deny check: a locked keeper
    /// has to be able to notice that the user is entitled again.
    check: Arc<dyn ModeCheck>,
    /// Raised by a lock-mode call that found the user entitled, so the keeper
    /// leaves at once instead of waiting for the next tick.
    flip: Flip,
}

impl Subscription {
    /// A keeper serving the vault: the one check both serves calls and
    /// answers ticks.
    pub fn full<C>(check: Arc<C>, in_flight: InFlight) -> Self
    where
        C: EntitlementCheck + ModeCheck,
    {
        Self {
            gate: Gate::new(check.clone(), in_flight),
            mode: KeeperMode::Full,
            check,
            // Nothing to unlock: only lock mode raises it.
            flip: Flip::new(),
        }
    }

    /// A locked keeper: calls are answered by [`LockModeCheck`], which never
    /// serves one and raises the flip when entitlement returns.
    pub fn lock<C>(check: Arc<C>, in_flight: InFlight) -> Self
    where
        C: EntitlementCheck + ModeCheck,
    {
        let flip = Flip::new();
        let locked: Arc<dyn EntitlementCheck> =
            Arc::new(LockModeCheck::new(check.clone(), flip.clone()));
        Self {
            gate: Gate::new(locked, in_flight),
            mode: KeeperMode::Lock,
            check,
            flip,
        }
    }

    /// Which mode this keeper is running in.
    pub fn mode(&self) -> KeeperMode {
        self.mode
    }
}

/// Serve until idle, yield, handover or shutdown.
///
/// # Errors
///
/// The keeper could not listen at all, or could not publish its discovery
/// file (at start, or when moving to a fresh endpoint). Serving errors on
/// individual connections are logged, never fatal.
pub async fn serve<F>(
    adapter: Arc<dyn Adapter>,
    keys: HandshakeKeys,
    settings: KeeperSettings,
    subscription: Option<Subscription>,
    shutdown: F,
) -> VaultResult<KeeperExit>
where
    F: std::future::Future<Output = ()> + Send,
{
    let keys = Arc::new(keys);
    let gate = subscription.as_ref().map(|s| s.gate.clone());
    let (mut listener, mut identity) = bind_fresh(&settings)?;
    publish(&settings, &identity)?;
    // The tenure the discovery file currently describes — what cleanup must
    // remove, whichever exit path is taken.
    let mut published = hex::encode(identity.nonce);
    tracing::info!(
        target: "vault_app::keeper",
        endpoint = %identity.endpoint,
        "keeper serving"
    );

    let active = Arc::new(AtomicUsize::new(0));
    let last_warn: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let (stop_tx, mut stop_rx) = mpsc::channel::<KeeperExit>(1);
    let mut pending: VecDeque<(AbortHandle, Arc<AtomicBool>)> = VecDeque::new();
    // Every connection task, authenticated or not, so a stopping keeper can
    // close them all rather than leave sessions serving after it has gone.
    let mut connections: Vec<AbortHandle> = Vec::new();
    let mut idle_since = Some(Instant::now());
    let mut idle_tick = tokio::time::interval(settings.idle_check);
    // At most one mode evaluation at a time: a refresh can take 5 s, and
    // ticks come every `idle_check`.
    let deciding = Arc::new(AtomicBool::new(false));
    // Lock mode only, and acted on once: a raised flip stays raised, so
    // selecting on it again would spin the loop.
    let mut pending_flip = subscription
        .as_ref()
        .filter(|s| s.mode == KeeperMode::Lock)
        .map(|s| s.flip.clone());
    tokio::pin!(shutdown);

    let exit = loop {
        // Decide what happened first, then act on it outside `select!`, so no
        // branch future still borrows the listener when it is replaced.
        let event = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(stream) => Event::Accepted(stream),
                Err(e) => Event::ListenerFailed(e),
            },
            _ = idle_tick.tick() => Event::Tick,
            () = next_flip(pending_flip.as_ref()) => Event::Flip,
            Some(reason) = stop_rx.recv() => Event::Stop(reason),
            () = &mut shutdown => Event::Stop(KeeperExit::Shutdown),
        };

        match event {
            Event::Accepted(stream) => {
                // Silent unauthenticated connections are cheap to open; evict
                // the OLDEST rather than refuse the newest, so a flood cannot
                // lock real relays out.
                pending.retain(|(h, authed)| !h.is_finished() && !authed.load(Ordering::SeqCst));
                if pending.len() >= settings.max_pending_handshakes {
                    if let Some((oldest, _)) = pending.pop_front() {
                        oldest.abort();
                    }
                }
                let authed = Arc::new(AtomicBool::new(false));
                let task = tokio::spawn(handle_connection(
                    stream,
                    ConnectionContext {
                        keys: keys.clone(),
                        identity: identity.clone(),
                        adapter: adapter.clone(),
                        gate: gate.clone(),
                        active: active.clone(),
                        authed: authed.clone(),
                        stop_tx: stop_tx.clone(),
                        last_warn: last_warn.clone(),
                        frame1_deadline: settings.frame1_deadline,
                        handshake_deadline: settings.handshake_deadline,
                    },
                ));
                connections.retain(|h| !h.is_finished());
                connections.push(task.abort_handle());
                pending.push_back((task.abort_handle(), authed));
            }
            Event::ListenerFailed(e) => {
                // The listener can no longer guarantee an instance. Move to a
                // fresh endpoint rather than advertise a pipe nobody can
                // reach. Existing sessions are unaffected.
                tracing::warn!(
                    target: "vault_app::keeper",
                    error = %e,
                    "listener failed; rotating to a fresh endpoint"
                );
                // On failure, stop through the SAME cleanup as every other
                // exit: an early return here once left the discovery file
                // pointing at a dead endpoint and every session still open.
                let rotated = bind_fresh(&settings).and_then(|(fresh, fresh_identity)| {
                    publish(&settings, &fresh_identity)?;
                    Ok((fresh, fresh_identity))
                });
                match rotated {
                    Ok((fresh, fresh_identity)) => {
                        listener = fresh;
                        identity = fresh_identity;
                        published = hex::encode(identity.nonce);
                    }
                    Err(e) => break Err(e),
                }
            }
            Event::Tick => {
                if active.load(Ordering::SeqCst) == 0 {
                    let since = *idle_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= settings.idle_exit {
                        break Ok(KeeperExit::Idle);
                    }
                } else {
                    idle_since = None;
                }
                // §6.2: both modes re-evaluate entitlement on each tick, off
                // the loop, and exit only when it has flipped.
                if let Some(sub) = subscription.as_ref() {
                    if !deciding.swap(true, Ordering::SeqCst) {
                        spawn_mode_evaluation(
                            sub.mode,
                            Arc::clone(&sub.check),
                            sub.gate.in_flight().clone(),
                            Arc::clone(&deciding),
                            stop_tx.clone(),
                        );
                    }
                }
            }
            Event::Flip => {
                // A call has already established that the user is entitled;
                // all that is left is to let calls in flight finish.
                pending_flip = None;
                if let Some(sub) = subscription.as_ref() {
                    spawn_mode_exit(sub.gate.in_flight().clone(), stop_tx.clone());
                }
            }
            Event::Stop(reason) => break Ok(reason),
        }
    };

    // Discovery first, so no relay dials a keeper that is going away; then
    // stop accepting, then close every open connection. A relay whose
    // connection closes fails its pending call at once and reconnects, rather
    // than talking to a keeper that has stopped.
    if let Err(e) = discovery::remove_if_ours(&settings.vault_root, &published) {
        tracing::warn!(target: "vault_app::keeper", error = %e, "could not remove discovery file");
    }
    drop(listener);
    for connection in &connections {
        connection.abort();
    }
    match &exit {
        Ok(reason) => {
            tracing::info!(target: "vault_app::keeper", exit = ?reason, "keeper stopping")
        }
        Err(e) => {
            tracing::error!(target: "vault_app::keeper", error = %e, "keeper stopping: no endpoint")
        }
    }
    exit
}

/// One turn of the serve loop.
enum Event {
    Accepted(ServerStream),
    ListenerFailed(std::io::Error),
    Tick,
    /// A lock-mode call found the user entitled again.
    Flip,
    Stop(KeeperExit),
}

/// Lock mode's flip, until it has been acted on. A future that never resolves
/// otherwise, so a full keeper never selects on it and an acted-on flip does
/// not spin the loop.
async fn next_flip(flip: Option<&Flip>) {
    match flip {
        Some(flip) => flip.wait().await,
        None => std::future::pending().await,
    }
}

/// Re-evaluate entitlement off the serve loop, and exit if it has flipped.
///
/// Spawned rather than run in the loop: `refresh_and_peek` can take 5 s and
/// the loop also owns `accept`, so evaluating inline would stop the keeper
/// answering new connections for as long as a refresh takes.
fn spawn_mode_evaluation(
    mode: KeeperMode,
    check: Arc<dyn ModeCheck>,
    in_flight: InFlight,
    deciding: Arc<AtomicBool>,
    stop_tx: mpsc::Sender<KeeperExit>,
) {
    tokio::spawn(async move {
        let flipped = match mode {
            // Locked now? A lease that is merely stale is not a reason to
            // unload and reload 2.86 GB, so refresh before deciding (§6.2).
            KeeperMode::Full => match check.peek().await {
                Verdict::Entitled => false,
                Verdict::Locked(_) => {
                    matches!(check.refresh_and_peek().await, Verdict::Locked(_))
                }
            },
            // Entitled again? A read is enough: a lease arrives through a
            // call or through the desktop app, never through this tick.
            KeeperMode::Lock => check.peek().await == Verdict::Entitled,
        };
        if flipped {
            in_flight.wait_idle().await;
            let _ = stop_tx.try_send(KeeperExit::ModeChanged);
            // Latched on purpose. The keeper is leaving, so no later
            // evaluation can mean anything — and `interval` bursts the ticks
            // it missed during a slow refresh, so releasing the latch here
            // would start a second refresh before the loop has even read the
            // stop message.
            return;
        }
        deciding.store(false, Ordering::SeqCst);
    });
}

/// Leave as soon as every call in flight has answered (§6.2: the idle exit
/// requires zero connections, `ModeChanged` does not, so it must not cut a
/// write off mid-way).
fn spawn_mode_exit(in_flight: InFlight, stop_tx: mpsc::Sender<KeeperExit>) {
    tokio::spawn(async move {
        in_flight.wait_idle().await;
        let _ = stop_tx.try_send(KeeperExit::ModeChanged);
    });
}

/// Everything one connection's task needs.
struct ConnectionContext {
    keys: Arc<HandshakeKeys>,
    identity: KeeperIdentity,
    adapter: Arc<dyn Adapter>,
    /// The subscription gate, when this build carries a sign-in (ADR-104).
    /// Every connection shares the one gate, so the whole keeper counts its
    /// calls in flight together (§8.26 §6.2).
    gate: Option<Gate>,
    active: Arc<AtomicUsize>,
    authed: Arc<AtomicBool>,
    stop_tx: mpsc::Sender<KeeperExit>,
    last_warn: Arc<Mutex<Option<Instant>>>,
    frame1_deadline: Duration,
    handshake_deadline: Duration,
}

/// Decrements the authenticated-connection count even if the task is aborted.
struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn handle_connection(mut stream: ServerStream, ctx: ConnectionContext) {
    let challenge = match random_bytes::<CHALLENGE_LEN>() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(target: "vault_app::keeper", error = %e, "no randomness for challenge");
            return;
        }
    };
    let accepted = keeper_handshake(
        &mut stream,
        &ctx.keys,
        &ctx.identity,
        challenge,
        ctx.frame1_deadline,
        ctx.handshake_deadline,
    )
    .await;

    match accepted {
        Ok(KeeperAccept::Serve { boundaries }) => {
            ctx.authed.store(true, Ordering::SeqCst);
            ctx.active.fetch_add(1, Ordering::SeqCst);
            let _guard = ActiveGuard(ctx.active.clone());
            let server = StdioServer::new(ctx.adapter, boundaries);
            serve_session(vault_mcp::maybe_gated(ctx.gate.as_ref(), server), stream).await;
        }
        Ok(KeeperAccept::Yield) => {
            tracing::info!(target: "vault_app::keeper", "a newer relay asked this keeper to yield");
            let _ = ctx.stop_tx.try_send(KeeperExit::Yielded);
        }
        Ok(KeeperAccept::Handover) => {
            tracing::info!(target: "vault_app::keeper", "asked to hand the vault over; stopping");
            let _ = ctx.stop_tx.try_send(KeeperExit::HandedOver);
        }
        Err(e) => note_rejection(&e, &ctx.last_warn),
    }
}

/// Serve one authenticated session until its stream closes.
async fn serve_session<S>(service: S, stream: ServerStream)
where
    S: Service<RoleServer>,
{
    match service.serve(stream).await {
        Ok(running) => {
            let _ = running.waiting().await;
        }
        Err(e) => {
            tracing::warn!(target: "vault_app::keeper", error = %e, "session setup failed");
        }
    }
}

/// Timeouts and dropped connections are normal (relays die, other accounts
/// open the pipe read-only). Proof and framing failures are security-relevant
/// and logged — at most once per [`WARN_INTERVAL`].
fn note_rejection(e: &HandshakeError, last_warn: &Mutex<Option<Instant>>) {
    match e {
        HandshakeError::Io(_) | HandshakeError::Timeout => {
            tracing::debug!(target: "vault_app::keeper", error = %e, "connection closed before authenticating");
        }
        _ => {
            let now = Instant::now();
            let due = match last_warn.lock() {
                Ok(mut guard) => {
                    let due = guard.map_or(true, |t| now.duration_since(t) >= WARN_INTERVAL);
                    if due {
                        *guard = Some(now);
                    }
                    due
                }
                Err(_) => true,
            };
            if due {
                tracing::warn!(target: "vault_app::keeper", error = %e, "rejected a connection that failed authentication");
            }
        }
    }
}

fn bind_fresh(settings: &KeeperSettings) -> VaultResult<(Listener, KeeperIdentity)> {
    let mut last_err = None;
    for _ in 0..MAX_ROTATIONS {
        let endpoint = discovery::new_endpoint(&settings.vault_root)
            .map_err(|e| VaultError::Io(std::io::Error::other(format!("random source: {e}"))))?;
        match Listener::bind(&endpoint) {
            Ok(listener) => {
                let identity = KeeperIdentity {
                    nonce: random_bytes::<NONCE_LEN>().map_err(VaultError::Io)?,
                    endpoint,
                    pid: std::process::id(),
                };
                return Ok((listener, identity));
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(VaultError::Io(last_err.unwrap_or_else(|| {
        std::io::Error::other("could not bind any keeper endpoint")
    })))
}

fn publish(settings: &KeeperSettings, identity: &KeeperIdentity) -> VaultResult<()> {
    let at = chrono::Utc::now().to_rfc3339();
    discovery::write(
        &settings.vault_root,
        &Discovery::keeper(identity, WIRE, &settings.version, &at),
    )
    .map_err(VaultError::Io)
}

fn random_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    let mut out = [0u8; N];
    getrandom::getrandom(&mut out)
        .map_err(|e| std::io::Error::other(format!("random source: {e}")))?;
    Ok(out)
}
