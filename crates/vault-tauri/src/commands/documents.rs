//! Settings › Documents (founder, session 69; ADR-SEC-036): open a guide or
//! policy on the website in the person's browser.
//!
//! Gated, like the rest of Settings past the lock: the page is only reached
//! by somebody already in. The page names only **which** document, from the
//! closed list `vault_app::external_link::Document`; the address is looked up
//! there, so no URL crosses the IPC boundary from the page (ADR-030's rule).

use serde_json::{json, Value};
use tauri::State;
use vault_app::external_link::{Document, ExternalLink};

use crate::guard::Entitlement;

/// The document's address could not be built. Not expected: every listed
/// document has a fixed address, which `vault-app` tests.
pub const ERR_DOCUMENT_REFUSED: &str = "document_refused";

/// Open `doc`. Answers `{ "page": address, "opened": bool }`: the address is
/// display-only, for the line the page shows when the browser did not open.
///
/// # Errors
///
/// A lock code while locked; [`ERR_DOCUMENT_REFUSED`] if the address could
/// not be built. A name outside the list is refused by Tauri before this
/// runs.
#[tauri::command]
pub async fn open_document(
    entitlement: State<'_, Entitlement>,
    doc: Document,
) -> Result<Value, String> {
    let _entitled = entitlement.require().await?;
    let link = ExternalLink::document(doc).map_err(|_| ERR_DOCUMENT_REFUSED.to_string())?;
    let opened = link.open().is_ok();
    tracing::info!(target: "vault_tauri::documents", ?doc, opened, "opened a document");
    Ok(json!({ "page": link.as_str(), "opened": opened }))
}

#[cfg(test)]
mod tests {
    /// ADR-SEC-036: the page names a document and nothing else.
    #[test]
    fn the_command_takes_nothing_but_the_document() {
        let code: String = include_str!("documents.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let args = code
            .split_once("pub async fn open_document(")
            .expect("open_document is defined here")
            .1
            .split_once(") -> ")
            .expect("its arguments close")
            .0;
        let args: Vec<&str> = args
            .split('\n')
            .map(|a| a.trim().trim_end_matches(','))
            .filter(|a| !a.is_empty())
            .collect();
        assert_eq!(
            args,
            ["entitlement: State<'_, Entitlement>", "doc: Document"]
        );
    }
}
