//! The ranking model's acquisition, owned by the keeper (ADR-108 D10).
//!
//! Before D4 the desktop downloaded the model itself while the keeper also
//! fetched it at start: two downloaders on one `.partial` path (session-35
//! review K6). Now one [`EngineCell`] per keeper does it, and both the
//! keeper's own start and the desktop's "get it now" (`admin_engine_fetch`)
//! go through the same single flight.
//!
//! - **One at a time:** a fetch already running is joined, never repeated.
//! - **Cold on failure:** a failed fetch leaves the cell ready to try again,
//!   so onboarding's retry works (review B-M4).
//! - **Progress** is a byte count the desktop polls and shows; nothing here
//!   names the model (ADR-086).

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::model_fetch;

/// Where the acquisition is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// Not asked yet this keeper.
    Idle,
    /// Downloading or verifying now.
    Running,
    /// The files are present and verified.
    Done,
    /// The last attempt failed; asking again retries.
    Failed,
}

impl Fetch {
    /// Stable wire string for the admin surface.
    pub fn as_wire_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// What acquiring the files runs. The real one downloads and verifies; tests
/// substitute their own.
pub type Acquire = Arc<dyn Fn(PathBuf, watch::Sender<(u64, u64)>) -> Acquiring + Send + Sync>;

/// One acquisition in progress.
pub type Acquiring = Pin<Box<dyn Future<Output = Result<(), ()>> + Send>>;

/// One keeper's acquisition of the ranking model.
pub struct EngineCell {
    models_dir: PathBuf,
    acquire: Acquire,
    state: Mutex<Fetch>,
    progress: watch::Sender<(u64, u64)>,
    done: watch::Sender<Fetch>,
}

impl EngineCell {
    /// The shipped cell: the real download and verification into
    /// `models_dir`.
    pub fn new(models_dir: PathBuf) -> Arc<Self> {
        Self::with_acquire(
            models_dir,
            Arc::new(
                |dir: PathBuf, progress: watch::Sender<(u64, u64)>| -> Acquiring {
                    Box::pin(real_acquire(dir, progress))
                },
            ),
        )
    }

    /// A cell whose acquisition is `acquire` (tests).
    pub fn with_acquire(models_dir: PathBuf, acquire: Acquire) -> Arc<Self> {
        Arc::new(Self {
            models_dir,
            acquire,
            state: Mutex::new(Fetch::Idle),
            progress: watch::Sender::new((0, model_fetch::RERANKER_TOTAL_BYTES)),
            done: watch::Sender::new(Fetch::Idle),
        })
    }

    /// Where the acquisition is now.
    pub fn fetch(&self) -> Fetch {
        self.state.lock().map_or(Fetch::Failed, |s| *s)
    }

    /// Bytes accounted for, and the total.
    pub fn progress(&self) -> (u64, u64) {
        *self.progress.borrow()
    }

    /// Start the acquisition unless it is running or done, then run
    /// `after_success` (the keeper warms the model) once it succeeds. Returns
    /// at once; the work runs on its own task, so a desktop that closes its
    /// window does not stop a download halfway.
    pub fn start<F>(self: &Arc<Self>, after_success: F)
    where
        F: FnOnce() + Send + 'static,
    {
        {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if matches!(*state, Fetch::Running | Fetch::Done) {
                return;
            }
            *state = Fetch::Running;
        }
        let _ = self.done.send_replace(Fetch::Running);
        let cell = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = (cell.acquire)(cell.models_dir.clone(), cell.progress.clone()).await;
            let next = if outcome.is_ok() {
                Fetch::Done
            } else {
                Fetch::Failed
            };
            if let Ok(mut state) = cell.state.lock() {
                *state = next;
            }
            let _ = cell.done.send_replace(next);
            if next == Fetch::Done {
                after_success();
            }
        });
    }

    /// Wait until the acquisition is no longer running (tests, and the
    /// keeper's own start).
    pub async fn settled(&self) -> Fetch {
        let mut done = self.done.subscribe();
        let settled = done.wait_for(|f| *f != Fetch::Running).await.map(|f| *f);
        settled.unwrap_or_else(|_| self.fetch())
    }
}

