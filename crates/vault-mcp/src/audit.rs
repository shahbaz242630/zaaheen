//! `mcp.tool_invoke` audit detail body — typed representation of the
//! `details_json` payload locked by ADR-024 (HANDOFF.md §ADR-024 +
//! T0.1.9_PLAN.md §5 / §6.2 rule 2).
//!
//! ## Schema (ADR-024, do not paraphrase)
//!
//! ```json
//! {
//!   "tool": "memory_search" | "memory_write" | "memory_update" | "memory_delete",
//!   "duration_ms": <u64>,
//!   "result_count": <u32>,        // search only; 0 / 1 for write/update/delete
//!   "boundary_count": <u32>,
//!   "max_results": <u32>,         // search only — ABSENT (not null) on write/update/delete
//!   "score_threshold": <f32>,     // search only
//!   "include_archived": <bool>,   // search only
//!   "query_length": <u32>,        // search only
//!   "error": { "type": "<VaultError variant_name>", "detail": <Value> }   // optional
//! }
//! ```
//!
//! - **Search-only fields** are `Option<T>` with
//!   `#[serde(skip_serializing_if = "Option::is_none")]` so they're
//!   ABSENT (not the JSON value `null`) on write/update/delete per Q1.
//! - **`error`** is absent on success.
//! - **`error.type`** is the `VaultError` variant name in PascalCase
//!   (e.g. `"DimensionMismatch"`); serde derives this from the Rust
//!   variant ident on [`ToolInvokeError`].
//! - **`error.detail`** is variant-specific per ADR-024 (lines 787–790):
//!   - `AccessDenied` → `{"boundary_attempted": "<name>"}`
//!   - `DimensionMismatch` → `{"expected": <u32>, "actual": <u32>}`
//!   - `ModelIntegrityFailed` → `{"file": "<path>", "expected": "<sha256>", "actual": "<sha256>"}`
//!   - `Storage` / `Embedding` / `Retrieval` / `InvalidInput` → `{"message": "<msg>"}`
//!
//! ## Canonical sorted-key JSON (BRD §11.9.2)
//!
//! The audit chain hashes `details_json` verbatim — bytes go straight
//! into BLAKE3 alongside the previous chain hash. To make the hash
//! reproducible across runs / processes / future-Claude, the JSON
//! must use sorted-key ordering at every nesting level.
//!
//! [`ToolInvokeDetails::to_canonical_json`] is the single producer of
//! the audit-chain string. It serialises via [`std::collections::BTreeMap`]
//! at every level (top + `error.detail`) so key order is independent
//! of struct field declaration order. The
//! [`canonical_json_orders_keys_alphabetically`] unit test pins the
//! invariant.
//!
//! Direct `serde_json::to_string(&details)` is **NOT** the audit-chain
//! producer — it serialises in struct field declaration order, which
//! is fine for tracing/debug output but would silently break audit
//! chain hashing if a future field add reordered the JSON. The trait
//! method [`Adapter::append_tool_invoke_audit`](crate::Adapter::append_tool_invoke_audit)
//! is documented to use `to_canonical_json`.

use std::collections::BTreeMap;

use serde::Serialize;
use vault_core::{VaultError, VaultResult};

/// Typed representation of the `details_json` body for an
/// `mcp.tool_invoke` audit event.
///
/// Constructed by the `tool_*` handlers in [`crate::server`] at
/// invocation exit (success or error path), passed to
/// [`Adapter::append_tool_invoke_audit`](crate::Adapter::append_tool_invoke_audit)
/// for chain persistence + emitted in parallel via
/// `tracing::info!(target: "vault_mcp::tool_invoke", ...)` per Q5/Q6
/// (operational log carries the same shape minus content).
///
/// Per ADR-024 + Q1: search-only fields are `Option<T>` with
/// `skip_serializing_if = "Option::is_none"`. Non-search tools
/// construct with `None` for those fields, producing JSON that omits
/// the keys entirely.
#[derive(Debug, Clone, Serialize)]
pub struct ToolInvokeDetails {
    /// MCP tool name — `"memory_search"`, `"memory_write"`,
    /// `"memory_update"`, or `"memory_delete"`.
    pub tool: &'static str,
    /// Wall-clock duration of the handler dispatch, in milliseconds.
    pub duration_ms: u64,
    /// Number of results returned to the caller. For search this is
    /// the post-filter count; for write/update/delete this is `1` on
    /// success, `0` on error.
    pub result_count: u32,
    /// Size of the trusted `authorized_boundaries` slice the handler
    /// used. Records the auth-context shape, never the boundary names.
    pub boundary_count: u32,

