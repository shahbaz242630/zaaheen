//! The "every area" grant (ADR-115, ADR-SEC-050, session 113).
//!
//! The founder's bug: memories typed into the Zaaheen app (area `default`)
//! and a new area (`Health`) were invisible to every AI app, which was
//! granted `personal` only. Under the Every grant a connection reaches every
//! registered area AS IT STANDS AT EACH CALL; a save into an area that does
//! not exist writes nothing and is answered readably; a fixed grant behaves
//! exactly as before.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use vault_core::{Boundary, MemoryId, NewMemory, VaultError, VaultResult};
use vault_mcp::{
    Adapter, AreaInfo, Grant, HelpToolParams, ReadToolParams, SearchToolParams, StdioServer,
    ToolInvokeDetails, UpdateToolParams, WriteToolParams,
};
use vault_retrieval::{ReadQuery, RetrievalQuery, RetrievedMemory, StructuredReadResponse};

/// A registry the test can grow while the server is running, and a record of
/// what every call was scoped to.
#[derive(Default)]
struct AreasAdapter {
    areas: Mutex<Vec<AreaInfo>>,
    /// The memory `lookup_boundary` reports, if any.
    stored: Mutex<Option<Boundary>>,
    searched: Mutex<Vec<Vec<Boundary>>>,
    read: Mutex<Vec<Vec<Boundary>>>,
    written: Mutex<Vec<NewMemory>>,
    updated: Mutex<Vec<NewMemory>>,
    audits: Mutex<Vec<ToolInvokeDetails>>,
}

impl AreasAdapter {
    fn with(names: &[&str]) -> Arc<Self> {
        let adapter = Arc::new(Self::default());
        for n in names {
            adapter.add(n, 0);
        }
        adapter
    }

    fn add(&self, name: &str, memory_count: u64) {
        self.areas.lock().unwrap().push(AreaInfo {
            name: Boundary::new(name).unwrap(),
            description: None,
            memory_count,
        });
    }
}

#[async_trait]
impl Adapter for AreasAdapter {
    async fn search(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        self.searched
            .lock()
            .unwrap()
            .push(query.authorized_boundaries);
        Ok(Vec::new())
    }

    async fn read(&self, query: ReadQuery) -> VaultResult<StructuredReadResponse> {
        self.read.lock().unwrap().push(query.authorized_boundaries);
        Err(VaultError::Retrieval("recorded".into()))
    }

    async fn write(&self, new_memory: NewMemory) -> VaultResult<MemoryId> {
        self.written.lock().unwrap().push(new_memory);
        Ok(MemoryId::new())
    }

    async fn update(&self, _id: MemoryId, new_memory: NewMemory) -> VaultResult<()> {
        self.updated.lock().unwrap().push(new_memory);
        Ok(())
    }

    async fn delete(&self, _id: MemoryId) -> VaultResult<()> {
        Ok(())
    }

    async fn lookup_boundary(&self, _id: MemoryId) -> VaultResult<Option<Boundary>> {
        Ok(self.stored.lock().unwrap().clone())
    }

    async fn list_areas(&self) -> VaultResult<Vec<AreaInfo>> {
        Ok(self.areas.lock().unwrap().clone())
    }

    async fn append_tool_invoke_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        self.audits.lock().unwrap().push(details);
        Ok(())
    }
}

fn names(list: &[Boundary]) -> Vec<&str> {
    list.iter().map(Boundary::as_str).collect()
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

fn write(boundary: &str) -> Parameters<WriteToolParams> {
    Parameters(WriteToolParams {
        content: "The user has back pain sometimes.".into(),
        boundary: boundary.into(),
        memory_type: None,
        source_agent: None,
        confidence: None,
        as_of: None,
    })
}

fn update(boundary: &str) -> Parameters<UpdateToolParams> {
    Parameters(UpdateToolParams {
        id: MemoryId::new().to_string(),
        content: "The user likes to play football.".into(),
        boundary: boundary.into(),
        memory_type: None,
        source_agent: None,
        confidence: None,
    })
}

fn search() -> SearchToolParams {
    SearchToolParams {
        query: "what sports does the user play".into(),
        max_results: None,
        score_threshold: None,
        include_archived: None,
    }
}

/// The s113 bug, end to end at this layer: a memory typed into the app sits
/// in `default`; an AI app's search and read must reach it.
#[tokio::test]
async fn every_area_reaches_the_areas_the_user_typed_into() {
    let adapter = AreasAdapter::with(&["default", "personal"]);
    let server = StdioServer::every_area(adapter.clone());
    server.handle_search(search()).await.unwrap();
    let _ = server
        .handle_read(ReadToolParams {
            query: "what sports does the user play".into(),
        })
        .await;
    assert_eq!(
        names(&adapter.searched.lock().unwrap()[0]),
        ["default", "personal"]
    );
    assert_eq!(
        names(&adapter.read.lock().unwrap()[0]),
        ["default", "personal"]
    );
}

/// An area the user creates is reached on the very next call: no restart,
/// no reconnect (founder: "if user creates new boundary... agents should see
/// it").
#[tokio::test]
async fn an_area_created_after_connecting_is_reached_at_the_next_call() {
    let adapter = AreasAdapter::with(&["default", "personal"]);
    let server = StdioServer::every_area(adapter.clone());
    server.handle_search(search()).await.unwrap();
    adapter.add("Health", 0);
    server.handle_search(search()).await.unwrap();
    let searched = adapter.searched.lock().unwrap();
    assert_eq!(names(&searched[0]), ["default", "personal"]);
    assert_eq!(names(&searched[1]), ["default", "personal", "Health"]);
}

#[tokio::test]
async fn a_save_into_a_new_area_is_written() {
    let adapter = AreasAdapter::with(&["default", "personal"]);
    let server = StdioServer::every_area(adapter.clone());
    adapter.add("Health", 0);
    let result = server.tool_write(write("Health")).await.unwrap();
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    let written = adapter.written.lock().unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].boundary.as_str(), "Health");
}

