//! Error types shared across the Memory Vault workspace.
//!
//! Every fallible operation in `vault-*` crates returns [`VaultResult<T>`].
//! The error variants are organised by failure category, not by source crate,
//! so callers can pattern-match on intent rather than implementation detail.
//!
//! See `Agent Build Specification.txt` §5.1 for the canonical error catalogue.

use thiserror::Error;

/// The single error type used across the entire Memory Vault workspace.
///
/// Variants describe failure *categories*. Each variant carries a `String`
/// message rather than nesting source-crate error types — this keeps
/// downstream crates from depending on, say, `rusqlite::Error` just to
/// match on a storage failure.
///
/// New variants must be added when a genuinely new failure category emerges.
/// Avoid "OtherError(String)" or similar grab-bags — they make pattern
/// matching meaningless.
#[derive(Error, Debug)]
pub enum VaultError {
    /// Persistent storage failure (SQLite, LanceDB, DuckDB, encrypted blobs).
    #[error("storage error: {0}")]
    Storage(String),

    /// Embedding generation failed (model loading, tokenisation, inference).
    #[error("embedding error: {0}")]
    Embedding(String),

    /// Local LLM inference failed (model loading, prompt execution, structured output).
    #[error("llm error: {0}")]
    Llm(String),

    /// Retrieval pipeline failure (query classification, strategy execution, reranking).
    #[error("retrieval error: {0}")]
    Retrieval(String),

    /// Sleep-cycle consolidation failed (clustering, merging, decay, checkpointing).
    #[error("consolidation error: {0}")]
    Consolidation(String),

    /// MCP protocol or transport failure (adapter, server, tool dispatch).
    #[error("mcp error: {0}")]
    Mcp(String),

    /// Cross-device sync failure (CRDT, encryption, cloud API).
    #[error("sync error: {0}")]
    Sync(String),

    /// Third-party connector failure (OAuth, fetch, extraction).
    #[error("connector error: {0}")]
    Connector(String),

    /// Authentication failure (Clerk session, capability token, device pairing).
    #[error("authentication error: {0}")]
    Auth(String),

    /// Mandatory access control denied the operation. The boundary check
    /// failed — see BRD §11.4.3. **Always returned generically** (no info leak).
    #[error("access denied: {0}")]
    AccessDenied(String),

