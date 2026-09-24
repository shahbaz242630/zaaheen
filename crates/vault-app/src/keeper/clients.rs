//! The AI apps connected to the keeper right now, for the desktop's Agents
//! tab: `<vault_root>/.vault-clients.json`.
//!
//! **Why.** With Claude and Cursor both using the vault, the Agents tab said
//! "No agents connected yet" (session 36, and again session 59): it listed
//! only the HTTP daemon's token grants, and the everyday path (an app's
//! `zaaheen mcp serve` relay → the keeper) registered nothing the desktop
//! could see. The design is session 36's (`HANDOFF_V0.2_PART4_ARCHIVE.md`,
//! next-session item 1), built in session 59:
//! - the relay introduces itself to the keeper under its AI app's own MCP
//!   `clientInfo` name (`keeper/relay.rs`), an MCP-level change after the
//!   authenticated handshake: no wire change;
//! - the keeper keeps this list, one entry per authenticated session, and
//!   rewrites the file atomically when it changes;
//! - the desktop reads it ([`read_live`]) and shows friendly names.
//!
//! **What the file holds: app names and times, nothing else.** Not the
//! discovery file (`.vault-host.json`), which ADR-SEC-019 pins to its public
//! fields. A name is whatever the app calls itself (`claude-ai`,
//! `cursor-vscode`), self-declared and shown only, never trusted for anything:
//! it is cut to [`MAX_NAME`] printable characters. The file is removed when
//! the keeper stops, and it is in the vault's inventory
//! (`erasure::VAULT_ENTRIES`), so erasure removes it and the at-rest sweep
//! knows it.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::discovery::{self, Role};

/// File name under the vault root.
pub const CLIENTS_FILE: &str = ".vault-clients.json";

/// A file larger than this is not ours.
const MAX_BYTES: u64 = 16 * 1024;

/// The longest app name kept, in characters.
pub const MAX_NAME: usize = 64;

/// The most sessions listed; beyond this the oldest are left out of the file.
const MAX_SESSIONS: usize = 32;

/// A call is recorded in the file at most this often per keeper, so a busy
/// app does not rewrite it on every question.
const USED_WRITE_EVERY: Duration = Duration::from_secs(30);

/// The name used until an app has said who it is (or if it never does).
pub const UNNAMED: &str = "an AI app";

const FORMAT: u32 = 1;

/// One connected app, as the file and the desktop see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedApp {
    pub name: String,
    /// RFC 3339: when this app's session began.
    pub since: String,
    /// RFC 3339: its last tool call, if any yet.
    pub last_used: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClientsFile {
    v: u32,
    /// The keeper that wrote it: the desktop trusts the list only while the
    /// discovery file names the same live keeper.
    pid: u32,
    apps: Vec<ConnectedApp>,
}

struct Session {
    name: String,
    since: chrono::DateTime<chrono::Utc>,
    last_used: Option<chrono::DateTime<chrono::Utc>>,
}

struct State {
    next: u64,
    sessions: BTreeMap<u64, Session>,
    last_used_write: Option<Instant>,
}

/// The keeper's list. One per keeper tenure, shared by every connection.
pub struct Clients {
    vault_root: PathBuf,
    pid: u32,
    state: Mutex<State>,
}

impl Clients {
    pub fn new(vault_root: PathBuf, pid: u32) -> Arc<Self> {
        Arc::new(Self {
            vault_root,
            pid,
            state: Mutex::new(State {
                next: 0,
                sessions: BTreeMap::new(),
                last_used_write: None,
            }),
        })
    }

    /// An authenticated session began. The entry lasts as long as the guard.
    pub fn connected(self: &Arc<Self>) -> SessionGuard {
        let id = match self.state.lock() {
            Ok(mut st) => {
                let id = st.next;
                st.next += 1;
                st.sessions.insert(
                    id,
                    Session {
                        name: UNNAMED.to_string(),
                        since: chrono::Utc::now(),
                        last_used: None,
                    },
                );
                self.write(&st);
                id
            }
            Err(_) => u64::MAX,
        };
        SessionGuard {
            clients: Arc::clone(self),
            id,
        }
    }

    fn name(&self, id: u64, name: &str) {
        if let Ok(mut st) = self.state.lock() {
            if let Some(session) = st.sessions.get_mut(&id) {
                session.name = clean_name(name);
                self.write(&st);
            }
        }
    }

    fn used(&self, id: u64) {
        if let Ok(mut st) = self.state.lock() {
            let Some(session) = st.sessions.get_mut(&id) else {
                return;
            };
            session.last_used = Some(chrono::Utc::now());
            let due = st
                .last_used_write
                .map_or(true, |at| at.elapsed() >= USED_WRITE_EVERY);
            if due {
                st.last_used_write = Some(Instant::now());
                self.write(&st);
            }
        }
    }

    fn disconnected(&self, id: u64) {
        if let Ok(mut st) = self.state.lock() {
            if st.sessions.remove(&id).is_some() {
                self.write(&st);
            }
        }
    }

