//! `RelayServer` — the stdio face of `zaaheen mcp serve` when a keeper owns
//! the vault (ADR-102).
//!
//! AI apps start one `zaaheen mcp serve` per connection, and Claude Desktop
//! starts several at once. Only one process may own the vault, so every one of
//! them is a relay: it answers the MCP handshake and `tools/list` itself, and
//! forwards each `tools/call` to the keeper over an authenticated local stream
//! (the [`Upstream`], implemented in `vault-app`).
//!
//! **Why answer `initialize` and `tools/list` locally.** They carry no user
//! data, and the tool contract is compiled into this binary. Forwarding them
//! would make an AI app's startup wait on the keeper starting — and Claude
//! Desktop's startup probe gives a server well under a second. The contract
//! comes from a real `StdioServer` over [`NoVaultAdapter`], so it is identical
//! to what the keeper serves by construction.
//!
//! **Retry policy.** A call is resent after the connection died mid-call ONLY
//! if repeating it has the same effect as doing it once: `memory_read`,
//! `memory_search`, `memory_delete` (idempotent, ADR-056). A save whose reply
//! was lost may already have committed; resending it would store the memory
//! twice. Correctness over convenience — the agent is told to check first.

use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ListToolsResult, PaginatedRequestParams, ServerInfo,
    Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use vault_core::{Boundary, MemoryId, NewMemory, VaultError, VaultResult};
use vault_retrieval::{ReadQuery, RetrievalQuery, RetrievedMemory, StructuredReadResponse};

use crate::{Adapter, StdioServer, ToolInvokeDetails};

/// Agent-facing message when the connection died after a save was sent.
pub const MSG_OUTCOME_UNKNOWN: &str = "the vault restarted while handling this request; if it \
     was a save, check with memory_read before saving again";

/// Agent-facing message when the keeper did not answer a read, search or
/// delete in time. Repeating those is harmless.
pub const MSG_TIMED_OUT: &str = "the vault took too long to answer; try again";

/// Agent-facing message when the keeper did not answer a SAVE in time. The
/// keeper keeps working after the relay stops waiting, so the save may still
/// land: "try again" would invite a duplicate.
pub const MSG_TIMED_OUT_SAVE: &str = "the vault took too long to answer; the save may still \
     complete, so check with memory_read before saving again";

/// The keeper connection, as the relay sees it.
#[async_trait]
pub trait Upstream: Send + Sync {
    /// Forward one tool call to the keeper.
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
    ) -> Result<CallToolResult, UpstreamError>;
}

/// How a forwarded call failed.
#[derive(Debug)]
pub enum UpstreamError {
    /// The request never reached the keeper. Carries the agent-facing reason
    /// (keeper starting, maintenance, update needed, ...).
    NotSent(&'static str),
    /// The connection died after the request went out: outcome unknown.
    Lost,
    /// The keeper did not answer in time: outcome unknown.
    TimedOut,
    /// The keeper answered with an MCP error; passed through untouched.
    Keeper(McpError),
}

/// Stdio handler for relay mode.
pub struct RelayServer {
    contract: StdioServer,
    upstream: Arc<dyn Upstream>,
}

impl RelayServer {
    pub fn new(upstream: Arc<dyn Upstream>) -> Self {
        Self {
            contract: StdioServer::new(Arc::new(NoVaultAdapter), Vec::new()),
            upstream,
        }
    }
}

/// Repeating these has the same effect as calling them once.
fn is_repeat_safe(tool: &str) -> bool {
    matches!(tool, "memory_read" | "memory_search" | "memory_delete")
}

fn to_mcp(result: Result<CallToolResult, UpstreamError>) -> Result<CallToolResult, McpError> {
    match result {
        Ok(r) => Ok(r),
        Err(UpstreamError::Keeper(e)) => Err(e),
        Err(UpstreamError::NotSent(reason)) => Err(McpError::internal_error(reason, None)),
        Err(UpstreamError::Lost) => Err(McpError::internal_error(MSG_OUTCOME_UNKNOWN, None)),
        Err(UpstreamError::TimedOut) => Err(McpError::internal_error(MSG_TIMED_OUT, None)),
    }
}

impl ServerHandler for RelayServer {
    fn get_info(&self) -> ServerInfo {
        self.contract.get_info()
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.contract.get_tool(name)
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        self.contract.list_tools(request, context).await
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let repeat_safe = is_repeat_safe(&request.name);
        let outcome = match self.upstream.call_tool(request.clone()).await {
            Err(UpstreamError::Lost) if repeat_safe => self.upstream.call_tool(request).await,
            other => other,
        };
        match outcome {
            Err(UpstreamError::TimedOut) if !repeat_safe => {
                Err(McpError::internal_error(MSG_TIMED_OUT_SAVE, None))
            }
            other => to_mcp(other),
        }
    }
}

/// Adapter for a relay's contract-only `StdioServer`. A relay has no vault;
/// every call it would need is forwarded instead, so none of these is ever
/// reached. If one ever were, it fails closed rather than pretending.
pub struct NoVaultAdapter;

fn no_vault<T>() -> VaultResult<T> {
    Err(VaultError::Config(
        "relay has no local vault; calls are forwarded to the keeper".into(),
    ))
}

#[async_trait]
impl Adapter for NoVaultAdapter {
    async fn search(&self, _query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        no_vault()
    }

    async fn read(&self, _query: ReadQuery) -> VaultResult<StructuredReadResponse> {
        no_vault()
    }

    async fn write(&self, _new_memory: NewMemory) -> VaultResult<MemoryId> {
        no_vault()
    }

    async fn update(&self, _id: MemoryId, _new_memory: NewMemory) -> VaultResult<()> {
        no_vault()
    }

    async fn delete(&self, _id: MemoryId) -> VaultResult<()> {
        no_vault()
    }

    async fn lookup_boundary(&self, _id: MemoryId) -> VaultResult<Option<Boundary>> {
        no_vault()
    }

    async fn append_tool_invoke_audit(&self, _details: ToolInvokeDetails) -> VaultResult<()> {
        // The keeper audits the real call; a relay records nothing.
        Ok(())
    }
}
