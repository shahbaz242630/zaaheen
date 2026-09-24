//! The desktop app's link to the keeper (ADR-108 D4-D6).
//!
//! The desktop never opens the vault: every screen's data comes from the
//! keeper over an authenticated admin connection. This module is that
//! connection as the commands use it:
//!
//! - **One link per window**, the relays' own `KeeperPool` with the admin
//!   purpose, so finding, starting, authenticating, idling out and version
//!   handling are the ones the AI apps already rely on.
//! - **Who may start a keeper** (D4): only a command that is running, and
//!   only when a key exists or the desktop's guard has just let a gated
//!   command through. Open commands on a computer with no key answer here,
//!   without a keeper ([`KeyState`]).
//! - **Stable codes** (BRD §11.7.2): every way a call can fail is one of
//!   [`ALL_CODES`], each with a plain line in the desktop bundle.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::Value;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use vault_app::keeper::discovery::{self, Role};
use vault_app::keeper::relay::{
    CallError, KeeperPool, KeeperStarter, KeychainKeySource, RelaySettings, ResolveError,
    DESKTOP_FIND_BUDGET,
};
use vault_app::keeper::start_failure;
use vault_app::location::Homes;

/// The vault is being tidied ("Run now", or the nightly run): try soon.
pub const ERR_MAINTENANCE: &str = "vault_maintenance_in_progress";
/// This window is older than the keeper: close and reopen.
pub const ERR_UPDATE_NEEDED: &str = "update_needed";
/// A save whose answer was lost: it may or may not have been kept.
pub const ERR_OUTCOME_UNKNOWN: &str = "outcome_unknown";
/// The background part could not be reached or started.
pub const ERR_KEEPER_UNREACHABLE: &str = "keeper_unreachable";
/// The keeper is answering other questions (or handing over): ask again.
pub const ERR_ADMIN_BUSY: &str = vault_app::admin::ADMIN_BUSY;

/// Every code this module returns, for the test pinning each to a plain line
/// in the desktop bundle.
pub const ALL_CODES: &[&str] = &[
    ERR_MAINTENANCE,
    ERR_UPDATE_NEEDED,
    ERR_OUTCOME_UNKNOWN,
    ERR_KEEPER_UNREACHABLE,
    ERR_ADMIN_BUSY,
];

/// A desktop call's time: finding a keeper that is still opening the vault,
/// then the call itself (review B R2-S1).
const CALL_BUDGET: Duration = Duration::from_secs(55);

/// How long a "starting", "failed" or "maintenance" record is believed, as
/// the relays do.
const RECORD_VALID_FOR: Duration = Duration::from_secs(3 * 60);

/// Whether a call may be repeated after its answer was lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Changes nothing: repeated once if its answer was lost.
    Read,
    /// Changes the vault: never repeated; a lost answer is
    /// [`ERR_OUTCOME_UNKNOWN`].
    Write,
}

/// The vault key, as the desktop may read it: read-only, never created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    Present,
    /// No key. `keyed_data` says whether memories sealed under one are on
    /// disk anyway (a lost key, another Windows account): then nothing may
    /// say "nothing yet" (review A R2-2).
    Absent {
        keyed_data: bool,
    },
    /// The credential store could not be read.
    Unreadable,
}

/// The desktop's link. Managed state; one per window.
pub struct KeeperLink {
    pool: Arc<KeeperPool>,
    homes: Homes,
    /// The last call reached the keeper.
    connected: AtomicBool,
    /// "Run now" is running: say "tidying" at once rather than reconnect into
    /// the hand-over (review B-S3).
    tidying: AtomicBool,
    /// The search in flight, cancelled when a newer one starts so abandoned
    /// searches never queue ahead of the AI apps' questions (review B-S2).
    search: Mutex<Option<CancellationToken>>,
}

impl KeeperLink {
    /// The shipped link: the recorded vault location, the per-user keeper
    /// task (with a direct start as the fallback), the key read read-only.
    pub fn new(homes: Homes) -> Self {
        let starter: Arc<dyn KeeperStarter> = Arc::new(crate::keeper_start::DesktopStarter);
        let pool = KeeperPool::new(
            RelaySettings::desktop(homes.clone()),
            starter,
            Arc::new(KeychainKeySource),
        );
        Self::with_pool(pool, homes)
    }

    /// A link over a given pool (tests).
    pub fn with_pool(pool: Arc<KeeperPool>, homes: Homes) -> Self {
        // Dropped after 15 idle minutes, as a relay's is, so the keeper can
        // idle out and the nightly run proceeds with the window open.
        let _reaper = pool.spawn_idle_reaper();
        Self {
            pool,
            homes,
            connected: AtomicBool::new(false),
            tidying: AtomicBool::new(false),
            search: Mutex::new(None),
        }
    }

