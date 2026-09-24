//! Shared vault access (ADR-102): ONE keeper process owns the vault and every
//! AI app's `zaaheen mcp serve` relays to it.
//!
//! **The defect this answers.** AI apps start one MCP server process per
//! connection, and Claude Desktop starts several at once (its chat plus a
//! shared pool for Cowork and Code sessions, within milliseconds). The vault
//! admits one owner (`.vault.lock`, ADR-SEC-002/020), so every copy but the
//! first died — and which copy won was a race, so a Claude Desktop chat could
//! show "Server disconnected" on any launch. Observed on hardware 2026-08-31 to
//! 2026-09-10.
//!
//! **The shape.** A relay answers the MCP handshake and tool list itself and
//! forwards each tool call over authenticated local IPC to the keeper, which
//! serves it with the same `StdioServer` stdio uses today — so the tool
//! surface, audit, and boundary enforcement are reused unchanged.
//!
//! - [`acl`] — restrict the vault folder to its owner (runs first).
//! - [`discovery`] — the non-secret file telling relays where the keeper is.
//! - [`handshake`] — mutual authentication bound to one stream (ADR-SEC-019).
//! - [`transport`] — named pipes on Windows, unix sockets elsewhere.
//! - [`runtime`] — the keeper's serve loop.
//! - [`relay`] — the relay's connection to the keeper.
//! - [`intent`] / [`exclusive`] — taking the vault away from a running keeper
//!   (erasure), without a fresh keeper taking it back in the gap.

pub mod acl;
pub mod clients;
pub mod discovery;
pub mod exclusive;
pub mod handshake;
pub mod intent;
pub mod relay;
pub mod runtime;
pub mod start_failure;
pub mod transport;

/// Prefix of the keeper's per-user Task Scheduler entry; the user's SID
/// completes it. The installer deletes `<prefix>[UserSID]` on uninstall, and
/// `vault-tauri/tests/installer_contract.rs` pins that the two agree.
pub const KEEPER_TASK_ID_PREFIX: &str = "com.zaaheen.keeper.";

/// Version marker for the keeper's Task Scheduler entry. Changing the task's
/// shape means bumping this, which makes every starter replace the old entry.
/// Shared by the relays (vault-cli) and the desktop (ADR-108 D4), so the two
/// never register different tasks and replace each other's.
pub const KEEPER_TASK_LABEL: &str = "zaaheen-keeper-task-v1";

/// The Windows-subsystem launcher the task runs (ADR-SEC-015), so no console
/// window ever appears.
pub const LAUNCHER_EXE: &str = if cfg!(windows) {
    "zaaheen-maintenance.exe"
} else {
    "zaaheen-maintenance"
};

/// The launcher's arguments for keeper mode. One function, for the same
/// reason as [`KEEPER_TASK_LABEL`]: Task Scheduler compares the arguments
/// too, so two spellings would re-register the task back and forth.
pub fn keeper_task_args(log_dir: &std::path::Path) -> Vec<String> {
    vec![
        "--keeper".to_string(),
        "--log-dir".to_string(),
        log_dir.to_string_lossy().into_owned(),
    ]
}
