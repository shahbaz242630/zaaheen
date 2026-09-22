//! Browser sign-in: PKCE authorization request plus the loopback listener
//! that receives the redirect (§8.26 §3, RFC 8252 §7.3).
//!
//! # The listener's rules (§8.26 §3, quoted)
//!
//! > listener on `127.0.0.1:0`, concurrent connections, ≤ 8 KB and ≤ 5 s
//! > each, 10-minute window, only `GET /callback` considered, `state`
//! > (constant time) **and** `iss` must match, `error=` ends the flow with a
//! > static page, no kill-after-N
//!
//! and from v2 §3.3: any other path → 404; success → a static page; no
//! script, no reflected input.
//!
//! # How this crate reads them
//!
//! - **One request per connection**, then close. A second request pipelined
//!   on the same connection is never looked at.
//! - **Only a fully valid callback ends the flow**: right `state`, right
//!   `iss`, and either one `code` or one `error`. Everything else (wrong or
//!   missing `state`, wrong `iss`, duplicated parameters, other paths or
//!   methods, oversized or malformed requests, silence) is answered with a
//!   static page and the listener keeps waiting. That includes an `error=`
//!   callback carrying the wrong `state`, since otherwise any local process
//!   could cancel a sign-in ("no kill-after-N").
//! - **Single use**: the first valid callback claims the flow; any other
//!   valid-looking callback racing it gets the "not valid" page, and the
//!   listener is closed as soon as the flow is decided.

mod http;
#[cfg(test)]
mod tests;

use std::fmt;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use url::Url;
use zeroize::Zeroizing;

use self::http::{evaluate, read_head, response, HeadRead, Page, Verdict};
use crate::config::{AccountConfig, SCOPES};
use crate::error::{AccountError, AccountResult};
use crate::pkce::{random_token, Pkce};

/// The listener's bounds. [`Default`] is the §8.26 §3 specification; tests
/// shorten the durations so no test runs longer than 5 s (BRD §2.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenerLimits {
    /// Largest request head read from one connection (§8.26 §3: 8 KB).
    pub max_request_bytes: usize,
    /// How long one connection may take to send its request (§8.26 §3: 5 s).
    pub per_connection: Duration,
    /// How long the whole sign-in waits for a valid callback (§8.26 §3:
    /// 10 minutes).
    pub window: Duration,
}

impl Default for ListenerLimits {
    fn default() -> Self {
        Self {
            max_request_bytes: 8 * 1024,
            per_connection: Duration::from_secs(5),
            window: Duration::from_secs(10 * 60),
        }
    }
}

/// Which page the person asked for (§8.41). Both finish the same sign-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInEntry {
    /// "Sign in": the authorize URL itself.
    SignIn,
    /// "Create an account": the hosted sign-up page, which returns through
    /// the authorize URL.
    SignUp,
}

/// A sign-in in progress: the loopback listener is bound and the
/// authorization URL is ready for the caller to open in the browser.
pub struct PendingSignIn {
    listener: TcpListener,
    authorize_url: Url,
    redirect_uri: String,
    state: Zeroizing<String>,
    pkce: Pkce,
    issuer: String,
    /// The hosted sign-up page, when the issuer's naming gives one (§8.41).
    sign_up_page: Option<String>,
}

impl PendingSignIn {
    /// Bind `127.0.0.1:0`, generate PKCE and `state`, and build the
    /// authorization URL.
    ///
    /// # Errors
    ///
    /// [`AccountError::Io`] if the listener cannot bind;
    /// [`AccountError::Random`] if the OS random source fails.
    #[tracing::instrument(skip_all)]
    pub async fn start(config: &AccountConfig) -> AccountResult<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let pkce = Pkce::generate()?;
        let state = Zeroizing::new(random_token()?);
        let authorize_url = Url::parse_with_params(
            &config.endpoint("/oauth/authorize"),
            [
                ("response_type", "code"),
                ("client_id", config.client_id()),
                ("redirect_uri", redirect_uri.as_str()),
                ("scope", SCOPES),
                ("state", state.as_str()),
                ("code_challenge", pkce.challenge()),
                ("code_challenge_method", "S256"),
            ],
        )
        .map_err(|_| AccountError::InvalidConfig("authorization URL".into()))?;
        tracing::info!(port, "sign-in listener ready");
        Ok(Self {
            listener,
            authorize_url,
            redirect_uri,
            state,
            pkce,
            issuer: config.issuer().to_owned(),
            sign_up_page: config.sign_up_page(),
        })
    }

    /// The URL to open in the system browser.
    pub fn authorize_url(&self) -> &Url {
        &self.authorize_url
    }

    /// The page to open for the person's choice (§8.41). "Sign in" is the
    /// authorize URL itself. "Create an account" is the hosted sign-up page
    /// with this authorize URL as its way back, so the sign-in that finishes
    /// is this one — same `state`, same PKCE challenge, same loopback — and
    /// nothing about the callback's checks changes. With no known sign-up
    /// page it is the authorize URL too, whose sign-in page links to sign-up.
    pub fn browser_url(&self, entry: SignInEntry) -> Url {
        match (entry, self.sign_up_page.as_deref()) {
            (SignInEntry::SignUp, Some(page)) => {
                Url::parse_with_params(page, [("redirect_url", self.authorize_url.as_str())])
                    .unwrap_or_else(|_| self.authorize_url.clone())
            }
            _ => self.authorize_url.clone(),
        }
    }

    /// Wait for the browser's redirect.
    ///
    /// # Errors
    ///
    /// [`AccountError::SignInTimedOut`] if no valid callback arrives within
    /// `limits.window`; [`AccountError::Io`] if the listener itself fails.
    #[tracing::instrument(skip_all)]
    pub async fn wait(self, limits: ListenerLimits) -> AccountResult<SignInOutcome> {
        let PendingSignIn {
            listener,
            redirect_uri,
            state,
            pkce,
            issuer,
            ..
        } = self;
        let expected = Arc::new(Expected {
            state,
            issuer,
            claimed: AtomicBool::new(false),
        });
        let (decided_tx, mut decided_rx) = mpsc::channel::<Decision>(1);
        let mut connections = JoinSet::new();
        let window = tokio::time::sleep(limits.window);
        tokio::pin!(window);

        let decision = loop {
            tokio::select! {
                () = &mut window => {
                    tracing::info!("sign-in window expired");
                    return Err(AccountError::SignInTimedOut);
                }
                Some(decision) = decided_rx.recv() => break decision,
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        connections.spawn(serve(
                            stream,
                            Arc::clone(&expected),
                            decided_tx.clone(),
                            limits,
                        ));
                    }
                    Err(e) => {
                        // A failed accept is about one connection, not the
                        // flow; pause briefly so a persistent failure can't spin.
                        tracing::debug!(error = %e, "sign-in listener accept failed");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                },
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
            }
        };

        // Single use: close the port and drop every other connection now.
        connections.abort_all();
        drop(listener);
        Ok(match decision {
            Decision::Code(code) => {
                tracing::info!("sign-in callback accepted");
                SignInOutcome::Authorized(AuthorizedCode {
                    code,
                    verifier: pkce.into_verifier(),
                    redirect_uri,
                })
            }
            Decision::Cancelled => {
                tracing::info!("sign-in cancelled in the browser");
                SignInOutcome::Cancelled
            }
        })
    }
}

