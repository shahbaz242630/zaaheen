//! The subscription gate (ADR-104 + ADR-SEC-022; `SIGNIN-DESIGN.md` §8.26
//! §6.1, quoted below as "§6.1", with amendment 6, §8.32).
//!
//! [`EntitledService`] wraps any MCP service and decides, request by request,
//! whether the user's trial or subscription lets it through. It is a
//! `Service<RoleServer>` wrapper rather than a `ServerHandler`, so it sees
//! every request the client can send, in the one `match` below.
//!
//! **What always passes, without asking.** `ping`, `initialize`,
//! `server/discover` and `tools/list` carry no user data, and `initialize`
//! must stay fast (ADR-102 K8): an AI app's startup probe gives a server well
//! under a second. `server/discover` is the 2026-07-28 spec's `initialize`
//! (SEP-2575; ADR-SEC-032), answered from the same server info. The prompt,
//! resource and resource-template lists pass too: §6.1 answers them either
//! way (they are empty), so asking would only add a refresh's wait (§8.32).
//!
//! **What asks.** Everything else asks the [`EntitlementCheck`] (refresh-then-
//! decide, §8.26 §4). Entitled, the request is passed on. Locked:
//! - a tool call answers with a tool result flagged `isError`, carrying the
//!   fixed message, because the agent must READ it to tell the user (ADR-103
//!   D2: a protocol error reaches the model as "Tool execution failed");
//! - every other request is refused with the fixed message.
//!
//! (Under rmcp 2.2 a client could ask for a "task-style" tool call, which got
//! rmcp's own invalid-params refusal. rmcp 3's tasks (SEP-2663) are created
//! by the SERVER instead, and ours never creates one, so no such call exists
//! any more; ADR-SEC-032.)
//!
//! **Deny by default.** The `match` names every `ClientRequest` variant rmcp
//! 3.4.1 has. rmcp 3 made the enum `#[non_exhaustive]`, so the one catch-all
//! arm the compiler now requires sends any future kind to [`Kind::Other`]:
//! asked, and refused while locked. §6.1's "a future rmcp variant fails to
//! compile" became "a future variant fails closed" (ADR-SEC-032), and the
//! exact version pin keeps a new kind from arriving without an upgrade that
//! re-reads this list. `tests/entitlement_gate.rs` pins both.
//!
//! **Nothing reaches the vault while locked.** A refused request is logged to
//! the application log only (§6.6), with its kind and the reason and nothing
//! the client sent: no audit row, no adapter call.
//!
//! **Calls in flight.** [`InFlight`] counts every request from the moment it
//! arrives until it has answered, including while the check runs. The keeper
//! uses it to let a write finish before it changes mode (§6.2).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{
    CallToolResult, ClientNotification, ClientRequest, ContentBlock, ServerConfig, ServerResult,
};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::{ErrorData as McpError, RoleServer, Service};
use tokio::sync::watch;

use crate::server::ERROR_CODE_ACCESS_DENIED;

/// Why the vault is locked. Each has fixed wording (§8.26 §6 "Messages":
/// fixed text, no links, no user data).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockReason {
    /// Nobody is signed in on this computer.
    SignedOut,
    /// The subscription could not be confirmed (offline, a stale lease, a
    /// clock that moved backwards).
    CannotConfirm,
    /// The free trial ended without a subscription.
    TrialEnded,
    /// A paid subscription ended.
    SubscriptionEnded,
    /// The user is entitled again, and the keeper is restarting into full
    /// mode (lock mode's answer, §6.2).
    Unlocking,
}

