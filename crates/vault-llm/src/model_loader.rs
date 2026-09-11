//! Model file management: download, SHA-256 integrity verification, air-gap
//! fallback.
//!
//! The air-gap fallback path (iteration 2 fresh scope per Shahbaz's call) is
//! NOT a separate branch — it falls out naturally from `ensure_model_at_path`:
//! if a user manually places the GGUF file at the expected path with the
//! correct hash before first launch, the function returns Ok without ever
//! attempting a download. The operational doc that names this user-facing
//! workflow lands at Phase 3 alongside ADR-043.
//!
//! ## ADR-043 contract surface (drafted at Phase 5, locked here)
//!
//! - **Cache + air-gap**: if `path` exists AND SHA-256 matches `expected_sha256_hex`,
//!   return `Ok(())` immediately. INFO log naming the file. No HTTP call.
//! - **Stale cache**: if `path` exists but SHA-256 mismatches, delete the file
//!   (WARN log) and fall through to download.
//! - **Streaming-abort heuristic** (concern #2): if HTTP `Content-Length` is wildly
//!   off the expected byte count (`< expected_bytes / 2` or `> expected_bytes * 2`),
//!   abort with `DownloadFailed` (likely wrong file or redirect HTML).
//! - **`.partial` strategy: restart-not-resume** (concern #3): any pre-existing
//!   `.partial` from a prior crashed run is clobbered by `File::create`. No HTTP
//!   Range header use.
//! - **Atomic finalize**: write to `.partial`, verify SHA-256 post-stream, only
//!   `rename` to final path on hash pass. Failed hash → delete `.partial`,
//!   return `IntegrityCheckFailed`.
//! - **Disk-full fail-closed**: any I/O error during write propagates as
//!   `VaultLlmError::Io`. Tauri can surface the error via a fatal dialog
//!   ("Insufficient disk space — need ~3 GB free at <path>").

use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

use crate::error::{VaultLlmError, VaultLlmResult};

/// 8 MB chunks for streaming hash compute — large enough that syscall overhead
/// is negligible vs hash compute, small enough to keep RAM bounded.
const HASH_READ_CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// Compute the SHA-256 of a file by streaming + hashing in 8 MB chunks.
/// Used for both post-download integrity verification AND cache-hit
/// re-verification when a model file already exists on disk.
///
/// # Why this offloads to `spawn_blocking` (ADR-087 follow-up)
///
/// This previously read the file through `tokio::fs` directly inside the async
/// fn. Two problems, both measured on the 1.15 GB reranker ONNX (2026-07-20):
///
/// 1. **~32x slower than the hardware.** The old form hashed at ~17 MB/s
///    (67s for 1.15 GB). The same file on the same machine hashes at
///    545 MB/s cold / 717 MB/s page-cached via .NET's `Get-FileHash` — and
///    17 MB/s is below even unaccelerated software SHA-256 (~150-250 MB/s),
///    so the hash function was never the bottleneck. `tokio::fs` caps each
///    read at its internal max buffer and round-trips every one through the
///    blocking pool, so a 1.15 GB file became hundreds of scheduler hops.
///    Reading synchronously inside ONE blocking task removes the hops.
///
/// 2. **It stalled the runtime.** Hashing gigabytes is CPU-bound work, and it
///    was running on the async runtime rather than off it. BRD §2 is explicit:
///    all I/O is async, CPU-bound work is sync and called via `spawn_blocking`.
///    This was a standing violation of that rule, not merely a slow path.
///
/// The buffer size and chunking behaviour are unchanged; only where the work
/// runs changed.
pub async fn compute_sha256_of_file(path: &Path) -> VaultLlmResult<[u8; 32]> {
    let owned = path.to_path_buf();
    tokio::task::spawn_blocking(move || compute_sha256_of_file_blocking(&owned))
        .await
        .map_err(|e| {
            // A JoinError means the blocking task panicked or was cancelled.
            // Surfacing it as Io keeps the error surface unchanged for callers
            // (fail-closed either way — no caller treats Io as recoverable).
            VaultLlmError::Io(std::io::Error::other(format!(
                "sha256 hashing task failed: {e}"
            )))
        })?
}

