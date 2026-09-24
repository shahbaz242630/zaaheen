//! The admin MCP server the keeper runs for the desktop app (ADR-108,
//! ADR-SEC-033): one tool per desktop operation, reached only over an
//! authenticated `ADMIN` connection.
//!
//! Every tool answers with JSON text; a failure is a tool result flagged
//! `isError` whose text is the stable code, or the error text, the desktop
//! showed before D4 — so the screens do not change.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use vault_core::MemoryId;
use vault_mcp::{ReadDesk, Verdict, DESK_BUDGET};

use super::host::{AdminHost, FullHost};
use super::ops::{self, DesktopEvent};
use super::table::{locked_code, Access};
use crate::entitlement::{Flip, ModeCheck};

/// The answer when the keeper cannot take the question now (the read desk is
/// full, or the keeper is handing over). The desktop shows its plain line.
pub const ADMIN_BUSY: &str = "admin_busy";

/// The answer to a question the desktop itself withdrew.
pub const ADMIN_CANCELLED: &str = "admin_cancelled";

/// The export page could not be read (the desktop's existing code).
pub const ERR_EXPORT_READ_FAILED: &str = "export_read_failed";

/// A cursor the desktop sent that could not be parsed.
pub const ERR_INVALID_CURSOR: &str = "invalid_input";

/// Who may use which tool on this keeper.
pub enum AdminGate {
    /// A build with no sign-in: every tool.
    Open,
    /// An entitled keeper. Its own view, read from disk only: the desktop's
    /// one real ask has just refreshed the same files (review B-M3).
    Full(Arc<dyn ModeCheck>),
    /// A locked keeper: a gated tool never runs here. If the files now say
    /// entitled, the keeper leaves so a full one can start (review A R2-3).
    Locked {
        check: Arc<dyn ModeCheck>,
        flip: Flip,
    },
}

impl AdminGate {
    /// `None` to serve; otherwise the code to answer with.
    async fn refuse(&self, access: Access) -> Option<&'static str> {
        if access == Access::Open {
            return None;
        }
        match self {
            Self::Open => None,
            Self::Full(check) => match check.peek().await {
                Verdict::Entitled => None,
                Verdict::Locked(reason) => Some(locked_code(reason)),
            },
            Self::Locked { check, flip } => match check.peek().await {
                Verdict::Entitled => {
                    flip.raise();
                    Some(locked_code(vault_mcp::LockReason::Unlocking))
                }
                Verdict::Locked(reason) => Some(locked_code(reason)),
            },
        }
    }
}

