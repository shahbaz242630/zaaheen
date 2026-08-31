//! First-run model acquisition for the desktop app (BRD §5.11, ADR-087).
//!
//! BRD §5.11 puts installer concerns — *"model download, first-run setup"* and
//! the *"Stub installer pattern: ~10MB initial install, model download on first
//! launch"* — at the Tauri layer. This module is that layer for the Qwen3
//! reranker.
//!
//! ## Why the reranker downloads rather than ships in the installer
//!
//! The reranker is 1.15 GB. Bundling it would put the installer an order of
//! magnitude over the ~10 MB §5.11 calls for, and every model revision would
//! force a full re-download of the whole installer. Phi-4 already established
//! the download-on-demand precedent (ADR-043); this makes the reranker
//! consistent with it rather than inventing a second acquisition story.
//!
//! ## Division of responsibility
//!
//! - **Integrity** (the expected SHA-256 of each file) lives in
//!   `vault-embedding`, next to the code that consumes the files. This module
//!   does not restate hashes — it references the pinned constants, so there is
//!   exactly one place a hash can be wrong.
//! - **Acquisition** (where the bytes come from) lives here, because the URL is
//!   an installer concern, not an inference concern. `vault-embedding` stays a
//!   pure inference crate that is handed paths.
//!
//! ## Progress
//!
//! [`ensure_reranker_with_progress`] reports aggregate progress across both
//! files so first launch never looks frozen behind a silent 1.15 GB transfer.
//! The plain [`ensure_reranker`] keeps the silent form for callers with no UI
//! (tests, scripts). This module remains the transport — the presentation of
//! that progress belongs to the Tauri command layer and the frontend.

use std::path::{Path, PathBuf};

use vault_embedding::integrity::{QWEN3_RERANKER_MODEL_SHA256, QWEN3_RERANKER_TOKENIZER_SHA256};
use vault_llm::model_loader::ensure_model_at_path_with_progress;
use vault_llm::{Phi4MiniConfig, VaultLlmResult};

/// Directory name the reranker files live under, inside the app's models dir.
///
/// Deliberately matches the dev-fixture directory name
/// (`crates/vault-embedding/test-fixtures/qwen3-reranker-0.6b-seq-cls/`) so a
/// developer's existing fixtures and an installed user's download have the same
/// shape, and the air-gap path (drop the files in by hand) works identically in
/// both.
pub const RERANKER_SUBDIR: &str = "qwen3-reranker-0.6b-seq-cls";

/// File name of the reranker ONNX model, both on the Hub and on disk.
pub const RERANKER_MODEL_FILE: &str = "model.onnx";

/// File name of the reranker tokenizer, both on the Hub and on disk.
pub const RERANKER_TOKENIZER_FILE: &str = "tokenizer.json";

/// Hugging Face `/resolve/` URL for the reranker ONNX model.
///
/// Repo: `shawnw3i/Qwen3-Reranker-0.6B-seq-cls-ONNX` (Apache-2.0) — the same
/// provenance already recorded against the pinned hash in
/// `vault_embedding::integrity`. Verified 2026-07-20 against the live repo
/// listing: `model.onnx` sits at the repo ROOT (not under `onnx/`) and is
/// 1,192,779,696 bytes, byte-identical in size to the validated fixture.
///
/// `/resolve/` is the file-download endpoint Hugging Face documents for
/// application clients and rate-limits most generously; never use the metadata
/// API for this.
pub const RERANKER_MODEL_URL: &str =
    "https://huggingface.co/shawnw3i/Qwen3-Reranker-0.6B-seq-cls-ONNX/resolve/main/model.onnx";

/// Hugging Face `/resolve/` URL for the reranker tokenizer. See
/// [`RERANKER_MODEL_URL`] for provenance.
pub const RERANKER_TOKENIZER_URL: &str =
    "https://huggingface.co/shawnw3i/Qwen3-Reranker-0.6B-seq-cls-ONNX/resolve/main/tokenizer.json";

