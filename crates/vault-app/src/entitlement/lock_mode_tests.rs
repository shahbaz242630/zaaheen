//! Lock mode's check (`SIGNIN-DESIGN.md` §8.26 §6.2, §8.35).
//!
//! The real check is a stand-in here, so nothing touches Credential Manager,
//! the network or the disk. The flip signal is inspected directly rather than
//! awaited, so every test is deterministic and none sleeps.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use vault_mcp::{EntitlementCheck, LockReason, Verdict};

use super::{Flip, LockModeCheck};

/// A check that answers what the test scripted, counting the asks.
struct ScriptedCheck {
    answers: Vec<Verdict>,
    asked: AtomicUsize,
}

impl ScriptedCheck {
    fn always(verdict: Verdict) -> Arc<Self> {
        Self::answering(vec![verdict])
    }

    /// One answer per ask; the last one repeats.
    fn answering(answers: Vec<Verdict>) -> Arc<Self> {
        Arc::new(Self {
            answers,
            asked: AtomicUsize::new(0),
        })
    }

    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EntitlementCheck for ScriptedCheck {
    async fn check(&self) -> Verdict {
        let n = self.asked.fetch_add(1, Ordering::SeqCst);
        let last = self.answers.len() - 1;
        self.answers[n.min(last)]
    }
}

/// Every reason a real denial can carry, so nothing is special-cased.
const DENIALS: [LockReason; 4] = [
    LockReason::SignedOut,
    LockReason::CannotConfirm,
    LockReason::TrialEnded,
    LockReason::SubscriptionEnded,
];

fn lock_mode(real: Arc<ScriptedCheck>) -> (LockModeCheck, Flip) {
    let flip = Flip::new();
    (LockModeCheck::new(real, flip.clone()), flip)
}

/// §6.2: "A lock-mode call that finds the user now entitled answers 'Zaaheen
/// is unlocking. Try again in a moment.'" A yes is never served in lock mode
/// — there is no vault behind this keeper to serve it with.
#[tokio::test]
async fn a_call_that_finds_the_user_entitled_answers_unlocking() {
    let (check, _flip) = lock_mode(ScriptedCheck::always(Verdict::Entitled));
    assert_eq!(
        check.check().await,
        Verdict::Locked(LockReason::Unlocking),
        "lock mode must answer unlocking, not serve the call"
    );
}

/// ...and "triggers the exit", so a user who has just paid is unlocked on
/// their next call rather than waiting for a tick.
#[tokio::test]
async fn a_call_that_finds_the_user_entitled_raises_the_flip() {
    let (check, flip) = lock_mode(ScriptedCheck::always(Verdict::Entitled));
    assert!(!flip.raised(), "nothing has happened yet");
    let _ = check.check().await;
    assert!(flip.raised(), "an entitled answer must trigger the exit");
}

/// While the user really is locked, lock mode is a pass-through: the reason
/// the real check gave is the reason the agent reads, word for word.
#[tokio::test]
async fn a_call_that_is_still_locked_passes_the_reason_through() {
    for reason in DENIALS {
        let (check, flip) = lock_mode(ScriptedCheck::always(Verdict::Locked(reason)));
        assert_eq!(
            check.check().await,
            Verdict::Locked(reason),
            "{reason:?} must not be rewritten"
        );
        assert!(!flip.raised(), "{reason:?} is not a reason to change mode");
    }
}

/// The invariant that makes lock mode safe: whatever the real check says,
/// this one never serves a call. The keeper it belongs to has no vault open.
#[tokio::test]
async fn lock_mode_can_never_answer_entitled() {
    let every_verdict = [
        Verdict::Entitled,
        Verdict::Locked(LockReason::SignedOut),
        Verdict::Locked(LockReason::CannotConfirm),
        Verdict::Locked(LockReason::TrialEnded),
        Verdict::Locked(LockReason::SubscriptionEnded),
        Verdict::Locked(LockReason::Unlocking),
    ];
    for verdict in every_verdict {
        let (check, _flip) = lock_mode(ScriptedCheck::always(verdict));
        assert_ne!(
            check.check().await,
            Verdict::Entitled,
            "lock mode served a call after the real check said {verdict:?}"
        );
    }
}

/// The real check is asked exactly once per call: it is the expensive one
/// (refresh-then-decide, up to 5 s), and asking twice would double a locked
/// call's cost.
#[tokio::test]
async fn each_call_asks_the_real_check_once() {
    let real = ScriptedCheck::always(Verdict::Locked(LockReason::TrialEnded));
    let (check, _flip) = lock_mode(real.clone());
    for _ in 0..3 {
        let _ = check.check().await;
    }
    assert_eq!(real.asked(), 3);
}

/// Several calls can find the user entitled at once (an AI app makes
/// concurrent calls). Raising the flip twice is not an error, and it stays
/// raised so the keeper cannot miss it.
#[tokio::test]
async fn the_flip_stays_raised_and_is_safe_to_raise_twice() {
    let (check, flip) = lock_mode(ScriptedCheck::always(Verdict::Entitled));
    let _ = check.check().await;
    let _ = check.check().await;
    assert!(flip.raised());
    // Already raised, so waiting on it resolves at once.
    flip.wait().await;
}

/// A keeper that dropped its end of the signal must not make a call panic:
/// the call still answers, and the keeper is on its way out anyway.
#[tokio::test]
async fn a_call_still_answers_when_the_keeper_has_stopped_listening() {
    let flip = Flip::new();
    let check = LockModeCheck::new(ScriptedCheck::always(Verdict::Entitled), flip);
    assert_eq!(check.check().await, Verdict::Locked(LockReason::Unlocking));
}