    /// Remove the file: the keeper is stopping, so nothing is connected.
    pub fn clear(&self) {
        if let Ok(mut st) = self.state.lock() {
            st.sessions.clear();
        }
        match std::fs::remove_file(clients_path(&self.vault_root)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(target: "vault_app::keeper", error = %e, "could not remove the connected-apps file");
            }
        }
    }

    /// Best effort: the list is shown, never relied on, so a failed write is
    /// logged and serving carries on.
    fn write(&self, st: &State) {
        let apps: Vec<ConnectedApp> = st
            .sessions
            .values()
            .rev()
            .take(MAX_SESSIONS)
            .map(|s| ConnectedApp {
                name: s.name.clone(),
                since: s.since.to_rfc3339(),
                last_used: s.last_used.map(|t| t.to_rfc3339()),
            })
            .collect();
        let file = ClientsFile {
            v: FORMAT,
            pid: self.pid,
            apps,
        };
        if let Err(e) = write_atomic(&self.vault_root, &file) {
            tracing::warn!(target: "vault_app::keeper", error = %e, "could not write the connected-apps file");
        }
    }
}

/// One session's entry; removed from the list when dropped, including when
/// the connection's task is aborted.
pub struct SessionGuard {
    clients: Arc<Clients>,
    id: u64,
}

impl SessionGuard {
    /// The app said who it is (its MCP `clientInfo` name).
    pub fn name(&self, name: &str) {
        self.clients.name(self.id, name);
    }

    /// The app made a tool call.
    pub fn used(&self) {
        self.clients.used(self.id);
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.clients.disconnected(self.id);
    }
}

/// An MCP service that marks its session "used" on every tool call, so the
/// Agents tab can say when each app last asked something.
pub struct Tracked<S> {
    inner: S,
    session: Arc<SessionGuard>,
}

impl<S> Tracked<S> {
    pub fn new(inner: S, session: Arc<SessionGuard>) -> Self {
        Self { inner, session }
    }
}

impl<S: rmcp::Service<rmcp::RoleServer>> rmcp::Service<rmcp::RoleServer> for Tracked<S> {
    async fn handle_request(
        &self,
        request: rmcp::model::ClientRequest,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ServerResult, rmcp::ErrorData> {
        if matches!(request, rmcp::model::ClientRequest::CallToolRequest(_)) {
            self.session.used();
        }
        self.inner.handle_request(request, context).await
    }

    async fn handle_notification(
        &self,
        notification: rmcp::model::ClientNotification,
        context: rmcp::service::NotificationContext<rmcp::RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        self.inner.handle_notification(notification, context).await
    }

    fn get_info(&self) -> rmcp::model::ServerConfig {
        self.inner.get_info()
    }

    fn supported_protocol_versions(
        &self,
    ) -> std::borrow::Cow<'static, [rmcp::model::ProtocolVersion]> {
        self.inner.supported_protocol_versions()
    }
}

/// A self-declared name, made safe to show: printable characters only,
/// trimmed, at most [`MAX_NAME`] characters; [`UNNAMED`] if nothing is left.
pub fn clean_name(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect();
    let kept = kept.trim();
    if kept.is_empty() {
        UNNAMED.to_string()
    } else {
        kept.to_string()
    }
}

pub fn clients_path(vault_root: &Path) -> PathBuf {
    vault_root.join(CLIENTS_FILE)
}