/// Founder decision A: refuse, name the areas, suggest the close match, give
/// the steps. Nothing is written; the refusal is audited.
#[tokio::test]
async fn a_save_into_an_unknown_area_writes_nothing_and_says_why() {
    let adapter = AreasAdapter::with(&["Health", "default", "personal"]);
    let server = StdioServer::every_area(adapter.clone());
    let result = server
        .tool_write(write("health"))
        .await
        .expect("a readable tool result, not a protocol error");
    assert_eq!(result.is_error, Some(true));
    let text = text_of(&result);
    assert!(text.starts_with("Not saved: there is no area called \"health\""));
    assert!(text.contains("Did you mean \"Health\"?"));
    assert!(text.contains("Health, default, personal"));
    assert!(text.contains("+ New boundary"));
    assert!(
        adapter.written.lock().unwrap().is_empty(),
        "nothing written"
    );
    let audits = adapter.audits.lock().unwrap();
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].tool, "memory_write");
    assert_eq!(audits[0].result_count, 0);
    assert!(audits[0].error.is_some());
}

/// A name the vault could never accept ("my health" has a space) is answered
/// the same readable way, not as a bare protocol error.
#[tokio::test]
async fn an_invalid_area_name_is_answered_readably() {
    let adapter = AreasAdapter::with(&["default", "personal"]);
    let server = StdioServer::every_area(adapter.clone());
    let result = server.tool_write(write("my health")).await.unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(text_of(&result).contains("no area called \"my health\""));
    assert!(adapter.written.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_edit_into_an_unknown_area_changes_nothing_and_says_why() {
    let adapter = AreasAdapter::with(&["default", "personal"]);
    *adapter.stored.lock().unwrap() = Some(Boundary::new("default").unwrap());
    let server = StdioServer::every_area(adapter.clone());
    let result = server.tool_update(update("sports")).await.unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(text_of(&result).contains("no area called \"sports\""));
    assert!(adapter.updated.lock().unwrap().is_empty());
}

/// An app may move a memory between existing areas (it reaches both).
#[tokio::test]
async fn an_edit_between_existing_areas_is_applied() {
    let adapter = AreasAdapter::with(&["default", "personal"]);
    *adapter.stored.lock().unwrap() = Some(Boundary::new("default").unwrap());
    let server = StdioServer::every_area(adapter.clone());
    let result = server.tool_update(update("personal")).await.unwrap();
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    assert_eq!(adapter.updated.lock().unwrap().len(), 1);
}

/// BRD §11.4.3: a retrieval always declares its areas. No areas at all is
/// refused, never run unfiltered.
#[tokio::test]
async fn no_areas_at_all_is_access_denied_never_unfiltered() {
    let adapter = AreasAdapter::with(&[]);
    let server = StdioServer::every_area(adapter.clone());
    let err = server.handle_search(search()).await.unwrap_err();
    assert!(matches!(err, VaultError::AccessDenied(_)), "{err:?}");
    assert!(adapter.searched.lock().unwrap().is_empty());
}

/// A fixed grant is unchanged: it never reaches another area, and a save
/// into one is the generic access denied (ADR-024: no detail).
#[tokio::test]
async fn a_fixed_grant_is_unchanged() {
    let adapter = AreasAdapter::with(&["default", "personal"]);
    let server = StdioServer::new(adapter.clone(), vec![Boundary::new("personal").unwrap()]);
    assert_eq!(
        server.grant(),
        &Grant::Only(vec![Boundary::new("personal").unwrap()])
    );
    server.handle_search(search()).await.unwrap();
    assert_eq!(names(&adapter.searched.lock().unwrap()[0]), ["personal"]);
    let err = server.tool_write(write("default")).await.unwrap_err();
    assert_eq!(err.code.0, -32001, "generic access denied");
    assert!(!err.message.contains("default"));
    assert!(adapter.written.lock().unwrap().is_empty());
}

#[tokio::test]
async fn help_lists_every_area_with_its_count() {
    let adapter = AreasAdapter::with(&["default"]);
    adapter.add("Health", 1);
    let server = StdioServer::every_area(adapter.clone());
    let result = server
        .tool_help(Parameters(HelpToolParams { topic: None }))
        .await
        .unwrap();
    let text = text_of(&result);
    assert!(text.contains("- default: 0 memories"));
    assert!(text.contains("- Health: 1 memory"));
    assert_eq!(adapter.audits.lock().unwrap()[0].tool, "memory_help");
}

#[tokio::test]
async fn help_on_a_fixed_grant_names_only_its_own_areas() {
    let adapter = AreasAdapter::with(&["default", "personal", "Health"]);
    let server = StdioServer::new(adapter.clone(), vec![Boundary::new("personal").unwrap()]);
    let text = text_of(
        &server
            .tool_help(Parameters(HelpToolParams { topic: None }))
            .await
            .unwrap(),
    );
    assert!(text.contains("- personal:"));
    assert!(!text.contains("- Health:"));
    assert!(!text.contains("- default:"));
}

#[tokio::test]
async fn help_serves_a_topic() {
    let adapter = AreasAdapter::with(&["personal"]);
    let server = StdioServer::every_area(adapter);
    let text = text_of(
        &server
            .tool_help(Parameters(HelpToolParams {
                topic: Some("areas".into()),
            }))
            .await
            .unwrap(),
    );
    assert!(text.contains("+ New boundary"));
}