/// What a callback must match, shared by every connection of one sign-in.
struct Expected {
    state: Zeroizing<String>,
    issuer: String,
    claimed: AtomicBool,
}

impl Expected {
    /// The first valid callback wins; every later one loses.
    fn claim(&self) -> bool {
        self.claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

/// How the flow ended, sent from the winning connection to [`PendingSignIn::wait`].
enum Decision {
    Code(Zeroizing<String>),
    Cancelled,
}

/// One connection: read one bounded request head within the per-connection
/// limit, answer it with a fixed page, and report a decision if it carried
/// the valid callback. The page is written before the decision is sent, so
/// the browser still gets its answer when `wait` closes everything down.
async fn serve(
    mut stream: TcpStream,
    expected: Arc<Expected>,
    decided: mpsc::Sender<Decision>,
    limits: ListenerLimits,
) {
    let read = tokio::time::timeout(
        limits.per_connection,
        read_head(&mut stream, limits.max_request_bytes),
    )
    .await;
    let head = match read {
        Ok(Ok(HeadRead::Complete(head))) => head,
        Ok(Ok(HeadRead::TooLarge)) => {
            tracing::debug!("sign-in listener: request too large");
            respond(&mut stream, Page::TooLarge, limits).await;
            return;
        }
        // Hung up, failed, or too slow: close without a word.
        _ => return,
    };

    let (page, decision) = match evaluate(&head, &expected.state, &expected.issuer) {
        Verdict::Code(code) if expected.claim() => (Page::SignedIn, Some(Decision::Code(code))),
        Verdict::Cancelled if expected.claim() => (Page::Cancelled, Some(Decision::Cancelled)),
        Verdict::Code(_) | Verdict::Cancelled | Verdict::Invalid => (Page::Invalid, None),
        Verdict::NotFound => (Page::NotFound, None),
        Verdict::MethodNotAllowed => (Page::MethodNotAllowed, None),
    };
    if decision.is_none() {
        tracing::debug!(?page, "sign-in listener: request refused");
    }
    respond(&mut stream, page, limits).await;
    if let Some(decision) = decision {
        // The receiver only goes away once the flow is already decided.
        let _ = decided.send(decision).await;
    }
}

/// Write a page and close the connection, within the per-connection limit.
async fn respond(stream: &mut TcpStream, page: Page, limits: ListenerLimits) {
    let write = async {
        stream.write_all(&response(page)).await?;
        stream.shutdown().await
    };
    if let Ok(Err(e)) = tokio::time::timeout(limits.per_connection, write).await {
        tracing::debug!(error = %e, "sign-in listener: could not write the page");
    }
}

impl fmt::Debug for PendingSignIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingSignIn")
            .field("redirect_uri", &self.redirect_uri)
            .field("state", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// How a sign-in ended.
#[derive(Debug)]
pub enum SignInOutcome {
    /// The browser came back with a code. Exchange it with
    /// [`crate::OAuthClient::exchange`].
    Authorized(AuthorizedCode),
    /// The user (or the account service) cancelled: `error=` with the right
    /// `state` and `iss`.
    Cancelled,
}

/// Everything the token exchange needs, and nothing else. Wiped on drop,
/// never printed.
pub struct AuthorizedCode {
    pub(crate) code: Zeroizing<String>,
    pub(crate) verifier: Zeroizing<String>,
    pub(crate) redirect_uri: String,
}

impl fmt::Debug for AuthorizedCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthorizedCode")
            .field("code", &"<redacted>")
            .field("verifier", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}