    // -------------------------------------------------------------------
    // Search-only fields — ABSENT (not null) on write/update/delete per Q1.
    // -------------------------------------------------------------------
    /// `RetrievalQuery::max_results` cap — search only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
    /// `RetrievalOptions::score_threshold` — search only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_threshold: Option<f32>,
    /// `RetrievalOptions::include_archived` — search only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_archived: Option<bool>,
    /// Byte length of `RetrievalQuery::query_text` — search only.
    /// Length only, never the query string itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_length: Option<u32>,

    /// Structured error description per ADR-024. Absent on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolInvokeError>,
}

/// Audit-row error description per ADR-024 (HANDOFF.md lines 786–790).
///
/// Serialises to `{ "type": "<VariantName>", "detail": <Value> }` via
/// `#[serde(tag = "type", content = "detail")]`. Variant idents are
/// PascalCase, matching the corresponding `VaultError` variant name.
///
/// Only the seven variants listed in ADR-024's mapping table are
/// represented. `VaultError` variants outside the table (e.g. `Llm`,
/// `Crypto`, `Io`) are mapped to a generic `Internal` audit error in
/// the from-VaultError conversion below — same privacy-preserving
/// default as `vault_error_to_mcp`'s catch-all.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "detail")]
pub enum ToolInvokeError {
    /// Boundary check failed. `boundary_attempted` records the name
    /// the agent supplied, not anything from the trusted slice.
    AccessDenied { boundary_attempted: String },
    /// Embedding produced a vector of unexpected dimension.
    DimensionMismatch { expected: u32, actual: u32 },
    /// Model / tokenizer file integrity check failed at provider load.
    ModelIntegrityFailed {
        file: String,
        expected: String,
        actual: String,
    },
    /// Underlying storage error — `message` is the verbatim
    /// `VaultError::Storage(String)` payload.
    Storage { message: String },
    /// Embedding pipeline error — verbatim `VaultError::Embedding`.
    Embedding { message: String },
    /// Retrieval pipeline error — verbatim `VaultError::Retrieval`.
    Retrieval { message: String },
    /// Input validation error — verbatim `VaultError::InvalidInput`.
    InvalidInput { message: String },
    /// `VaultError` variants outside ADR-024's mapping table land
    /// here. `category` records the variant ident so operators can
    /// triage; `message` carries the verbatim payload (or `Display`
    /// for variants that don't carry a string). The grouping is
    /// deliberate — ADR-024 is silent on these variants and the
    /// privacy-preserving default is to collapse them into one
    /// audit-row category until the table is extended.
    Internal { category: String, message: String },
}

