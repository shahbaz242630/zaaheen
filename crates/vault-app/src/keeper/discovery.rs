//! The keeper's discovery file, `<vault_root>/.vault-host.json`.
//!
//! How a relay finds the keeper: the endpoint to connect to plus the public
//! identity (nonce, pid) the handshake binds. **Nothing here is secret** — the
//! file is readable by any principal with read access to the vault folder
//! (on the founder's machine that included a second account group), so it
//! must never carry a token or key. The handshake keys come from the OS
//! keychain instead.
//!
//! Everything read from it is treated as untrusted input (SP-7): capped in
//! size, and the endpoint must match the one exact shape our keeper creates.
//! Without that check a tampered file could point a relay at a UNC path
//! (`\\host\pipe\x`), which Windows would reach over SMB while authenticating
//! as the user.
//!
//! Unknown fields and roles are TOLERATED on read, deliberately: during an
//! update an older relay can meet a newer writer, and refusing its file would
//! send the relay into a loop of start requests. Tolerance grants nothing — a
//! writer who can plant fields can already plant any known one, and every field
//! a relay acts on is still validated.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::handshake::{KeeperIdentity, NONCE_LEN};

/// File name under the vault root.
pub const DISCOVERY_FILE: &str = ".vault-host.json";

/// Directory holding the POSIX socket. Created 0700 by the keeper.
pub const SOCKET_DIR: &str = ".keeper";

/// Discovery file format version.
const FORMAT: u32 = 1;

/// A discovery file larger than this is not ours.
const MAX_BYTES: u64 = 4096;

/// Endpoint names are `zaaheen-` plus this many lowercase hex characters.
const ENDPOINT_HEX_LEN: usize = 16;

#[cfg(windows)]
const PIPE_PREFIX: &str = r"\\.\pipe\zaaheen-";

/// What the process that wrote the file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// A keeper is (or was) serving at `endpoint`.
    Keeper,
    /// A keeper took the lock but could not start. Relays surface this rather
    /// than re-requesting a start in a tight loop.
    Failed,
    /// A keeper holds the lock and is opening the vault. Relays wait for it
    /// instead of asking for more starts.
    Starting,
    /// A maintenance run holds the vault. Relays answer "busy" at once rather
    /// than spend their whole wait asking for keepers that cannot start.
    Maintenance,
    /// A role from a newer build. Treated as "no usable keeper".
    #[serde(other)]
    Unknown,
}

/// The file's contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    pub v: u32,
    pub role: Role,
    pub pid: u32,
    pub endpoint: String,
    /// Hex of the keeper tenure's nonce (64 characters), empty for `Failed`.
    pub nonce: String,
    pub wire: u16,
    pub version: String,
    /// RFC 3339 time the file was written.
    pub at: String,
}

/// Why a discovery file cannot be used.
#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryError {
    NotAKeeper,
    BadEndpoint,
    BadNonce,
    BadFormat,
}

impl Discovery {
    /// A serving keeper's record.
    pub fn keeper(identity: &KeeperIdentity, wire: u16, version: &str, at: &str) -> Self {
        Self {
            v: FORMAT,
            role: Role::Keeper,
            pid: identity.pid,
            endpoint: identity.endpoint.clone(),
            nonce: hex::encode(identity.nonce),
            wire,
            version: version.to_string(),
            at: at.to_string(),
        }
    }

    /// A keeper that took the lock but failed to start.
    pub fn failed(pid: u32, wire: u16, version: &str, at: &str) -> Self {
        Self::without_endpoint(Role::Failed, pid, wire, version, at)
    }

    /// A keeper that holds the lock and is opening the vault.
    pub fn starting(pid: u32, wire: u16, version: &str, at: &str) -> Self {
        Self::without_endpoint(Role::Starting, pid, wire, version, at)
    }

    /// A maintenance run that holds the vault.
    pub fn maintenance(pid: u32, wire: u16, version: &str, at: &str) -> Self {
        Self::without_endpoint(Role::Maintenance, pid, wire, version, at)
    }