    /// Input failed validation at a public API boundary (length, charset,
    /// schema, range). See BRD §11.7.1.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Embedding vector dimensionality did not match the configured store
    /// dimension. Carried as a structured variant (rather than as a string
    /// inside [`Self::Storage`]) because the cascading retry queue's
    /// `is_permanent` classifier in `vault-storage` matches on it
    /// exhaustively to dead-letter on attempt 1 — a dimension mismatch is
    /// always a contract / config error, never transient. See T0.1.6_PLAN
    /// Q2 and ADR-009 amendment.
    #[error("dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch {
        /// Dimension the store was configured for.
        expected: usize,
        /// Dimension actually presented by the caller.
        actual: usize,
    },

    /// Model or tokenizer file integrity check failed at provider construction.
    /// Carried as a structured variant so vault-app / vault-tauri can pattern-
    /// match exhaustively to surface a fatal-error dialog (per T0.1.7 plan
    /// Q5: integrity failure is fatal at startup, no degraded mode). The
    /// `file` field names which artefact failed (so the dialog can be specific:
    /// "model" vs "tokenizer"); `expected` and `actual` are hex-encoded
    /// SHA-256 strings so operators can verify against the canonical hash
    /// in MODEL_PROVENANCE.md. See ADR-020 (lands in T0.1.7 Phase 2).
    #[error("model integrity check failed for {file}: expected {expected}, got {actual}")]
    ModelIntegrityFailed {
        /// Logical name of the artefact that failed (e.g. "model", "tokenizer").
        file: String,
        /// Expected SHA-256 hex string (compiled into the binary).
        expected: String,
        /// Actual SHA-256 hex string of the file on disk.
        actual: String,
    },

    /// An OPTIONAL model component is not present on disk, so the provider
    /// that needs it could not be constructed. The caller MAY degrade to a
    /// path that does not need it.
    ///
    /// **Deliberately distinct from [`Self::ModelIntegrityFailed`], and the
    /// distinction is the whole point (ADR-089).** Absent means "the bytes
    /// have not arrived yet" — the ordinary state of a first-run install
    /// before [`vault_tauri::model_fetch`] has fetched them, and of any
    /// install whose optional models were never downloaded. Integrity-failed
    /// means "the bytes are here and they are WRONG", which is a tamper
    /// signal and stays fatal. Collapsing the two would mean either erroring
    /// on an ordinary pre-download state (taking recall down for a missing
    /// nice-to-have) or silently degrading past a tampered model file. Both
    /// are unacceptable, so the two states carry different variants.
    ///
    /// Verify-before-use (BRD §11.12 vault-embedding: *"Model files signed and
    /// verified before loading"*) is untouched: a component that is absent is
    /// never loaded, so there is nothing unverified to use.
    #[error("model component unavailable: {component}")]
    ModelUnavailable {
        /// Logical name of the absent component (e.g. "reranker-model").
        component: String,
    },

    /// The requested resource (memory, entity, boundary) does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// Cryptographic operation failed (key derivation, encryption, decryption,
    /// signature verification). Authentication-tag failures land here.
    #[error("crypto error: {0}")]
    Crypto(String),

    /// Configuration is missing or malformed (config file, env vars, model paths).
    #[error("config error: {0}")]
    Config(String),

    /// Underlying I/O failure (filesystem, network) that doesn't fit a more
    /// specific category. Wraps [`std::io::Error`] for context.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization or deserialization failed (JSON, CRDT payload).
    #[error("serde error: {0}")]
    Serde(String),

    /// Cascading retry worker failed to spawn during application
    /// startup. T0.1.10 Phase 2a: distinct from `McpBindFailed` so
    /// callers can pattern-match for startup-fatal vs degraded
    /// reporting (per the user pre-flag at Phase 2 plan-paragraph
    /// review — "minimum WorkerSpawnFailed + McpBindFailed as separate
    /// pattern-matchable variants, NOT a generic StartupFailed bucket").
    ///
    /// **Currently unreachable as of Phase 2a**: `RetryWorker::new` is
    /// infallible struct construction and `tokio::spawn` is infallible.
    /// Reserved for future fallible spawn paths — e.g., bounded-thread-
    /// pool spawning, config-driven worker startup, or worker-side
    /// resource acquisition that returns `Result`.
    ///
    /// **Re-evaluation milestone**: if no consumer surfaces by V0.2
    /// alpha cut, remove and re-add when concretely needed. Tracked
    /// in HANDOFF.md tech-debt; concrete-vs-hypothetical test applied
    /// to the variant's own existence.
    #[error("worker spawn failed: {0}")]
    WorkerSpawnFailed(String),

    /// MCP server bind failed during application startup. T0.1.10
    /// Phase 2: distinct from `WorkerSpawnFailed` per the same
    /// pattern-matching pre-flag.
    #[error("mcp server bind failed: {0}")]
    McpBindFailed(String),

    /// A consolidation run was requested but another run is already in
    /// progress for this vault (the on-disk lockfile is held by another
    /// process or this process). Carried as a structured variant so
    /// vault-cli / vault-tauri can surface a "consolidator busy" exit code
    /// or UI message distinct from a generic configuration / I/O failure.
    /// See [[locked-next-arc-t03x]] Step 4 wiring + ADR-051 invalidate
    /// semantics for the V0.2 cross-process lockfile contract.
    #[error("consolidator busy: {0}")]
    ConsolidatorBusy(String),

    /// A consolidation run exceeded its hard time budget (30 min by
    /// default per the locked-next-arc Step 4 operational-safety contract)
    /// and was cancelled. Distinct from [`Self::Consolidation`] so callers
    /// can decide whether to retry next nightly cycle (timeout — likely
    /// transient resource pressure) or alert the operator
    /// (Consolidation — a real correctness or storage fault). Payload
    /// carries the elapsed-at-cancellation duration in seconds.
    #[error("consolidator timeout: hard budget exceeded after {0}s")]
    ConsolidatorTimeout(u64),

    /// At-rest-key provenance failed: the OS keychain could not be read,
    /// the entry could not be created on first run, or the platform is
    /// not supported by the V0.2 Phase 1 keychain wiring.
    ///
    /// Carried as a structured variant (rather than nested into
    /// [`Self::Crypto`] or [`Self::Config`]) so vault-tauri / vault-cli
    /// can pattern-match exhaustively to surface a fatal-dialog flow
    /// distinct from generic crypto / config errors. Per ADR-040 +
    /// ADR-040 amendment, any non-`NotFound` keychain error fails closed
    /// — vault-app exits non-zero rather than proceeding with a
    /// partially-recovered or empty key.
    ///
    /// `NotFound` is NOT carried via this variant. `vault_app::keychain`
    /// treats a confirmed "no entry" as a fresh install only when no data
    /// sealed under a key exists (ADR-SEC-029 D4); otherwise it returns
    /// [`Self::VaultKey`] with [`VaultKeyFailure::Missing`].
    ///
    /// Since ADR-SEC-029 this variant means one thing to the user: the
    /// credential store itself is failing (startup message 2).
    #[error("keychain provenance error: {0}")]
    KeychainProvenance(String),

    /// The vault's master key could not be opened for a reason the user can
    /// act on (ADR-SEC-029 U1). Each [`VaultKeyFailure`] has its own startup
    /// message. It carries no text: the detail is logged where it happens,
    /// and nothing here can name a path, a credential or key material.
    #[error("vault key: {0}")]
    VaultKey(VaultKeyFailure),

    /// The vault's folder could not be found (ADR-105 L1). No process ever
    /// creates a folder or falls back to a default in answer to this: that is
    /// how a second, empty vault gets made. Carries no path; the detail is
    /// logged where it happens.
    #[error("vault location: {0}")]
    VaultLocation(VaultLocationFailure),

    /// Registering, querying, or removing an OS-level scheduled task failed
    /// (Windows Task Scheduler, macOS launchd, Linux systemd/cron).
    ///
    /// Carried as its own category — scheduling is a first-class subsystem
    /// (`vault-scheduler`) per BRD §2, not a config or I/O sub-case — so
    /// vault-tauri / vault-cli can surface a scheduler-specific message
    /// distinct from generic configuration / I/O failures. The wrapped
    /// message already carries the failing backend's context via
    /// `SchedulerError`'s display. See ADR-092.
    ///
    /// Spec-validation failures (illegal task id, control character in an
    /// argument) are deliberately NOT carried here — they map onto
    /// [`Self::InvalidInput`] because they are input-validation failures at
    /// the API boundary (ADR-SEC-005), caught before any OS call is made.
    #[error("scheduler error: {0}")]
    Scheduler(String),
}