    /// The vault folder, when one is recorded and present.
    pub fn vault_root(&self) -> Option<PathBuf> {
        vault_app::location::resolve(&self.homes)
            .ok()
            .map(|dir| dir.path().to_path_buf())
    }

    /// Call an admin tool; the tool's JSON text, or a stable code (or the
    /// error text the desktop always showed for it).
    ///
    /// # Errors
    ///
    /// A code from [`ALL_CODES`], a `locked_*` code, or the tool's own error.
    pub async fn call(
        &self,
        tool: &'static str,
        args: Value,
        kind: Kind,
    ) -> Result<String, String> {
        self.call_with(tool, args, kind, CancellationToken::new())
            .await
    }

    /// [`Self::call`] for a search: a newer search cancels this one.
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub async fn call_search(&self, tool: &'static str, args: Value) -> Result<String, String> {
        let cancel = CancellationToken::new();
        if let Ok(mut slot) = self.search.lock() {
            if let Some(previous) = slot.replace(cancel.clone()) {
                previous.cancel();
            }
        }
        self.call_with(tool, args, Kind::Read, cancel).await
    }

    async fn call_with(
        &self,
        tool: &'static str,
        args: Value,
        kind: Kind,
        cancel: CancellationToken,
    ) -> Result<String, String> {
        if self.tidying.load(Ordering::SeqCst) {
            return Err(ERR_MAINTENANCE.to_string());
        }
        let mut params = CallToolRequestParams::new(tool);
        params.arguments = args.as_object().cloned();
        let deadline = Instant::now() + DESKTOP_FIND_BUDGET + CALL_BUDGET;
        let mut resent = false;
        loop {
            match self
                .pool
                .call(params.clone(), deadline, cancel.clone())
                .await
            {
                Ok(result) => {
                    self.connected.store(true, Ordering::SeqCst);
                    let text = text_of(&result);
                    return if result.is_error == Some(true) {
                        Err(text)
                    } else {
                        Ok(text)
                    };
                }
                Err(CallError::NotSent(reason)) => {
                    self.connected.store(false, Ordering::SeqCst);
                    return Err(self.code_for(reason).await);
                }
                Err(CallError::TimedOut) => {
                    return Err(match kind {
                        Kind::Write => ERR_OUTCOME_UNKNOWN,
                        Kind::Read => ERR_ADMIN_BUSY,
                    }
                    .to_string());
                }
                // ADR-102 D6: a read is repeated once; a save never.
                Err(CallError::Lost) => {
                    self.connected.store(false, Ordering::SeqCst);
                    match kind {
                        Kind::Write => return Err(ERR_OUTCOME_UNKNOWN.to_string()),
                        Kind::Read if !resent => resent = true,
                        Kind::Read => return Err(ERR_KEEPER_UNREACHABLE.to_string()),
                    }
                }
                Err(CallError::Keeper(e)) => {
                    tracing::warn!(target: "vault_tauri::link", tool, error = %e.message, "the keeper refused an admin call");
                    return Err("invalid_input".to_string());
                }
            }
        }
    }

    async fn code_for(&self, reason: ResolveError) -> String {
        match reason {
            ResolveError::Busy => ERR_MAINTENANCE,
            ResolveError::UpdateNeeded => {
                // A newer keeper: this window must never start or evict one.
                self.pool.poison().await;
                ERR_UPDATE_NEEDED
            }
            ResolveError::Cancelled => vault_app::admin::ADMIN_CANCELLED,
            ResolveError::Starting
            | ResolveError::Failed { .. }
            | ResolveError::KeyChanged
            | ResolveError::Refused
            | ResolveError::LocationMissing
            | ResolveError::SignedOut
            | ResolveError::Poisoned => ERR_KEEPER_UNREACHABLE,
        }
        .to_string()
    }

    /// "Run now" has started the runner: say "tidying" until it ends.
    pub fn set_tidying(&self, tidying: bool) {
        self.tidying.store(tidying, Ordering::SeqCst);
    }

    /// Close the connection (erasure and the move do this first).
    pub async fn disconnect(&self) {
        self.connected.store(false, Ordering::SeqCst);
        self.pool.disconnect().await;
    }

    /// Never start or reach a keeper again (after "Delete everything").
    pub async fn poison(&self) {
        self.connected.store(false, Ordering::SeqCst);
        self.pool.poison().await;
    }

    /// Undo [`Self::poison`]: the erasure or the move did not happen.
    pub fn clear_poison(&self) {
        self.pool.clear_poison();
    }