/// Synchronous SHA-256 of a file, hashed in 8 MB chunks.
///
/// Runs on a blocking thread — see [`compute_sha256_of_file`] for why. Kept
/// separate (rather than inlined into the closure) so it stays directly
/// testable without a runtime.
fn compute_sha256_of_file_blocking(path: &Path) -> VaultLlmResult<[u8; 32]> {
    use std::io::Read as _;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_READ_CHUNK_SIZE];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

/// Bytes transferred so far for one file, reported to a progress callback.
///
/// `total_bytes` is the caller's EXPECTED size (the pinned constant), not the
/// server's `Content-Length`. The two are cross-checked by the
/// streaming-abort heuristic before any progress is reported, and using the
/// pinned value keeps the denominator stable even if a server omits the
/// header — a progress bar that can jump to a different total mid-download is
/// worse than no progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    /// Bytes written to the in-flight `.partial` file so far.
    pub downloaded_bytes: u64,
    /// Expected total size of this file, in bytes.
    pub total_bytes: u64,
}

/// Emit a progress callback at most once per this many bytes.
///
/// Byte-quantised rather than time-quantised on purpose: it keeps the callback
/// cadence deterministic (so a test can assert exactly how many times it
/// fires) and avoids a clock read per chunk. At 8 MB this is ~143 callbacks
/// across the 1.15 GB reranker — smooth enough for a progress bar, far too
/// coarse to flood the Tauri event channel.
const PROGRESS_EMIT_INTERVAL_BYTES: u64 = 8 * 1024 * 1024;

/// Ensure the model file is available at `path` with verified SHA-256.
///
/// Three operational paths converge here per ADR-043:
/// 1. **Cache hit**: file exists + hash matches → return Ok immediately.
/// 2. **Air-gap fallback**: user manually placed the file → same as cache hit
///    from this function's POV (no distinction in the runtime behavior).
/// 3. **Fresh download**: file absent OR hash mismatch → stream from `url`,
///    hash on the fly, atomic rename to final path on hash pass.
///
/// Reports no progress. Use [`ensure_model_at_path_with_progress`] when a UI
/// has to show the transfer.
pub async fn ensure_model_at_path(
    path: &Path,
    url: &str,
    expected_sha256_hex: &str,
    expected_bytes: u64,
) -> VaultLlmResult<()> {
    ensure_model_at_path_with_progress(path, url, expected_sha256_hex, expected_bytes, |_| {}).await
}

/// As [`ensure_model_at_path`], but invokes `on_progress` as bytes arrive.
///
/// The callback fires roughly every [`PROGRESS_EMIT_INTERVAL_BYTES`], plus
/// once at completion so a UI always lands on 100% rather than stalling at the
/// last partial interval. It does NOT fire on the cache-hit path — a cached
/// file transfers nothing, and reporting fake progress for it would make the
/// first-run UI claim work it never did.
///
/// The callback runs inline on the download task, so it must be cheap and must
/// not block; anything expensive belongs behind a channel.
pub async fn ensure_model_at_path_with_progress<F>(
    path: &Path,
    url: &str,
    expected_sha256_hex: &str,
    expected_bytes: u64,
    on_progress: F,
) -> VaultLlmResult<()>
where
    F: FnMut(DownloadProgress),
{
    if cached_and_verified(path, expected_sha256_hex).await? {
        return Ok(());
    }

    // One download per file across PROCESSES (ADR-102 review, session 35):
    // the desktop app and the vault keeper can both decide to fetch the same
    // model at once. Sharing one `.partial`, each hashed its OWN stream while
    // their writes interleaved in the file, so a download could "verify" and
    // install a corrupt model. The loser of the lock waits, then finds the
    // winner's verified file.
    let _fetch_lock = FetchLock::acquire(path).await?;
    if cached_and_verified(path, expected_sha256_hex).await? {
        return Ok(());
    }

    download_with_verify(path, url, expected_sha256_hex, expected_bytes, on_progress).await
}

