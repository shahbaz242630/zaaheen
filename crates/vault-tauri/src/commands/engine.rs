//! Recall-engine acquisition command — the first-run trigger for ADR-087's
//! downloader (ADR-089).
//!
//! ADR-087 built the transport (`model_fetch::ensure_reranker`) but nothing
//! called it, so an installed user never obtained the ranking model. This is
//! the trigger, plus the progress channel the "Quiet" onboarding shows it
//! through.
//!
//! ## Security shape (BRD §11.7.1 / §11.12 vault-tauri: "All IPC commands
//! validated")
//!
//! [`ensure_recall_engine`] takes **no parameters**. The download location is
//! derived from the app's own data directory at startup and held in managed
//! state; the URL and expected hashes are compile-time constants. There is
//! deliberately no caller-supplied path, filename or URL, so this command
//! adds no arbitrary-file-write and no server-choice surface — the strongest
//! form of "validated input" is input that does not exist.
//!
//! ## Error shape (BRD §11.7.2)
//!
//! Failures return a short stable CODE, never a message built from the
//! underlying error. Paths, URLs and internals stay in the local log where
//! operators can read them; the frontend maps the code to its own copy. This
//! also keeps ADR-086 intact — an error string built from a URL would leak the
//! model repository name straight into the UI.
//!
//! ## Why this command writes no audit row
//!
//! Every other command in this module appends a `TauriCommandInvoke` row
//! (ADR-024 amendment). This one does not, deliberately: BRD §11.9.1 scopes
//! the audit log to vault-state changes and memory/boundary access. Fetching a
//! model file touches no vault state and reads no memory, so an audit row here
//! would be noise in a chain whose value depends on every entry meaning
//! something. It also has no `Application` handle by design — acquisition must
//! work before the engine is ready.
//!
//! ## ADR-086 (white-label)
//!
//! Nothing here names a model, a repository or a runtime. The event channel is
//! `recall-engine://progress` and carries only byte counts.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{Emitter, State};

use crate::guard::Entitlement;
use crate::model_fetch;

/// Event channel the frontend listens on for acquisition progress.
pub const PROGRESS_EVENT: &str = "recall-engine://progress";

/// Stable error code returned to the frontend when acquisition fails.
///
/// Deliberately opaque: the frontend owns the user-facing wording, and no
/// detail from the underlying error crosses the IPC boundary (§11.7.2).
pub const ERR_ACQUISITION_FAILED: &str = "recall_engine_unavailable";

/// Progress payload emitted on [`PROGRESS_EVENT`].
#[derive(Debug, Clone, Copy, Serialize)]
pub struct RecallEngineProgress {
    /// Bytes accounted for so far across every file.
    pub downloaded_bytes: u64,
    /// Total bytes the acquisition covers.
    pub total_bytes: u64,
    /// Whole-number percent, pre-computed so the UI cannot divide by zero.
    pub percent: u8,
}

impl RecallEngineProgress {
    fn new(downloaded_bytes: u64, total_bytes: u64) -> Self {
        let percent = if total_bytes == 0 {
            100
        } else {
            // Saturating: a server that sends more than the expected size is
            // rejected by the downloader's Content-Length gate long before
            // here, but a progress bar must never render 103%.
            let pct = downloaded_bytes.saturating_mul(100) / total_bytes;
            pct.min(100) as u8
        };
        Self {
            downloaded_bytes,
            total_bytes,
            percent,
        }
    }
}

/// Managed state for first-run acquisition.
///
/// The [`tokio::sync::OnceCell`] gives two properties the onboarding flow
/// needs and would otherwise have to build itself:
///
/// 1. **Concurrent callers share one download.** The frontend starts the fetch
///    on the welcome screen and awaits the same operation again when
///    onboarding completes; without this, a fast typist would trigger a second
///    1.15 GB transfer racing the first onto the same `.partial` path.
/// 2. **A failed attempt stays retryable.** `get_or_try_init` leaves the cell
///    cold on `Err`, so a dropped connection does not permanently poison the
///    session — the retry path required by ADR-087's founder decision falls
///    out of the type rather than needing its own flag.
pub struct RecallEngineFetch {
    models_dir: PathBuf,
    once: tokio::sync::OnceCell<()>,
}

impl RecallEngineFetch {
    /// Bind acquisition to `models_dir` (`<app data dir>/models`).
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            once: tokio::sync::OnceCell::new(),
        }
    }

    /// Whether acquisition has already completed successfully this session.
    pub fn is_complete(&self) -> bool {
        self.once.initialized()
    }
}

