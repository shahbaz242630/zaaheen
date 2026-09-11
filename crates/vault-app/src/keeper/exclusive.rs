//! Taking the vault away from a running keeper, for "Delete everything"
//! (ADR-102 amendment, session 35).
//!
//! **The defect this closes.** Erasure destroys the master key and deletes
//! the files, but a keeper serving AI apps holds keys derived from it in
//! memory and the stores open. Left running, it would keep answering an AI
//! app's reads — and taking its writes — after the user was told everything
//! was deleted, until it happened to go idle.
//!
//! **The sequence.**
//! 1. Declare the intent ([`IntentGuard`]): from here on no NEW keeper starts,
//!    however often AI apps ask Windows for one.
//! 2. Ask the serving keeper to hand the vault over (handshake purpose 2). It
//!    removes its discovery file, closes every connection and exits.
//! 3. Take `.vault.lock`. The OS releases the keeper's lock only once its
//!    process is gone, so holding it proves no keeper is left.
//!
//! A maintenance run that holds the vault is not interrupted: the wait runs
//! out and the caller reports "busy" with nothing deleted.

use std::path::Path;
use std::time::Duration;

use tokio::time::Instant;
use vault_core::{VaultError, VaultResult};

use super::discovery;
use super::handshake::{request_handover, HandshakeKeys, KeeperIdentity, CHALLENGE_LEN, NONCE_LEN};
use super::intent::IntentGuard;
use super::relay::MasterKeySource;
use super::transport;
use crate::{ConsolidatorLock, VAULT_LOCKFILE_NAME};

/// How often to re-check while waiting.
const POLL: Duration = Duration::from_millis(100);

/// The vault, held exclusively: no keeper serves it, none can start, and no
/// maintenance run can take it, until this is dropped.
#[derive(Debug)]
pub struct ExclusiveVault {
    // Declaration order is drop order: the vault lock goes first, then the
    // intent, so a keeper that starts the moment the intent clears finds the
    // vault free rather than exiting on a lock we are about to release.
    _lock: ConsolidatorLock,
    _intent: IntentGuard,
}

/// Take the vault exclusively, asking a serving keeper to hand it over.
/// Waits up to `wait` in total; `step` bounds each handshake step.
///
/// # Errors
///
/// [`VaultError::ConsolidatorBusy`] when the vault could not be taken in time
/// (a maintenance run holds it, or a keeper did not let go);
/// [`VaultError::Io`] for anything else. Nothing has been changed either way.
pub async fn take_exclusive(
    vault_root: &Path,
    keys: &dyn MasterKeySource,
    wait: Duration,
    step: Duration,
) -> VaultResult<ExclusiveVault> {
    let deadline = Instant::now() + wait;

    let intent = loop {
        match IntentGuard::acquire(vault_root, Duration::ZERO) {
            Ok(guard) => break guard,
            Err(VaultError::ConsolidatorBusy(_)) if Instant::now() < deadline => {
                tokio::time::sleep(POLL).await;
            }
            Err(e) => return Err(e),
        }
    };

    // Tenures already asked, so a keeper on its way out is not asked again.
    let mut asked: Vec<[u8; NONCE_LEN]> = Vec::new();
    loop {
        match ConsolidatorLock::try_acquire_named(vault_root, VAULT_LOCKFILE_NAME) {
            Ok(lock) => {
                tracing::info!(target: "vault_app::keeper", "vault taken exclusively");
                return Ok(ExclusiveVault {
                    _lock: lock,
                    _intent: intent,
                });
            }
            Err(VaultError::ConsolidatorBusy(msg)) => {
                if Instant::now() >= deadline {
                    return Err(VaultError::ConsolidatorBusy(msg));
                }
            }
            Err(e) => return Err(e),
        }

        // A keeper that was starting when the intent was declared publishes
        // its record once it serves; it is asked then.
        if let Ok(Some(record)) = discovery::read(vault_root) {
            if let Ok(identity) = record.keeper_identity(vault_root) {
                if !asked.contains(&identity.nonce) {
                    match ask_to_hand_over(&identity, keys, step).await {
                        Ok(()) => asked.push(identity.nonce),
                        Err(e) => tracing::debug!(
                            target: "vault_app::keeper",
                            error = %e,
                            "handover request not accepted yet"
                        ),
                    }
                }
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn ask_to_hand_over(
    identity: &KeeperIdentity,
    keys: &dyn MasterKeySource,
    step: Duration,
) -> Result<(), String> {
    let handshake_keys = {
        let master_key = keys.read().ok_or("no key to authenticate with")?;
        HandshakeKeys::derive(&master_key)
    };
    let mut stream = transport::connect(&identity.endpoint)
        .await
        .map_err(|e| e.to_string())?;
    let mut challenge = [0u8; CHALLENGE_LEN];
    getrandom::getrandom(&mut challenge).map_err(|e| e.to_string())?;
    request_handover(&mut stream, &handshake_keys, identity, challenge, step)
        .await
        .map_err(|e| e.to_string())
}
