//! "Delete everything" command — ADR-SEC-008.
//!
//! Exposes [`vault_app::erase_vault`] to the Settings tab: destroy the
//! master_key in the OS credential store (cryptographic erasure, BRD
//! §11.5.4), then remove the vault's data files.
//!
//! ## Why this command exists at all
//!
//! Uninstalling Memory Vault does NOT remove vault data — Windows Installer
//! leaves user data behind by design, and deleting someone's memories as a
//! side effect of a routine uninstall is unrecoverable data loss. Keeping
//! the data is the right default; keeping it with no deliberate way to
//! destroy it is not. This is that deliberate way.
//!
//! ## ADR-086 (UI content policy)
//!
//! User-facing strings here name no vendor, model, or database engine. The
//! deliberate ADR-086 carve-out for security specifics (encryption standard,
//! OS credential store) DOES apply to this surface — a user deleting their
//! data is entitled to know the key is destroyed in the operating system's
//! credential store, because that is the substance of the guarantee.
//!
//! ## Honest reporting
//!
//! The command reports what actually happened. It never claims success on
//! the key step when that step failed, because a failed key destruction
//! means the data is still readable — the single thing the user is trying
//! to prevent. Files that could not be deleted are reported separately and
//! explicitly do NOT downgrade the confidentiality result: once the key is
//! gone those bytes are undecryptable ciphertext.

use std::time::Duration;

use tauri::State;
use vault_app::keeper::exclusive::take_exclusive;
use vault_app::keeper::relay::KeychainKeySource;
use vault_app::keychain::KeyLocation;
use vault_app::Application;

use crate::commands::account::AccountSlot;

/// Opaque error code: the vault could not be taken for erasure (a maintenance
/// run holds it). Nothing was deleted.
pub const ERR_ERASURE_BUSY: &str = "erasure_busy";

/// Opaque error code: erasure failed at the key step. Nothing was deleted.
pub const ERR_ERASURE_FAILED: &str = "erasure_failed";

/// How long to wait for the vault to be handed over. A serving keeper lets go
/// in well under a second; one that is still opening the vault takes a few
/// seconds; a maintenance run is not interrupted and exhausts this.
const TAKE_WAIT: Duration = Duration::from_secs(20);

/// Bound on each step of the handover request.
const TAKE_STEP: Duration = Duration::from_secs(5);

/// Inner implementation, `Application`-only so it is testable without a
/// Tauri runtime (same pattern as the other command modules).
///
/// # Errors
///
/// Returns an opaque error code when cryptographic erasure fails or cannot
/// start. The caller MUST treat this as "the vault was NOT erased".
pub async fn erase_everything_inner(app: &Application) -> Result<serde_json::Value, String> {
    let vault_root = app.vault_root().to_path_buf();

    // ADR-102 amendment (session 35): first take the vault from the keeper
    // serving any connected AI app. Left running, it would keep answering
    // that app from its open stores after the user was told everything was
    // deleted. Held until the erasure is over, so no keeper starts meanwhile.
    let held = take_exclusive(&vault_root, &KeychainKeySource, TAKE_WAIT, TAKE_STEP)
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "vault_tauri::erasure",
                error = %e,
                "erasure not started: the vault is in use; nothing was deleted"
            );
            ERR_ERASURE_BUSY.to_string()
        })?;

    // Blocking work (credential store + recursive file removal) off the
    // async runtime, per BRD §2.
    // ADR-SEC-029 E1: the key step and the files run under the one key lock,
    // so no process can open, move or restore the key meanwhile.
    let outcome = tokio::task::spawn_blocking(move || {
        KeyLocation::production().and_then(|key| {
            let outcome = vault_app::erase_vault(&vault_root, &key)?;
            // ADR-105 L5: an earlier move's old copy that could not be
            // removed yet goes too — its key is gone with this one. Best
            // effort: a copy still in use is removed at the next start.
            if let Ok(homes) = vault_app::location::Homes::production() {
                let old_copy = vault_app::location::moving::clean_old_copy_now(&homes, &key);
                tracing::info!(target: "vault_tauri::erasure", ?old_copy, "an earlier move's old copy after erasure");
            }
            Ok(outcome)
        })
    })
    .await
    .map_err(|_| ERR_ERASURE_FAILED.to_string())?
    .map_err(|e| {
        tracing::error!(
            target: "vault_tauri::erasure",
            error = %e,
            "erasure FAILED; the vault is still readable"
        );
        ERR_ERASURE_FAILED.to_string()
    })?;
    drop(held);

    // NOTE: no audit row. The audit log lives inside the vault we just
    // destroyed — see the `vault_app::erasure` module docs. The operation
    // is recorded in the application log, which survives.

    Ok(serde_json::json!({
        "key_destroyed": outcome.key_destroyed,
        "entries_removed": outcome.entries_removed,
        "undeletable_count": outcome.undeletable.len(),
        // Always true once we reach here: the key step either succeeded or
        // returned Err above. Sent explicitly so the UI never has to infer
        // the security outcome from the file counts.
        "data_is_unrecoverable": outcome.data_is_unrecoverable(),
    }))
}

