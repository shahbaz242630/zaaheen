//! A keeper whose account could not be read (ADR-108's amendment to ADR-104).
//!
//! Before D4 the keeper refused to start when its account settings were
//! present but broken, while the desktop kept running as "cannot confirm" so
//! the export stayed reachable. Under D4 the export lives in the keeper, so a
//! refusal would take it away. Instead the keeper serves lock mode behind
//! this check: every call hears "cannot confirm" (fail-secure — nothing is
//! served ungated), and each tick tries the account again. The moment it can
//! be read and says entitled, the keeper leaves so a full one can start.

use std::sync::Arc;

use async_trait::async_trait;
use vault_mcp::{EntitlementCheck, LockReason, Verdict};

use super::ModeCheck;

/// Build the real check again: `Ok(None)` for a build with no account
/// settings, `Err` while it still cannot be read.
pub type Rebuild = Box<dyn Fn() -> Result<Option<Arc<dyn ModeCheck>>, ()> + Send + Sync>;

/// The check a keeper serves behind while its account cannot be read.
pub struct UnreadableAccount {
    rebuild: Rebuild,
}

impl UnreadableAccount {
    pub fn new(rebuild: Rebuild) -> Self {
        Self { rebuild }
    }
}

#[async_trait]
impl EntitlementCheck for UnreadableAccount {
    async fn check(&self) -> Verdict {
        Verdict::Locked(LockReason::CannotConfirm)
    }
}

#[async_trait]
impl ModeCheck for UnreadableAccount {
    async fn peek(&self) -> Verdict {
        match (self.rebuild)() {
            // Readable again: whatever it says now decides.
            Ok(Some(check)) => check.peek().await,
            // A build with no account settings serves everyone.
            Ok(None) => Verdict::Entitled,
            Err(()) => Verdict::Locked(LockReason::CannotConfirm),
        }
    }

    async fn refresh_and_peek(&self) -> Verdict {
        self.peek().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fixed(Verdict);

    #[async_trait]
    impl ModeCheck for Fixed {
        async fn peek(&self) -> Verdict {
            self.0
        }
        async fn refresh_and_peek(&self) -> Verdict {
            self.0
        }
    }

    #[tokio::test]
    async fn every_call_hears_cannot_confirm() {
        let check = UnreadableAccount::new(Box::new(|| Err(())));
        assert_eq!(
            check.check().await,
            Verdict::Locked(LockReason::CannotConfirm)
        );
        assert_eq!(
            check.peek().await,
            Verdict::Locked(LockReason::CannotConfirm)
        );
    }

    /// Once the account can be read and says entitled, the tick sees it (the
    /// keeper then leaves for a full one); calls still never are served here.
    #[tokio::test]
    async fn a_readable_account_decides_the_next_tick() {
        let readable = Arc::new(Mutex::new(false));
        let r = Arc::clone(&readable);
        let check = UnreadableAccount::new(Box::new(move || {
            if *r.lock().unwrap() {
                Ok(Some(
                    Arc::new(Fixed(Verdict::Entitled)) as Arc<dyn ModeCheck>
                ))
            } else {
                Err(())
            }
        }));
        assert_ne!(check.peek().await, Verdict::Entitled);
        *readable.lock().unwrap() = true;
        assert_eq!(check.peek().await, Verdict::Entitled);
        assert_eq!(
            check.check().await,
            Verdict::Locked(LockReason::CannotConfirm),
            "a call is never served by this check"
        );
    }

    #[tokio::test]
    async fn a_readable_account_that_is_locked_stays_locked() {
        let check = UnreadableAccount::new(Box::new(|| {
            Ok(Some(
                Arc::new(Fixed(Verdict::Locked(LockReason::TrialEnded))) as Arc<dyn ModeCheck>,
            ))
        }));
        assert_eq!(check.peek().await, Verdict::Locked(LockReason::TrialEnded));
    }
}
