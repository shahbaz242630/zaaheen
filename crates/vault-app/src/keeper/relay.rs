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
//! **Time budget (ADR-103 D1).** Every call carries the deadline its relay
//! fixed when the call arrived ([`vault_mcp::RELAY_CALL_BUDGET`], inside the
//! 60 s default request timeout of the MCP TypeScript SDK most AI apps use).
//! Resolution stops at the earlier of its own budget and that deadline, and
//! the forwarded call stops AT the deadline — so a slow keeper gets all the
//! time the client will actually wait, and never more: a client that gives up
//! while the keeper still commits a save is how duplicates happen.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{Peer, RunningService, ServiceError};
use rmcp::{RoleClient, ServiceExt};
use tokio::time::Instant;
use vault_core::Boundary;
use vault_mcp::{LockReason, Upstream, UpstreamError};
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

/// §8.26 §6.3: the marker is checked twice, this far apart.
pub const MARKER_RECHECK: Duration = Duration::from_millis(200);

/// Agent-facing reasons a call never reached the keeper.
pub const MSG_STARTING: &str = "the vault is starting; try again in a moment";
pub const MSG_BUSY: &str =
    "the vault is busy (maintenance or another program is using it); try again shortly";
pub const MSG_FAILED: &str = "the vault could not start; open the Zaaheen app to check it";
pub const MSG_UPDATE: &str = "Zaaheen was updated; restart every app that uses Zaaheen";
pub const MSG_KEY_CHANGED: &str = "the vault's key changed; restart every app that uses Zaaheen";
pub const MSG_REFUSED: &str = "the vault refused this connection";
/// ADR-105 L3: the recorded vault folder is missing or is not this vault (an
/// unplugged drive, a different stick at the same letter).
pub const MSG_LOCATION_MISSING: &str =
    "Zaaheen can't find your memories; open the Zaaheen app to check where they are";

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

/// Which vault folder the relay looks for a keeper in.
#[derive(Clone, Debug)]
pub enum RelayVault {
    /// A fixed folder (tests, explicit paths).
    Fixed(PathBuf),
    /// The recorded location (ADR-105 L3), re-resolved on every connect
    /// round, so a relay alive across a move follows it. Nothing recorded
    /// yet → a keeper is started (it runs the first-run setup); a record
    /// whose folder is missing or wrong → answered at once, no start.
    Recorded(crate::location::Homes),
}

/// What one connect round found for the vault folder.
enum RoundVault {
    Folder(PathBuf),
    /// Nothing recorded yet: only a keeper's first-run setup can answer.
    Unset,
}

impl RelayVault {
    fn resolve(&self) -> Result<RoundVault, &'static str> {
        match self {
            Self::Fixed(p) => Ok(RoundVault::Folder(p.clone())),
            Self::Recorded(homes) => match crate::location::resolve(homes) {
                Ok(dir) => Ok(RoundVault::Folder(dir.path().to_path_buf())),
                Err(vault_core::VaultError::VaultLocation(
                    vault_core::VaultLocationFailure::Unset,
                )) => Ok(RoundVault::Unset),
                Err(_) => Err(MSG_LOCATION_MISSING),
            },
        }
    }
}

/// Tunables. [`RelaySettings::production`] holds the shipped values.
#[derive(Clone, Debug)]
pub struct RelaySettings {
    pub vault: RelayVault,
    pub boundaries: Vec<Boundary>,
    pub resolve_budget: Duration,
    pub start_every: Duration,
    pub poll_every: Duration,
    pub failed_cooldown: Duration,
    pub step_deadline: Duration,
    pub idle_drop: Duration,
    /// The account folder to read the sign-in marker from, when this build
    /// carries a sign-in (§8.26 §6.3). `None` — every build before this arc —
    /// means the relay never short-circuits.
    ///
    /// Read-only: only the keeper and the desktop write in this folder
    /// (§8.26 §4), so the relay must not even create it.
    pub account_dir: Option<PathBuf>,
    /// How long between the two marker checks. Sign-in writes the marker, so
    /// one look at the wrong moment would tell somebody who has just signed in
    /// to sign in again.
    pub marker_recheck: Duration,
}

impl RelaySettings {
    /// Shipped values. At most 15 s of a call's deadline is spent finding or
    /// starting the keeper; the call itself runs until the deadline (ADR-103
    /// D1 — there is no separate call timeout any more).
    pub fn production(vault_root: PathBuf, boundaries: Vec<Boundary>) -> Self {
        Self::for_vault(RelayVault::Fixed(vault_root), boundaries)
    }

    /// Shipped values over the recorded location (ADR-105): what an AI app's
    /// `zaaheen mcp serve` uses.
    pub fn recorded(homes: crate::location::Homes, boundaries: Vec<Boundary>) -> Self {
        Self::for_vault(RelayVault::Recorded(homes), boundaries)
    }

