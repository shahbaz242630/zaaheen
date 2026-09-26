//! The links this application may hand to the operating system, and the last
//! check before it does (S3 step 4b; the fourth, Cursor's install request,
//! ADR-106 in `CONNECT-APPS-DESIGN.md`).
//!
//! # Why this is a type and not a function
//!
//! Sign-in and Subscribe both end with the person's browser opening
//! something. Two of the three links are built from values the **account
//! service supplied** — a transaction id, a portal URL — so if that service
//! were ever impersonated or compromised, whatever it returned would be a
//! string this process hands to `ShellExecute`. §8.26 §5 is blunt about it:
//! *"Nothing from the server reaches the OS unchecked."*
//!
//! So there is no `open(&str)` anywhere. [`ExternalLink`] wraps a `Url`, its
//! field is private, and its three constructors are the only ways to make
//! one. Each takes something already validated — a [`TransactionId`] or a
//! [`PortalUrl`] from `vault-account`, or an authorize URL this process built
//! itself — and every one of them ends at the same [`ExternalLink::checked`]
//! gate. "Opened something unvalidated" is not a sentence the calling code
//! can say.
//!
//! **No Tauri command takes a URL.** The desktop asks for "the sign-in link"
//! or "the subscribe link"; the answer is built here. Nothing the frontend
//! sends can steer it, which is what keeps ADR-030's rule — no
//! user-controlled input reaching a process launcher — true for this path
//! too.
//!
//! # The launcher
//!
//! The `open` crate, with `default-features = false` and **never** its
//! `insecure` feature, whose own documentation says it *"restores the legacy
//! `cmd /c start` launcher on Windows"* and *"must not be enabled when paths
//! or URLs may be attacker-controlled."* Ours may be.

use std::fmt;

use url::Url;
use vault_account::{PortalUrl, TransactionId};

use crate::server_command::ServerCommand;

/// The pay page, locked by `SIGNIN-DESIGN.md` §8.31 — **with** the trailing
/// slash, which the site's redirect otherwise adds and Paddle's parameter
/// does not survive.
const PAY_PAGE: &str = "https://zaaheen.com/pay/";

/// Paddle's transaction parameter on that page.
const PAY_PARAM: &str = "_ptxn";

/// Cursor's own install request for Zaaheen (ADR-106): Cursor asks the person
/// and writes its own settings. `config` carries the server, `{"command":
/// <the ServerCommand>, "args":["mcp","serve"]}`, in base64, which is how
/// Cursor's documented links carry it; the command is the installed program's
/// full path (ADR-111), so the base64 is percent-encoded, which Cursor
/// decodes (session 64 spike).
const CURSOR_INSTALL_BASE: &str = "cursor://anysphere.cursor-deeplink/mcp/install";

/// A link this application is allowed to open.
#[derive(Clone, PartialEq, Eq)]
pub struct ExternalLink(Url);

/// Why a link was refused. Carries no detail: a caller shows the person a
/// fixed line, and the specifics go to the log (BRD §11.7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// The link was not `https`, or carried credentials.
    NotAllowed,
    /// The browser could not be started.
    CouldNotOpen,
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkError::NotAllowed => f.write_str("that link is not one this app may open"),
            LinkError::CouldNotOpen => f.write_str("the browser could not be opened"),
        }
    }
}

impl std::error::Error for LinkError {}

impl ExternalLink {
    /// The sign-in page, as built by `vault_account::PendingSignIn`.
    ///
    /// # Errors
    ///
    /// [`LinkError::NotAllowed`] if it is not `https` without credentials.
    pub fn sign_in(authorize: &Url) -> Result<Self, LinkError> {
        Self::checked(authorize.clone())
    }

    /// The pay page for a transaction the checkout client already validated.
    ///
    /// # Errors
    ///
    /// [`LinkError::NotAllowed`] only if [`PAY_PAGE`] itself were edited into
    /// something that is not `https` — kept as a check rather than an
    /// `expect` so a bad edit is a refusal, never a panic in front of a user.
    pub fn pay(txn: &TransactionId) -> Result<Self, LinkError> {
        let mut url = Url::parse(PAY_PAGE).map_err(|_| LinkError::NotAllowed)?;
        // `append_pair` percent-encodes, so even a transaction id that had
        // slipped past its own check could not add a second parameter here.
        url.query_pairs_mut().append_pair(PAY_PARAM, txn.as_str());
        Self::checked(url)
    }