async fn real_acquire(models_dir: PathBuf, progress: watch::Sender<(u64, u64)>) -> Result<(), ()> {
    match model_fetch::ensure_reranker_with_progress(&models_dir, |p| {
        let _ = progress.send_replace((p.downloaded_bytes, p.total_bytes));
    })
    .await
    {
        Ok(_) => {
            let total = model_fetch::RERANKER_TOTAL_BYTES;
            let _ = progress.send_replace((total, total));
            Ok(())
        }
        Err(e) => {
            tracing::warn!(error = %e, "reranker acquisition failed; reads use the cosine gate");
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// An acquisition that counts its runs and answers from a script.
    fn scripted(
        runs: Arc<AtomicUsize>,
        answers: Arc<Mutex<Vec<bool>>>,
        gate: Arc<tokio::sync::Semaphore>,
    ) -> Acquire {
        Arc::new(
            move |_dir: PathBuf, progress: watch::Sender<(u64, u64)>| -> Acquiring {
                let runs = Arc::clone(&runs);
                let answers = Arc::clone(&answers);
                let gate = Arc::clone(&gate);
                Box::pin(async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    let _ = progress.send_replace((5, 10));
                    let _permit = gate.acquire().await;
                    let ok = answers.lock().unwrap().remove(0);
                    if ok {
                        Ok(())
                    } else {
                        Err(())
                    }
                })
            },
        )
    }

    /// The keeper's start and the desktop's request share ONE download.
    #[tokio::test]
    async fn two_callers_share_one_acquisition() {
        let runs = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let cell = EngineCell::with_acquire(
            PathBuf::from("models"),
            scripted(
                Arc::clone(&runs),
                Arc::new(Mutex::new(vec![true])),
                Arc::clone(&gate),
            ),
        );
        let warmed = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            let w = Arc::clone(&warmed);
            cell.start(move || {
                w.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert_eq!(cell.fetch(), Fetch::Running);
        gate.add_permits(1);
        assert_eq!(cell.settled().await, Fetch::Done);
        assert_eq!(runs.load(Ordering::SeqCst), 1, "one download");
        tokio::task::yield_now().await;
        assert_eq!(warmed.load(Ordering::SeqCst), 1, "warmed once");

        cell.start(|| {});
        assert_eq!(
            cell.fetch(),
            Fetch::Done,
            "a finished cell is not started again"
        );
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    /// A failed download can be tried again (onboarding's retry).
    #[tokio::test]
    async fn a_failed_acquisition_can_be_retried() {
        let runs = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(2));
        let cell = EngineCell::with_acquire(
            PathBuf::from("models"),
            scripted(
                Arc::clone(&runs),
                Arc::new(Mutex::new(vec![false, true])),
                gate,
            ),
        );
        cell.start(|| {});
        assert_eq!(cell.settled().await, Fetch::Failed);
        cell.start(|| {});
        assert_eq!(cell.settled().await, Fetch::Done);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn progress_is_published_while_it_runs() {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let cell = EngineCell::with_acquire(
            PathBuf::from("models"),
            scripted(
                Arc::new(AtomicUsize::new(0)),
                Arc::new(Mutex::new(vec![true])),
                Arc::clone(&gate),
            ),
        );
        cell.start(|| {});
        for _ in 0..50 {
            if cell.progress() == (5, 10) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(cell.progress(), (5, 10));
        gate.add_permits(1);
        cell.settled().await;
    }

    #[test]
    fn the_wire_strings_name_no_model() {
        for f in [Fetch::Idle, Fetch::Running, Fetch::Done, Fetch::Failed] {
            let s = f.as_wire_str();
            assert!(!s.contains("qwen") && !s.contains("rerank"), "{s}");
        }
    }
}
