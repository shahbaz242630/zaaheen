//! Recall-engine commands — the first-run trigger for ADR-087's downloader
//! (ADR-089), and the engine's state (ADR-090).
//!
//! Since ADR-108 (D10) the keeper owns the download: one acquisition per
//! keeper, shared by its own start and this "get it now", so two processes
//! never fetch onto the same file (session-35 review K6). The desktop asks
//! the keeper to start it and shows its progress on the same event channel as
//! before, so the onboarding screen does not change.
//!
//! ## Security shape (BRD §11.7.1 / §11.12 vault-tauri)
//!
//! [`ensure_recall_engine`] takes **no parameters**: no caller-supplied path,
//! filename or URL. The location and the expected hashes are the keeper's
//! own.
//!
//! ## Error shape (BRD §11.7.2)
//!
//! Failures return a short stable CODE, never a message built from the
//! underlying error.
//!
//! ## Why this command writes no audit row
//!
//! BRD §11.9.1 scopes the audit log to vault-state changes and memory or
//! boundary access. Fetching a model file touches neither.
//!
//! ## ADR-086 (white-label)
//!
//! Nothing here names a model, a repository or a runtime. The event channel is
//! `recall-engine://progress` and carries only byte counts.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{Emitter, State};

use crate::guard::Entitlement;
use crate::link::{decoded, KeeperLink, Kind};

/// Event channel the frontend listens on for acquisition progress.
pub const PROGRESS_EVENT: &str = "recall-engine://progress";

/// Stable error code returned to the frontend when acquisition fails.
pub const ERR_ACQUISITION_FAILED: &str = "recall_engine_unavailable";

/// How often the desktop asks the keeper how the download is going.
const POLL: Duration = Duration::from_millis(500);

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
            // A progress bar must never render 103%.
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

/// The keeper's view of the engine.
async fn status(link: &KeeperLink) -> Result<Value, String> {
    let text = link
        .call("admin_engine_status", json!({}), Kind::Read)
        .await?;
    decoded(&text)
}

/// Ensure the recall engine's files are present, reporting progress.
///
/// Idempotent and safe to call concurrently: the keeper runs one download and
/// every caller watches the same one. Returns once the files are present and
/// verified, so onboarding can gate on it; a failed download can be retried
/// by calling again (review B-M4).
///
/// # Errors
///
/// [`ERR_ACQUISITION_FAILED`] on any failure, or a `locked_*` code.
#[tauri::command]
pub async fn ensure_recall_engine(
    app: tauri::AppHandle,
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
) -> Result<(), String> {
    // A locked computer must not download 2.86 GB of models.
    let _entitled = entitlement.require().await?;
    let unavailable = |code: String| {
        tracing::warn!(code, "recall engine acquisition could not be followed");
        ERR_ACQUISITION_FAILED.to_string()
    };
    link.call("admin_engine_fetch", json!({}), Kind::Read)
        .await
        .map_err(unavailable)?;
    loop {
        let now = status(link.inner()).await.map_err(unavailable)?;
        let done = now["downloaded_bytes"].as_u64().unwrap_or(0);
        let total = now["total_bytes"].as_u64().unwrap_or(0);
        // A closed window means nobody is listening: never fatal.
        let _ = app.emit(PROGRESS_EVENT, RecallEngineProgress::new(done, total));
        match now["fetch"].as_str() {
            Some("done") => {
                let _ = app.emit(PROGRESS_EVENT, RecallEngineProgress::new(total, total));
                return Ok(());
            }
            Some("failed") => return Err(ERR_ACQUISITION_FAILED.to_string()),
            _ => tokio::time::sleep(POLL).await,
        }
    }
}

/// Report what the ranking model is doing (ADR-090): one of
/// `vault_app::RerankerState`'s wire strings, from the keeper.
#[tauri::command]
pub async fn recall_engine_state(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
) -> Result<String, String> {
    let _entitled = entitlement.require().await?;
    let now = status(link.inner()).await?;
    Ok(now["state"].as_str().unwrap_or("preparing").to_string())
}

/// Ask the keeper to load the ranking model in the background (ADR-090).
/// Idempotent; returns at once.
#[tauri::command]
pub async fn warm_recall_engine(
    link: State<'_, KeeperLink>,
    entitlement: State<'_, Entitlement>,
) -> Result<bool, String> {
    let _entitled = entitlement.require().await?;
    let text = link
        .call("admin_engine_warm", json!({}), Kind::Read)
        .await?;
    let warming: bool = decoded(&text)?;
    tracing::info!(warming, "ranking model warm-up requested (ADR-090)");
    Ok(warming)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_fetch;

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

    /// One downloader (ADR-108 D10): the desktop never fetches the ranking
    /// model itself.
    #[test]
    fn the_desktop_never_downloads_the_ranking_model_itself() {
        let source = include_str!("engine.rs").replace("\r\n", "\n");
        let code = source
            .split_once("#[cfg(test)]")
            .map_or(source.as_str(), |(c, _)| c);
        assert!(!code.contains("ensure_reranker"));
    }
}