/// Expected size of the reranker ONNX model, in bytes.
///
/// Feeds the downloader's Content-Length sanity check, which aborts early when
/// the response is wildly the wrong size (the signature of a redirect-to-HTML
/// or a repo that swapped the file). Confirmed against the live Hub listing.
pub const RERANKER_MODEL_BYTES: u64 = 1_192_779_696;

/// Expected size of the reranker tokenizer, in bytes. See
/// [`RERANKER_MODEL_BYTES`].
pub const RERANKER_TOKENIZER_BYTES: u64 = 11_422_654;

/// Compile-time sanity bounds on the byte constants above.
///
/// These began as runtime test assertions; clippy correctly pointed out that
/// comparing two constants at runtime proves nothing, and const blocks are the
/// right home. Promoting them is strictly stronger — a transposed or truncated
/// digit now fails the BUILD rather than a test run.
///
/// This is not a substitute for the SHA-256 gate, which is what actually
/// guarantees the right bytes. It guards a narrower failure: a mangled constant
/// widening the downloader's Content-Length sanity window until it accepts
/// anything.
const _: () = {
    assert!(
        RERANKER_MODEL_BYTES > 1_000_000_000,
        "reranker ONNX is ~1.15 GB — a smaller bound would let a truncated \
         download through the Content-Length check"
    );
    assert!(
        RERANKER_TOKENIZER_BYTES > 1_000_000 && RERANKER_TOKENIZER_BYTES < 100_000_000,
        "reranker tokenizer is ~11 MB"
    );
    assert!(
        RERANKER_MODEL_BYTES > RERANKER_TOKENIZER_BYTES,
        "model/tokenizer byte constants look swapped"
    );
};

/// Resolved on-disk locations of the reranker's two files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerankerPaths {
    /// The ONNX model file.
    pub model: PathBuf,
    /// The tokenizer file.
    pub tokenizer: PathBuf,
}

/// Compute where the reranker files belong under `models_dir`, without
/// touching the filesystem or the network.
///
/// Split out from [`ensure_reranker`] so startup can ask "where would these
/// be?" — for a readiness check or a UI prompt — without triggering a 1.15 GB
/// download as a side effect.
pub fn reranker_paths_in(models_dir: &Path) -> RerankerPaths {
    let dir = models_dir.join(RERANKER_SUBDIR);
    RerankerPaths {
        model: dir.join(RERANKER_MODEL_FILE),
        tokenizer: dir.join(RERANKER_TOKENIZER_FILE),
    }
}

/// Ensure both reranker files are present under `models_dir` with verified
/// SHA-256, downloading whichever are missing or corrupt.
///
/// Inherits ADR-043's semantics wholesale via [`ensure_model_at_path`]: a
/// present-and-matching file short-circuits with no HTTP call (which is also
/// the air-gap path — a user may place the files by hand), a present-but-wrong
/// file is deleted and refetched, and a download only lands at its final name
/// after its hash verifies.
///
/// # Errors
///
/// Propagates [`vault_llm::VaultLlmError`] on network failure, integrity
/// mismatch, or I/O failure (including a full disk). Fails closed — a partial
/// or unverified file is never left at the final path for the reranker to load.
#[tracing::instrument(skip(models_dir), fields(dir = %models_dir.display()))]
pub async fn ensure_reranker(models_dir: &Path) -> VaultLlmResult<RerankerPaths> {
    ensure_reranker_with_progress(models_dir, |_| {}).await
}

/// Total bytes the reranker acquisition transfers on a cold first run.
///
/// The denominator for overall progress. A cache hit on either file still
/// counts toward this total (see [`ensure_reranker_with_progress`]), so a
/// partially-cached run still finishes at 100% rather than at some fraction.
pub const RERANKER_TOTAL_BYTES: u64 = RERANKER_MODEL_BYTES + RERANKER_TOKENIZER_BYTES;

