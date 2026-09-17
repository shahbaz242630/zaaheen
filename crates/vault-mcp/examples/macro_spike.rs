//! `macro_spike` — runtime confirmation of rmcp 1.5.0's tool macro contract.
//!
//! ## Purpose
//!
//! T0.1.9 Phase 2 (per `T0.1.9_PLAN.md` v1.1) wires the four vault tools
//! (`memory_search` / `memory_write` / `memory_update` / `memory_delete`) on
//! `StdioServer` using rmcp's `#[tool_router]` + `#[tool]` + `#[tool_handler]`
//! attribute macros. This spike verifies — at compile + runtime — every
//! contract Phase 2 will rely on, against an actual rmcp 1.5.0 build:
//!
//! 1. **Macro names + structure.** `#[tool_router]` on `impl Server`,
//!    `#[tool]` on the method, `#[tool_handler]` on `impl ServerHandler`.
//!    (Earlier plan drafts used the spelling `#[tool_router(server_handler)]`
//!    which is NOT what rmcp ships — corrected at spike time.)
//! 2. **Param-struct derives.** `Serialize + Deserialize + schemars::JsonSchema`
//!    is the minimum trio for a `Parameters<T>` arg to compile + schema-gen.
//!    `rmcp::schemars` is re-exported from the `server` feature — no separate
//!    workspace `schemars` dep needed.
//! 3. **Return-type adaptation.** `Result<CallToolResult, McpError>` is the
//!    explicit shape the macro layer maps to JSON-RPC `result`/`error`. We
//!    use this shape because Phase 2 needs to map `VaultError::DimensionMismatch`
//!    → `InvalidParams` (per ADR-024) and that needs `Err(McpError::...)`.
//! 4. **Direct method invocation.** The macro decorates the method without
//!    replacing it — calling `server.tool(Parameters(p)).await` directly
//!    works just like a plain async fn. This is load-bearing for Q7's
//!    handler-mediated audit answer: the handler body is the natural site
//!    for `duration_ms` + `result_count` + audit append.
//! 5. **Custom `get_info()`.** `#[tool_handler]` auto-generates `get_info`
//!    when the impl body is empty; it must coexist cleanly with a
//!    user-supplied `get_info` (Phase 2 needs a custom one to set
//!    `server_info.name = "vault-mcp"` + pin protocol version).
//! 6. **`tool_attr()` helper.** The macro generates `Server::tool_name_tool_attr()`
//!    returning a `Tool` with `name` + `description` + `input_schema`. This
//!    is what Step 9's "tool list is exactly {memory_search, memory_write,
//!    memory_update, memory_delete}" pin will read.
//!
//! ## Re-run trigger
//!
//! Run this spike whenever the workspace `rmcp` pin advances. Same retention
//! pattern as `crates/vault-sync/examples/dryoc_spike.rs` (ADR-008) and
//! `crates/vault-storage/examples/lance_corruption_spike.rs` (ADR-018):
//! executable documentation that survives the version it was written for.
//!
//! ```text
//! cargo run -p vault-mcp --example macro_spike
//! ```
//!
//! Exit 0 == every contract above still holds. Non-zero == something drifted;
//! the failure message names the contract that broke.

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use serde::{Deserialize, Serialize};

// =============================================================================
// 1. Param struct mirroring SearchToolParams's derive trio
// =============================================================================
//
// Phase 2 will add these derives to the real `SearchToolParams` /
// `WriteToolParams` in `crates/vault-mcp/src/server.rs`. The spike proves
// the trio compiles before we touch production code.

/// Mirror of `SearchToolParams` shape — verifies the derive trio compiles.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct SpikeSearchParams {
    /// The search query.
    query: String,
    /// Optional max results.
    #[serde(default)]
    max_results: Option<u32>,
}

/// Used to prove a no-arg tool also works (Phase 2's `memory_delete` takes
/// just an id; we'll see if the no-arg pattern fits or if we need a
/// trivial wrapper struct).
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct SpikeDeleteParams {
    /// The id of the memory to delete.
    id: String,
}

// =============================================================================
// 2. SpikeServer — minimum useful surface mirroring StdioServer's planned shape
// =============================================================================

/// Mirror of `StdioServer` minus the trust-boundary slice + Adapter — the
/// spike's role is the macro contract, not the trust-boundary mediation
/// (which Phase 1 already pins via the existing scaffold).
#[derive(Clone)]
#[allow(dead_code)] // tool_router field is read by the #[tool_handler] macro,
                    // but dead-code analysis can't see through macro expansion.
                    // Mirrors rmcp's own test_tool_macros suppression pattern.
struct SpikeServer {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SpikeServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    /// Mirrors `memory_search` — typed params + `Result<CallToolResult, McpError>`
    /// return + simulated error path.
    #[tool(description = "Spike search tool — mirrors memory_search shape.")]
    async fn search(
        &self,
        params: Parameters<SpikeSearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(p) = params;
        if p.query.is_empty() {
            // Mirrors the Phase 2 dim-mismatch / invalid-params mapping
            // from ADR-024: generic message, no leak in error.data.
            return Err(McpError::invalid_params(
                "query must be non-empty".to_string(),
                None,
            ));
        }
        let payload = serde_json::json!({
            "query_echo": p.query,
            "max_results": p.max_results.unwrap_or(10),
        });
        Ok(CallToolResult::success(vec![ContentBlock::json(payload)?]))
    }

