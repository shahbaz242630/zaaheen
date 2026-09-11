//! The relay's connection to the keeper (ADR-102): find it, ask Windows to
//! start it if there is none, authenticate, and forward tool calls.
//!
//! One [`KeeperPool`] per `zaaheen mcp serve` process. Resolution is
//! serialised behind one lock, so concurrent calls from the same AI app share
//! a single attempt instead of each starting their own.
//!
//! **A relay never touches the vault.** It never takes `.vault.lock` (probing
//! it could briefly steal it from a keeper that is starting), never opens a
//! store, never downloads a model, and reads the master key READ-ONLY — only
//! the keeper, holding the lock, may create it. (Two processes creating it at
//! once on a fresh install would leave the vault encrypted under a key that
//! was then overwritten.)
//!
//! **Time budget.** Resolution plus the call stays inside ~50 s, below the
//! 60 s default request timeout of the MCP TypeScript SDK most AI apps use; a
//! client that gives up while the keeper still commits a save is how
//! duplicates happen.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{Peer, RunningService, ServiceError};
use rmcp::{RoleClient, ServiceExt};
use tokio::time::Instant;
use vault_core::Boundary;
use vault_mcp::{Upstream, UpstreamError};
use zeroize::Zeroizing;

use super::discovery::{self, Role};
use super::handshake::{
    relay_handshake, HandshakeError, HandshakeKeys, KeeperIdentity, RelayOutcome, CHALLENGE_LEN,
    NONCE_LEN,
};
use super::transport::{self, ConnectError};

/// How long a keeper's "starting" record is trusted. Opening the vault takes
/// seconds; a record older than this belongs to a keeper that died starting,
/// and the relay goes back to asking for one.
const STARTING_VALID_FOR: Duration = Duration::from_secs(60);

/// How long a maintenance run's record is trusted. The run rewrites it every
/// [`MAINTENANCE_HEARTBEAT`], so a run that crashed stops blocking relays
/// within minutes rather than for the run's whole 30-minute cap.
const MAINTENANCE_VALID_FOR: Duration = Duration::from_secs(3 * 60);

/// How often a maintenance run refreshes its record.
pub const MAINTENANCE_HEARTBEAT: Duration = Duration::from_secs(60);

/// Agent-facing reasons a call never reached the keeper.
pub const MSG_STARTING: &str = "the vault is starting; try again in a moment";
pub const MSG_BUSY: &str =
    "the vault is busy (maintenance or another program is using it); try again shortly";
pub const MSG_FAILED: &str = "the vault could not start; open the Zaaheen app to check it";
pub const MSG_UPDATE: &str = "Zaaheen was updated; restart every app that uses Zaaheen";
pub const MSG_KEY_CHANGED: &str = "the vault's key changed; restart every app that uses Zaaheen";
pub const MSG_REFUSED: &str = "the vault refused this connection";

/// Asks the platform to start a keeper (Task Scheduler on Windows). Must be
/// idempotent: it is called repeatedly while waiting, and a keeper that is
/// already running must make it a no-op.
pub trait KeeperStarter: Send + Sync {
    /// # Errors
    ///
    /// The start request could not be issued. The relay keeps polling.
    fn request_start(&self) -> std::io::Result<()>;
}

/// Read-only access to the vault's master key.
pub trait MasterKeySource: Send + Sync {
    /// `None` when there is no key yet (only the keeper creates it).
    fn read(&self) -> Option<Zeroizing<[u8; 32]>>;
}

/// The production key source: the OS keychain, READ-ONLY. It never creates
/// the key — a relay that did could overwrite the key a starting keeper had
/// just used to create the vault.
pub struct KeychainKeySource;

impl MasterKeySource for KeychainKeySource {
    fn read(&self) -> Option<Zeroizing<[u8; 32]>> {
        match crate::keychain::read_existing_master_key(
            crate::keychain::PRODUCTION_NAMESPACE,
            crate::keychain::VAULT_ID,
        ) {
            Ok(key) => key,
            Err(e) => {
                // Polled while waiting for a keeper, so debug, not warn.
                tracing::debug!(target: "vault_app::relay", error = %e, "keychain read failed");
                None
            }
        }
    }
}