impl LockReason {
    /// The fixed text the agent reads and relays (§8.26 §6, verbatim).
    pub fn message(self) -> &'static str {
        match self {
            LockReason::SignedOut => {
                "Zaaheen needs you to sign in. Open the Zaaheen app on this computer to sign in."
            }
            LockReason::CannotConfirm => {
                "Zaaheen couldn't confirm your subscription. Check your internet connection and \
                 try again."
            }
            LockReason::TrialEnded => {
                "Your Zaaheen free trial has ended. Open the Zaaheen app and choose Subscribe to \
                 keep using your memories. Nothing has been deleted."
            }
            LockReason::SubscriptionEnded => {
                "Your Zaaheen subscription has ended. Open the Zaaheen app and choose Subscribe \
                 to keep using your memories. Nothing has been deleted."
            }
            LockReason::Unlocking => "Zaaheen is unlocking. Try again in a moment.",
        }
    }
}

/// What the check decided for one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Serve it.
    Entitled,
    /// Refuse it, saying why.
    Locked(LockReason),
}

/// Something the agent should pass on while the vault is still open
/// (§8.26 §6; `SIGNIN-DESIGN.md` §8.40). It rides on a served call only —
/// never on a refusal — and reaches `memory_read`'s `health.warnings` as one
/// of the two `SUBSCRIPTION_*` codes (ADR-054 Contract 2, amendment 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountNotice {
    /// The free trial ends within five days: whole days left, rounded down
    /// (so 0 is "less than a day").
    TrialEnding {
        /// Whole days left, 0 to 4.
        days_left: u32,
    },
    /// The last payment did not go through. Still entitled (§8.26 §5's
    /// `past_due`).
    PaymentFailed,
}

impl AccountNotice {
    /// The first sentence the agent relays: what is happening. Fixed text
    /// (§8.26 §6 "Messages": no links, no user data); the days count is the
    /// only thing that varies. Founder-approved 2026-09-21 (§8.40).
    pub fn detail(self) -> String {
        match self {
            AccountNotice::TrialEnding { days_left: 0 } => {
                "Your Zaaheen free trial ends in less than a day.".to_string()
            }
            AccountNotice::TrialEnding { days_left: 1 } => {
                "Your Zaaheen free trial ends in 1 day.".to_string()
            }
            AccountNotice::TrialEnding { days_left } => {
                format!("Your Zaaheen free trial ends in {days_left} days.")
            }
            AccountNotice::PaymentFailed => {
                "Your last Zaaheen payment didn't go through.".to_string()
            }
        }
    }

    /// The second sentence: what the person does about it. With
    /// [`Self::detail`] it reads as the approved words (§8.26 §6, §8.40).
    pub fn recovery_hint(self) -> &'static str {
        match self {
            AccountNotice::TrialEnding { .. } => {
                "Open the Zaaheen app and choose Subscribe to keep using your memories."
            }
            AccountNotice::PaymentFailed => "Open the Zaaheen app to update your card.",
        }
    }
}

/// Decides whether the user may use the vault now. Implemented in
/// `vault-app`, over `vault-account` (this crate does not depend on it).
#[async_trait]
pub trait EntitlementCheck: Send + Sync {
    /// Asked once per gated request. May refresh first (≤ 5 s, §8.26 §4), so
    /// it is never asked for a request that is answered either way.
    async fn check(&self) -> Verdict;

    /// The same question, with anything the agent should pass on — only
    /// ever alongside [`Verdict::Entitled`] (§8.40). Asked instead of
    /// [`Self::check`] for a tool call, never as well as it.
    ///
    /// The default has nothing to pass on, so a check that knows no account
    /// (lock mode, the desktop's guard, a test's stand-in) needs nothing
    /// more.
    async fn check_with_notice(&self) -> (Verdict, Option<AccountNotice>) {
        (self.check().await, None)
    }
}

/// Counts requests inside an [`EntitledService`], from arrival until they
/// have answered. Cheap to clone; clones share one count, so every connection
/// the keeper serves can report into the same counter.
#[derive(Clone, Debug)]
pub struct InFlight {
    count: Arc<watch::Sender<usize>>,
    /// Set once, when the keeper is handing over (ADR-SEC-033 D3). From then
    /// on no new tool call is admitted, so the drain converges instead of
    /// waiting for calls that arrived after the keeper decided to leave.
    closed: Arc<AtomicBool>,
}