impl ToolInvokeError {
    /// Map a `VaultError` to its `ToolInvokeError` audit-row
    /// representation. Variants in ADR-024's mapping table get their
    /// per-row shape; remaining variants land in [`Self::Internal`]
    /// with the variant ident as `category`.
    ///
    /// **Note on `AccessDenied`:** the `boundary_attempted` field is
    /// populated from the underlying `String` payload of
    /// `VaultError::AccessDenied(String)`. Handler call sites that
    /// construct `AccessDenied` MUST embed the agent-supplied
    /// boundary name in that string (not anything from the trusted
    /// slice) per ADR-025.
    pub fn from_vault_error(err: &VaultError) -> Self {
        match err {
            VaultError::AccessDenied(boundary_attempted) => Self::AccessDenied {
                boundary_attempted: boundary_attempted.clone(),
            },
            VaultError::DimensionMismatch { expected, actual } => Self::DimensionMismatch {
                expected: *expected as u32,
                actual: *actual as u32,
            },
            VaultError::ModelIntegrityFailed {
                file,
                expected,
                actual,
            } => Self::ModelIntegrityFailed {
                file: file.clone(),
                expected: expected.clone(),
                actual: actual.clone(),
            },
            VaultError::Storage(message) => Self::Storage {
                message: message.clone(),
            },
            VaultError::Embedding(message) => Self::Embedding {
                message: message.clone(),
            },
            VaultError::Retrieval(message) => Self::Retrieval {
                message: message.clone(),
            },
            VaultError::InvalidInput(message) => Self::InvalidInput {
                message: message.clone(),
            },
            // ADR-024-silent variants — collapse to Internal with
            // the variant ident as category. Adding a new VaultError
            // variant requires a deliberate decision here (the match
            // is exhaustive in `vault_error_to_mcp`, but kept loose
            // here so audit-row coverage doesn't block on table
            // amendments — the audit row records the category, the
            // wire response stays generic).
            VaultError::Llm(message) => Self::Internal {
                category: "Llm".to_string(),
                message: message.clone(),
            },
            VaultError::Consolidation(message) => Self::Internal {
                category: "Consolidation".to_string(),
                message: message.clone(),
            },
            // ADR-092: automatic-maintenance scheduling is a Tauri-layer
            // concern and never reaches MCP tool dispatch; recorded with its
            // own category for audit completeness if it ever does.
            VaultError::Scheduler(message) => Self::Internal {
                category: "Scheduler".to_string(),
                message: message.clone(),
            },
            VaultError::Mcp(message) => Self::Internal {
                category: "Mcp".to_string(),
                message: message.clone(),
            },
            VaultError::Sync(message) => Self::Internal {
                category: "Sync".to_string(),
                message: message.clone(),
            },
            VaultError::Connector(message) => Self::Internal {
                category: "Connector".to_string(),
                message: message.clone(),
            },
            VaultError::Auth(message) => Self::Internal {
                category: "Auth".to_string(),
                message: message.clone(),
            },
            VaultError::NotFound(message) => Self::Internal {
                category: "NotFound".to_string(),
                message: message.clone(),
            },
            VaultError::Crypto(message) => Self::Internal {
                category: "Crypto".to_string(),
                message: message.clone(),
            },
            VaultError::Config(message) => Self::Internal {
                category: "Config".to_string(),
                message: message.clone(),
            },
            VaultError::Io(io_err) => Self::Internal {
                category: "Io".to_string(),
                message: io_err.to_string(),
            },
            VaultError::Serde(message) => Self::Internal {
                category: "Serde".to_string(),
                message: message.clone(),
            },
            // T0.1.10 Phase 2: WorkerSpawnFailed / McpBindFailed are
            // startup errors. Collapsed to Internal with explicit
            // category names so audit-row coverage stays exhaustive
            // even though these variants should never reach the audit
            // path in practice (startup failure aborts before MCP
            // accepts requests).
            VaultError::WorkerSpawnFailed(message) => Self::Internal {
                category: "WorkerSpawnFailed".to_string(),
                message: message.clone(),
            },
            VaultError::McpBindFailed(message) => Self::Internal {
                category: "McpBindFailed".to_string(),
                message: message.clone(),
            },
            // T0.2.0 Phase 1 (2026-05-09): KeychainProvenance is the same
            // class as WorkerSpawnFailed / McpBindFailed — surfaces during
            // vault-tauri setup() BEFORE Application::new is reached, never
            // via MCP dispatch. Audit-coverage exhaustive via Internal with
            // explicit category name. Per ADR-040 + ADR-040 amendment.
            VaultError::KeychainProvenance(message) => Self::Internal {
                category: "KeychainProvenance".to_string(),
                message: message.clone(),
            },
            // ADR-SEC-029: same class as KeychainProvenance — a startup
            // failure before any tool dispatch. The kind carries no path,
            // credential or key material.
            VaultError::VaultKey(kind) => Self::Internal {
                category: "VaultKey".to_string(),
                message: kind.to_string(),
            },
            // ADR-105: the vault folder could not be found; no path is carried.
            VaultError::VaultLocation(kind) => Self::Internal {
                category: "VaultLocation".to_string(),
                message: kind.to_string(),
            },
            // T0.3.x Batch A (2026-05-26): consolidator safety-wrapper
            // errors. Surface via `Application::run_consolidation_with_safety`
            // (vault-cli `consolidate run` subcommand) — never via MCP tool
            // dispatch in V0.2. Audit-coverage exhaustive via Internal with
            // explicit category names matching `Application::run_consolidation_with_safety`'s
            // error contract (locked-next-arc Step 4).
            VaultError::ConsolidatorBusy(message) => Self::Internal {
                category: "ConsolidatorBusy".to_string(),
                message: message.clone(),
            },
            VaultError::ConsolidatorTimeout(elapsed_secs) => Self::Internal {
                category: "ConsolidatorTimeout".to_string(),
                message: format!("hard budget exceeded after {elapsed_secs}s"),
            },
            // ADR-089 (2026-07-20): an optional model component is absent —
            // the ordinary state of an install whose first-run download has
            // not landed. Should NEVER reach the audit path: both rerank
            // consumers catch this variant and degrade (see
            // `RerankedRetriever::rerank_pool` /
            // `StructuredReadPipeline::apply_reranker`), so a tool call never
            // fails with it. Recorded defensively with its own category so
            // that if one ever DOES surface here, the audit log names it
            // rather than burying it — an unexpected `ModelUnavailable` in
            // the chain means a degrade path was missed, which is exactly the
            // kind of regression worth being able to grep for.
            VaultError::ModelUnavailable { component } => Self::Internal {
                category: "ModelUnavailable".to_string(),
                message: component.clone(),
            },
        }
    }
}