/// Overall progress across every file the reranker needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RerankerFetchProgress {
    /// Bytes accounted for so far, across all files.
    pub downloaded_bytes: u64,
    /// [`RERANKER_TOTAL_BYTES`], carried so a UI never has to know the
    /// constants.
    pub total_bytes: u64,
}

/// As [`ensure_reranker`], but reports aggregate progress across both files.
///
/// Progress is reported against the COMBINED expected size, so the fraction a
/// UI shows moves monotonically from 0 to 1 across the whole acquisition
/// rather than resetting between files.
///
/// **A cached file jumps the counter forward rather than reporting a
/// transfer.** `ensure_model_at_path_with_progress` stays silent on a cache
/// hit (it moved no bytes), so this function credits that file's full size the
/// moment it completes. That keeps a warm run honest — it shows completion,
/// not a fabricated download.
///
/// # Errors
///
/// As [`ensure_reranker`].
#[tracing::instrument(skip(models_dir, on_progress), fields(dir = %models_dir.display()))]
pub async fn ensure_reranker_with_progress<F>(
    models_dir: &Path,
    mut on_progress: F,
) -> VaultLlmResult<RerankerPaths>
where
    F: FnMut(RerankerFetchProgress),
{
    let paths = reranker_paths_in(models_dir);

    // Create the subdirectory before either download — `File::create` on the
    // `.partial` target fails if the parent is absent, and on a fresh install
    // it always is.
    if let Some(parent) = paths.model.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Tokenizer first: it is ~100x smaller, so a broken URL, a captive-portal
    // redirect or a dead network surfaces in seconds rather than after a
    // long partial transfer of the big file.
    ensure_model_at_path_with_progress(
        &paths.tokenizer,
        RERANKER_TOKENIZER_URL,
        QWEN3_RERANKER_TOKENIZER_SHA256,
        RERANKER_TOKENIZER_BYTES,
        |p| {
            on_progress(RerankerFetchProgress {
                downloaded_bytes: p.downloaded_bytes,
                total_bytes: RERANKER_TOTAL_BYTES,
            })
        },
    )
    .await?;
    on_progress(RerankerFetchProgress {
        downloaded_bytes: RERANKER_TOKENIZER_BYTES,
        total_bytes: RERANKER_TOTAL_BYTES,
    });

    ensure_model_at_path_with_progress(
        &paths.model,
        RERANKER_MODEL_URL,
        QWEN3_RERANKER_MODEL_SHA256,
        RERANKER_MODEL_BYTES,
        |p| {
            on_progress(RerankerFetchProgress {
                downloaded_bytes: RERANKER_TOKENIZER_BYTES.saturating_add(p.downloaded_bytes),
                total_bytes: RERANKER_TOTAL_BYTES,
            })
        },
    )
    .await?;
    on_progress(RerankerFetchProgress {
        downloaded_bytes: RERANKER_TOTAL_BYTES,
        total_bytes: RERANKER_TOTAL_BYTES,
    });

    tracing::info!("reranker model files present + integrity verified");
    Ok(paths)
}

// ─── Phi-4 (the maintenance / consolidation model) ──────────────────────────
//
// Phi-4 is the LLM the nightly consolidation ("automatic maintenance") runs on.
// Per the founder decision (2026-07-24), it downloads for EVERY user during
// onboarding so the real product is one toggle away — but, unlike the reranker,
// it does NOT gate onboarding completion: recall never needs it, only
// maintenance does, so it streams in the background while the user gets going.
//
// The pinned constants (URL, SHA-256, size, filename) live in
// `vault_llm::Phi4MiniConfig` next to the provider that loads the file. This
// module references them via `Phi4MiniConfig::v0_2_default` rather than
// restating them, so — exactly as with the reranker — a hash has one home.

/// Compute where the Phi-4 GGUF belongs under `models_dir`, without touching
/// the filesystem or the network. The file lives directly under `models_dir`
/// (no subdirectory), matching how `vault-app` resolves it.
pub fn phi4_path_in(models_dir: &Path) -> PathBuf {
    let config = Phi4MiniConfig::v0_2_default(models_dir.to_path_buf());
    models_dir.join(config.model_filename)
}