/// Tunables. [`RelaySettings::production`] holds the shipped values.
#[derive(Clone, Debug)]
pub struct RelaySettings {
    pub vault_root: PathBuf,
    pub boundaries: Vec<Boundary>,
    pub resolve_budget: Duration,
    pub start_every: Duration,
    pub poll_every: Duration,
    pub failed_cooldown: Duration,
    pub step_deadline: Duration,
    pub call_timeout: Duration,
    pub idle_drop: Duration,
}

impl RelaySettings {
    /// Shipped values: 15 s to find or start the keeper plus 35 s for the
    /// call keeps a whole request under ~50 s.
    pub fn production(vault_root: PathBuf, boundaries: Vec<Boundary>) -> Self {
        Self {
            vault_root,
            boundaries,
            resolve_budget: Duration::from_secs(15),
            start_every: Duration::from_millis(2500),
            poll_every: Duration::from_millis(200),
            failed_cooldown: Duration::from_secs(60),
            step_deadline: Duration::from_secs(5),
            call_timeout: Duration::from_secs(35),
            idle_drop: Duration::from_secs(15 * 60),
        }
    }
}

/// The relay's single upstream connection.
pub struct KeeperPool {
    settings: RelaySettings,
    starter: Arc<dyn KeeperStarter>,
    keys: Arc<dyn MasterKeySource>,
    state: tokio::sync::Mutex<PoolState>,
    /// Kept OUTSIDE `state`: a finished call records its time here without
    /// queueing behind another call that holds `state` for a whole
    /// resolution (up to 15 s), which used to delay its reply by as much.
    last_used: Mutex<Instant>,
}

struct PoolState {
    service: Option<RunningService<RoleClient, ()>>,
    /// Bumped per new connection, so a failure on an old connection never
    /// tears down its replacement.
    generation: u64,
}

/// Outcome of one connection attempt.
enum Attempt {
    Connected(RunningService<RoleClient, ()>),
    /// Alive but every pipe instance is taken; do not request a start.
    Busy,
    /// Nothing usable at the endpoint.
    Gone,
    /// An older keeper agreed to step aside; wait for its successor.
    Yielded,
    KeeperNewer,
    ProofMismatch,
    Rejected,
}

impl KeeperPool {
    pub fn new(
        settings: RelaySettings,
        starter: Arc<dyn KeeperStarter>,
        keys: Arc<dyn MasterKeySource>,
    ) -> Arc<Self> {
        Arc::new(Self {
            settings,
            starter,
            keys,
            state: tokio::sync::Mutex::new(PoolState {
                service: None,
                generation: 0,
            }),
            last_used: Mutex::new(Instant::now()),
        })
    }

    fn touch(&self) {
        if let Ok(mut last) = self.last_used.lock() {
            *last = Instant::now();
        }
    }

    fn idle_for(&self) -> Duration {
        self.last_used
            .lock()
            .map(|last| last.elapsed())
            .unwrap_or_default()
    }