/// `true` when `path` holds exactly the expected file. A file with the wrong
/// hash is deleted (it will be re-downloaded).
async fn cached_and_verified(path: &Path, expected_sha256_hex: &str) -> VaultLlmResult<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let file_label = display_label(path);
    let actual = hex::encode(compute_sha256_of_file(path).await?);
    if actual == expected_sha256_hex {
        tracing::info!(
            file = %file_label,
            "model file already present + hash verified (cache hit or air-gap)"
        );
        return Ok(true);
    }
    tracing::warn!(
        file = %file_label,
        expected = %expected_sha256_hex,
        actual = %actual,
        "existing model file hash mismatch; deleting and re-downloading"
    );
    std::fs::remove_file(path)?;
    Ok(false)
}

/// How often a process waiting for another's download re-checks the lock.
const FETCH_LOCK_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// An OS lock on `<model>.fetch.lock`, held for one download. The file is
/// never deleted (deleting a lockfile by name is how two holders happen,
/// ADR-SEC-020); the OS releases the lock when the handle closes, including
/// when the holder crashes.
struct FetchLock {
    file: std::fs::File,
}

impl FetchLock {
    async fn acquire(model_path: &Path) -> VaultLlmResult<Self> {
        let lock_path = with_suffix(model_path, ".fetch.lock");
        if let Some(dir) = lock_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        let mut announced = false;
        loop {
            if try_lock_exclusive(&file)? {
                return Ok(Self { file });
            }
            if !announced {
                tracing::info!(
                    file = %display_label(model_path),
                    "another process is downloading this model; waiting for it"
                );
                announced = true;
            }
            tokio::time::sleep(FETCH_LOCK_POLL).await;
        }
    }
}

impl Drop for FetchLock {
    fn drop(&mut self) {
        let _ = unlock(&self.file);
    }
}

/// `File::try_lock` is stable since Rust 1.89; the toolchain is pinned to
/// 1.92 and the declared `rust-version` is stale (tracked as tech debt, as in
/// vault-app's `consolidator_lock`).
#[allow(clippy::incompatible_msrv)]
fn try_lock_exclusive(file: &std::fs::File) -> std::io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

#[allow(clippy::incompatible_msrv)]
fn unlock(file: &std::fs::File) -> std::io::Result<()> {
    file.unlock()
}