fn write_atomic(vault_root: &Path, file: &ClientsFile) -> std::io::Result<()> {
    let target = clients_path(vault_root);
    let tmp = vault_root.join(format!("{CLIENTS_FILE}.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, bytes)?;
    let mut attempt = 1;
    loop {
        match std::fs::rename(&tmp, &target) {
            Ok(()) => return Ok(()),
            // Antivirus scanners hold fresh files for a moment on Windows.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && attempt < 5 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
}

/// The apps connected right now, one entry per app name (Claude Desktop runs
/// several sessions at once), newest use first. Empty unless the file was
/// written by the keeper the discovery file names as serving: a keeper that
/// has stopped removes both, and a leftover list must never claim an app is
/// connected.
pub fn read_live(vault_root: &Path) -> Vec<ConnectedApp> {
    let keeper_pid = match discovery::read(vault_root) {
        Ok(Some(d)) if d.role == Role::Keeper => d.pid,
        _ => return Vec::new(),
    };
    let Some(file) = read_file(vault_root) else {
        return Vec::new();
    };
    if file.v != FORMAT || file.pid != keeper_pid {
        return Vec::new();
    }
    let mut by_name: BTreeMap<String, ConnectedApp> = BTreeMap::new();
    for app in file.apps {
        let app = ConnectedApp {
            name: clean_name(&app.name),
            ..app
        };
        match by_name.get_mut(&app.name) {
            Some(seen) => {
                if app.since < seen.since {
                    seen.since = app.since;
                }
                if app.last_used > seen.last_used {
                    seen.last_used = app.last_used;
                }
            }
            None => {
                by_name.insert(app.name.clone(), app);
            }
        }
    }
    let mut apps: Vec<ConnectedApp> = by_name.into_values().collect();
    apps.sort_by(|a, b| {
        b.last_used
            .cmp(&a.last_used)
            .then_with(|| a.name.cmp(&b.name))
    });
    apps
}

fn read_file(vault_root: &Path) -> Option<ClientsFile> {
    let file = std::fs::File::open(clients_path(vault_root)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keeper::discovery::{Discovery, DISCOVERY_FILE};
    use crate::keeper::handshake::{KeeperIdentity, NONCE_LEN};
    use tempfile::TempDir;

    fn serving(root: &Path, pid: u32) {
        let identity = KeeperIdentity {
            nonce: [7; NONCE_LEN],
            endpoint: discovery::new_endpoint(root).unwrap(),
            pid,
        };
        discovery::write(root, &Discovery::keeper(&identity, 1, "0.2.2", "t")).unwrap();
    }

    fn names(root: &Path) -> Vec<String> {
        read_live(root).into_iter().map(|a| a.name).collect()
    }

    #[test]
    fn a_session_is_listed_under_its_apps_name_until_it_ends() {
        let tmp = TempDir::new().unwrap();
        serving(tmp.path(), 42);
        let clients = Clients::new(tmp.path().to_path_buf(), 42);
        let claude = clients.connected();
        assert_eq!(names(tmp.path()), [UNNAMED], "listed from the handshake on");
        claude.name("claude-ai");
        let cursor = clients.connected();
        cursor.name("cursor-vscode");
        assert_eq!(names(tmp.path()), ["claude-ai", "cursor-vscode"]);

        drop(claude);
        assert_eq!(names(tmp.path()), ["cursor-vscode"]);
        drop(cursor);
        assert!(names(tmp.path()).is_empty());
    }

    #[test]
    fn several_sessions_of_one_app_show_once_with_its_latest_use() {
        let tmp = TempDir::new().unwrap();
        serving(tmp.path(), 42);
        let clients = Clients::new(tmp.path().to_path_buf(), 42);
        let chat = clients.connected();
        chat.name("claude-ai");
        let pool = clients.connected();
        pool.name("claude-ai");
        pool.used();
        let apps = read_live(tmp.path());
        assert_eq!(apps.len(), 1, "{apps:?}");
        assert!(
            apps[0].last_used.is_some(),
            "the pool session's call counts"
        );
    }

    /// The file holds names and times only (never a key, a path or a query).
    #[test]
    fn the_file_holds_only_names_and_times() {
        let tmp = TempDir::new().unwrap();
        let clients = Clients::new(tmp.path().to_path_buf(), 42);
        let s = clients.connected();
        s.name("claude-ai");
        s.used();
        let text = std::fs::read_to_string(clients_path(tmp.path())).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut top: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        top.sort_unstable();
        assert_eq!(top, ["apps", "pid", "v"]);
        let mut fields: Vec<&str> = value["apps"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(fields, ["last_used", "name", "since"]);
    }

    /// A list from a keeper that is not the one serving (it crashed, or a
    /// newer one took over) must not claim anything is connected.
    #[test]
    fn a_list_from_another_or_no_keeper_is_not_shown() {
        let tmp = TempDir::new().unwrap();
        let clients = Clients::new(tmp.path().to_path_buf(), 42);
        let s = clients.connected();
        s.name("claude-ai");
        assert!(names(tmp.path()).is_empty(), "no keeper serving");
        serving(tmp.path(), 43);
        assert!(names(tmp.path()).is_empty(), "another keeper serving");
        serving(tmp.path(), 42);
        assert_eq!(names(tmp.path()), ["claude-ai"]);
        std::fs::remove_file(tmp.path().join(DISCOVERY_FILE)).unwrap();
        assert!(names(tmp.path()).is_empty(), "the keeper has gone");
    }

    #[test]
    fn a_stopping_keeper_removes_the_file() {
        let tmp = TempDir::new().unwrap();
        let clients = Clients::new(tmp.path().to_path_buf(), 42);
        let _s = clients.connected();
        assert!(clients_path(tmp.path()).exists());
        clients.clear();
        assert!(!clients_path(tmp.path()).exists());
    }

    /// Names are self-declared: shown, never trusted, and kept short and
    /// printable.
    #[test]
    fn names_are_cleaned_before_they_are_kept() {
        assert_eq!(clean_name("  cursor-vscode  "), "cursor-vscode");
        assert_eq!(clean_name("evil\u{0007}\nname"), "evilname");
        assert_eq!(clean_name(""), UNNAMED);
        assert_eq!(clean_name("\n\t"), UNNAMED);
        assert_eq!(clean_name(&"x".repeat(500)).chars().count(), MAX_NAME);
    }

    #[test]
    fn an_oversized_or_foreign_file_is_ignored() {
        let tmp = TempDir::new().unwrap();
        serving(tmp.path(), 42);
        std::fs::write(clients_path(tmp.path()), vec![b' '; 20_000]).unwrap();
        assert!(names(tmp.path()).is_empty());
        std::fs::write(clients_path(tmp.path()), b"not json").unwrap();
        assert!(names(tmp.path()).is_empty());
    }
}