/// Why the vault's master key could not be opened (ADR-SEC-029 U1).
///
/// A credential store that is itself failing is
/// [`VaultError::KeychainProvenance`]; these are the other four cases, each
/// with its own startup message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultKeyFailure {
    /// There is no key on this Windows account, but data sealed under a key
    /// exists (D4). A new key would not open it, so none is made.
    Missing,
    /// The vault folder, or the folder holding the key's lock and marker,
    /// could not be checked or cleared.
    FolderUnavailable,
    /// A key is stored but is not one this app could have written (the
    /// wrong size). Nothing is overwritten.
    Unusable,
    /// Another process held the key lock for longer than the wait (for
    /// example, "Delete everything" running).
    Busy,
}

impl std::fmt::Display for VaultKeyFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Missing => "the key is missing while keyed data exists",
            Self::FolderUnavailable => "the vault or key folder could not be checked or cleared",
            Self::Unusable => "the stored key is not usable",
            Self::Busy => "the key lock is held by another process",
        })
    }
}

/// Why the vault's folder could not be found (ADR-105 L1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultLocationFailure {
    /// No location has been recorded yet: the first run of this build, which
    /// only the setup (ADR-105 L2) may answer — a relay asks for a keeper.
    Unset,
    /// A location is recorded, but its folder, its `.vault-id`, or the ID
    /// itself is not there or not right (an unplugged drive, another stick at
    /// the same letter, a damaged record).
    Missing,
}

impl std::fmt::Display for VaultLocationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unset => "no vault location has been recorded yet",
            Self::Missing => "the recorded vault folder is missing or is not this vault",
        })
    }
}

/// Standard result alias used throughout the workspace.
pub type VaultResult<T> = Result<T, VaultError>;

