//! `vault-storage` — persistent storage across LanceDB (vectors), DuckDB (graph),
//! and SQLite/SQLCipher (metadata) with cascading writes.
//!
//! See `Agent Build Specification.txt` §5.2 for the public API specification.
//!
//! ## V0.1 progress
//!
//! - **T0.1.3:** [`MetadataStore`] — encrypted SQLite + tamper-evident audit log
//! - **T0.1.4:** [`LanceVectorStore`] — LanceDB-backed vector store + boundary-leak guard
//! - **T0.1.5 (in flight):** [`DuckDbGraphStore`] — DuckDB graph store with bi-temporal
//!   relationships and ADR-015 boundary-scoped entities
//! - T0.1.6: `StorageBackend` orchestrator with cascading writes + retry queue

#![forbid(unsafe_code)]

pub mod agent_token_store;
pub mod audit;
pub mod boundary_store;
pub mod cascading;
pub mod checkpoint;
pub mod consolidation_state;
pub mod dead_letter;
pub mod divergence;
#[cfg(test)]
pub(crate) mod fault_injection;
pub mod graph_store;
pub mod key;
pub mod metadata_store;
pub(crate) mod migrations;
pub(crate) mod migrations_graph;
pub mod pending_sync;
pub mod recently_deleted;
pub mod report_layout;
pub mod retry_queue;
pub mod retry_worker;
pub mod sealed_object_store;
pub mod vector_store;

pub use agent_token_store::{hash_capability_token, AgentToken};
pub use audit::{
    seal, verify_chain, ActorKind, AuditEvent, AuditEventType, AuditResult, PendingAuditEvent,
    AUDIT_GENESIS_HASH,
};
pub use boundary_store::{BoundaryInfo, MAX_BOUNDARY_DESCRIPTION_LEN};
pub use cascading::{Ack, DegradedMode, StorageBackend, MAX_RETRY_QUEUE_DEPTH};
pub use checkpoint::{
    ChangeType, CheckpointEntry, CheckpointId, CheckpointStatus, CheckpointSummary, RollbackReport,
    CHECKPOINT_PAYLOAD_FORMAT_VERSION, CHECKPOINT_RETENTION,
};
pub use dead_letter::{
    DeadLetter, DeadLetterEntry, NewDeadLetter, Resolution, FAILURE_REASON_MAX_BYTES,
};
pub use divergence::{DivergenceDetector, DivergenceReport, RECENT_WINDOW, SAMPLES_PER_STRATUM};
pub use graph_store::{
    DuckDbGraphStore, GraphStore, TraversalOptions, CROSS_BOUNDARY_RELATION_TYPES,
};
pub use key::SqlCipherKey;
pub use metadata_store::{
    rekey_in_place, verify_sqlcipher_passphrase, MemoryFilter, MetadataStore,
};
pub use pending_sync::{PendingSync, PendingSyncEntry};
pub use recently_deleted::{BinBy, BinEntry, BinKind, APP_CHANGES_PER_HOUR, RECENTLY_DELETED_DAYS};
pub use report_layout::{
    sealed_report_filename, sealed_report_relative_path, REPORTS_DIRNAME,
    REPORT_LEGACY_PLAINTEXT_SUFFIX, REPORT_SEALED_SUFFIX,
};
pub use retry_queue::{
    base_backoff_secs, compute_next_attempt, is_permanent, CascadeOperation, DeadLetterReason,
    FailureOutcome, FixedJitter, JitterSource, NewRetry, RetryEntry, RetryQueue, SeededJitter,
    LAST_ERROR_MAX_BYTES, MAX_ATTEMPTS, PAYLOAD_FORMAT_VERSION,
};
pub use retry_worker::{RetryWorker, StepResult, DEFAULT_POLL_INTERVAL};
pub use sealed_object_store::{
    make_vault_sealed_uri, seal_vault_blob, unseal_vault_blob, SealedFileStoreProvider,
    VAULT_SEALED_SCHEME,
};
pub use vector_store::{LanceVectorStore, VectorStore};
