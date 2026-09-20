//! The subscription gate (ADR-104 + ADR-SEC-022; `SIGNIN-DESIGN.md` §8.26
//! §6.1, quoted below as "§6.1", with amendment 6, §8.32).
//!
//! [`EntitledService`] wraps any MCP service and decides, request by request,
//! whether the user's trial or subscription lets it through. It is a
//! `Service<RoleServer>` wrapper rather than a `ServerHandler`, so it sees
//! every request the client can send, in the one `match` below.
//!
//! **What always passes, without asking.** `ping`, `initialize` and
//! `tools/list` carry no user data, and `initialize` must stay fast (ADR-102
//! K8): an AI app's startup probe gives a server well under a second. The
//! prompt, resource and resource-template lists pass too: §6.1 answers them
//! either way (they are empty), so asking would only add a refresh's wait
//! (§8.32).
//!
//! **What asks.** Everything else asks the [`EntitlementCheck`] (refresh-then-
//! decide, §8.26 §4). Entitled, the request is passed on. Locked:
//! - a tool call answers with a tool result flagged `isError`, carrying the
//!   fixed message, because the agent must READ it to tell the user (ADR-103
//!   D2: a protocol error reaches the model as "Tool execution failed");
//! - a task-style tool call gets the invalid-params error our server gives
//!   any task-style call (our tools forbid tasks), built here so the locked
//!   request never reaches the server;
//! - every other request is refused with the fixed message.
//!
//! **Deny by default.** The `match` names every `ClientRequest` variant and
//! has no catch-all arm, so a request kind a future rmcp adds fails to compile
//! here instead of slipping through (§6.1: "a future rmcp variant fails to
//! compile"). `tests/entitlement_gate.rs` pins that the arm stays absent.
//!
//! **Nothing reaches the vault while locked.** A refused request is logged to
//! the application log only (§6.6), with its kind and the reason and nothing
//! the client sent: no audit row, no adapter call.
//!
//! **Calls in flight.** [`InFlight`] counts every request from the moment it
//! arrives until it has answered, including while the check runs. The keeper
//! uses it to let a write finish before it changes mode (§6.2).

use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{
    CallToolResult, ClientNotification, ClientRequest, ContentBlock, ServerInfo, ServerResult,
};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::{ErrorData as McpError, RoleServer, Service};
use tokio::sync::watch;

use crate::server::ERROR_CODE_ACCESS_DENIED;

/// rmcp 2.2.0's own answer to a task-style call of a tool that forbids tasks
/// (`handler/server.rs`, `TaskSupport::Forbidden`), which every one of our
/// tools does. `tests/entitlement_gate.rs` compares the two answers, so a
/// change on rmcp's side fails a test rather than drifting.
const TASK_CALL_REFUSED: &str = "Tool does not support task-based invocation";

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

/// Decides whether the user may use the vault now. Implemented in
/// `vault-app`, over `vault-account` (this crate does not depend on it).
#[async_trait]
pub trait EntitlementCheck: Send + Sync {
    /// Asked once per gated request. May refresh first (≤ 5 s, §8.26 §4), so
    /// it is never asked for a request that is answered either way.
    async fn check(&self) -> Verdict;
}

/// Counts requests inside an [`EntitledService`], from arrival until they
/// have answered. Cheap to clone; clones share one count, so every connection
/// the keeper serves can report into the same counter.
#[derive(Clone, Debug)]
pub struct InFlight {
    count: Arc<watch::Sender<usize>>,
}

impl InFlight {
    /// A counter at zero.
    pub fn new() -> Self {
        Self {
            count: Arc::new(watch::Sender::new(0)),
        }
    }

    /// Requests in flight now.
    pub fn count(&self) -> usize {
        *self.count.borrow()
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

/// How the gate treats a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// No user data, or answered the same either way: never asks.
    Open,
    /// `tools/call`; `task` when the caller asked for a task.
    ToolCall { task: bool },
    /// Anything else: asks, and is refused while locked.
    Other,
}

/// Every variant named, no catch-all (see the module docs).
fn classify(request: &ClientRequest) -> Kind {
    match request {
        ClientRequest::PingRequest(_)
        | ClientRequest::InitializeRequest(_)
        | ClientRequest::ListToolsRequest(_)
        | ClientRequest::ListPromptsRequest(_)
        | ClientRequest::ListResourcesRequest(_)
        | ClientRequest::ListResourceTemplatesRequest(_) => Kind::Open,
        ClientRequest::CallToolRequest(call) => Kind::ToolCall {
            task: call.params.task.is_some(),
        },
        ClientRequest::CompleteRequest(_)
        | ClientRequest::SetLevelRequest(_)
        | ClientRequest::GetPromptRequest(_)
        | ClientRequest::ReadResourceRequest(_)
        | ClientRequest::SubscribeRequest(_)
        | ClientRequest::UnsubscribeRequest(_)
        | ClientRequest::GetTaskRequest(_)
        | ClientRequest::ListTasksRequest(_)
        | ClientRequest::GetTaskPayloadRequest(_)
        | ClientRequest::CancelTaskRequest(_)
        | ClientRequest::CustomRequest(_) => Kind::Other,
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
}

/// Put `inner` behind `gate` when there is one.
pub fn maybe_gated<S>(gate: Option<&Gate>, inner: S) -> MaybeGated<S> {
    match gate {
        Some(gate) => MaybeGated::Gated(gate.wrap(inner)),
        None => MaybeGated::Plain(inner),
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
        }
    }

    fn get_info(&self) -> ServerInfo {
        match self {
            MaybeGated::Gated(server) => server.get_info(),
            MaybeGated::Plain(server) => server.get_info(),
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
            Kind::ToolCall { task } => match self.check.check().await {
                Verdict::Entitled => self.inner.handle_request(request, context).await,
                Verdict::Locked(reason) => {
                    log_refusal("tools/call", reason);
                    if task {
                        Err(McpError::invalid_params(TASK_CALL_REFUSED, None))
                    } else {
                        Ok(ServerResult::CallToolResult(CallToolResult::error(vec![
                            ContentBlock::text(reason.message()),
                        ])))
                    }
                }
            },
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

    fn get_info(&self) -> ServerInfo {
        self.inner.get_info()
    }
}
