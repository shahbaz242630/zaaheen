//! The read desk: recall answers one question at a time (session 59).
//!
//! **Why.** Every `memory_read` and `memory_search` ends in the relevance
//! check (the reranker), which runs one question at a time: it is too big to
//! run twice at once on a laptop. On 2026-09-24 Cursor's model sent six reads
//! at once; each took ~25-30 s on the founder's laptop, so reads 2-6 waited
//! 55-200 s, past the 55 s the relay gives a call, and came back "took too
//! long" although the vault answered every one. Worse, the keeper went on
//! answering reads whose caller had already given up, so a new chat's single
//! question queued behind them and failed too. The AI apps themselves stop
//! waiting at 60 s, so there is no more time to give: the fix is to keep the
//! line from filling with work nobody wants.
//!
//! **What the desk does.**
//! - Questions take turns at one seat ([`ReadDesk::sit`]). Waiting is an async
//!   wait, so a question whose caller cancels (the relay now sends MCP
//!   `notifications/cancelled` when it gives up, `vault-app` `keeper/relay.rs`)
//!   simply leaves the line: the server drops its future.
//! - It learns how long a turn takes (the median of the last few, counting
//!   only turns that finished), and a newcomer who could not be answered
//!   within [`DESK_BUDGET`] is told at once that the vault is busy
//!   ([`MSG_BUSY`]), with words the agent can act on, instead of waiting out
//!   the whole budget to fail. Until it has seen a turn it never says busy.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, SemaphorePermit};

use crate::RELAY_CALL_BUDGET;

/// How long a question may expect to take, waiting included, before the desk
/// says "busy" instead: the relay's budget less a margin for the answer to
/// travel back.
pub const DESK_BUDGET: Duration = RELAY_CALL_BUDGET.saturating_sub(Duration::from_secs(5));

/// What an agent is told when the line is too long to answer in time. It says
/// what to do, so the agent asks again rather than reporting a failure.
pub const MSG_BUSY: &str = "Zaaheen is still answering other questions and answers one at a \
     time. Ask again in a moment, and ask one question at a time, waiting for each answer.";

/// How many finished turns the estimate looks at.
const RECENT: usize = 3;

/// The line is too long for this question to be answered in time.
#[derive(Debug, PartialEq, Eq)]
pub struct Busy;

/// One seat, shared by every connection a keeper serves.
pub struct ReadDesk {
    seat: Semaphore,
    waiting: AtomicUsize,
    recent: Mutex<VecDeque<Duration>>,
    /// When the current turn began, if one is under way.
    serving_since: Mutex<Option<Instant>>,
}

impl Default for ReadDesk {
    fn default() -> Self {
        Self {
            seat: Semaphore::new(1),
            waiting: AtomicUsize::new(0),
            recent: Mutex::new(VecDeque::with_capacity(RECENT)),
            serving_since: Mutex::new(None),
        }
    }
}

impl ReadDesk {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// How long a turn takes: the median of the last finished turns, or
    /// `None` before any has finished.
    fn turn(&self) -> Option<Duration> {
        let recent = self.recent.lock().ok()?;
        if recent.is_empty() {
            return None;
        }
        let mut sorted: Vec<Duration> = recent.iter().copied().collect();
        sorted.sort_unstable();
        Some(sorted[sorted.len() / 2])
    }

    /// How long a newcomer would take: what is left of the current turn, a
    /// turn for each question already waiting, and its own.
    fn expected_wait(&self, turn: Duration) -> Duration {
        let current_left = self
            .serving_since
            .lock()
            .ok()
            .and_then(|since| *since)
            .map_or(Duration::ZERO, |since| turn.saturating_sub(since.elapsed()));
        let waiting = u32::try_from(self.waiting.load(Ordering::SeqCst)).unwrap_or(u32::MAX);
        current_left + turn.saturating_mul(waiting) + turn
    }

    /// Take the seat, waiting in line for it; or [`Busy`] at once when the
    /// wait plus this question's own turn would exceed `budget`. Dropping the
    /// returned future (a cancelled request) leaves the line.
    ///
    /// # Errors
    ///
    /// [`Busy`], as above.
    pub async fn sit(&self, budget: Duration) -> Result<Seat<'_>, Busy> {
        let occupied = self.seat.available_permits() == 0;
        if occupied {
            if let Some(turn) = self.turn() {
                if self.expected_wait(turn) > budget {
                    return Err(Busy);
                }
            }
        }
        let _waiting = Waiting::join(&self.waiting);
        // Fails only once the desk is closed (a keeper handing over): the
        // question leaves as busy rather than being served unseated.
        let permit = self.seat.acquire().await.map_err(|_| Busy)?;
        if let Ok(mut since) = self.serving_since.lock() {
            *since = Some(Instant::now());
        }
        Ok(Seat {
            desk: self,
            _permit: permit,
            began: Instant::now(),
        })
    }

    /// The keeper is handing over (ADR-SEC-033 D3): everyone waiting leaves at
    /// once as [`Busy`], and so does every later question. The turn under way
    /// keeps its seat and finishes.
    pub fn close(&self) {
        self.seat.close();
    }

    fn finished(&self, took: Duration) {
        if let Ok(mut recent) = self.recent.lock() {
            if recent.len() == RECENT {
                recent.pop_front();
            }
            recent.push_back(took);
        }
    }
}

/// Counts a question in line until it has the seat or has left.
struct Waiting<'a>(&'a AtomicUsize);

