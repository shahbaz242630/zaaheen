//! One model download, retried when the connection breaks (ADR-043
//! amendment 1, session 78).
//!
//! A 2.5 GB download over a home or hotel connection will sometimes break.
//! Before this, one broken connection ended the download ("stream chunk: error
//! decoding response body", seen on the test Mac in session 77) and nothing
//! tried again until the person changed a setting.
//!
//! - **Retried:** a failed connection, a broken or stalled stream, a response
//!   that ends early, and the server's own trouble (408, 429, 5xx). Up to
//!   [`RetryPolicy::attempts`] tries, waiting a little longer before each.
//! - **Resumed within one download:** a retry asks only for the bytes still
//!   missing (`Range`) and carries on the same hash. A server that ignores the
//!   request and sends the whole file starts the download over, cleanly.
//! - **Not retried:** any other refusal (404, 403, …), a size far off the
//!   pinned one, a disk that cannot be written, and a finished file whose hash
//!   is wrong. Retrying cannot fix those.
//! - **Across runs** it still restarts, as ADR-043 set: a `.partial` left by
//!   an earlier run is never trusted, because nothing vouches for its bytes.

use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::{display_label, partial_path_for, DownloadProgress, PROGRESS_EMIT_INTERVAL_BYTES};
use crate::error::{VaultLlmError, VaultLlmResult};

/// How hard one download tries.
#[derive(Debug, Clone, Copy)]
pub(super) struct RetryPolicy {
    /// Tries in all, the first included.
    pub attempts: u32,
    /// The wait before each retry; the last one repeats.
    pub backoff: &'static [Duration],
    /// The longest a connection may send nothing before it counts as broken.
    pub idle_limit: Duration,
    /// The longest a connection may take to open.
    pub connect_limit: Duration,
}

impl RetryPolicy {
    /// The shipped policy: six tries across about a minute of waiting.
    pub(super) const SHIPPED: RetryPolicy = RetryPolicy {
        attempts: 6,
        backoff: &[
            Duration::from_secs(2),
            Duration::from_secs(5),
            Duration::from_secs(10),
            Duration::from_secs(20),
            Duration::from_secs(30),
        ],
        idle_limit: Duration::from_secs(60),
        connect_limit: Duration::from_secs(30),
    };

    fn wait_before_retry(&self, failed_attempt: u32) -> Duration {
        let index = (failed_attempt.saturating_sub(1) as usize).min(self.backoff.len() - 1);
        self.backoff[index]
    }
}

/// Why one try stopped.
enum Stop {
    /// Worth another try.
    Retry(VaultLlmError),
    /// Another try cannot help.
    GiveUp(VaultLlmError),
}

/// What one download has received so far, across its tries.
struct Received<'a, F> {
    file: tokio::fs::File,
    hasher: Sha256,
    bytes: u64,
    last_reported: u64,
    expected_bytes: u64,
    on_progress: &'a mut F,
}

impl<F: FnMut(DownloadProgress)> Received<'_, F> {
    /// Throw away what arrived and start from the first byte.
    async fn start_over(&mut self) -> Result<(), Stop> {
        self.file.set_len(0).await.map_err(io_stop)?;
        self.file
            .seek(std::io::SeekFrom::Start(0))
            .await
            .map_err(io_stop)?;
        self.hasher = Sha256::new();
        self.bytes = 0;
        self.last_reported = 0;
        Ok(())
    }

    async fn take(&mut self, chunk: &[u8]) -> Result<(), Stop> {
        self.hasher.update(chunk);
        self.file.write_all(chunk).await.map_err(io_stop)?;
        self.bytes = self.bytes.saturating_add(chunk.len() as u64);
        if self.bytes - self.last_reported >= PROGRESS_EMIT_INTERVAL_BYTES {
            self.last_reported = self.bytes;
            (self.on_progress)(DownloadProgress {
                downloaded_bytes: self.bytes,
                total_bytes: self.expected_bytes,
            });
        }
        Ok(())
    }
}

/// A disk that cannot be written will not recover on its own.
fn io_stop(e: std::io::Error) -> Stop {
    Stop::GiveUp(VaultLlmError::Io(e))
}