    fn without_endpoint(role: Role, pid: u32, wire: u16, version: &str, at: &str) -> Self {
        Self {
            v: FORMAT,
            role,
            pid,
            endpoint: String::new(),
            nonce: String::new(),
            wire,
            version: version.to_string(),
            at: at.to_string(),
        }
    }

    /// The handshake identity this file claims, after validating every field
    /// a relay is about to act on.
    ///
    /// # Errors
    ///
    /// Any [`DiscoveryError`]; the relay then treats the keeper as absent.
    pub fn keeper_identity(&self, vault_root: &Path) -> Result<KeeperIdentity, DiscoveryError> {
        if self.v != FORMAT {
            return Err(DiscoveryError::BadFormat);
        }
        if self.role != Role::Keeper {
            return Err(DiscoveryError::NotAKeeper);
        }
        if !endpoint_is_ours(&self.endpoint, vault_root) {
            return Err(DiscoveryError::BadEndpoint);
        }
        let bytes = hex::decode(&self.nonce).map_err(|_| DiscoveryError::BadNonce)?;
        let nonce: [u8; NONCE_LEN] = bytes.try_into().map_err(|_| DiscoveryError::BadNonce)?;
        Ok(KeeperIdentity {
            nonce,
            endpoint: self.endpoint.clone(),
            pid: self.pid,
        })
    }
}

/// A fresh endpoint for a new keeper tenure: random, so a name cannot be
/// squatted in advance.
///
/// # Errors
///
/// The OS random source failed.
pub fn new_endpoint(vault_root: &Path) -> Result<String, getrandom::Error> {
    let mut raw = [0u8; ENDPOINT_HEX_LEN / 2];
    getrandom::getrandom(&mut raw)?;
    let suffix = hex::encode(raw);
    Ok(endpoint_with_suffix(vault_root, &suffix))
}

#[cfg(windows)]
fn endpoint_with_suffix(_vault_root: &Path, suffix: &str) -> String {
    format!("{PIPE_PREFIX}{suffix}")
}

#[cfg(not(windows))]
fn endpoint_with_suffix(vault_root: &Path, suffix: &str) -> String {
    vault_root
        .join(SOCKET_DIR)
        .join(format!("zaaheen-{suffix}.sock"))
        .to_string_lossy()
        .into_owned()
}

