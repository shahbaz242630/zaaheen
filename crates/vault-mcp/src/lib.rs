//! `vault-mcp` — MCP adapter layer + stdio server for Memory Vault.
//!
//! See `Agent Build Specification.txt` §5.7 for the public API specification
//! and `T0.1.9_PLAN.md` for the V0.1 design (5 surfaces, 3-phase split,
//! ADRs 023 / 024 / 025 / 026).
//!
//! V0.1 (T0.1.9) shipped **stdio-only**, **single-user**, **strictly serial**
//! request handling, with four tools: `memory_search` / `memory_write` /
//! `memory_update` / `memory_delete`.
//!
//! T0.2.7 Phase 4 (2026-05-20) added a fifth tool: `memory_read`.
//! Commit 6 (locked-next-arc, 2026-05-26 — ADR-052 + ADR-054) rewrote
//! its response shape: the tool now surfaces
//! [`vault_retrieval::StructuredReadPipeline`] output (deterministic
//! `relevant_facts` + `abstain` + `health.warnings`) instead of the
//! V0.2-era Qwen-7B `ReadResponse`. The structured-fact agent
//! contract is taught in the `memory_read` tool description (see
//! [`crate::server::StdioServer::tool_read`]).
//!
//! ## Trust boundary (ADR-025 — load-bearing)
//!
//! - **UNTRUSTED:** every field of the JSON-RPC request body. Tool args
//!   from the MCP client (an AI agent) are NEVER read for authorization
//!   decisions.
//! - **TRUSTED:** the `authorized_boundaries: Vec<Boundary>` slice handed
//!   to [`StdioServer::new`] at startup by `Application` after passphrase
//!   unlock. This is the SOLE auth-gate input.
//! - **Handlers MUST NOT** parse boundary names from request bodies,
//!   interpolate request data into auth-gate inputs, or accept boundary
//!   overrides via headers / metadata. The handler always uses
//!   `self.authorized_boundaries.clone()`.
//!
//! ## Audit (ADR-024 — locked schema)
//!
//! Every tool call appends one [`vault_storage::AuditEventType::McpToolInvoke`]
//! event to the local audit chain (Phase 2). `details_json` shape:
//!
//! ```json
//! {
//!   "tool": "memory_search" | "memory_write" | "memory_update" | "memory_delete",
//!   "duration_ms": <u64>,
//!   "result_count": <u32>,
//!   "boundary_count": <u32>,
//!   "max_results": <u32>,         // search only
//!   "score_threshold": <f32>,     // search only
//!   "include_archived": <bool>,   // search only
//!   "query_length": <u32>,        // search only
//!   "error": { "type": "<VaultError variant>", "detail": <Value> }   // optional
//! }
//! ```
//!
//! Plus a per-call `tracing::info!(target: "vault_mcp::tool_invoke", ...)`
//! for operational observability (rate-limit hooks deferred to V0.2 per
//! plan §3.5).
//!
//! ## rmcp threat-surface scope (ADR-026)
//!
//! vault-mcp is a stdio **server** — spawned by a host (Claude Desktop,
//! Cursor, future vault-tauri host functionality), reads stdin, writes
//! stdout, does NOT spawn child processes. The April 15 RCE class
//! (OX Security 2026-04-15, 13 CVEs) affects MCP **hosts/clients**, not
//! servers. vault-mcp at T0.1.9 is structurally not in that threat
//! surface; the forward pointer for whichever future task introduces
//! MCP-host functionality lives in ADR-026 + the §10 tech-debt entry.

#![forbid(unsafe_code)]

mod adapter;
mod audit;
mod daemon;
mod gate;
mod relay;
mod server;

pub use adapter::Adapter;
pub use audit::{ToolInvokeDetails, ToolInvokeError};
pub use daemon::DaemonServer;
pub use gate::{EntitledService, EntitlementCheck, InFlight, LockReason, Verdict};
pub use relay::{
    NoVaultAdapter, RelayServer, Upstream, UpstreamError, MSG_OUTCOME_UNKNOWN, MSG_TIMED_OUT,
    MSG_TIMED_OUT_SAVE, RELAY_CALL_BUDGET,
};
pub use server::{
    DeleteToolParams, SearchToolParams, StdioServer, UpdateToolParams, WriteToolParams,
};