    /// Close the upstream after `idle_drop` without calls, so the keeper can
    /// idle out and free the vault for nightly maintenance while the AI app
    /// stays open. The next call reconnects.
    pub fn spawn_idle_reaper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let weak: Weak<Self> = Arc::downgrade(self);
        let every = self.settings.idle_drop.min(Duration::from_secs(60));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(pool) = weak.upgrade() else {
                    return;
                };
                let mut st = pool.state.lock().await;
                if st.service.is_some() && pool.idle_for() >= pool.settings.idle_drop {
                    tracing::info!(target: "vault_app::relay", "closing idle keeper connection");
                    st.service = None;
                }
            }
        })
    }

    /// A live peer, connecting (and starting a keeper) if needed.
    async fn peer(&self) -> Result<(Peer<RoleClient>, u64), &'static str> {
        let mut st = self.state.lock().await;
        let alive = st
            .service
            .as_ref()
            .is_some_and(|svc| !svc.is_transport_closed());
        if !alive {
            st.service = None;
            let service = self.resolve().await?;
            st.generation += 1;
            st.service = Some(service);
        }
        self.touch();
        match st.service.as_ref() {
            Some(svc) => Ok((svc.peer().clone(), st.generation)),
            None => Err(MSG_STARTING),
        }
    }

    /// Drop the connection of `generation` — never a newer one.
    async fn forget(&self, generation: u64) {
        let mut st = self.state.lock().await;
        if st.generation == generation {
            st.service = None;
        }
    }

    async fn resolve(&self) -> Result<RunningService<RoleClient, ()>, &'static str> {
        let s = &self.settings;
        let deadline = Instant::now() + s.resolve_budget;
        let mut last_start: Option<Instant> = None;
        // Keeper tenures whose proof did not verify with our key.
        let mut mismatched: Vec<[u8; NONCE_LEN]> = Vec::new();
        // What to report if the budget runs out: the latest thing seen.
        let mut last_problem: Option<&'static str> = None;

        loop {
            let mut want_start = true;
            // Re-read each round: on a fresh install the key appears only once
            // the keeper we asked for has created it.
            if let Some(master_key) = self.keys.read() {
                let keys = HandshakeKeys::derive(&master_key);
                drop(master_key);
                // No file, or one that cannot be read, means no usable keeper.
                if let Ok(Some(d)) = discovery::read(&s.vault_root) {
                    match d.role {
                        Role::Failed if is_recent(&d.at, s.failed_cooldown) => {
                            return Err(MSG_FAILED);
                        }
                        // A keeper cannot start until the run ends: say so at
                        // once rather than spend the budget asking for one.
                        Role::Maintenance if is_recent(&d.at, MAINTENANCE_VALID_FOR) => {
                            return Err(MSG_BUSY);
                        }
                        Role::Starting if is_recent(&d.at, STARTING_VALID_FOR) => {
                            want_start = false;
                            last_problem = None;
                        }
                        Role::Keeper => {
                            if let Ok(identity) = d.keeper_identity(&s.vault_root) {
                                // A tenure that already failed our key is
                                // not retried: the endpoint is a squatter's or
                                // the key is not ours, and a fresh keeper (new
                                // tenure) settles which.
                                if !mismatched.contains(&identity.nonce) {
                                    match self.attempt(&identity, &keys).await {
                                        Attempt::Connected(service) => return Ok(service),
                                        Attempt::Busy => {
                                            want_start = false;
                                            last_problem = Some(MSG_BUSY);
                                        }
                                        Attempt::Gone | Attempt::Yielded => last_problem = None,
                                        Attempt::KeeperNewer => return Err(MSG_UPDATE),
                                        Attempt::ProofMismatch => {
                                            // One tenure can be a squatter on
                                            // a dead keeper's name. Two
                                            // DIFFERENT tenures both failing
                                            // means the key is not the vault's.
                                            // Counting attempts instead let one
                                            // squatter report "key changed" in
                                            // under a second.
                                            mismatched.push(identity.nonce);
                                            if mismatched.len() >= 2 {
                                                return Err(MSG_KEY_CHANGED);
                                            }
                                            last_problem = Some(MSG_KEY_CHANGED);
                                        }
                                        Attempt::Rejected => return Err(MSG_REFUSED),
                                    }
                                }
                            }
                        }
                        // Stale records, and roles from a newer build: no
                        // usable keeper, so ask for one.
                        _ => {}
                    }
                }
            }

            let start_due = last_start.map_or(true, |t| t.elapsed() >= s.start_every);
            if want_start && start_due {
                let starter = self.starter.clone();
                match tokio::task::spawn_blocking(move || starter.request_start()).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::warn!(target: "vault_app::relay", error = %e, "keeper start request failed");
                    }
                    Err(e) => {
                        tracing::warn!(target: "vault_app::relay", error = %e, "keeper start task failed");
                    }
                }
                last_start = Some(Instant::now());
            }

            if Instant::now() >= deadline {
                return Err(last_problem.unwrap_or(MSG_STARTING));
            }
            tokio::time::sleep(s.poll_every).await;
        }
    }

    async fn attempt(&self, identity: &KeeperIdentity, keys: &HandshakeKeys) -> Attempt {
        let mut stream = match transport::connect(&identity.endpoint).await {
            Ok(stream) => stream,
            Err(ConnectError::Busy) => return Attempt::Busy,
            Err(_) => return Attempt::Gone,
        };
        let mut challenge = [0u8; CHALLENGE_LEN];
        if getrandom::getrandom(&mut challenge).is_err() {
            return Attempt::Gone;
        }
        match relay_handshake(
            &mut stream,
            keys,
            identity,
            &self.settings.boundaries,
            challenge,
            self.settings.step_deadline,
        )
        .await
        {
            // Bounded like every other step: the MCP initialize exchange
            // with a keeper that stalls after the handshake (or a peer that
            // proved itself and then went silent) must not hold this relay's
            // single resolution — and every call queued behind it — forever.
            Ok(RelayOutcome::Serving) => {
                match tokio::time::timeout(self.settings.step_deadline, ().serve(stream)).await {
                    Ok(Ok(service)) => Attempt::Connected(service),
                    Ok(Err(e)) => {
                        tracing::warn!(target: "vault_app::relay", error = %e, "keeper session setup failed");
                        Attempt::Gone
                    }
                    Err(_) => {
                        tracing::warn!(target: "vault_app::relay", "keeper session setup timed out");
                        Attempt::Gone
                    }
                }
            }
            Ok(RelayOutcome::KeeperYielded) => Attempt::Yielded,
            Err(HandshakeError::ProofMismatch) => Attempt::ProofMismatch,
            Err(HandshakeError::KeeperNewer { .. }) => Attempt::KeeperNewer,
            Err(HandshakeError::Rejected) => Attempt::Rejected,
            Err(_) => Attempt::Gone,
        }
    }
}

