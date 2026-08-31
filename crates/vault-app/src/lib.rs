//! `vault-app` — application core. The only place where concrete implementations
//! of all module traits are wired together. All other crates depend on `dyn Trait`,
//! never on concrete types like `BgeSmallProvider` or `Phi4MiniProvider`.
//!
//! See `Agent Build Specification.txt` §5.10 for the public API specification.
//!
//! ## V0.1 progress
//!
//! - **T0.1.9 Phase 2 Step 6:** [`VaultAdapter`] — production impl of
//!   `vault_mcp::Adapter` (4 deps: `Arc<dyn Retriever>` +
//!   `Arc<dyn EmbeddingProvider>` + `StorageBackend` + `MetadataStore`).
//!   Per Q2 carry-forward (`T0.1.9_PLAN.md`), this is the
//!   minimal-just-the-type landing — Application / config / lifecycle
//!   bind together at T0.1.10.
//! - **T0.1.10 Phase 1:** [`Application::new`] — minimum composition-
//!   root construction surface. Wires the full V0.1 dep graph
//!   (`BgeSmallProvider × StorageBackend × LanceVectorStore ×
//!   DuckDbGraphStore × MetadataStore × SemanticRetriever ×
//!   VaultAdapter`) against real backends. No lifecycle, no MCP server
//!   bind, no retry-worker spawn — those land in Phase 2. Exercised
//!   end-to-end by `tests/integration_smoke.rs` per the four pre-
//!   declared stop-and-escalate triggers (HANDOFF.md session-open
//!   Decision 3).

#![forbid(unsafe_code)]

mod adapter;
mod application;
mod config;
mod consolidator_lock;
pub mod erasure;
pub mod install_paths;
pub mod keychain;
pub mod logging;
pub mod maintenance_state;
pub mod model_fetch;
mod normalization;
pub mod process_exit;
pub mod signal_source;

pub use adapter::VaultAdapter;
pub use application::{Application, ApplicationHandle, RerankerState};
pub use config::AppConfig;
pub use consolidator_lock::{ConsolidatorLock, VAULT_LOCKFILE_NAME};
pub use erasure::{erase_vault, ErasureOutcome};
pub use process_exit::{LiveProcessExit, ProcessExit};
pub use signal_source::{LiveSignalSource, SignalSource};
/// Re-export `EMBEDDING_DIM` from `vault_embedding` so vault-tauri (which
/// doesn't depend on vault-embedding directly) can use the same canonical
/// constant when calling `vault_storage::migration::migrate_v0_1_to_sealed_if_needed`
/// at startup step 5b. Single source of truth for the BGE dimension.
pub use vault_embedding::EMBEDDING_DIM;