// ----- parameters -----

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AddParams {
    pub content: String,
    pub memory_type: String,
    pub boundary: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SearchParams {
    pub query: String,
    pub limit: usize,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct UpdateParams {
    pub id: String,
    pub content: String,
    pub memory_type: String,
    pub boundary: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct IdParams {
    pub id: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct LimitParams {
    pub limit: usize,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct BoundaryCreateParams {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AgentParams {
    pub agent_name: String,
}

/// Where the next export page starts: after the last memory of the previous
/// page. Both absent for the first page.
#[derive(Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ExportPageParams {
    #[serde(default)]
    pub after_created_at: Option<String>,
    #[serde(default)]
    pub after_id: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AuditParams {
    pub event: DesktopEvent,
    pub duration_ms: u64,
    pub result_count: u32,
    pub failed: bool,
}

// ----- the server -----

/// One admin connection's server.
#[derive(Clone)]
pub struct AdminServer {
    host: Arc<AdminHost>,
    gate: Arc<AdminGate>,
    desk: Arc<ReadDesk>,
    /// Populated by `#[tool_router]` and read by `#[tool_handler]`; see
    /// vault-mcp's `StdioServer` for why dead-code analysis cannot see it.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

fn answer<T: Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string(value) {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(e) => {
            tracing::error!(target: "vault_app::admin", error = %e, "could not encode an admin answer");
            CallToolResult::error(vec![ContentBlock::text("admin_encode_failed")])
        }
    }
}

fn refused(code: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(code.into())])
}

fn done<T: Serialize>(result: Result<T, String>) -> Result<CallToolResult, McpError> {
    Ok(match result {
        Ok(value) => answer(&value),
        Err(text) => refused(text),
    })
}

fn parse_cursor(p: &ExportPageParams) -> Result<Option<(DateTime<Utc>, MemoryId)>, ()> {
    match (&p.after_created_at, &p.after_id) {
        (None, None) => Ok(None),
        (Some(at), Some(id)) => {
            let at = DateTime::parse_from_rfc3339(at)
                .map_err(|_| ())?
                .with_timezone(&Utc);
            let id: MemoryId = id.parse().map_err(|_| ())?;
            Ok(Some((at, id)))
        }
        _ => Err(()),
    }
}

#[tool_router]
impl AdminServer {
    pub fn new(host: Arc<AdminHost>, gate: Arc<AdminGate>, desk: Arc<ReadDesk>) -> Self {
        Self {
            host,
            gate,
            desk,
            tool_router: Self::tool_router(),
        }
    }

    /// The full vault, after the gate has let a gated tool through. A locked
    /// host here would mean the gate and the host disagree; it answers as
    /// "unlocking" rather than touching anything.
    fn full(&self) -> Result<&FullHost, CallToolResult> {
        match self.host.as_ref() {
            AdminHost::Full(full) => Ok(full),
            AdminHost::Locked(_) => Err(refused(locked_code(vault_mcp::LockReason::Unlocking))),
        }
    }

    async fn gated(&self) -> Result<&FullHost, CallToolResult> {
        if let Some(code) = self.gate.refuse(Access::Gated).await {
            return Err(refused(code));
        }
        self.full()
    }

    #[tool(description = "Add a memory as the vault's owner.")]
    async fn admin_memory_add(
        &self,
        Parameters(p): Parameters<AddParams>,
    ) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        done(ops::memory_add(full.adapter(), p.content, p.memory_type, p.boundary).await)
    }

    #[tool(description = "Search every boundary as the vault's owner, one question at a time.")]
    async fn admin_memory_search(
        &self,
        Parameters(p): Parameters<SearchParams>,
        ct: CancellationToken,
    ) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        // The same desk the AI apps' questions wait at (ADR-107).
        let seat = tokio::select! {
            seat = self.desk.sit(DESK_BUDGET) => seat,
            () = ct.cancelled() => return Ok(refused(ADMIN_CANCELLED)),
        };
        let Ok(seat) = seat else {
            return Ok(refused(ADMIN_BUSY));
        };
        let result = tokio::select! {
            result = ops::memory_search(full.adapter(), p.query, p.limit) => result,
            () = ct.cancelled() => return Ok(refused(ADMIN_CANCELLED)),
        };
        if result.is_ok() {
            seat.done();
        }
        done(result)
    }

    #[tool(description = "Replace a memory as the vault's owner.")]
    async fn admin_memory_update(
        &self,
        Parameters(p): Parameters<UpdateParams>,
    ) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        done(ops::memory_update(full.adapter(), p.id, p.content, p.memory_type, p.boundary).await)
    }

    #[tool(description = "Delete a memory as the vault's owner.")]
    async fn admin_memory_delete(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        done(ops::memory_delete(full.adapter(), p.id).await)
    }

    #[tool(description = "The newest memories across every boundary.")]
    async fn admin_memory_list_recent(
        &self,
        Parameters(p): Parameters<LimitParams>,
    ) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        done(ops::memory_list_recent(full.adapter(), p.limit).await)
    }

    #[tool(description = "Every registered boundary with its count.")]
    async fn admin_boundary_list(&self) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        done(ops::boundary_list(full.adapter()).await)
    }

    #[tool(description = "Register a named boundary.")]
    async fn admin_boundary_create(
        &self,
        Parameters(p): Parameters<BoundaryCreateParams>,
    ) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        done(ops::boundary_create(full.adapter(), p.name, p.description).await)
    }

    #[tool(description = "Every registered access key, active and revoked.")]
    async fn admin_agent_list(&self) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        done(ops::agent_list(full.adapter()).await)
    }

    #[tool(description = "Revoke an access key by name.")]
    async fn admin_agent_revoke(
        &self,
        Parameters(p): Parameters<AgentParams>,
    ) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        done(ops::agent_revoke(full.adapter(), p.agent_name).await)
    }

    #[tool(description = "The Settings numbers. Open: the lock screen uses it.")]
    async fn admin_settings_info(&self) -> Result<CallToolResult, McpError> {
        let data_dir = crate::location::display_path(self.host.vault_root());
        match self.host.as_ref() {
            AdminHost::Full(full) => done(ops::settings_info(full.adapter(), data_dir).await),
            AdminHost::Locked(locked) => match locked.store().await {
                Ok(Some(store)) => done(ops::settings_info(store, data_dir).await),
                Ok(None) => done::<serde_json::Value>(Ok(ops::settings_info_empty(data_dir))),
                // The open's text names the file and the engine (BRD
                // §11.7.2, ADR-086): the log gets it, the screen a code
                // (security review S1).
                Err(e) => {
                    tracing::error!(target: "vault_app::admin", error = %e, "a locked keeper could not open the memories for Settings");
                    Ok(refused(ERR_EXPORT_READ_FAILED))
                }
            },
        }
    }

    #[tool(description = "Where the ranking model is: its state, the fetch, the bytes.")]
    async fn admin_engine_status(&self) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        let (downloaded_bytes, total_bytes) = full.engine().progress();
        Ok(answer(&serde_json::json!({
            "state": full.app().reranker_state().as_wire_str(),
            "fetch": full.engine().fetch().as_wire_str(),
            "downloaded_bytes": downloaded_bytes,
            "total_bytes": total_bytes,
        })))
    }

    #[tool(description = "Start (or join) the ranking model's download; returns at once.")]
    async fn admin_engine_fetch(&self) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        full.fetch_engine();
        Ok(answer(&full.engine().fetch().as_wire_str()))
    }

    #[tool(description = "Load the ranking model in the background; returns at once.")]
    async fn admin_engine_warm(&self) -> Result<CallToolResult, McpError> {
        let full = match self.gated().await {
            Ok(full) => full,
            Err(r) => return Ok(r),
        };
        Ok(answer(&full.app().spawn_reranker_warmup()))
    }

    #[tool(description = "One page of the export, newest first. Open: export is always available.")]
    async fn admin_export_page(
        &self,
        Parameters(p): Parameters<ExportPageParams>,
    ) -> Result<CallToolResult, McpError> {
        let Ok(before) = parse_cursor(&p) else {
            return Ok(refused(ERR_INVALID_CURSOR));
        };
        let page = match self.host.as_ref() {
            AdminHost::Full(full) => ops::export_page(full.adapter(), before, p.limit).await,
            AdminHost::Locked(locked) => match locked.store().await {
                Ok(Some(store)) => ops::export_page(store, before, p.limit).await,
                Ok(None) => Ok(Vec::new()),
                Err(e) => Err(e),
            },
        };
        match page {
            Ok(memories) => Ok(answer(&memories)),
            Err(e) => {
                tracing::error!(target: "vault_app::admin", error = %e, "could not read an export page");
                Ok(refused(ERR_EXPORT_READ_FAILED))
            }
        }
    }

    #[tool(description = "Record a desktop event's audit row (a closed list).")]
    async fn admin_audit_event(
        &self,
        Parameters(p): Parameters<AuditParams>,
    ) -> Result<CallToolResult, McpError> {
        let access = if p.event.open() {
            Access::Open
        } else {
            Access::Gated
        };
        if let Some(code) = self.gate.refuse(access).await {
            return Ok(refused(code));
        }
        let written = match self.host.as_ref() {
            AdminHost::Full(full) => {
                ops::audit_event(
                    full.adapter(),
                    p.event,
                    p.duration_ms,
                    p.result_count,
                    p.failed,
                )
                .await
            }
            AdminHost::Locked(locked) => match locked.store().await {
                Ok(Some(store)) => {
                    ops::audit_event(store, p.event, p.duration_ms, p.result_count, p.failed).await
                }
                // No vault, so nothing happened to record.
                Ok(None) => Ok(()),
                Err(e) => {
                    tracing::error!(target: "vault_app::admin", error = %e, "a locked keeper could not open the memories to record an event");
                    Err(ERR_EXPORT_READ_FAILED.to_string())
                }
            },
        };
        done(written.map(|()| true))
    }
}

#[tool_handler]
impl ServerHandler for AdminServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            rmcp::model::Implementation::new("zaaheen-admin", env!("CARGO_PKG_VERSION")),
        )
    }
}
