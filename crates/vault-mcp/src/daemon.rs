//! `DaemonServer` — the HTTP-daemon MCP handler (ADR-SEC-001, local multi-agent
//! daemon).
//!
//! ## Why a separate handler from `StdioServer`
//!
//! [`StdioServer`](crate::server::StdioServer) is served over **stdio**, where
//! the OS process boundary IS the security: the host spawns one server per
//! agent, so a fixed construction-time `authorized_boundaries` slice is safe.
//!
//! The multi-agent daemon is served over **rmcp streamable-HTTP on loopback**,
//! shared by many agents. A loopback socket has no OS-process gate — any local
//! process can connect — so `DaemonServer` authenticates EVERY request: it reads
//! the `Authorization: Bearer <token>` header, resolves the token to the
//! connecting agent's authorized boundaries (ADR-SEC-001 D3/D4), and dispatches
//! through a boundary-SCOPED [`StdioServer`]. That delegation reuses the stdio
//! server's entire tool surface — descriptions, schema validation, audit append,
//! ADR-024 error mapping — with zero duplication; only the per-request boundary
//! resolution differs.
//!
//! ## Security posture
//!
//! - **Authenticate before dispatch (BRD §11.4.4 step 1):** a missing,
//!   malformed, or unknown/revoked token yields the SAME generic `access denied`
//!   (SP-4 fail-secure, no info leak — an attacker can't tell "no header" from
//!   "bad token").
//! - **Per-request scoping (D4):** boundaries are resolved fresh each call, so a
//!   live `agent set-boundaries` / `agent revoke` takes effect on the next
//!   request with no restart.
//! - `tools/list` + `get_info` carry NO user data and need no boundary scope, so
//!   they delegate to an empty-scoped `StdioServer` purely for the tool contract.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ListToolsResult, PaginatedRequestParams,
    ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use vault_core::Boundary;

use crate::server::{vault_error_to_mcp, StdioServer, ERROR_CODE_ACCESS_DENIED};
use crate::{Adapter, Gate, Verdict};

/// HTTP-daemon MCP handler. Authenticates each request via a bearer capability
/// token, then dispatches through a boundary-scoped [`StdioServer`]. Cheap to
/// clone — shares the inner `Arc<dyn Adapter>`.
#[derive(Clone)]
pub struct DaemonServer {
    adapter: Arc<dyn Adapter>,
    gate: Option<Gate>,
}

impl DaemonServer {
    /// Construct from the shared adapter (the same one the stdio server uses)
    /// and, when this build carries a sign-in, the subscription gate
    /// (ADR-104; `SIGNIN-DESIGN.md` §8.26 §6.1: the gate is consulted **after**
    /// the agent is authenticated, so an unknown token is still refused first
    /// and learns nothing about the subscription).
    pub fn new(adapter: Arc<dyn Adapter>, gate: Option<Gate>) -> Self {
        Self { adapter, gate }
    }

    /// Resolve the per-request authorized boundaries from the bearer token in
    /// the inbound HTTP request, or a generic `access denied`. SP-4 fail-secure:
    /// a missing header, a malformed header, and an unknown/revoked token all map
    /// to the SAME error so nothing is leaked about *why* a request was rejected.
    async fn authorize(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<(String, Vec<Boundary>), McpError> {
        let denied = || McpError::new(ERROR_CODE_ACCESS_DENIED, "access denied", None);

        // rmcp's streamable-HTTP transport injects the request's
        // `http::request::Parts` (headers included) into the request extensions.
        // Absent => not served over HTTP (defensive — `DaemonServer` is only ever
        // mounted on the HTTP transport).
        let parts = context
            .extensions
            .get::<http::request::Parts>()
            .ok_or_else(denied)?;

        let token = parts
            .headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .ok_or_else(denied)?;

        let token_hash = vault_storage::hash_capability_token(token);
        self.adapter
            .resolve_token_boundaries(&token_hash)
            .await
            .map_err(vault_error_to_mcp)?
            .ok_or_else(denied)
    }

    /// Build a boundary-scoped `StdioServer` to dispatch ONE authorized request.
    /// `StdioServer::new` regenerates its tool router — cheap relative to the
    /// embedding/storage work a tool call performs.
    fn scoped(&self, boundaries: Vec<Boundary>) -> StdioServer {
        StdioServer::new(self.adapter.clone(), boundaries)
    }
}

impl ServerHandler for DaemonServer {
    fn get_info(&self) -> ServerInfo {
        // Same advertised contract as the stdio server; no dispatch, so an
        // empty-scoped server is fine just to produce the metadata.
        self.scoped(Vec::new()).get_info()
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.scoped(Vec::new()).get_tool(name)
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        // tools/list exposes the tool contract only — no user data, no auth gate.
        let listing = self.scoped(Vec::new());
        listing.list_tools(request, context).await
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // Authenticate + resolve boundaries BEFORE any dispatch (BRD §11.4.4).
        let (agent_name, boundaries) = self.authorize(&context).await?;
        // Then the subscription gate, so a locked vault answers with words the
        // agent can read and nothing reaches the adapter (§8.26 §6.1). The
        // daemon dispatches per request rather than wrapping a server, so it
        // asks the check itself; calls in flight are the keeper's concern.
        let mut context = context;
        if let Some(gate) = &self.gate {
            let (verdict, notice) = gate.verdict_with_notice().await;
            if let Verdict::Locked(reason) = verdict {
                tracing::info!(
                    target: "vault_mcp::gate",
                    request = "tools/call",
                    reason = ?reason,
                    "refused: the vault is locked"
                );
                return Ok(CallToolResult::error(vec![ContentBlock::text(
                    reason.message(),
                )]));
            }
            // Served, so anything the agent should pass on travels with the
            // call, as `EntitledService` does it (§8.40).
            if let Some(notice) = notice {
                context.extensions.insert(notice);
            }
        }
        // Step 5 — per-agent operational attribution (§11.9.2): which agent ran
        // which tool. (Threading the name into the PERSISTENT audit row's
        // `actor_name` is a focused follow-up — it touches the ADR-024
        // canonical-JSON audit path; this gives immediate per-agent visibility.)
        tracing::info!(
            target: "vault_mcp::daemon",
            agent = %agent_name,
            tool = %request.name,
            "authenticated multi-agent tool call"
        );
        let scoped = self.scoped(boundaries);
        scoped.call_tool(request, context).await
    }
}