fn is_endpoint_suffix(s: &str) -> bool {
    s.len() == ENDPOINT_HEX_LEN && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// The one exact shape our keeper creates: a local named pipe on Windows, a
/// socket inside the vault's own `.keeper/` directory elsewhere.
#[cfg(windows)]
fn endpoint_is_ours(endpoint: &str, _vault_root: &Path) -> bool {
    endpoint
        .strip_prefix(PIPE_PREFIX)
        .is_some_and(is_endpoint_suffix)
}

#[cfg(not(windows))]
fn endpoint_is_ours(endpoint: &str, vault_root: &Path) -> bool {
    let path = Path::new(endpoint);
    let expected_dir = vault_root.join(SOCKET_DIR);
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    path.parent() == Some(expected_dir.as_path())
        && name
            .strip_prefix("zaaheen-")
            .and_then(|rest| rest.strip_suffix(".sock"))
            .is_some_and(is_endpoint_suffix)
}

/// Path of the discovery file.
pub fn discovery_path(vault_root: &Path) -> PathBuf {
    vault_root.join(DISCOVERY_FILE)
}

/// Read the discovery file. `Ok(None)` when there is none.
///
/// # Errors
///
/// I/O failures, a file over [`MAX_BYTES`], or unparseable contents
/// (`InvalidData`). A relay treats all of these as "no usable keeper".
pub fn read(vault_root: &Path) -> std::io::Result<Option<Discovery>> {
    let file = match std::fs::File::open(discovery_path(vault_root)) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "discovery file too large",
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Write the discovery file atomically: a per-process temp file, then a
/// rename over the target. A reader sees the old file or the new one, never a
/// torn write. The rename is retried briefly because antivirus scanners hold
/// freshly written files for a moment on Windows.
///
/// # Errors
///
/// I/O failures after the retries.
pub fn write(vault_root: &Path, discovery: &Discovery) -> std::io::Result<()> {
    let target = discovery_path(vault_root);
    let tmp = vault_root.join(format!("{DISCOVERY_FILE}.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(discovery)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, bytes)?;

    let mut attempt = 1;
    loop {
        match std::fs::rename(&tmp, &target) {
            Ok(()) => return Ok(()),
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

/// Remove the discovery file only if it still describes the given tenure —
/// never a successor's.
///
/// # Errors
///
/// I/O failures other than "already gone".
pub fn remove_if_ours(vault_root: &Path, nonce_hex: &str) -> std::io::Result<()> {
    match read(vault_root) {
        Ok(Some(d)) if d.nonce == nonce_hex => {
            match std::fs::remove_file(discovery_path(vault_root)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Ok(()),
        Err(e) => Err(e),
    }
}

/// Remove the discovery file only if it is still the `role` record this
/// process (`pid`) wrote — for the records that carry no nonce (a maintenance
/// run ending, a keeper that failed before serving).
///
/// # Errors
///
/// I/O failures other than "already gone".
pub fn remove_if_written_by(vault_root: &Path, role: Role, pid: u32) -> std::io::Result<()> {
    match read(vault_root) {
        Ok(Some(d)) if d.role == role && d.pid == pid => {
            match std::fs::remove_file(discovery_path(vault_root)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn identity(root: &Path) -> KeeperIdentity {
        KeeperIdentity {
            nonce: [0xab; NONCE_LEN],
            endpoint: endpoint_with_suffix(root, "0123456789abcdef"),
            pid: 4242,
        }
    }

    #[test]
    fn a_written_keeper_record_reads_back_to_the_same_identity() {
        let tmp = TempDir::new().unwrap();
        let id = identity(tmp.path());
        write(
            tmp.path(),
            &Discovery::keeper(&id, 1, "0.2.2", "2026-09-10T20:00:00Z"),
        )
        .unwrap();
        let d = read(tmp.path()).unwrap().expect("file present");
        assert_eq!(d.keeper_identity(tmp.path()).unwrap(), id);
    }

    /// The file is readable by other principals; it must never carry key
    /// material. The only high-entropy field is the PUBLIC nonce.
    #[test]
    fn the_file_holds_only_the_public_fields() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            &Discovery::keeper(&identity(tmp.path()), 1, "0.2.2", "t"),
        )
        .unwrap();
        let text = std::fs::read_to_string(discovery_path(tmp.path())).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["at", "endpoint", "nonce", "pid", "role", "v", "version", "wire"],
            "a new field in the discovery file is a new thing other accounts can read"
        );
    }

    #[test]
    fn a_missing_file_is_none_not_an_error() {
        let tmp = TempDir::new().unwrap();
        assert!(read(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn an_oversized_file_is_refused() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(discovery_path(tmp.path()), vec![b' '; 5000]).unwrap();
        assert_eq!(
            read(tmp.path()).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    /// During an update an older relay reads a newer writer's file. It must
    /// still parse — an unreadable file would send the relay into a loop of
    /// start requests — and still be refused as a keeper when a field it acts
    /// on is not valid.
    #[test]
    fn fields_and_roles_from_a_newer_build_still_parse() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            discovery_path(tmp.path()),
            r#"{"v":1,"role":"keeper","pid":1,"endpoint":"x","nonce":"","wire":1,"version":"","at":"","extra":42}"#,
        )
        .unwrap();
        let d = read(tmp.path()).unwrap().expect("file present");
        assert_eq!(
            d.keeper_identity(tmp.path()),
            Err(DiscoveryError::BadEndpoint),
            "parsing tolerantly must not relax validation"
        );

        std::fs::write(
            discovery_path(tmp.path()),
            r#"{"v":1,"role":"draining","pid":1,"endpoint":"","nonce":"","wire":2,"version":"","at":""}"#,
        )
        .unwrap();
        let d = read(tmp.path()).unwrap().expect("file present");
        assert_eq!(d.role, Role::Unknown);
        assert_eq!(
            d.keeper_identity(tmp.path()),
            Err(DiscoveryError::NotAKeeper)
        );
    }

    #[test]
    fn starting_and_maintenance_records_are_not_keepers() {
        let tmp = TempDir::new().unwrap();
        for d in [
            Discovery::starting(7, 1, "0.2.2", "t"),
            Discovery::maintenance(7, 1, "0.2.2", "t"),
        ] {
            write(tmp.path(), &d).unwrap();
            let back = read(tmp.path()).unwrap().unwrap();
            assert_eq!(back, d, "round-trips");
            assert_eq!(
                back.keeper_identity(tmp.path()),
                Err(DiscoveryError::NotAKeeper)
            );
        }
    }

    /// A maintenance run removes its own record, never a keeper's that
    /// replaced it, nor another process's.
    #[test]
    fn removal_by_writer_only_touches_that_writers_record() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), &Discovery::maintenance(7, 1, "0.2.2", "t")).unwrap();

        remove_if_written_by(tmp.path(), Role::Maintenance, 8).unwrap();
        assert!(read(tmp.path()).unwrap().is_some(), "another pid");
        remove_if_written_by(tmp.path(), Role::Starting, 7).unwrap();
        assert!(read(tmp.path()).unwrap().is_some(), "another role");

        remove_if_written_by(tmp.path(), Role::Maintenance, 7).unwrap();
        assert!(read(tmp.path()).unwrap().is_none());
        remove_if_written_by(tmp.path(), Role::Maintenance, 7).unwrap();
    }

    /// Only the exact shape our keeper creates is acceptable — above all never
    /// a UNC path, which Windows would open over the network as the user.
    #[test]
    fn foreign_endpoints_are_refused() {
        let tmp = TempDir::new().unwrap();
        let mut d = Discovery::keeper(&identity(tmp.path()), 1, "0.2.2", "t");
        for bad in [
            r"\\evil-host\pipe\zaaheen-0123456789abcdef",
            r"\\.\pipe\zaaheen-0123456789ABCDEF",
            r"\\.\pipe\zaaheen-0123456789abcde",
            r"\\.\pipe\other-0123456789abcdef",
            "/tmp/zaaheen-0123456789abcdef.sock",
            "",
        ] {
            d.endpoint = bad.to_string();
            assert_eq!(
                d.keeper_identity(tmp.path()),
                Err(DiscoveryError::BadEndpoint),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn a_failed_record_is_not_a_keeper() {
        let tmp = TempDir::new().unwrap();
        let d = Discovery::failed(7, 1, "0.2.2", "t");
        assert_eq!(
            d.keeper_identity(tmp.path()),
            Err(DiscoveryError::NotAKeeper)
        );
    }

    #[test]
    fn a_bad_nonce_is_refused() {
        let tmp = TempDir::new().unwrap();
        let mut d = Discovery::keeper(&identity(tmp.path()), 1, "0.2.2", "t");
        d.nonce = "abcd".to_string();
        assert_eq!(d.keeper_identity(tmp.path()), Err(DiscoveryError::BadNonce));
    }

    #[test]
    fn new_endpoints_are_valid_and_distinct() {
        let tmp = TempDir::new().unwrap();
        let a = new_endpoint(tmp.path()).unwrap();
        let b = new_endpoint(tmp.path()).unwrap();
        assert_ne!(a, b);
        assert!(endpoint_is_ours(&a, tmp.path()), "{a}");
    }

    /// A keeper shutting down removes its own record, never a successor's.
    #[test]
    fn removal_only_touches_the_matching_tenure() {
        let tmp = TempDir::new().unwrap();
        let id = identity(tmp.path());
        write(tmp.path(), &Discovery::keeper(&id, 1, "0.2.2", "t")).unwrap();

        remove_if_ours(tmp.path(), &"cd".repeat(NONCE_LEN)).unwrap();
        assert!(
            read(tmp.path()).unwrap().is_some(),
            "another tenure's nonce must not remove the file"
        );

        remove_if_ours(tmp.path(), &hex::encode(id.nonce)).unwrap();
        assert!(read(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn a_rewrite_replaces_the_previous_record() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), &Discovery::failed(1, 1, "0.2.2", "t")).unwrap();
        let id = identity(tmp.path());
        write(tmp.path(), &Discovery::keeper(&id, 1, "0.2.2", "t")).unwrap();
        assert_eq!(read(tmp.path()).unwrap().unwrap().role, Role::Keeper);
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no temp file may be left behind");
    }
}