#[async_trait]
impl Upstream for KeeperPool {
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
    ) -> Result<CallToolResult, UpstreamError> {
        let (peer, generation) = self.peer().await.map_err(UpstreamError::NotSent)?;
        let outcome =
            tokio::time::timeout(self.settings.call_timeout, peer.call_tool(params)).await;
        self.touch();
        match outcome {
            // A slow keeper is not a dead one: keep the connection.
            Err(_) => Err(UpstreamError::TimedOut),
            Ok(Ok(result)) => Ok(result),
            Ok(Err(ServiceError::McpError(e))) => Err(UpstreamError::Keeper(e)),
            // Never left this process: safe to report as not sent.
            Ok(Err(ServiceError::TransportSend(_))) => {
                self.forget(generation).await;
                Err(UpstreamError::NotSent(MSG_STARTING))
            }
            // Anything else after sending: outcome unknown.
            Ok(Err(_)) => {
                self.forget(generation).await;
                Err(UpstreamError::Lost)
            }
        }
    }
}

/// `true` when `at` (RFC 3339) is within `window` of now.
fn is_recent(at: &str, window: Duration) -> bool {
    let Ok(written) = chrono::DateTime::parse_from_rfc3339(at) else {
        return false;
    };
    let age = chrono::Utc::now().signed_duration_since(written);
    age >= chrono::Duration::zero() && age.to_std().is_ok_and(|a| a < window)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_means_within_the_window_and_not_in_the_future() {
        let now = chrono::Utc::now();
        assert!(is_recent(&now.to_rfc3339(), Duration::from_secs(60)));
        let old = now - chrono::Duration::seconds(120);
        assert!(!is_recent(&old.to_rfc3339(), Duration::from_secs(60)));
        let future = now + chrono::Duration::seconds(120);
        assert!(!is_recent(&future.to_rfc3339(), Duration::from_secs(60)));
        assert!(!is_recent("not a time", Duration::from_secs(60)));
    }

    #[test]
    fn production_budget_stays_under_the_client_timeout() {
        let s = RelaySettings::production(PathBuf::from("x"), vec![]);
        assert!(
            s.resolve_budget + s.call_timeout <= Duration::from_secs(50),
            "resolution plus the call must finish before a 60 s client timeout"
        );
    }
}