    /// Mirrors `memory_delete` — no-frills arg shape proves the macro handles
    /// the simpler tools too. Returns `()` (the macro adapts unit to a
    /// successful empty `CallToolResult`).
    #[tool(description = "Spike delete tool — mirrors memory_delete shape.")]
    async fn delete(
        &self,
        params: Parameters<SpikeDeleteParams>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(p) = params;
        let payload = serde_json::json!({ "deleted": p.id });
        Ok(CallToolResult::success(vec![ContentBlock::json(payload)?]))
    }
}

// =============================================================================
// 3. ServerHandler with custom get_info — proves coexistence with #[tool_handler]
// =============================================================================
//
// Phase 2 needs `server_info.name = "vault-mcp"` + pinned protocol version;
// the spike proves that providing a custom `get_info()` doesn't conflict
// with `#[tool_handler]`'s auto-generated dispatch.

#[tool_handler]
impl ServerHandler for SpikeServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("vault-mcp-spike", "0.0.0"))
            .with_instructions("Spike server — verifies rmcp 1.5.0 macro contract.")
    }
}

// =============================================================================
// 4. Drive every contract end-to-end
// =============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = SpikeServer::new();

    // -------------------------------------------------------------------------
    // Contract 6: tool_attr() helpers exist + carry name/description/schema
    // -------------------------------------------------------------------------
    let search_attr = SpikeServer::search_tool_attr();
    let delete_attr = SpikeServer::delete_tool_attr();

    println!("== Contract 6: tool_attr() ==");
    println!(
        "search.name={}  description={:?}",
        search_attr.name,
        search_attr.description.as_deref().unwrap_or("(none)")
    );
    println!(
        "delete.name={}  description={:?}",
        delete_attr.name,
        delete_attr.description.as_deref().unwrap_or("(none)")
    );
    assert_eq!(
        search_attr.name, "search",
        "tool name should match method name"
    );
    assert_eq!(
        delete_attr.name, "delete",
        "tool name should match method name"
    );

    // The input_schema is the JSON Schema 2020-12 doc the MCP client sees.
    // For SpikeSearchParams we expect properties.query.type == "string".
    let search_schema = serde_json::Value::Object((*search_attr.input_schema).clone());
    println!(
        "search.input_schema={}",
        serde_json::to_string_pretty(&search_schema)?
    );
    let query_type = search_schema
        .get("properties")
        .and_then(|p| p.get("query"))
        .and_then(|q| q.get("type"))
        .and_then(|t| t.as_str())
        .ok_or("schema missing properties.query.type")?;
    assert_eq!(query_type, "string", "query field should be type:string");

    // -------------------------------------------------------------------------
    // Contract 4: direct method invocation works (load-bearing for Q7)
    // -------------------------------------------------------------------------
    println!("\n== Contract 4: direct method invocation ==");
    let result = server
        .search(Parameters(SpikeSearchParams {
            query: "hello".to_string(),
            max_results: Some(5),
        }))
        .await?;
    println!("search(\"hello\") => is_error={:?}", result.is_error);
    assert_eq!(
        result.is_error,
        Some(false),
        "successful search should not flag is_error"
    );

    // -------------------------------------------------------------------------
    // Contract 3: error-path mapping (verifies the McpError ergonomic that
    // Phase 2's dim_mismatch test will rely on)
    // -------------------------------------------------------------------------
    println!("\n== Contract 3: error-path mapping ==");
    let err = server
        .search(Parameters(SpikeSearchParams {
            query: String::new(),
            max_results: None,
        }))
        .await
        .expect_err("empty query should error");
    println!(
        "search(\"\") => code={}  message={}",
        err.code.0, err.message
    );
    // McpError::invalid_params returns code -32602 per JSON-RPC spec
    assert_eq!(err.code.0, -32602, "invalid_params should be -32602");
    assert!(
        err.data.is_none(),
        "error.data MUST be absent for the no-info-leak invariant"
    );

    // -------------------------------------------------------------------------
    // Contract 5: custom get_info coexists with #[tool_handler]
    // -------------------------------------------------------------------------
    println!("\n== Contract 5: custom get_info ==");
    let info = server.get_info();
    println!(
        "server_info.name={}  capabilities.tools.is_some()={}",
        info.server_info.name,
        info.capabilities.tools.is_some()
    );
    assert_eq!(info.server_info.name, "vault-mcp-spike");
    assert!(
        info.capabilities.tools.is_some(),
        "tools capability should be set"
    );

    // -------------------------------------------------------------------------
    // Contract 2 implied: this whole file compiled, so the derive trio
    // (Serialize + Deserialize + schemars::JsonSchema) works.
    // -------------------------------------------------------------------------
    println!("\n== Contract 2: derive trio compiles (implicit by reaching here) ==");

    // -------------------------------------------------------------------------
    // Contract 1 implied: the macros all expanded, so the names + structure
    // (`#[tool_router]` + `#[tool]` + `#[tool_handler]`) are correct.
    // -------------------------------------------------------------------------
    println!("== Contract 1: macro names + structure (implicit by reaching here) ==");

    println!("\nSpike PASSED — all 6 contracts verified against rmcp 1.5.0.");
    Ok(())
}