impl ToolInvokeDetails {
    /// Produce the canonical sorted-key JSON string for audit-chain
    /// hashing. Sorted at every nesting level (top + `error.detail`).
    ///
    /// This is the single producer of the audit-chain `details_json`
    /// — the trait method
    /// [`Adapter::append_tool_invoke_audit`](crate::Adapter::append_tool_invoke_audit)
    /// is documented to call this. Direct `serde_json::to_string(&details)`
    /// uses struct field declaration order, which is fine for
    /// tracing/debug but would silently break audit chain hashing if
    /// a future field reorder happens.
    pub fn to_canonical_json(&self) -> VaultResult<String> {
        let mut top: BTreeMap<&'static str, serde_json::Value> = BTreeMap::new();
        top.insert("tool", serde_json::Value::String(self.tool.to_string()));
        top.insert("duration_ms", serde_json::json!(self.duration_ms));
        top.insert("result_count", serde_json::json!(self.result_count));
        top.insert("boundary_count", serde_json::json!(self.boundary_count));
        if let Some(v) = self.max_results {
            top.insert("max_results", serde_json::json!(v));
        }
        if let Some(v) = self.score_threshold {
            top.insert("score_threshold", serde_json::json!(v));
        }
        if let Some(v) = self.include_archived {
            top.insert("include_archived", serde_json::json!(v));
        }
        if let Some(v) = self.query_length {
            top.insert("query_length", serde_json::json!(v));
        }
        if let Some(err) = &self.error {
            top.insert("error", err.to_canonical_value()?);
        }
        serde_json::to_string(&top).map_err(VaultError::from)
    }
}

impl ToolInvokeError {
    /// Produce a sorted-key JSON `Value` for use in canonical
    /// composition. Outer shape is `{"type": "...", "detail": {...}}`
    /// with detail keys also sorted alphabetically.
    fn to_canonical_value(&self) -> VaultResult<serde_json::Value> {
        let (variant_name, detail) = match self {
            Self::AccessDenied { boundary_attempted } => {
                let mut detail: BTreeMap<&'static str, serde_json::Value> = BTreeMap::new();
                detail.insert(
                    "boundary_attempted",
                    serde_json::Value::String(boundary_attempted.clone()),
                );
                ("AccessDenied", detail)
            }
            Self::DimensionMismatch { expected, actual } => {
                let mut detail: BTreeMap<&'static str, serde_json::Value> = BTreeMap::new();
                detail.insert("actual", serde_json::json!(actual));
                detail.insert("expected", serde_json::json!(expected));
                ("DimensionMismatch", detail)
            }
            Self::ModelIntegrityFailed {
                file,
                expected,
                actual,
            } => {
                let mut detail: BTreeMap<&'static str, serde_json::Value> = BTreeMap::new();
                detail.insert("actual", serde_json::Value::String(actual.clone()));
                detail.insert("expected", serde_json::Value::String(expected.clone()));
                detail.insert("file", serde_json::Value::String(file.clone()));
                ("ModelIntegrityFailed", detail)
            }
            Self::Storage { message } => single_message_detail("Storage", message),
            Self::Embedding { message } => single_message_detail("Embedding", message),
            Self::Retrieval { message } => single_message_detail("Retrieval", message),
            Self::InvalidInput { message } => single_message_detail("InvalidInput", message),
            Self::Internal { category, message } => {
                let mut detail: BTreeMap<&'static str, serde_json::Value> = BTreeMap::new();
                detail.insert("category", serde_json::Value::String(category.clone()));
                detail.insert("message", serde_json::Value::String(message.clone()));
                ("Internal", detail)
            }
        };

        let mut outer: BTreeMap<&'static str, serde_json::Value> = BTreeMap::new();
        outer.insert(
            "detail",
            serde_json::to_value(&detail).map_err(VaultError::from)?,
        );
        outer.insert("type", serde_json::Value::String(variant_name.to_string()));
        serde_json::to_value(&outer).map_err(VaultError::from)
    }
}