/// Download `url` to `path` through its `.partial`, verify the SHA-256, and
/// only then move it into place (ADR-043), retrying per `policy`.
pub(super) async fn download_with_verify<F>(
    path: &Path,
    url: &str,
    expected_sha256_hex: &str,
    expected_bytes: u64,
    mut on_progress: F,
    policy: RetryPolicy,
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

    let client = reqwest::Client::builder()
        .connect_timeout(policy.connect_limit)
        .build()
        .map_err(|e| VaultLlmError::DownloadFailed(format!("HTTP client: {e}")))?;

    // Restart-not-resume across runs: create truncates any earlier .partial.
    let partial_path = partial_path_for(path);
    let mut received = Received {
        file: tokio::fs::File::create(&partial_path).await?,
        hasher: Sha256::new(),
        bytes: 0,
        last_reported: 0,
        expected_bytes,
        on_progress: &mut on_progress,
    };

    let mut attempt = 1;
    loop {
        match one_try(&client, url, &mut received, policy.idle_limit).await {
            Ok(()) => break,
            Err(Stop::Retry(e)) if attempt < policy.attempts => {
                let wait = policy.wait_before_retry(attempt);
                tracing::warn!(
                    file = %file_label,
                    attempt,
                    received_bytes = received.bytes,
                    retry_in_secs = wait.as_secs_f32(),
                    error = %e,
                    "model download interrupted; trying again"
                );
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Err(Stop::Retry(e) | Stop::GiveUp(e)) => {
                drop(received);
                let _ = std::fs::remove_file(&partial_path);
                return Err(e);
            }
        }
    }

    received.file.flush().await?;
    let Received {
        file,
        hasher,
        bytes,
        last_reported,
        on_progress,
        ..
    } = received;
    drop(file);

    // Always land on a final report. Without this the last partial interval
    // would leave a progress bar short of 100% while the (potentially long)
    // hash verification runs, which reads as a stall.
    if bytes != last_reported {
        on_progress(DownloadProgress {
            downloaded_bytes: bytes,
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

/// One request: the missing bytes when some already arrived, else the file.
async fn one_try<F: FnMut(DownloadProgress)>(
    client: &reqwest::Client,
    url: &str,
    received: &mut Received<'_, F>,
    idle_limit: Duration,
) -> Result<(), Stop> {
    let mut request = client.get(url);
    if received.bytes > 0 {
        request = request.header(RANGE, format!("bytes={}-", received.bytes));
    }
    let resp = request.send().await.map_err(|e| {
        Stop::Retry(VaultLlmError::DownloadFailed(format!(
            "HTTP GET {url}: {e}"
        )))
    })?;

    let status = resp.status();
    let body_len = resp.content_length();
    let resumed = status == StatusCode::PARTIAL_CONTENT && received.bytes > 0;
    if resumed {
        let start = resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(range_start);
        if start != Some(received.bytes) {
            // Not the bytes we asked for: the next try fetches the whole file.
            received.start_over().await?;
            return Err(Stop::Retry(VaultLlmError::DownloadFailed(
                "the server resumed at the wrong place".into(),
            )));
        }
    } else if status.is_success() {
        if received.bytes > 0 {
            tracing::info!("the server sent the whole file again; starting over");
            received.start_over().await?;
        }
        // Streaming-abort heuristic per ADR-043: reject an obviously wrong
        // payload early (a redirect page, another quantisation).
        if let Some(cl) = body_len {
            let cl_low = received.expected_bytes / 2;
            let cl_high = received.expected_bytes.saturating_mul(2);
            if cl < cl_low || cl > cl_high {
                return Err(Stop::GiveUp(VaultLlmError::DownloadFailed(format!(
                    "Content-Length {cl} bytes wildly off expected ~{} bytes \
                     (acceptable range [{cl_low}, {cl_high}]) — aborting (likely wrong file or redirect HTML)",
                    received.expected_bytes
                ))));
            }
        }
    } else {
        let error = VaultLlmError::DownloadFailed(format!("HTTP non-2xx: {status} for {url}"));
        return Err(if is_worth_retrying(status) {
            Stop::Retry(error)
        } else {
            Stop::GiveUp(error)
        });
    }

    // The byte count this response should bring the file to, when known.
    let end = body_len.map(|len| received.bytes.saturating_add(len));

    let mut stream = resp.bytes_stream();
    loop {
        let next = tokio::time::timeout(idle_limit, stream.next())
            .await
            .map_err(|_| {
                Stop::Retry(VaultLlmError::DownloadFailed(format!(
                    "no data for {} s",
                    idle_limit.as_secs()
                )))
            })?;
        match next {
            Some(Ok(chunk)) => received.take(&chunk).await?,
            Some(Err(e)) => {
                return Err(Stop::Retry(VaultLlmError::DownloadFailed(format!(
                    "stream chunk: {e}"
                ))))
            }
            None => break,
        }
    }

    match end {
        Some(end) if received.bytes < end => Err(Stop::Retry(VaultLlmError::DownloadFailed(
            format!("the connection ended at {} of {end} bytes", received.bytes),
        ))),
        _ => Ok(()),
    }
}

/// The server's own trouble, which often clears.
fn is_worth_retrying(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

/// The first byte of a `Content-Range: bytes <start>-<end>/<size>` header.
fn range_start(header: &str) -> Option<u64> {
    header
        .strip_prefix("bytes ")?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
#[path = "download_tests.rs"]
mod tests;