    /// The billing portal, as validated by `vault_account::PortalUrl`.
    ///
    /// # Errors
    ///
    /// [`LinkError::NotAllowed`] if it somehow fails this gate too. `PortalUrl`
    /// is stricter (it also pins the host), so this is belt and braces.
    pub fn portal(portal: &PortalUrl) -> Result<Self, LinkError> {
        let url = Url::parse(portal.as_str()).map_err(|_| LinkError::NotAllowed)?;
        Self::checked(url)
    }

    /// Cursor's install request for Zaaheen (ADR-106, ADR-111). Not a web
    /// page, so it cannot pass [`Self::checked`] (`https` only); it has its
    /// own, narrower gate instead ([`Self::is_cursor_install`]). `command`
    /// is a [`ServerCommand`], which only the operating system's answer to
    /// "where is this program" can make: nothing from the page.
    ///
    /// # Errors
    ///
    /// [`LinkError::NotAllowed`] only if the built link failed its own gate.
    pub fn install_in_cursor(command: &ServerCommand) -> Result<Self, LinkError> {
        use base64::Engine as _;

        let server =
            serde_json::to_vec(&command.server_json()).map_err(|_| LinkError::NotAllowed)?;
        let config = base64::engine::general_purpose::STANDARD.encode(server);
        let mut url = Url::parse(CURSOR_INSTALL_BASE).map_err(|_| LinkError::NotAllowed)?;
        // The query encoder percent-encodes `+`, `/` and `=`: a raw `+`
        // would otherwise read as a space.
        url.query_pairs_mut()
            .append_pair("name", "zaaheen")
            .append_pair("config", &config);
        if !Self::is_cursor_install(&url, command) {
            return Err(LinkError::NotAllowed);
        }
        Ok(Self(url))
    }

    /// Cursor's scheme, host and path; no credentials; exactly `name=zaaheen`
    /// and a `config` that decodes to exactly `command`'s server.
    fn is_cursor_install(url: &Url, command: &ServerCommand) -> bool {
        use base64::Engine as _;

        let place = url.scheme() == "cursor"
            && url.host_str() == Some("anysphere.cursor-deeplink")
            && url.path() == "/mcp/install"
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none();
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        let [(name_key, name), (config_key, config)] = pairs.as_slice() else {
            return false;
        };
        let server = base64::engine::general_purpose::STANDARD
            .decode(config)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        place
            && name_key == "name"
            && name == "zaaheen"
            && config_key == "config"
            && server.as_ref() == Some(&command.server_json())
    }

    /// The one gate every web constructor passes through.
    fn checked(url: Url) -> Result<Self, LinkError> {
        // `https` only. This is what stops `file:`, `javascript:`, `data:`
        // and every other scheme the OS would happily act on.
        if url.scheme() != "https" {
            return Err(LinkError::NotAllowed);
        }
        // Credentials are part of the authority, and a browser reads
        // `https://zaaheen.com@evil.test/` as a visit to evil.test.
        if !url.username().is_empty() || url.password().is_some() {
            return Err(LinkError::NotAllowed);
        }
        if url.host_str().is_none() {
            return Err(LinkError::NotAllowed);
        }
        Ok(Self(url))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Hand it to the person's browser.
    ///
    /// # Errors
    ///
    /// [`LinkError::CouldNotOpen`] if no browser could be started. The
    /// underlying reason goes to the log, never to the caller.
    #[tracing::instrument(skip_all)]
    pub fn open(&self) -> Result<(), LinkError> {
        // `open::that` and NOT `open::with`: the crate's docs note that an
        // application chosen with `with()` has its own command-line grammar,
        // so the crate cannot promise a dash-leading argument is treated as
        // data. We never choose the application.
        open::that(self.0.as_str()).map_err(|e| {
            tracing::warn!(error = %e, "could not open a link in the browser");
            LinkError::CouldNotOpen
        })
    }
}

/// Never prints the query, which carries the transaction id.
impl fmt::Debug for ExternalLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ExternalLink")
            .field(&format_args!(
                "{}://{}{}",
                self.0.scheme(),
                self.0.host_str().unwrap_or("?"),
                self.0.path()
            ))
            .finish()
    }
}

#[cfg(test)]
mod tests;
