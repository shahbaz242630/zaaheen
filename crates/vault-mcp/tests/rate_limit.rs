//! The AI-app limit on deletes and edits (ADR-SEC-049, s109).
//!
//! The vault refuses a change past the hourly limit with
//! `VaultError::RateLimited`; `memory_delete` and `memory_update` must turn
//! that into a tool result the agent can READ and pass on (ADR-103 D2), with
//! the fixed sentence and nothing else, and still record the audit row.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use vault_core::{Boundary, MemoryId, NewMemory, VaultError, VaultResult};
use vault_mcp::{
    Adapter, DeleteToolParams, StdioServer, ToolInvokeDetails, ToolInvokeError, UpdateToolParams,
    MSG_PAUSED,
};
use vault_retrieval::{ReadQuery, RetrievalQuery, RetrievedMemory, StructuredReadResponse};

/// Every change is over the limit; the memory exists in `work`.
#[derive(Default)]
struct PausedAdapter {
    audits: Mutex<Vec<ToolInvokeDetails>>,
}

#[async_trait]
impl Adapter for PausedAdapter {
    async fn search(&self, _query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        unimplemented!("not used")
    }

    async fn read(&self, _query: ReadQuery) -> VaultResult<StructuredReadResponse> {
        unimplemented!("not used")
    }

    async fn write(&self, _new_memory: NewMemory) -> VaultResult<MemoryId> {
        unimplemented!("not used")
    }

    async fn update(&self, _id: MemoryId, _new_memory: NewMemory) -> VaultResult<()> {
        Err(VaultError::RateLimited(
            "AI apps made 30 changes in the last hour".into(),
        ))
    }

    async fn delete(&self, _id: MemoryId) -> VaultResult<()> {
        Err(VaultError::RateLimited(
            "AI apps made 30 changes in the last hour".into(),
        ))
    }

    async fn lookup_boundary(&self, _id: MemoryId) -> VaultResult<Option<Boundary>> {
        Ok(Some(Boundary::new("work").unwrap()))
    }

    async fn append_tool_invoke_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        self.audits.lock().unwrap().push(details);
        Ok(())
    }
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

fn server() -> (Arc<PausedAdapter>, StdioServer) {
    let adapter = Arc::new(PausedAdapter::default());
    let server = StdioServer::new(adapter.clone(), vec![Boundary::new("work").unwrap()]);
    (adapter, server)
}

#[tokio::test]
async fn a_delete_past_the_limit_is_a_readable_paused_result() {
    let (adapter, server) = server();
    let result = server
        .tool_delete(Parameters(DeleteToolParams {
            id: MemoryId::new().to_string(),
        }))
        .await
        .expect("a paused delete is a tool result, not a protocol error");
    assert_eq!(result.is_error, Some(true));
    assert_eq!(text_of(&result), MSG_PAUSED);

    let audits = adapter.audits.lock().unwrap().clone();
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].tool, "memory_delete");
    assert_eq!(audits[0].result_count, 0);
    assert!(matches!(
        &audits[0].error,
        Some(ToolInvokeError::Internal { category, .. }) if category == "RateLimited"
    ));
}

#[tokio::test]
async fn an_edit_past_the_limit_is_a_readable_paused_result() {
    let (_adapter, server) = server();
    let result = server
        .tool_update(Parameters(UpdateToolParams {
            id: MemoryId::new().to_string(),
            content: "Shahbaz lives in London".into(),
            boundary: "work".into(),
            memory_type: None,
            source_agent: None,
            confidence: None,
        }))
        .await
        .expect("a paused edit is a tool result, not a protocol error");
    assert_eq!(result.is_error, Some(true));
    assert_eq!(text_of(&result), MSG_PAUSED);
}

#[test]
fn the_paused_sentence_names_no_data_and_tells_the_user_what_to_do() {
    assert!(MSG_PAUSED.contains("Zaaheen app"));
    for digit in '0'..='9' {
        assert!(
            !MSG_PAUSED.contains(digit),
            "no counts or ids: {MSG_PAUSED}"
        );
    }
}
