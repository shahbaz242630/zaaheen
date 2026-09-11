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
pub mod discovery;
pub mod exclusive;
pub mod handshake;
pub mod intent;
pub mod relay;
pub mod runtime;
pub mod transport;

/// Prefix of the keeper's per-user Task Scheduler entry; the user's SID
/// completes it. The installer deletes `<prefix>[UserSID]` on uninstall, and
/// `vault-tauri/tests/installer_contract.rs` pins that the two agree.
pub const KEEPER_TASK_ID_PREFIX: &str = "com.zaaheen.keeper.";
