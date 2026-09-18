//! Error types for `vault-account`.
//!
//! Per BRD §2.4 this crate carries its own `thiserror` enum and converts to
//! [`VaultError`] at the workspace boundary.
//!
//! **No message ever carries a secret.** Every variant's text is either fixed
//! or built from values this crate chose (never a server's body, a code, a
//! verifier or a token), so an error can be logged as-is (BRD §11.7.2).
//!
//! [`AccountError::is_transient`] is the line §8.26 §4 draws: a network error,
//! a 5xx or a keychain error never discards a lease or signs anyone out.

use thiserror::Error;
use vault_core::VaultError;

/// Account-specific failure categories.
#[derive(Debug, Error)]
pub enum AccountError {
    /// The compiled-in account configuration is malformed (issuer, client id).
    #[error("invalid account configuration: {0}")]
    InvalidConfig(String),

    /// The operating system's secure random source failed.
    #[error("secure random source failed: {0}")]
    Random(String),

    /// No valid callback reached the loopback listener within its window
    /// (§8.26 §3: 10 minutes).
    #[error("sign-in window expired")]
    SignInTimedOut,

    /// The account service could not be reached, timed out, or answered 5xx.
    /// Transient: retried later, never treated as "signed out".
    #[error("could not reach the account service: {0}")]
    Network(String),

    /// The token endpoint answered `invalid_grant`: the code or refresh token
    /// is spent, revoked or unknown. §8.26 §15 decides what that means for a
    /// refresh (re-read the store once before concluding "signed out").
    #[error("the authorization grant was rejected")]
    InvalidGrant,

    /// The account service answered, but not in the shape the contract
    /// requires (wrong type, missing field, oversized, not JSON, a redirect).
    #[error("unexpected response from the account service: {0}")]
    Protocol(String),

    /// A lease failed verification or its payload rules (§8.26 §4). Never
    /// transient: "only a bad signature or a valid signed state changes the
    /// decision", and this is the bad-signature half.
    #[error("lease rejected: {0}")]
    LeaseRejected(String),

    /// The OS credential store failed (locked, busy, unavailable). Transient:
    /// §8.26 §3 "keychain errors are transient", never "signed out".
    #[error("the credential store failed: {0}")]
    Keychain(String),

    /// Another Zaaheen process held `refresh.lock` for the whole wait.
    /// Transient: that process is refreshing or signing out right now.
    #[error("the account is busy in another Zaaheen process")]
    Busy,

    /// Local I/O failure (binding the loopback listener).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl AccountError {
    /// True when the failure says nothing about the user's account and the
    /// operation should simply be tried again later (§8.26 §4).
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            AccountError::Network(_) | AccountError::Keychain(_) | AccountError::Busy
        )
    }
}

/// Standard result alias used throughout `vault-account`.
pub type AccountResult<T> = Result<T, AccountError>;

impl From<AccountError> for VaultError {
    fn from(value: AccountError) -> Self {
        match value {
            AccountError::InvalidConfig(msg) => VaultError::Config(msg),
            AccountError::Random(msg) => VaultError::Crypto(msg),
            AccountError::Keychain(msg) => VaultError::KeychainProvenance(msg),
            AccountError::Io(err) => VaultError::Io(err),
            other @ (AccountError::SignInTimedOut
            | AccountError::Network(_)
            | AccountError::InvalidGrant
            | AccountError::Protocol(_)
            | AccountError::LeaseRejected(_)
            | AccountError::Busy) => VaultError::Auth(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_network_keychain_and_busy_failures_are_transient() {
        assert!(AccountError::Network("timed out".into()).is_transient());
        assert!(AccountError::Keychain("busy".into()).is_transient());
        assert!(AccountError::Busy.is_transient());
        assert!(!AccountError::InvalidGrant.is_transient());
        assert!(!AccountError::Protocol("not json".into()).is_transient());
        assert!(!AccountError::SignInTimedOut.is_transient());
        assert!(!AccountError::LeaseRejected("bad signature".into()).is_transient());
        assert!(!AccountError::InvalidConfig("x".into()).is_transient());
    }

    #[test]
    fn account_failures_map_to_the_auth_category() {
        for err in [
            AccountError::SignInTimedOut,
            AccountError::Network("x".into()),
            AccountError::InvalidGrant,
            AccountError::Protocol("x".into()),
            AccountError::LeaseRejected("x".into()),
        ] {
            assert!(matches!(VaultError::from(err), VaultError::Auth(_)));
        }
    }

    #[test]
    fn config_random_and_io_keep_their_own_categories() {
        assert!(matches!(
            VaultError::from(AccountError::InvalidConfig("x".into())),
            VaultError::Config(_)
        ));
        assert!(matches!(
            VaultError::from(AccountError::Random("x".into())),
            VaultError::Crypto(_)
        ));
        assert!(matches!(
            VaultError::from(AccountError::Keychain("x".into())),
            VaultError::KeychainProvenance(_)
        ));
        let io: AccountError = std::io::Error::other("simulated").into();
        assert!(matches!(VaultError::from(io), VaultError::Io(_)));
    }
}