    /// Where the link is, for the home screen (`link_state`). Reads files
    /// only: it never starts a keeper (review A-S8).
    pub fn state(&self) -> LinkState {
        if self.tidying.load(Ordering::SeqCst) {
            return LinkState::Tidying;
        }
        let root = self.vault_root();
        let record = root
            .as_deref()
            .and_then(|r| discovery::read(r).ok().flatten());
        if let Some(d) = &record {
            if d.role == Role::Maintenance && is_recent(&d.at, RECORD_VALID_FOR) {
                return LinkState::Tidying;
            }
        }
        if self.connected.load(Ordering::SeqCst) {
            return LinkState::Serving;
        }
        if let (Some(root), Some(d)) = (root.as_deref(), &record) {
            if d.role == Role::Failed && is_recent(&d.at, RECORD_VALID_FOR) {
                if let Some(code) = start_failure::read_for(root, d.pid) {
                    if !code.is_transient() {
                        if let Some(err) = code.as_error() {
                            return LinkState::Failed(crate::format_keychain_error_dialog(&err));
                        }
                    }
                }
            }
        }
        LinkState::Connecting
    }
}

/// What the home screen shows about the link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    Connecting,
    Serving,
    Tidying,
    /// The keeper could not start; the message is the same startup message
    /// the desktop showed before D4, word for word.
    Failed(String),
}

impl LinkState {
    /// The wire shape for `link_state`.
    pub fn to_json(&self) -> Value {
        match self {
            Self::Connecting => serde_json::json!({ "state": "connecting" }),
            Self::Serving => serde_json::json!({ "state": "serving" }),
            Self::Tidying => serde_json::json!({ "state": "tidying" }),
            Self::Failed(message) => serde_json::json!({ "state": "failed", "message": message }),
        }
    }
}

impl KeeperLink {
    /// The desktop's view of the key, for this vault: read-only, never
    /// created (ADR-108 D1). A recorded folder that cannot be found (a drive
    /// pulled out) counts as "memories may be there": never "nothing yet"
    /// (security review N1). Only "nothing recorded yet" is a fresh install.
    pub fn key_state(&self) -> KeyState {
        let located = vault_app::location::resolve(&self.homes);
        let root = located.as_ref().ok().map(|dir| dir.path().to_path_buf());
        let fresh = matches!(
            located,
            Err(vault_core::VaultError::VaultLocation(
                vault_core::VaultLocationFailure::Unset
            ))
        );
        key_state_at(root.as_deref(), root.is_none() && !fresh)
    }
}

/// The key's state for `vault_root`; `location_missing` when a folder is
/// recorded but cannot be found.
fn key_state_at(vault_root: Option<&Path>, location_missing: bool) -> KeyState {
    match vault_app::keychain::read_existing_master_key(
        vault_app::keychain::PRODUCTION_NAMESPACE,
        vault_app::keychain::VAULT_ID,
    ) {
        Ok(Some(_)) => KeyState::Present,
        Ok(None) => KeyState::Absent {
            keyed_data: location_missing
                || vault_root.is_some_and(vault_app::keychain::keyed_data_present),
        },
        Err(e) => {
            tracing::warn!(target: "vault_tauri::link", error = %e, "the key could not be read");
            KeyState::Unreadable
        }
    }
}

/// Decode a keeper answer into what the command returns. An answer of any
/// other shape (a keeper from another build that slipped past the wire
/// check) is a stable code, never a parse error's text.
///
/// # Errors
///
/// [`ERR_KEEPER_UNREACHABLE`].
pub fn decoded<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_str(text).map_err(|_| ERR_KEEPER_UNREACHABLE.to_string())
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

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
    fn every_code_is_distinct_and_names_nothing_internal() {
        let mut codes = ALL_CODES.to_vec();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ALL_CODES.len());
        for code in ALL_CODES {
            assert!(
                code.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{code}"
            );
            for word in ["keeper_pool", "rmcp", "pipe", "sqlcipher"] {
                assert!(!code.contains(word), "{code}");
            }
        }
    }

    #[test]
    fn the_link_state_wire_shape_is_fixed() {
        assert_eq!(LinkState::Connecting.to_json()["state"], "connecting");
        assert_eq!(LinkState::Serving.to_json()["state"], "serving");
        assert_eq!(LinkState::Tidying.to_json()["state"], "tidying");
        let failed = LinkState::Failed("words".into()).to_json();
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["message"], "words");
    }

    #[test]
    fn a_record_is_recent_only_inside_its_window() {
        let now = chrono::Utc::now();
        assert!(is_recent(&now.to_rfc3339(), RECORD_VALID_FOR));
        let old = now - chrono::Duration::minutes(10);
        assert!(!is_recent(&old.to_rfc3339(), RECORD_VALID_FOR));
    }
}