impl<'a> Waiting<'a> {
    fn join(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A question's turn. Dropping it frees the seat for the next.
pub struct Seat<'a> {
    desk: &'a ReadDesk,
    _permit: SemaphorePermit<'a>,
    began: Instant,
}

impl Seat<'_> {
    /// The turn finished with an answer: its length teaches the estimate. A
    /// turn cut short (cancelled) is not counted, since it would teach the
    /// desk that turns are shorter than they are.
    pub fn done(self) {
        self.desk.finished(self.began.elapsed());
    }
}

impl Drop for Seat<'_> {
    fn drop(&mut self) {
        if let Ok(mut since) = self.desk.serving_since.lock() {
            *since = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(50);

    #[tokio::test]
    async fn questions_take_turns() {
        let desk = ReadDesk::new();
        let first = desk.sit(LONG).await.unwrap();
        let d = Arc::clone(&desk);
        let second = tokio::spawn(async move {
            let seat = d.sit(LONG).await.unwrap();
            seat.done();
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!second.is_finished(), "the second waits for the first");
        assert_eq!(desk.waiting.load(Ordering::SeqCst), 1);
        first.done();
        second.await.unwrap();
        assert_eq!(desk.waiting.load(Ordering::SeqCst), 0);
    }

    /// A cancelled question (its future dropped) leaves the line at once and
    /// never takes a turn: the heart of the fix.
    #[tokio::test]
    async fn a_cancelled_question_leaves_the_line_without_a_turn() {
        let desk = ReadDesk::new();
        let first = desk.sit(LONG).await.unwrap();
        let d = Arc::clone(&desk);
        let turned = Arc::new(AtomicUsize::new(0));
        let t = Arc::clone(&turned);
        let abandoned = tokio::spawn(async move {
            let _seat = d.sit(LONG).await.unwrap();
            t.fetch_add(1, Ordering::SeqCst);
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        abandoned.abort();
        let _ = abandoned.await;
        assert_eq!(desk.waiting.load(Ordering::SeqCst), 0, "left the line");
        first.done();
        let next = desk.sit(LONG).await.expect("the seat is free for the next");
        next.done();
        assert_eq!(
            turned.load(Ordering::SeqCst),
            0,
            "the abandoned one never had a turn"
        );
    }

    /// With turns known to take 15 s and a 50 s budget: the second question
    /// expects 15 + 15 = 30 s and the third 15 + 15 + 15 = 45 s, so both wait;
    /// the fourth would need 15 + 30 + 15 = 60 s and is told "busy" at once.
    #[tokio::test]
    async fn a_question_that_cannot_be_answered_in_time_hears_busy_at_once() {
        let desk = ReadDesk::new();
        for _ in 0..RECENT {
            desk.finished(Duration::from_secs(15));
        }
        let first = desk.sit(LONG).await.unwrap();
        assert!(
            desk.sit(Duration::from_secs(1)).await.is_err(),
            "a tiny budget cannot wait for anyone"
        );
        let d = Arc::clone(&desk);
        let second = tokio::spawn(async move { d.sit(LONG).await.map(Seat::done) });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let d = Arc::clone(&desk);
        let third = tokio::spawn(async move { d.sit(LONG).await.map(Seat::done) });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let started = Instant::now();
        assert_eq!(
            desk.sit(LONG).await.err(),
            Some(Busy),
            "the fourth hears busy"
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "and at once"
        );
        first.done();
        assert!(second.await.unwrap().is_ok());
        assert!(third.await.unwrap().is_ok());
    }

    /// Before any turn has finished (a keeper just started, the model still
    /// loading), the desk cannot know, so it never says busy.
    #[tokio::test]
    async fn before_any_turn_the_desk_never_says_busy() {
        let desk = ReadDesk::new();
        let first = desk.sit(LONG).await.unwrap();
        let d = Arc::clone(&desk);
        let second =
            tokio::spawn(async move { d.sit(Duration::from_millis(1)).await.map(Seat::done) });
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(first);
        assert!(second.await.unwrap().is_ok(), "waited instead of busy");
    }

    /// A turn cut short does not teach the estimate.
    #[tokio::test]
    async fn only_finished_turns_are_learned() {
        let desk = ReadDesk::new();
        drop(desk.sit(LONG).await.unwrap());
        assert_eq!(desk.turn(), None);
        desk.sit(LONG).await.unwrap().done();
        assert!(desk.turn().is_some());
    }

    /// A keeper handing over closes the desk: questions waiting in line leave
    /// at once as busy, newcomers too, and the one being answered finishes.
    #[tokio::test]
    async fn a_closed_desk_sends_the_line_away_and_lets_the_current_turn_finish() {
        let desk = ReadDesk::new();
        let first = desk.sit(LONG).await.unwrap();
        let d = Arc::clone(&desk);
        let waiting = tokio::spawn(async move { d.sit(LONG).await.map(Seat::done) });
        tokio::time::sleep(Duration::from_millis(50)).await;

        desk.close();
        let left = tokio::time::timeout(Duration::from_millis(500), waiting)
            .await
            .expect("the waiting question leaves at once")
            .unwrap();
        assert_eq!(left, Err(Busy));
        assert_eq!(desk.sit(LONG).await.err(), Some(Busy), "no newcomers");
        first.done();
        assert_eq!(desk.waiting.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_budget_leaves_room_for_the_answer_to_travel() {
        assert!(DESK_BUDGET < RELAY_CALL_BUDGET);
        assert!(DESK_BUDGET >= Duration::from_secs(45));
    }
}