impl From<serde_json::Error> for VaultError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serde(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_prefixed_by_category() {
        assert!(VaultError::Storage("disk full".into())
            .to_string()
            .starts_with("storage error:"));
        assert!(
            VaultError::AccessDenied("boundary 'work' not authorized".into())
                .to_string()
                .starts_with("access denied:")
        );
    }

    #[test]
    fn io_error_converts_via_from() {
        let io_err = std::io::Error::other("simulated");
        let vault_err: VaultError = io_err.into();
        assert!(matches!(vault_err, VaultError::Io(_)));
    }

    #[test]
    fn serde_error_converts_via_from() {
        let serde_err = serde_json::from_str::<serde_json::Value>("{invalid").unwrap_err();
        let vault_err: VaultError = serde_err.into();
        assert!(matches!(vault_err, VaultError::Serde(_)));
    }

    #[test]
    fn dimension_mismatch_is_structured_and_displays_both_dims() {
        let err = VaultError::DimensionMismatch {
            expected: 384,
            actual: 256,
        };
        let s = err.to_string();
        assert!(s.contains("384"), "display should mention expected: {s}");
        assert!(s.contains("256"), "display should mention actual: {s}");
        assert!(matches!(
            err,
            VaultError::DimensionMismatch {
                expected: 384,
                actual: 256
            }
        ));
    }

    #[test]
    fn consolidator_busy_displays_category_prefix_and_carries_context() {
        let err = VaultError::ConsolidatorBusy(
            "lockfile held by pid=1234 since 2026-05-26T03:00:01Z".into(),
        );
        let s = err.to_string();
        assert!(
            s.starts_with("consolidator busy:"),
            "display should start with 'consolidator busy:' prefix; got: {s}"
        );
        assert!(
            s.contains("pid=1234"),
            "display should carry the supplied context; got: {s}"
        );
        assert!(matches!(err, VaultError::ConsolidatorBusy(_)));
    }

    #[test]
    fn consolidator_timeout_carries_elapsed_seconds_in_message() {
        let err = VaultError::ConsolidatorTimeout(1800);
        let s = err.to_string();
        assert!(
            s.starts_with("consolidator timeout:"),
            "display should start with 'consolidator timeout:' prefix; got: {s}"
        );
        assert!(
            s.contains("1800"),
            "display should mention the elapsed seconds; got: {s}"
        );
        assert!(matches!(err, VaultError::ConsolidatorTimeout(1800)));
    }

    #[test]
    fn model_integrity_failed_is_structured_and_displays_all_fields() {
        let err = VaultError::ModelIntegrityFailed {
            file: "model".into(),
            expected: "abc123".into(),
            actual: "def456".into(),
        };
        let s = err.to_string();
        assert!(s.contains("model"), "display should name the file: {s}");
        assert!(s.contains("abc123"), "display should mention expected: {s}");
        assert!(s.contains("def456"), "display should mention actual: {s}");
        let matched = matches!(err, VaultError::ModelIntegrityFailed { .. });
        assert!(matched);
    }

    #[test]
    fn vault_key_failure_is_structured_and_prefixed() {
        let err = VaultError::VaultKey(VaultKeyFailure::Missing);
        assert!(err.to_string().starts_with("vault key:"), "{err}");
        assert!(matches!(
            err,
            VaultError::VaultKey(VaultKeyFailure::Missing)
        ));
        for kind in [
            VaultKeyFailure::Missing,
            VaultKeyFailure::FolderUnavailable,
            VaultKeyFailure::Unusable,
            VaultKeyFailure::Busy,
        ] {
            assert!(!kind.to_string().is_empty());
        }
    }

    #[test]
    fn vault_location_failure_is_structured_and_prefixed() {
        for kind in [VaultLocationFailure::Unset, VaultLocationFailure::Missing] {
            let err = VaultError::VaultLocation(kind);
            assert!(err.to_string().starts_with("vault location:"), "{err}");
        }
        assert_ne!(
            VaultLocationFailure::Unset.to_string(),
            VaultLocationFailure::Missing.to_string()
        );
    }

    #[test]
    fn scheduler_error_displays_category_prefix() {
        assert!(VaultError::Scheduler("schtasks exited 1".into())
            .to_string()
            .starts_with("scheduler error:"));
        assert!(matches!(
            VaultError::Scheduler("x".into()),
            VaultError::Scheduler(_)
        ));
    }

    #[test]
    fn model_unavailable_is_structured_and_names_the_component() {
        let err = VaultError::ModelUnavailable {
            component: "reranker-model".into(),
        };
        let s = err.to_string();
        assert!(
            s.contains("reranker-model"),
            "display should name the absent component: {s}"
        );
        let matched = matches!(err, VaultError::ModelUnavailable { .. });
        assert!(matched);
    }

    #[test]
    fn absent_and_tampered_models_are_different_variants_per_adr_089() {
        // THE property the degrade path depends on. `ModelUnavailable` means
        // "not downloaded yet" and callers may degrade past it;
        // `ModelIntegrityFailed` means "present and WRONG" and must stay
        // fatal. If these ever collapse into one variant, a tampered model
        // file would silently take the degrade path.
        let absent = VaultError::ModelUnavailable {
            component: "reranker-model".into(),
        };
        let tampered = VaultError::ModelIntegrityFailed {
            file: "reranker-model".into(),
            expected: "aa".into(),
            actual: "bb".into(),
        };
        assert!(matches!(absent, VaultError::ModelUnavailable { .. }));
        assert!(
            !matches!(tampered, VaultError::ModelUnavailable { .. }),
            "an integrity failure must NEVER classify as merely unavailable"
        );
    }
}
