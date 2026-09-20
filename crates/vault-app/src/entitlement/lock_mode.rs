//! Lock mode's check and its flip signal (ADR-104; `SIGNIN-DESIGN.md` §8.26
//! §6.2, with amendment 9, §8.35).
//!
//! A locked keeper serves the same MCP surface as a full one — same tool
//! list, same `WIRE`, same discovery role — over [`NoVaultAdapter`], with no
//! `Application` and no models behind it. [`LockModeCheck`] is what makes it
//! safe: it asks the real check, and whatever the answer, it never serves a
//! call. There is no vault open to serve one with.
//!
//! When the real check says the user is entitled again, the call is answered
//! with [`LockReason::Unlocking`] and [`Flip`] is raised, so the keeper exits
//! at once (`KeeperExit::ModeChanged`) instead of waiting for the next tick.
//! The next relay then starts a full keeper.
//!
//! [`NoVaultAdapter`]: vault_mcp::NoVaultAdapter

#[cfg(test)]
#[path = "lock_mode_tests.rs"]
mod lock_mode_tests;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::watch;
use vault_mcp::{EntitlementCheck, LockReason, Verdict};

/// Lock mode's signal that entitlement has come back.
///
/// Shaped like [`vault_mcp::InFlight`]: a `watch` channel, so it can be both
/// inspected without waiting and awaited without polling, and raising it
/// twice is not an error. Cheap to clone; clones share the one signal.
#[derive(Clone, Debug)]
pub struct Flip {
    raised: Arc<watch::Sender<bool>>,
}

impl Flip {
    /// A signal that has not been raised.
    pub fn new() -> Self {
        Self {
            raised: Arc::new(watch::Sender::new(false)),
        }
    }

    /// Raise it. Idempotent: several concurrent calls may find the user
    /// entitled at the same moment.
    pub fn raise(&self) {
        self.raised.send_replace(true);
    }

    /// Whether it has been raised.
    pub fn raised(&self) -> bool {
        *self.raised.borrow()
    }

    /// Resolves once it has been raised (at once if it already has).
    pub async fn wait(&self) {
        let mut raised = self.raised.subscribe();
        // `wait_for` looks at the current value first, and fails only if the
        // sender is gone — which `self` holds.
        let _ = raised.wait_for(|raised| *raised).await;
    }
}

impl Default for Flip {
    fn default() -> Self {
        Self::new()
    }
}

/// The check a locked keeper serves behind: the real one, except that a "yes"
/// is never served (§6.2).
pub struct LockModeCheck {
    real: Arc<dyn EntitlementCheck>,
    flip: Flip,
}

impl LockModeCheck {
    /// Wrap `real`, raising `flip` when it reports the user entitled again.
    pub fn new(real: Arc<dyn EntitlementCheck>, flip: Flip) -> Self {
        Self { real, flip }
    }
}

#[async_trait]
impl EntitlementCheck for LockModeCheck {
    async fn check(&self) -> Verdict {
        match self.real.check().await {
            // Entitled again. This keeper has no vault open, so the call
            // cannot be served here: say so, and start the keeper leaving so
            // the next call reaches a full one (§6.2).
            Verdict::Entitled => {
                self.flip.raise();
                Verdict::Locked(LockReason::Unlocking)
            }
            // Still locked: the agent reads the real reason, word for word.
            locked => locked,
        }
    }
}