    fn for_vault(vault: RelayVault, boundaries: Vec<Boundary>) -> Self {
        Self {
            vault,
            boundaries,
            resolve_budget: Duration::from_secs(15),
            start_every: Duration::from_millis(2500),
            poll_every: Duration::from_millis(200),
            failed_cooldown: Duration::from_secs(60),
            step_deadline: Duration::from_secs(5),
            idle_drop: Duration::from_secs(15 * 60),
            // Set by the caller when the build carries account settings; a
            // build without them has no sign-in and no marker to read.
            account_dir: None,
            marker_recheck: MARKER_RECHECK,
        }
    }

    /// Read the sign-in marker from `account_dir` (§8.26 §6.3). `None` keeps
    /// the relay's behaviour exactly as it was before this arc.
    #[must_use]
    pub fn with_account_dir(mut self, account_dir: Option<PathBuf>) -> Self {
        self.account_dir = account_dir;
        self
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

    /// A live peer, connecting (and starting a keeper) if needed — within the
    /// call's `deadline`.
    async fn peer(&self, deadline: Instant) -> Result<(Peer<RoleClient>, u64), &'static str> {
        let mut st = self.state.lock().await;
        let alive = st
            .service
            .as_ref()
            .is_some_and(|svc| !svc.is_transport_closed());
        if !alive {
            st.service = None;
            let service = self.resolve(deadline).await?;
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

    /// Find, start and authenticate a keeper. Stops at the earlier of the
    /// resolution budget and the call's deadline: resolving past the point the
    /// client has given up would only start a keeper nobody is waiting for.
    /// Whether this computer is definitely signed out (§8.26 §6.3).
    ///
    /// Checked twice, [`RelaySettings::marker_recheck`] apart, because sign-in
    /// writes the marker: one look at the wrong moment would tell somebody who
    /// has just signed in to sign in again. Anything other than a definite
    /// "nobody is signed in" — including a folder that cannot be read — leaves
    /// the decision to the keeper, which has the real check and can give the
    /// accurate reason.
    async fn signed_out(&self) -> bool {
        let Some(dir) = self.settings.account_dir.as_ref() else {
            return false;
        };
        if crate::account::sign_in_marker(dir) != Some(false) {
            return false;
        }
        tokio::time::sleep(self.settings.marker_recheck).await;
        crate::account::sign_in_marker(dir) == Some(false)
    }

    async fn resolve(
        &self,
        call_deadline: Instant,
    ) -> Result<RunningService<RoleClient, ()>, &'static str> {
        // Nobody signed in on this computer: a keeper could only say the same
        // thing, so say it here and start none (§6.3). Before the discovery
        // read and before any start request.
        if self.signed_out().await {
            tracing::info!(
                target: "vault_app::relay",
                "nobody is signed in on this computer; answering without starting a keeper"
            );
            return Err(LockReason::SignedOut.message());
        }
        let s = &self.settings;
        let deadline = (Instant::now() + s.resolve_budget).min(call_deadline);
        let mut last_start: Option<Instant> = None;
        // Keeper tenures whose proof did not verify with our key.
        let mut mismatched: Vec<[u8; NONCE_LEN]> = Vec::new();
        // What to report if the budget runs out: the latest thing seen.
        let mut last_problem: Option<&'static str> = None;

        loop {
            let mut want_start = true;
            // ADR-105 L3: the folder is re-resolved every round, so a relay
            // alive across a move follows it. A recorded folder that is
            // missing or wrong is answered at once, with no keeper start (a
            // keeper could not start there either); nothing recorded yet
            // leaves `want_start` on, and the keeper records it.
            let vault_root = match s.vault.resolve()? {
                RoundVault::Folder(root) => Some(root),
                RoundVault::Unset => None,
            };
            // Re-read each round: on a fresh install the key appears only once
            // the keeper we asked for has created it.
            let master_key = match vault_root {
                Some(_) => self.keys.read(),
                None => None,
            };
            if let (Some(vault_root), Some(master_key)) = (vault_root.as_deref(), master_key) {
                let keys = HandshakeKeys::derive(&master_key);
                drop(master_key);
                // No file, or one that cannot be read, means no usable keeper.
                if let Ok(Some(d)) = discovery::read(vault_root) {
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
                            if let Ok(identity) = d.keeper_identity(vault_root) {
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
        deadline: std::time::Instant,
    ) -> Result<CallToolResult, UpstreamError> {
        let deadline = Instant::from_std(deadline);
        let (peer, generation) = self.peer(deadline).await.map_err(UpstreamError::NotSent)?;
        // Until the call's own deadline (ADR-103 D1): all the time the client
        // will wait, however much of it finding the keeper used.
        let outcome = tokio::time::timeout_at(deadline, peer.call_tool(params)).await;
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
    fn resolution_leaves_most_of_the_call_budget_for_the_call() {
        // ADR-103 D1: resolution is bounded by BOTH its own budget and the
        // call's deadline; the whole call is bounded by RELAY_CALL_BUDGET,
        // which vault-mcp pins under the client's 60 s. A resolution budget
        // near the call budget would leave a just-started keeper no time to
        // answer.
        let s = RelaySettings::production(PathBuf::from("x"), vec![]);
        assert!(
            s.resolve_budget * 3 <= vault_mcp::RELAY_CALL_BUDGET,
            "finding the keeper must leave most of the call budget for the call"
        );
    }

    // ── ADR-105 L3: relays follow the recorded location ─────────────────

    struct CountingStarter(std::sync::atomic::AtomicUsize);

    impl KeeperStarter for CountingStarter {
        fn request_start(&self) -> std::io::Result<()> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    struct NoKey;

    impl MasterKeySource for NoKey {
        fn read(&self) -> Option<Zeroizing<[u8; 32]>> {
            None
        }
    }

    fn homes(tmp: &std::path::Path) -> crate::location::Homes {
        crate::location::Homes {
            local: tmp.join("Local"),
            roaming: tmp.join("Roaming"),
        }
    }

    /// A relay over the recorded location, with a short budget, counting
    /// the keeper starts it asks for.
    fn pool(homes: crate::location::Homes) -> (Arc<KeeperPool>, Arc<CountingStarter>) {
        let mut settings = RelaySettings::recorded(homes, vec![]);
        settings.resolve_budget = Duration::from_millis(600);
        settings.poll_every = Duration::from_millis(50);
        settings.start_every = Duration::from_millis(100);
        let starter = Arc::new(CountingStarter(std::sync::atomic::AtomicUsize::new(0)));
        let pool = KeeperPool::new(settings, starter.clone(), Arc::new(NoKey));
        (pool, starter)
    }

    fn starts(starter: &CountingStarter) -> usize {
        starter.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A recorded folder that is not there (a drive that is out): the AI app
    /// is told at once, and no keeper is started — one could not open the
    /// memories either, and starting one each round would loop (R2-4).
    #[tokio::test]
    async fn a_missing_location_is_answered_without_starting_a_keeper() {
        let tmp = tempfile::tempdir().unwrap();
        let h = homes(tmp.path());
        let usb = tmp.path().join("E").join("Zaaheen Memories");
        crate::location::pointer::write(
            &h.pointer_path(),
            &crate::location::Pointer::new(usb, crate::location::pointer::new_id().unwrap()),
        )
        .unwrap();
        let (pool, starter) = pool(h);
        let answer = pool.resolve(Instant::now() + Duration::from_secs(5)).await;
        assert_eq!(answer.err(), Some(MSG_LOCATION_MISSING));
        assert_eq!(
            starts(&starter),
            0,
            "no keeper start for a missing location"
        );
    }

    /// Nothing recorded yet (an AI app used before the desktop ever ran): a
    /// keeper is asked for, since its first-run setup records the location.
    #[tokio::test]
    async fn with_nothing_recorded_a_keeper_is_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        let (pool, starter) = pool(homes(tmp.path()));
        let answer = pool.resolve(Instant::now() + Duration::from_secs(5)).await;
        assert_eq!(answer.err(), Some(MSG_STARTING));
        assert!(
            starts(&starter) >= 1,
            "the keeper's setup records the location"
        );
    }

    /// Each round resolves the record afresh: a relay alive across a move
    /// finds the new folder.
    #[test]
    fn each_round_follows_the_record() {
        let tmp = tempfile::tempdir().unwrap();
        let h = homes(tmp.path());
        let key = crate::keychain::test_helpers::test_location("relay", tmp.path());
        let first = crate::location::prepare(&h, &key, &[]).unwrap();
        let vault = RelayVault::Recorded(h.clone());
        assert!(matches!(vault.resolve(), Ok(RoundVault::Folder(p)) if p == first.path()));

        let moved = tmp.path().join("E").join("Zaaheen Memories");
        std::fs::create_dir_all(&moved).unwrap();
        let id = first.pointer().vault_id.clone();
        std::fs::write(moved.join(crate::location::VAULT_ID_FILE), &id).unwrap();
        crate::location::pointer::write(
            &h.pointer_path(),
            &crate::location::Pointer::new(moved.clone(), id),
        )
        .unwrap();
        assert!(matches!(vault.resolve(), Ok(RoundVault::Folder(p)) if p == moved));
    }
}