/// Overall progress of the Phi-4 acquisition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Phi4FetchProgress {
    /// Bytes transferred so far.
    pub downloaded_bytes: u64,
    /// Expected total size of the GGUF.
    pub total_bytes: u64,
}

/// Ensure the Phi-4 GGUF is present under `models_dir` with verified SHA-256,
/// downloading it if missing or corrupt.
///
/// Inherits ADR-043's semantics via [`ensure_model_at_path_with_progress`]: a
/// present-and-matching file short-circuits with no HTTP call (also the air-gap
/// path), a present-but-wrong file is refetched, and a download only lands at
/// its final name once its hash verifies.
///
/// # Errors
///
/// Propagates [`vault_llm::VaultLlmError`] on network failure, integrity
/// mismatch, or I/O failure. Fails closed — a partial or unverified file is
/// never left at the final path.
#[tracing::instrument(skip(models_dir), fields(dir = %models_dir.display()))]
pub async fn ensure_phi4(models_dir: &Path) -> VaultLlmResult<PathBuf> {
    ensure_phi4_with_progress(models_dir, |_| {}).await
}

/// As [`ensure_phi4`], but reports transfer progress.
///
/// # Errors
///
/// As [`ensure_phi4`].
#[tracing::instrument(skip(models_dir, on_progress), fields(dir = %models_dir.display()))]
pub async fn ensure_phi4_with_progress<F>(
    models_dir: &Path,
    mut on_progress: F,
) -> VaultLlmResult<PathBuf>
where
    F: FnMut(Phi4FetchProgress),
{
    let config = Phi4MiniConfig::v0_2_default(models_dir.to_path_buf());
    let model_path = models_dir.join(&config.model_filename);

    // Create the models dir before the download — `File::create` on the
    // `.partial` target fails if the parent is absent, and on a fresh install
    // it always is.
    if let Some(parent) = model_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let total_bytes = config.expected_bytes;
    ensure_model_at_path_with_progress(
        &model_path,
        &config.model_url,
        &config.model_sha256,
        total_bytes,
        |p| {
            on_progress(Phi4FetchProgress {
                downloaded_bytes: p.downloaded_bytes,
                total_bytes,
            })
        },
    )
    .await?;
    on_progress(Phi4FetchProgress {
        downloaded_bytes: total_bytes,
        total_bytes,
    });

    tracing::info!("maintenance model present + integrity verified");
    Ok(model_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── path composition ───────────────────────────────────────────────

    #[test]
    fn paths_resolve_under_the_reranker_subdir() {
        let paths = reranker_paths_in(Path::new("/app/models"));
        assert_eq!(
            paths.model,
            PathBuf::from("/app/models/qwen3-reranker-0.6b-seq-cls/model.onnx")
        );
        assert_eq!(
            paths.tokenizer,
            PathBuf::from("/app/models/qwen3-reranker-0.6b-seq-cls/tokenizer.json")
        );
    }

    #[test]
    fn model_and_tokenizer_never_share_a_path() {
        // Guards the collision class that the `.gguf.partial` bug created:
        // two downloads racing on one file.
        let paths = reranker_paths_in(Path::new("/app/models"));
        assert_ne!(paths.model, paths.tokenizer);
    }

    #[test]
    fn path_lookup_does_not_create_anything_on_disk() {
        // `reranker_paths_in` must stay side-effect free so a readiness probe
        // can call it safely before the user has consented to a 1.15 GB fetch.
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = reranker_paths_in(tmp.path());
        assert!(!paths.model.exists());
        assert!(!paths.tokenizer.exists());
        assert!(!tmp.path().join(RERANKER_SUBDIR).exists());
    }

    // ─── URL / filename coherence ───────────────────────────────────────

    #[test]
    fn urls_end_with_the_same_filenames_used_on_disk() {
        // The failure this catches: someone edits the on-disk file name or the
        // URL alone, so we download `model.onnx` and then look for something
        // else — which presents as an infinite re-download loop, not an error.
        assert!(
            RERANKER_MODEL_URL.ends_with(RERANKER_MODEL_FILE),
            "model URL must fetch the file name we look for on disk"
        );
        assert!(
            RERANKER_TOKENIZER_URL.ends_with(RERANKER_TOKENIZER_FILE),
            "tokenizer URL must fetch the file name we look for on disk"
        );
    }

    #[test]
    fn urls_target_the_pinned_repo_over_https_via_the_resolve_endpoint() {
        for url in [RERANKER_MODEL_URL, RERANKER_TOKENIZER_URL] {
            assert!(
                url.starts_with("https://"),
                "model downloads must be HTTPS: {url}"
            );
            assert!(
                url.contains("shawnw3i/Qwen3-Reranker-0.6B-seq-cls-ONNX"),
                "URL must point at the repo the pinned hashes were captured \
                 from, or integrity verification will always fail: {url}"
            );
            assert!(
                url.contains("/resolve/"),
                "must use the /resolve/ file-download endpoint: {url}"
            );
        }
    }

    // NOTE: the byte-constant sanity bounds are NOT tested here. They are
    // enforced at compile time by the `const _: () = { ... }` block beside the
    // constants themselves — a stronger guarantee than a runtime assertion,
    // and the change clippy's `assertions_on_constants` lint prompted.

    // ─── integrity constants are referenced, not restated ───────────────

    #[test]
    fn integrity_hashes_come_from_vault_embedding_and_are_well_formed() {
        // Pins the ADR-087 division of responsibility: if someone ever inlines
        // a hash literal here instead of referencing the pinned constant, the
        // two copies can drift silently. These asserts fail loudly if the
        // referenced constants stop being real SHA-256 hex.
        for (label, hash) in [
            ("model", QWEN3_RERANKER_MODEL_SHA256),
            ("tokenizer", QWEN3_RERANKER_TOKENIZER_SHA256),
        ] {
            assert_eq!(hash.len(), 64, "{label} hash must be SHA-256 hex");
            assert!(
                hash.chars().all(|c| c.is_ascii_hexdigit()),
                "{label} hash must be hex"
            );
        }
        assert_ne!(
            QWEN3_RERANKER_MODEL_SHA256, QWEN3_RERANKER_TOKENIZER_SHA256,
            "two different files cannot share a hash"
        );
    }

    // ─── Phi-4 acquisition ──────────────────────────────────────────────

    #[test]
    fn phi4_path_is_directly_under_the_models_dir() {
        assert_eq!(
            phi4_path_in(Path::new("/app/models")),
            PathBuf::from("/app/models/Phi-4-mini-instruct-Q4_K_M.gguf")
        );
    }

    #[test]
    fn phi4_path_lookup_creates_nothing_on_disk() {
        // A readiness probe must be safe to call before a 2.49 GB fetch.
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(!phi4_path_in(tmp.path()).exists());
    }

    #[test]
    fn phi4_constants_are_coherent_and_reference_the_pinned_config() {
        // The URL/hash/size come from vault_llm's pinned config — this pins the
        // ADR-087 division of responsibility for Phi-4 too, and catches a
        // filename/URL drift that would present as an infinite re-download.
        let config = Phi4MiniConfig::v0_2_default(PathBuf::from("/app/models"));
        assert!(
            config.model_url.ends_with(&config.model_filename),
            "URL must fetch the file name we look for on disk"
        );
        assert!(config.model_url.starts_with("https://"));
        assert!(
            config.model_url.contains("/resolve/"),
            "must use the /resolve/ file-download endpoint"
        );
        assert!(
            config.expected_bytes > 2_000_000_000,
            "Phi-4 Q4_K_M is ~2.49 GB"
        );
        assert_eq!(config.model_sha256.len(), 64, "hash must be SHA-256 hex");
        assert!(config.model_sha256.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