/// Permanently destroy every memory in this vault, then sign this computer
/// out (§8.26 §7).
///
/// Irreversible by design and by mechanism: the key is destroyed, so the
/// data cannot be recovered from a backup of the data directory either.
///
/// # Errors
///
/// `"erasure_failed"` when the key could not be destroyed, `"erasure_busy"`
/// when the vault could not be taken (a maintenance run is using it). No
/// files are removed in either case — erasure either succeeds at the step
/// that matters or does nothing at all — and the person stays signed in.
#[tauri::command]
pub async fn erase_everything(
    app: State<'_, Application>,
    account: State<'_, AccountSlot>,
) -> Result<serde_json::Value, String> {
    let erased = erase_everything_inner(&app).await;
    sign_out_if_erased(erased, || account.sign_out_after_erasure()).await
}

/// §8.26 §7: *"Delete everything (erasure + local sign-out + revoke)"*.
///
/// The erasure alone decides what the person is told: `sign_out` runs only
/// once it has succeeded, is best effort, and cannot change the result. A
/// failed erasure changes nothing else — the person stays signed in, because
/// nothing was deleted (§8.38).
async fn sign_out_if_erased<T, E, F, Fut>(erased: Result<T, E>, sign_out: F) -> Result<T, E>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if erased.is_ok() {
        sign_out().await;
    }
    erased
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[tokio::test]
    async fn a_completed_erasure_signs_out_and_still_reports_the_erasure() {
        let signed_out = AtomicBool::new(false);
        let result: Result<&str, String> = sign_out_if_erased(Ok("erased"), || async {
            signed_out.store(true, Ordering::SeqCst);
        })
        .await;
        assert_eq!(result, Ok("erased"));
        assert!(signed_out.load(Ordering::SeqCst), "§7: erasure signs out");
    }

    /// Nothing was deleted, so nothing else may change: signing somebody out
    /// after a failed erasure would be a second surprise on top of the first.
    #[tokio::test]
    async fn a_failed_erasure_leaves_the_person_signed_in() {
        let signed_out = AtomicBool::new(false);
        let result: Result<&str, String> =
            sign_out_if_erased(Err(ERR_ERASURE_BUSY.to_string()), || async {
                signed_out.store(true, Ordering::SeqCst);
            })
            .await;
        assert_eq!(result, Err(ERR_ERASURE_BUSY.to_string()));
        assert!(!signed_out.load(Ordering::SeqCst));
    }

    /// The wrapper must actually go through the helper, or the two tests
    /// above prove nothing about the command.
    #[test]
    fn the_command_signs_out_through_the_erasure_first_helper() {
        // Either line ending: git autocrlf can hand a checkout CRLF.
        let source = include_str!("erasure.rs").replace("\r\n", "\n");
        let command = source
            .split_once("pub async fn erase_everything(")
            .expect("the command is defined here")
            .1
            .split_once("\n}\n")
            .expect("the command is closed")
            .0;
        assert!(command.contains("sign_out_if_erased("));
        assert_eq!(
            command.matches("sign_out_after_erasure").count(),
            1,
            "sign-out must be reached only through the helper"
        );
    }

    /// ADR-105 L5: "Delete everything" also removes an earlier move's old
    /// copy — only once the key is destroyed (`?` on the erasure first).
    #[test]
    fn erasure_also_removes_an_old_copy_after_the_key_is_gone() {
        let source = include_str!("erasure.rs").replace("\r\n", "\n");
        let body = source
            .split_once("pub async fn erase_everything_inner(")
            .expect("the inner command is defined here")
            .1
            .split_once("\n}\n")
            .expect("the inner command is closed")
            .0;
        let erased = body
            .find("vault_app::erase_vault(&vault_root, &key)?")
            .expect("the erasure, its failure returned first");
        let cleaned = body
            .find("clean_old_copy_now(")
            .expect("the old copy is removed too");
        assert!(erased < cleaned, "never before the key is destroyed");
    }
}