/// Ensure the recall engine's files are present, reporting progress.
///
/// Idempotent and safe to call concurrently: the first call performs the work
/// and later calls await the same operation. Returns once every file is
/// present AND integrity-verified, so a caller that awaits this knows the
/// engine is genuinely ready — which is what lets onboarding gate on it.
///
/// # Errors
///
/// [`ERR_ACQUISITION_FAILED`] on any failure. Details go to the log, not to
/// the caller (§11.7.2).
#[tauri::command]
pub async fn ensure_recall_engine(
    app: tauri::AppHandle,
    state: State<'_, RecallEngineFetch>,
    entitlement: State<'_, Entitlement>,
) -> Result<(), String> {
    // A locked computer must not download 2.86 GB of models.
    let _entitled = entitlement.require().await?;
    let models_dir = state.models_dir.clone();

    let result = state
        .once
        .get_or_try_init(|| async {
            tracing::info!("recall engine acquisition starting");
            model_fetch::ensure_reranker_with_progress(&models_dir, |p| {
                // Emit failures are non-fatal: a closed window means nobody is
                // listening, which must not abort a download in flight.
                if let Err(e) = app.emit(
                    PROGRESS_EVENT,
                    RecallEngineProgress::new(p.downloaded_bytes, p.total_bytes),
                ) {
                    tracing::debug!(error = %e, "progress emit failed (no listener?)");
                }
            })
            .await
            .map(|_| ())
        })
        .await;

    match result {
        Ok(()) => {
            tracing::info!("recall engine ready");
            // Final 100% so a listener that attached late still lands on
            // complete rather than sitting at whatever it last saw.
            let _ = app.emit(
                PROGRESS_EVENT,
                RecallEngineProgress::new(
                    model_fetch::RERANKER_TOTAL_BYTES,
                    model_fetch::RERANKER_TOTAL_BYTES,
                ),
            );
            Ok(())
        }
        Err(e) => {
            tracing::error!(error = %e, "recall engine acquisition failed");
            Err(ERR_ACQUISITION_FAILED.to_string())
        }
    }
}

/// Report what the ranking model is currently doing (ADR-090).
///
/// Returns one of [`vault_app::RerankerState`]'s wire strings. The frontend
/// branches on these to decide whether to show a "getting ready" state at all
/// — which is why `not_configured` and `unavailable` are distinct from
/// `preparing`: only the last is a wait that ends.
///
/// Cheap and side-effect free. Notably it does NOT trigger a load: asking
/// whether the engine is ready must not start the very work being asked about,
/// or a status poll would kick off a 1.2 GB read on every call.
///
/// No audit row, for the same reason [`ensure_recall_engine`] writes none —
/// this touches no vault state and reads no memory (BRD §11.9.1).
#[tauri::command]
pub async fn recall_engine_state(
    app: State<'_, vault_app::Application>,
    entitlement: State<'_, Entitlement>,
) -> Result<String, String> {
    let _entitled = entitlement.require().await?;
    Ok(app.reranker_state().as_wire_str().to_string())
}

/// Request a background load of the ranking model (ADR-090).
///
/// Idempotent and safe to call repeatedly: the load runs at most once, and a
/// failed attempt leaves the cell cold so a later call retries.
///
/// The frontend calls this after [`ensure_recall_engine`] resolves. That is
/// the case `main.rs`'s startup warm-up cannot cover: on a genuine first run
/// the files are still downloading when the app boots, so the startup attempt
/// correctly fails fast and the model would otherwise stay cold until the
/// user's first search — the exact ~24 s stall this ADR removes.
///
/// Returns immediately; the caller polls [`recall_engine_state`] for the
/// outcome. Infallible by design — "no reranker configured" is a legitimate
/// vault shape, not an error.
#[tauri::command]
pub async fn warm_recall_engine(
    app: State<'_, vault_app::Application>,
    entitlement: State<'_, Entitlement>,
) -> Result<bool, String> {
    let _entitled = entitlement.require().await?;
    let warming = app.spawn_reranker_warmup();
    tracing::info!(warming, "ranking model warm-up requested (ADR-090)");
    Ok(warming)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_is_bounded_and_never_divides_by_zero() {
        assert_eq!(RecallEngineProgress::new(0, 0).percent, 100);
        assert_eq!(RecallEngineProgress::new(0, 100).percent, 0);
        assert_eq!(RecallEngineProgress::new(50, 100).percent, 50);
        assert_eq!(RecallEngineProgress::new(100, 100).percent, 100);
        // Over-delivery cannot render >100%.
        assert_eq!(RecallEngineProgress::new(250, 100).percent, 100);
    }

    #[test]
    fn percent_does_not_overflow_on_realistic_byte_counts() {
        // 1.15 GB * 100 overflows u32 — pin that the arithmetic is done in a
        // width that holds it.
        let p = RecallEngineProgress::new(1_192_779_696, model_fetch::RERANKER_TOTAL_BYTES);
        assert!(p.percent <= 100);
        assert!(
            p.percent > 90,
            "the model alone is >90% of the total; got {}",
            p.percent
        );
    }

    #[test]
    fn error_code_leaks_no_internals_per_11_7_2() {
        // The code crossing the IPC boundary must not carry a path, a URL, a
        // repository name or a model name.
        let code = ERR_ACQUISITION_FAILED.to_lowercase();
        for forbidden in [
            "http", "hugging", "qwen", "onnx", "rerank", "c:\\", "/", ".json",
        ] {
            assert!(
                !code.contains(forbidden),
                "error code must not leak '{forbidden}'; got {code}"
            );
        }
    }

    #[test]
    fn progress_event_channel_names_no_stack_component_per_adr_086() {
        let ev = PROGRESS_EVENT.to_lowercase();
        for forbidden in ["qwen", "onnx", "bge", "phi", "gguf", "rerank", "llama"] {
            assert!(
                !ev.contains(forbidden),
                "ADR-086: event name must not name the stack ('{forbidden}'); got {ev}"
            );
        }
    }

    #[test]
    fn a_fresh_fetch_state_is_not_complete() {
        let state = RecallEngineFetch::new(PathBuf::from("/tmp/models"));
        assert!(
            !state.is_complete(),
            "acquisition must not report complete before it has run"
        );
    }
}