async fn download_with_verify<F>(
    path: &Path,
    url: &str,
    expected_sha256_hex: &str,
    expected_bytes: u64,
    mut on_progress: F,
) -> VaultLlmResult<()>
where
    F: FnMut(DownloadProgress),
{
    let file_label = display_label(path);
    tracing::info!(
        file = %file_label,
        url = %url,
        expected_bytes = expected_bytes,
        "starting model download"
    );

    let resp = reqwest::get(url)
        .await
        .map_err(|e| VaultLlmError::DownloadFailed(format!("HTTP GET {url}: {e}")))?
        .error_for_status()
        .map_err(|e| VaultLlmError::DownloadFailed(format!("HTTP non-2xx: {e}")))?;

    // Streaming-abort heuristic per ADR-043 / iteration 2 concern #2 —
    // reject obvious-mismatch early to save bandwidth on a clearly-wrong
    // payload (e.g., HF served a redirect HTML page, or pinned URL points
    // at a different quantization variant).
    if let Some(cl) = resp.content_length() {
        let cl_low = expected_bytes / 2;
        let cl_high = expected_bytes.saturating_mul(2);
        if cl < cl_low || cl > cl_high {
            return Err(VaultLlmError::DownloadFailed(format!(
                "Content-Length {cl} bytes wildly off expected ~{expected_bytes} bytes \
                 (acceptable range [{cl_low}, {cl_high}]) — aborting (likely wrong file or redirect HTML)"
            )));
        }
    }

    // Restart-not-resume: create truncates any pre-existing .partial.
    let partial_path = partial_path_for(path);
    let mut file = tokio::fs::File::create(&partial_path).await?;
    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();

    let mut downloaded: u64 = 0;
    let mut last_reported: u64 = 0;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result
            .map_err(|e| VaultLlmError::DownloadFailed(format!("stream chunk: {e}")))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;

        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded - last_reported >= PROGRESS_EMIT_INTERVAL_BYTES {
            last_reported = downloaded;
            on_progress(DownloadProgress {
                downloaded_bytes: downloaded,
                total_bytes: expected_bytes,
            });
        }
    }
    file.flush().await?;
    drop(file);

    // Always land on a final report. Without this the last partial interval
    // would leave a progress bar short of 100% while the (potentially long)
    // hash verification runs, which reads as a stall.
    if downloaded != last_reported {
        on_progress(DownloadProgress {
            downloaded_bytes: downloaded,
            total_bytes: expected_bytes,
        });
    }

    let actual = hex::encode(hasher.finalize());
    if actual != expected_sha256_hex {
        // Fail-closed: remove the (now-tainted) .partial file.
        let _ = std::fs::remove_file(&partial_path);
        return Err(VaultLlmError::IntegrityCheckFailed {
            file: file_label,
            expected: expected_sha256_hex.to_string(),
            actual,
        });
    }

    tokio::fs::rename(&partial_path, path).await?;
    tracing::info!(
        file = %file_label,
        sha256 = %actual,
        "model downloaded + integrity verified"
    );
    Ok(())
}

/// Build the in-flight download path by APPENDING `.partial` to the full file
/// name, rather than replacing the extension.
///
/// **Why this is not `path.with_extension("gguf.partial")`** (the form this
/// carried until ADR-087): `with_extension` REPLACES the final extension, so
/// that form silently rewrote any non-GGUF target — `model.onnx` became
/// `model.gguf.partial`. Harmless while Phi-4 was the only caller, actively
/// wrong once the Qwen3 reranker ONNX downloads through the same path. The
/// `.partial` file would carry a misleading extension and, more importantly,
/// two different models sharing a directory could collide on one partial name.
///
/// Appending preserves the pre-existing behaviour for `.gguf` inputs exactly
/// (`model.gguf` → `model.gguf.partial`), so Phi-4's download path is
/// byte-for-byte unchanged by this fix.
fn partial_path_for(path: &Path) -> PathBuf {
    with_suffix(path, ".partial")
}

/// `path` with `suffix` appended to the full file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut raw = path.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