impl InFlight {
    /// A counter at zero.
    pub fn new() -> Self {
        Self {
            count: Arc::new(watch::Sender::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Requests in flight now.
    pub fn count(&self) -> usize {
        *self.count.borrow()
    }

    /// Stop admitting tool calls, on every clone. Calls already running
    /// finish; [`Self::wait_idle`] then resolves once they have.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// Whether [`Self::close`] has been called.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Count `inner`'s requests here, for a server with no entitlement gate
    /// (a build with no sign-in, and the desktop's admin connection). After
    /// [`Self::close`] a new tool call is answered with `busy` — words its
    /// caller understands — before it reaches `inner`.
    pub fn counting<S>(&self, inner: S, busy: &'static str) -> Counted<S> {
        Counted {
            inner,
            in_flight: self.clone(),
            busy,
        }
    }

    /// Resolves once no request is in flight (at once if none is).
    pub async fn wait_idle(&self) {
        let mut idle = self.count.subscribe();
        // `wait_for` looks at the current value first. It fails only if the
        // sender is gone, and `self` holds it.
        let _ = idle.wait_for(|n| *n == 0).await;
    }

    /// Count one request until the returned guard is dropped. A guard, not a
    /// decrement after the call, so a request whose future is dropped (a
    /// panic unwinding, a task aborted) stops being counted all the same.
    fn admit(&self) -> Admitted {
        self.count.send_modify(|n| *n += 1);
        Admitted(self.clone())
    }
}

impl Default for InFlight {
    fn default() -> Self {
        Self::new()
    }
}

/// One request in flight; see [`InFlight::admit`].
struct Admitted(InFlight);

impl Drop for Admitted {
    fn drop(&mut self) {
        self.0.count.send_modify(|n| *n = n.saturating_sub(1));
    }
}

/// The retryable answer a closing keeper gives a new tool call.
fn closing(busy: &'static str) -> Result<ServerResult, McpError> {
    Ok(ServerResult::CallToolResult(CallToolResult::error(vec![
        ContentBlock::text(busy),
    ])))
}

/// A server counted in an [`InFlight`] with no entitlement gate; see
/// [`InFlight::counting`].
pub struct Counted<S> {
    inner: S,
    in_flight: InFlight,
    busy: &'static str,
}

impl<S: Service<RoleServer>> Service<RoleServer> for Counted<S> {
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, McpError> {
        let _admitted = self.in_flight.admit();
        if self.in_flight.is_closed() && classify(&request) == Kind::ToolCall {
            return closing(self.busy);
        }
        self.inner.handle_request(request, context).await
    }

    async fn handle_notification(
        &self,
        notification: ClientNotification,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.inner.handle_notification(notification, context).await
    }

    fn get_info(&self) -> ServerConfig {
        self.inner.get_info()
    }
}

/// How the gate treats a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// No user data, or answered the same either way: never asks.
    Open,
    /// `tools/call`.
    ToolCall,
    /// Anything else: asks, and is refused while locked.
    Other,
}

/// Every variant named, no catch-all (see the module docs).
fn classify(request: &ClientRequest) -> Kind {
    match request {
        ClientRequest::PingRequest(_)
        | ClientRequest::InitializeRequest(_)
        | ClientRequest::DiscoverRequest(_)
        | ClientRequest::ListToolsRequest(_)
        | ClientRequest::ListPromptsRequest(_)
        | ClientRequest::ListResourcesRequest(_)
        | ClientRequest::ListResourceTemplatesRequest(_) => Kind::Open,
        ClientRequest::CallToolRequest(_) => Kind::ToolCall,
        ClientRequest::CompleteRequest(_)
        | ClientRequest::SetLevelRequest(_)
        | ClientRequest::GetPromptRequest(_)
        | ClientRequest::ReadResourceRequest(_)
        | ClientRequest::SubscriptionsListenRequest(_)
        | ClientRequest::SubscribeRequest(_)
        | ClientRequest::UnsubscribeRequest(_)
        | ClientRequest::GetTaskRequest(_)
        | ClientRequest::UpdateTaskRequest(_)
        | ClientRequest::CancelTaskRequest(_)
        | ClientRequest::CustomRequest(_) => Kind::Other,
        // rmcp 3 marks `ClientRequest` `#[non_exhaustive]`, so the compiler
        // insists on this arm and a new variant no longer fails to compile
        // here (§6.1's alarm; ADR-SEC-032). It fails CLOSED instead: an
        // unknown kind asks the check and is refused while locked. Every kind
        // rmcp 3.4.1 has is still named above, and the exact `=3.4.1` pin
        // means a new one arrives only through an upgrade that re-reads this
        // list (`the_gate_names_every_request_kind_and_fails_closed_on_new_ones`).
        _ => Kind::Other,
    }
}

/// §6.6: the application log only, with nothing the client sent.
fn log_refusal(request: &'static str, reason: LockReason) {
    tracing::info!(
        target: "vault_mcp::gate",
        request,
        reason = ?reason,
        "refused: the vault is locked"
    );
}

/// The check and the counter, travelling together.
///
/// A build that carries no sign-in has no gate at all (`Option<Gate>` at each
/// place a server is built), which is what every build before this arc is.
/// Licensing is a business control, not a security boundary (§6.7).
#[derive(Clone)]
pub struct Gate {
    check: Arc<dyn EntitlementCheck>,
    in_flight: InFlight,
}

impl Gate {
    /// Build one. The same `in_flight` is shared by every server the keeper
    /// creates, so it counts the whole process's work (§6.2).
    pub fn new(check: Arc<dyn EntitlementCheck>, in_flight: InFlight) -> Self {
        Self { check, in_flight }
    }

    /// Put `inner` behind this gate.
    pub fn wrap<S>(&self, inner: S) -> EntitledService<S> {
        EntitledService::new(inner, Arc::clone(&self.check), self.in_flight.clone())
    }

    /// Ask the check directly, for a server that dispatches per request
    /// rather than being wrapped (the daemon).
    pub async fn verdict(&self) -> Verdict {
        self.check.check().await
    }

    /// [`Self::verdict`] with the notice, for the daemon's tool calls
    /// (§8.40). The notice is `None` whenever the verdict is not
    /// [`Verdict::Entitled`].
    pub async fn verdict_with_notice(&self) -> (Verdict, Option<AccountNotice>) {
        match self.check.check_with_notice().await {
            (Verdict::Entitled, notice) => (Verdict::Entitled, notice),
            // SP-4: a refusal says why, and nothing else.
            (locked, _) => (locked, None),
        }
    }

    /// The shared counter.
    pub fn in_flight(&self) -> &InFlight {
        &self.in_flight
    }
}

/// A server that may or may not stand behind the gate, so a build with a
/// sign-in and one without have the same type at the transport.
pub enum MaybeGated<S> {
    /// Behind the gate.
    Gated(EntitledService<S>),
    /// Served directly (a build with no account settings).
    Plain(S),
    /// Served directly but counted, so a keeper with no sign-in still drains
    /// before it hands over (ADR-SEC-033 D3).
    Counted(Counted<S>),
}

/// Put `inner` behind `gate` when there is one.
pub fn maybe_gated<S>(gate: Option<&Gate>, inner: S) -> MaybeGated<S> {
    match gate {
        Some(gate) => MaybeGated::Gated(gate.wrap(inner)),
        None => MaybeGated::Plain(inner),
    }
}

/// Put `inner` behind `gate` when there is one, and otherwise count it in
/// `in_flight`: either way every request is counted exactly once, in the one
/// counter the keeper drains (ADR-SEC-033 D3).
pub fn gated_or_counted<S>(gate: Option<&Gate>, in_flight: &InFlight, inner: S) -> MaybeGated<S> {
    match gate {
        Some(gate) => MaybeGated::Gated(gate.wrap(inner)),
        None => MaybeGated::Counted(in_flight.counting(inner, crate::desk::MSG_BUSY)),
    }
}

impl<S: Service<RoleServer>> Service<RoleServer> for MaybeGated<S> {
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, McpError> {
        match self {
            MaybeGated::Gated(server) => server.handle_request(request, context).await,
            MaybeGated::Plain(server) => server.handle_request(request, context).await,
            MaybeGated::Counted(server) => server.handle_request(request, context).await,
        }
    }

    async fn handle_notification(
        &self,
        notification: ClientNotification,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), McpError> {
        match self {
            MaybeGated::Gated(server) => server.handle_notification(notification, context).await,
            MaybeGated::Plain(server) => server.handle_notification(notification, context).await,
            MaybeGated::Counted(server) => server.handle_notification(notification, context).await,
        }
    }

    fn get_info(&self) -> ServerConfig {
        match self {
            MaybeGated::Gated(server) => server.get_info(),
            MaybeGated::Plain(server) => server.get_info(),
            MaybeGated::Counted(server) => server.get_info(),
        }
    }
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("in_flight", &self.in_flight.count())
            .finish_non_exhaustive()
    }
}

/// The gate: an MCP service that serves `inner` only while entitled.
pub struct EntitledService<S> {
    inner: S,
    check: Arc<dyn EntitlementCheck>,
    in_flight: InFlight,
}

impl<S> EntitledService<S> {
    /// Gate `inner` behind `check`, counting its requests in `in_flight`.
    pub fn new(inner: S, check: Arc<dyn EntitlementCheck>, in_flight: InFlight) -> Self {
        Self {
            inner,
            check,
            in_flight,
        }
    }
}

impl<S: Service<RoleServer>> Service<RoleServer> for EntitledService<S> {
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, McpError> {
        // Counted from arrival, before the check decides: otherwise the
        // keeper could see zero and change mode between a "yes" and the call.
        let _admitted = self.in_flight.admit();
        match classify(&request) {
            Kind::Open => self.inner.handle_request(request, context).await,
            // A closing keeper takes no new work, and does not ask the check
            // for work it will not do (ADR-SEC-033 D3).
            Kind::ToolCall if self.in_flight.is_closed() => closing(crate::desk::MSG_BUSY),
            Kind::ToolCall => {
                let (verdict, notice) = self.check.check_with_notice().await;
                match verdict {
                    Verdict::Entitled => {
                        // Served, so anything the agent should pass on rides
                        // with the call, in the request's own extensions:
                        // only this process can write there (§8.40).
                        let mut context = context;
                        if let Some(notice) = notice {
                            context.extensions.insert(notice);
                        }
                        self.inner.handle_request(request, context).await
                    }
                    Verdict::Locked(reason) => {
                        log_refusal("tools/call", reason);
                        Ok(ServerResult::CallToolResult(CallToolResult::error(vec![
                            ContentBlock::text(reason.message()),
                        ])))
                    }
                }
            }
            Kind::Other => match self.check.check().await {
                Verdict::Entitled => self.inner.handle_request(request, context).await,
                Verdict::Locked(reason) => {
                    log_refusal("other", reason);
                    Err(McpError::new(
                        ERROR_CODE_ACCESS_DENIED,
                        reason.message(),
                        None,
                    ))
                }
            },
        }
    }

    async fn handle_notification(
        &self,
        notification: ClientNotification,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.inner.handle_notification(notification, context).await
    }

    fn get_info(&self) -> ServerConfig {
        self.inner.get_info()
    }
}
