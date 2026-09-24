//! Where the desktop's link to the keeper is (ADR-108 D6), for the home
//! screen's "Opening your memories…".
//!
//! **Open, before the lock** — a widening of the locked allowlist, approved
//! with ADR-108 (session 60): it tells the page only whether the background
//! part is connecting, serving, tidying, or could not start (with the same
//! startup message the desktop showed before D4). No vault data, no account
//! data. It reads files and state only: it never starts a keeper (review
//! A-S8).

use serde_json::Value;
use tauri::State;

use crate::link::KeeperLink;

/// `{"state": "connecting" | "serving" | "tidying" | "failed", "message"?}`.
#[tauri::command]
pub async fn link_state(link: State<'_, KeeperLink>) -> Result<Value, String> {
    Ok(link.state().to_json())
}

#[cfg(test)]
mod tests {
    /// Given the link and nothing else — no account, no key, no vault
    /// handle — and it never calls the keeper.
    #[test]
    fn link_state_reads_and_never_calls() {
        let source = include_str!("keeper.rs").replace("\r\n", "\n");
        let code = source
            .split_once("#[cfg(test)]")
            .map_or(source.as_str(), |(c, _)| c);
        for forbidden in [
            "Entitlement",
            "AccountSlot",
            "keychain",
            "MasterKey",
            ".call(",
            "Application",
        ] {
            assert!(!code.contains(forbidden), "{forbidden}");
        }
        assert!(code.contains("pub async fn link_state(link: State<'_, KeeperLink>)"));
    }
}