fn display_label(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn tempfile_with_content(content: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("tempfile create");
        f.write_all(content).expect("tempfile write");
        f.flush().expect("tempfile flush");
        f
    }

    // ─── floor 5: SHA-256 verify success ────────────────────────────────

    #[tokio::test]
    async fn sha256_of_known_content_matches_canonical_hash() {
        // SHA-256("hello world") =
        //   b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        let f = tempfile_with_content(b"hello world");
        let h = compute_sha256_of_file(f.path()).await.expect("hash");
        assert_eq!(
            hex::encode(h),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    // ─── floor 6: cache-hit / air-gap short-circuit ─────────────────────

    #[tokio::test]
    async fn ensure_returns_ok_immediately_on_cache_hit_with_matching_hash() {
        let f = tempfile_with_content(b"hello world");
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        // URL deliberately set to an unreachable address; cache-hit short-circuit
        // MUST fire before any HTTP attempt. If the function ever tries to
        // download, this test fails or hangs (we'd see it instantly).
        let result = ensure_model_at_path(
            f.path(),
            "http://127.0.0.1:1/never-reached.bin",
            expected,
            11, // "hello world" is 11 bytes
        )
        .await;
        assert!(
            result.is_ok(),
            "cache hit must short-circuit before HTTP — got {result:?}"
        );
    }

    // ─── ADR-087: `.partial` naming generalised beyond GGUF ─────────────

    #[test]
    fn partial_path_appends_rather_than_replacing_extension() {
        // The ONNX case is the whole point of the fix: the previous
        // `with_extension("gguf.partial")` form produced `model.gguf.partial`
        // here, mislabelling an ONNX download as a GGUF one.
        assert_eq!(
            partial_path_for(Path::new("/models/model.onnx")),
            PathBuf::from("/models/model.onnx.partial")
        );
    }

    #[test]
    fn partial_path_preserves_prior_gguf_behaviour_exactly() {
        // Regression guard on the EXISTING Phi-4 path: this fix must not
        // change the partial filename for GGUF downloads, or an interrupted
        // pre-fix download would be orphaned rather than clobbered.
        assert_eq!(
            partial_path_for(Path::new("/models/Phi-4-mini-instruct-Q4_K_M.gguf")),
            PathBuf::from("/models/Phi-4-mini-instruct-Q4_K_M.gguf.partial")
        );
    }

    #[test]
    fn partial_paths_of_two_models_in_one_dir_do_not_collide() {
        // The failure this prevents: reranker model + tokenizer (or any two
        // models) sharing a directory and racing on ONE partial file — the
        // same class of bug that took down the weekly CI smoke job, where
        // three tests shared a single `.partial` path.
        let a = partial_path_for(Path::new("/m/model.onnx"));
        let b = partial_path_for(Path::new("/m/tokenizer.json"));
        assert_ne!(a, b, "distinct models must claim distinct partial paths");
    }

    // ─── floor 7: SHA-256 mismatch on cached file deletes + re-downloads
    //              (and downstream download fails closed = atomic-cleanup proof) ─

    #[tokio::test]
    async fn ensure_with_mismatched_cached_hash_deletes_file_then_attempts_redownload() {
        let f = tempfile_with_content(b"wrong content");
        let path = f.path().to_owned();
        // Release tempfile guard but the file stays on disk for the test.
        drop(f);

        // Wrong expected hash + unreachable URL — ensure_model_at_path
        // should: (1) hash the existing file, (2) detect mismatch, (3)
        // delete the file, (4) attempt the download, (5) fail on the
        // unreachable URL. The post-condition we assert is the file is
        // GONE (proving step 3) AND the result is Err (proving step 5).
        let wrong_expected = "0000000000000000000000000000000000000000000000000000000000000000";
        let result = ensure_model_at_path(
            &path,
            "http://127.0.0.1:1/never-reached.bin",
            wrong_expected,
            13, // "wrong content" is 13 bytes
        )
        .await;
        assert!(result.is_err(), "download to unreachable URL must fail");
        assert!(
            !path.exists(),
            "mismatched cached file must be deleted before redownload attempt"
        );
    }

    /// Two processes wanting the same model: the second waits for the first
    /// and then uses its verified file, never downloading alongside it. (The
    /// URL is unreachable, so any download attempt would fail the test.)
    #[tokio::test]
    async fn a_second_downloader_waits_for_the_first_and_uses_its_file() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("model.onnx");
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let first = FetchLock::acquire(&path).await.expect("first lock");

        let waiter = {
            let path = path.clone();
            tokio::spawn(async move {
                ensure_model_at_path(&path, "http://127.0.0.1:1/never-reached.bin", expected, 11)
                    .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !waiter.is_finished(),
            "must wait for the download in progress, not start its own"
        );

        // The first downloader lands its verified file and lets go.
        std::fs::write(&path, b"hello world").expect("write model");
        drop(first);

        waiter
            .await
            .expect("join")
            .expect("uses the first downloader's file");
    }
}