/// Helper for the four variants whose `detail` is `{"message": "<msg>"}`.
fn single_message_detail(
    variant: &'static str,
    message: &str,
) -> (&'static str, BTreeMap<&'static str, serde_json::Value>) {
    let mut detail: BTreeMap<&'static str, serde_json::Value> = BTreeMap::new();
    detail.insert("message", serde_json::Value::String(message.to_string()));
    (variant, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_details_serialise_with_all_fields_present() {
        let details = ToolInvokeDetails {
            tool: "memory_search",
            duration_ms: 12,
            result_count: 3,
            boundary_count: 1,
            max_results: Some(10),
            score_threshold: Some(0.5),
            include_archived: Some(false),
            query_length: Some(8),
            error: None,
        };
        let json: serde_json::Value =
            serde_json::from_str(&details.to_canonical_json().unwrap()).unwrap();
        assert_eq!(json["tool"], "memory_search");
        assert_eq!(json["duration_ms"], 12);
        assert_eq!(json["result_count"], 3);
        assert_eq!(json["boundary_count"], 1);
        assert_eq!(json["max_results"], 10);
        assert_eq!(json["score_threshold"], 0.5);
        assert_eq!(json["include_archived"], false);
        assert_eq!(json["query_length"], 8);
        assert!(
            json.get("error").is_none(),
            "error must be absent on success"
        );
    }

    #[test]
    fn write_details_omit_search_only_keys_entirely() {
        // Q1: search-only fields are ABSENT (not null) on
        // write/update/delete. Pin both the absence + that no `null`
        // sentinel appears.
        let details = ToolInvokeDetails {
            tool: "memory_write",
            duration_ms: 4,
            result_count: 1,
            boundary_count: 1,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: None,
        };
        let raw = details.to_canonical_json().unwrap();
        assert!(
            !raw.contains("max_results"),
            "max_results must be ABSENT on write, got: {raw}"
        );
        assert!(
            !raw.contains("score_threshold"),
            "score_threshold must be ABSENT on write, got: {raw}"
        );
        assert!(
            !raw.contains("include_archived"),
            "include_archived must be ABSENT on write, got: {raw}"
        );
        assert!(
            !raw.contains("query_length"),
            "query_length must be ABSENT on write, got: {raw}"
        );
        assert!(
            !raw.contains("null"),
            "no `null` sentinel may appear; absent != null per Q1, got: {raw}"
        );
    }

    #[test]
    fn dimension_mismatch_error_shape_matches_adr_024() {
        // ADR-024 line 788: DimensionMismatch detail is
        // `{"expected": <u32>, "actual": <u32>}`. Outer shape is
        // `{"type": "DimensionMismatch", "detail": {...}}`.
        let details = ToolInvokeDetails {
            tool: "memory_search",
            duration_ms: 2,
            result_count: 0,
            boundary_count: 1,
            max_results: Some(10),
            score_threshold: None,
            include_archived: Some(false),
            query_length: Some(8),
            error: Some(ToolInvokeError::DimensionMismatch {
                expected: 384,
                actual: 256,
            }),
        };
        let json: serde_json::Value =
            serde_json::from_str(&details.to_canonical_json().unwrap()).unwrap();
        assert_eq!(json["error"]["type"], "DimensionMismatch");
        assert_eq!(json["error"]["detail"]["expected"], 384);
        assert_eq!(json["error"]["detail"]["actual"], 256);
    }

    #[test]
    fn canonical_json_orders_keys_alphabetically_at_every_level() {
        // Audit chain hash is over the byte string — sorted-key
        // ordering at every nesting level is load-bearing for hash
        // determinism (BRD §11.9.2).
        let details = ToolInvokeDetails {
            tool: "memory_search",
            duration_ms: 12,
            result_count: 3,
            boundary_count: 1,
            max_results: Some(10),
            score_threshold: Some(0.5),
            include_archived: Some(false),
            query_length: Some(8),
            error: Some(ToolInvokeError::DimensionMismatch {
                expected: 384,
                actual: 256,
            }),
        };
        let raw = details.to_canonical_json().unwrap();

        // Top level: alphabetical = boundary_count, duration_ms,
        // error, include_archived, max_results, query_length,
        // result_count, score_threshold, tool.
        let positions = [
            "boundary_count",
            "duration_ms",
            "error",
            "include_archived",
            "max_results",
            "query_length",
            "result_count",
            "score_threshold",
            "tool",
        ];
        let mut last_pos = 0_usize;
        for key in &positions {
            let needle = format!("\"{key}\"");
            let pos = raw
                .find(&needle)
                .unwrap_or_else(|| panic!("missing key {key} in {raw}"));
            assert!(
                pos >= last_pos,
                "keys must be alphabetical: {key} appeared before previous key in {raw}"
            );
            last_pos = pos;
        }

        // error.detail level: alphabetical = actual, expected.
        let actual_pos = raw.find("\"actual\"").expect("actual present");
        let expected_pos = raw.find("\"expected\"").expect("expected present");
        assert!(
            actual_pos < expected_pos,
            "error.detail keys must be alphabetical (actual before expected) in {raw}"
        );

        // error outer level: alphabetical = detail, type.
        let detail_pos = raw.find("\"detail\"").expect("detail present");
        let type_pos = raw.find("\"type\"").expect("type present");
        assert!(
            detail_pos < type_pos,
            "error outer keys must be alphabetical (detail before type) in {raw}"
        );
    }

    #[test]
    fn from_vault_error_maps_adr_024_table_variants_with_full_detail() {
        // Table rows from ADR-024 lines 762–768 + 787–790.
        let dim = ToolInvokeError::from_vault_error(&VaultError::DimensionMismatch {
            expected: 384,
            actual: 256,
        });
        assert!(matches!(
            dim,
            ToolInvokeError::DimensionMismatch {
                expected: 384,
                actual: 256
            }
        ));

        let denied =
            ToolInvokeError::from_vault_error(&VaultError::AccessDenied("work".to_string()));
        match denied {
            ToolInvokeError::AccessDenied { boundary_attempted } => {
                assert_eq!(boundary_attempted, "work");
            }
            other => panic!("expected AccessDenied, got {other:?}"),
        }

        let integ = ToolInvokeError::from_vault_error(&VaultError::ModelIntegrityFailed {
            file: "model.onnx".to_string(),
            expected: "abc".to_string(),
            actual: "def".to_string(),
        });
        match integ {
            ToolInvokeError::ModelIntegrityFailed {
                file,
                expected,
                actual,
            } => {
                assert_eq!(file, "model.onnx");
                assert_eq!(expected, "abc");
                assert_eq!(actual, "def");
            }
            other => panic!("expected ModelIntegrityFailed, got {other:?}"),
        }
    }

    #[test]
    fn from_vault_error_collapses_unlisted_variants_to_internal() {
        // ADR-024 is silent on Llm, Crypto, Io, etc. — they collapse
        // to ToolInvokeError::Internal with the variant ident as
        // category. Pin the behaviour so future-Claude doesn't
        // silently change the audit-row category.
        let llm = ToolInvokeError::from_vault_error(&VaultError::Llm("oom".to_string()));
        match llm {
            ToolInvokeError::Internal { category, message } => {
                assert_eq!(category, "Llm");
                assert_eq!(message, "oom");
            }
            other => panic!("expected Internal, got {other:?}"),
        }

        let crypto = ToolInvokeError::from_vault_error(&VaultError::Crypto("bad tag".to_string()));
        match crypto {
            ToolInvokeError::Internal { category, message } => {
                assert_eq!(category, "Crypto");
                assert_eq!(message, "bad tag");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }
}
