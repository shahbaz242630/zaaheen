//! Account commands — sign in, sign out, state, Subscribe, "I've paid"
//! (S3 step 4b, `SIGNIN-DESIGN.md` §8.26 §3 and §6.4).
//!
//! # These are ungated on purpose
//!
//! Every command here is on §6.4's allowlist. It has to be: somebody whose
//! trial has ended must still be able to sign in and subscribe, and a gate on
//! *those* would lock a paying customer out of paying.
//!
//! What makes that safe is not the list. It is that [`AccountOps`] is never
//! given the vault — no `Application`, no adapter, no key — so these commands
//! cannot serve a memory even by mistake. `AccountOps`' own
//! `account_ops_never_holds_the_vault` pins the absence, and
//! [`crate::guard::no_account_command_receives_the_vault`] pins that no
//! command in this module asks for one.
//!
//! (Founder decision, 2026-09-20: *"Never give them the vault"*.)
//!
//! # No URL crosses this boundary, in either direction
//!
//! The frontend never sends a URL and never receives one. `Subscribe` asks
//! for a plan; the app decides where that leads, validates it, and opens it.
//! Nothing the webview says can reach the operating system (ADR-030's rule,
//! held here by there being no parameter to abuse).
//!
//! # One account, shared with the lock (ADR-SEC-028)
//!
//! The commands read the account through [`AccountSlot`], which
//! `guard::build()` fills from the same account the lock asks. A build with no
//! account settings, or one whose account could not be prepared, has an
//! empty slot: the commands then answer [`ERR_ACCOUNT_UNAVAILABLE`] rather
//! than Tauri's raw "state not managed" text (BRD §11.7.2).
//!
//! # "Is this computer locked?" (§8.38)
//!
//! [`account_access`] answers from the lock itself, never by reading the
//! account view and guessing: the view reports the lease's state, which is
//! not the verdict (a trial past its deadline with no internet still reads
//! `trial`). It fills the same **account** slot of §8.26 §6.4 as the other
//! five, and it is given the guard and nothing else.

use std::sync::Arc;

use tauri::State;
use vault_app::account_ops::{
    AccountOps, AccountView, OpsError, SignInEntry, SubscriptionPlan as Plan,
};

use crate::guard::Entitlement;

/// Opaque error code: the browser never came back, or the person closed it.
pub const ERR_SIGN_IN_DID_NOT_FINISH: &str = "account_sign_in_did_not_finish";
/// Opaque error code: another process holds the account folder.
pub const ERR_ACCOUNT_BUSY: &str = "account_busy";
/// Opaque error code: the account service could not be reached.
pub const ERR_ACCOUNT_UNREACHABLE: &str = "account_unreachable";
/// Opaque error code: the account service answered something we will not act
/// on, or this build carries no sign-in at all.
pub const ERR_ACCOUNT_REFUSED: &str = "account_refused";
/// Opaque error code: the plan was not one of the two.
pub const ERR_ACCOUNT_BAD_PLAN: &str = "account_bad_plan";
/// Opaque error code: this app has no account to act on — its account could
/// not be prepared when it started (ADR-SEC-023/028). Reopening the app is
/// the remedy, so it has its own line rather than "try again".
pub const ERR_ACCOUNT_UNAVAILABLE: &str = "account_unavailable";

/// Every code this module can return, for the test that pins each one to a
/// plain-English line in the desktop bundle.
pub const ALL_CODES: &[&str] = &[
    ERR_SIGN_IN_DID_NOT_FINISH,
    ERR_ACCOUNT_BUSY,
    ERR_ACCOUNT_UNREACHABLE,
    ERR_ACCOUNT_REFUSED,
    ERR_ACCOUNT_BAD_PLAN,
    ERR_ACCOUNT_UNAVAILABLE,
];

/// The account the commands act on: the one the lock asks, or nothing
/// (ADR-SEC-028). Filled only by [`crate::guard::build`].
pub struct AccountSlot(Option<Arc<AccountOps>>);

impl AccountSlot {
    /// Fill the slot. Crate-private: the guard builds it from the same
    /// account as the lock, and nothing else should.
    pub(crate) fn new(ops: Option<Arc<AccountOps>>) -> Self {
        Self(ops)
    }

    /// The account, or the stable code for "there is none".
    fn ops(&self) -> Result<&AccountOps, String> {
        self.0
            .as_deref()
            .ok_or_else(|| ERR_ACCOUNT_UNAVAILABLE.to_string())
    }

    /// What this computer's account looks like now; "could not confirm"
    /// when there is no account to ask (never "signed out", never "ended").
    async fn view(&self) -> AccountView {
        match self.ops() {
            Ok(ops) => ops.status().await,
            Err(_) => AccountOps::cannot_confirm(),
        }
    }

    /// The account for the background refresh at open and daily (§8.26 §4),
    /// which `main.rs` spawns. `None` when there is no account.
    #[must_use]
    pub fn for_background_refresh(&self) -> Option<Arc<AccountOps>> {
        self.0.clone()
    }

    /// Sign out after "Delete everything" (§8.26 §7), best effort: logged,
    /// never an error, because the erasure has already happened and is what
    /// the person is told about.
    pub(crate) async fn sign_out_after_erasure(&self) {
        let Ok(ops) = self.ops() else {
            return;
        };
        match ops.sign_out().await {
            Ok(_) => tracing::info!("signed out after erasure"),
            Err(e) => tracing::warn!(error = ?e, "could not sign out after erasure"),
        }
    }
}

/// Map an operation failure to its stable code. The reason never carries the
/// account service's own words (BRD §11.7.2).
fn code_for(error: OpsError) -> String {
    match error {
        OpsError::SignInDidNotFinish => ERR_SIGN_IN_DID_NOT_FINISH,
        OpsError::Busy => ERR_ACCOUNT_BUSY,
        OpsError::Unreachable => ERR_ACCOUNT_UNREACHABLE,
        // A link this app refused to open is not something the person can act
        // on differently, so it reads as the same generic refusal.
        OpsError::Refused | OpsError::Link(_) => ERR_ACCOUNT_REFUSED,
    }
    .to_string()
}

/// The view as the frontend reads it. Built by hand rather than derived, so
/// adding a field to [`AccountView`] cannot leak it to the webview by
/// accident (BRD §11.7.2's allowlist rule, as `agent.rs` already applies).
fn wire(view: &AccountView) -> serde_json::Value {
    serde_json::json!({
        "signed_in": view.signed_in,
        "email": view.email,
        "state": view.state,
        "days_left": view.days_left,
        "clock_wrong": view.clock_wrong,
    })
}

/// What [`account_access`] tells the frontend. Two fields, by hand, for the
/// same allowlist reason as [`wire`].
fn access_wire(sign_in: bool, locked: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "sign_in": sign_in,
        "locked": locked,
    })
}

/// Parse the plan the frontend asked for. Bounded and allowlisted
/// (BRD §11.7.1): two values, and anything else is refused before it can
/// reach the account service.
fn plan_of(raw: &str) -> Option<Plan> {
    match raw {
        "monthly" => Some(Plan::Monthly),
        "annual" => Some(Plan::Annual),
        _ => None,
    }
}

/// Is this computer locked, and why — asked of the lock itself (§8.38).
///
/// `sign_in` is `false` only in a build with no account settings, so a
/// developer build never shows a sign-in screen. `locked` is `null` when a
/// gated command would be served right now, else the `locked_*` code it
/// would be refused with. Given the guard and nothing else.
#[tauri::command]
pub async fn account_access(
    entitlement: State<'_, Entitlement>,
) -> Result<serde_json::Value, String> {
    let locked = entitlement.lock_code().await;
    Ok(access_wire(entitlement.sign_in_available(), locked))
}

/// What this computer's account looks like now. No network, no writes.
#[tauri::command]
pub async fn account_status(account: State<'_, AccountSlot>) -> Result<serde_json::Value, String> {
    Ok(wire(&account.view().await))
}

/// Which page the person asked for: "Sign in" or "Create an account"
/// (`SIGNIN-DESIGN.md` §8.41). A closed set, so nothing the webview sends
/// can name an address (§8.37: no command takes a URL).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInChoice {
    SignIn,
    SignUp,
}

impl From<SignInChoice> for SignInEntry {
    fn from(choice: SignInChoice) -> Self {
        match choice {
            SignInChoice::SignIn => SignInEntry::SignIn,
            SignInChoice::SignUp => SignInEntry::SignUp,
        }
    }
}

/// Open the browser — on the sign-in page, or on the sign-up page that
/// returns through the same sign-in (§8.41) — and sign in.
///
/// Returns only the resulting view: the authorization code, the PKCE
/// verifier and the redirect never cross this boundary.
#[tauri::command]
pub async fn account_sign_in(
    account: State<'_, AccountSlot>,
    entry: Option<SignInChoice>,
) -> Result<serde_json::Value, String> {
    let entry = entry.map_or(SignInEntry::SignIn, SignInEntry::from);
    account
        .ops()?
        .sign_in(entry)
        .await
        .map(|view| wire(&view))
        .map_err(code_for)
}

/// Sign out: revoke, delete the token, clear the folder.
#[tauri::command]
pub async fn account_sign_out(
    account: State<'_, AccountSlot>,
) -> Result<serde_json::Value, String> {
    account
        .ops()?
        .sign_out()
        .await
        .map(|view| wire(&view))
        .map_err(code_for)
}

/// Subscribe, or manage an existing subscription.
///
/// Takes a plan, never a URL. Where that leads is decided, validated and
/// opened inside the application.
/// Returns the account view as it stands afterwards, so a checkout that
/// ended with this computer signed out shows as signed out immediately
/// rather than the next time something happens to refresh.
#[tauri::command]
pub async fn account_subscribe(
    account: State<'_, AccountSlot>,
    plan: String,
) -> Result<serde_json::Value, String> {
    let Some(plan) = plan_of(&plan) else {
        return Err(ERR_ACCOUNT_BAD_PLAN.to_string());
    };
    account
        .ops()?
        .subscribe(plan)
        .await
        .map(|view| wire(&view))
        .map_err(code_for)
}

/// "I've paid", and the poll that follows a checkout. One refresh, then the
/// fresh view. Never fails outright: refusing to answer would read as a lost
/// subscription. With no account there is nothing to refresh, and the view
/// says "could not confirm".
#[tauri::command]
pub async fn account_refresh_now(
    account: State<'_, AccountSlot>,
) -> Result<serde_json::Value, String> {
    let view = match account.ops() {
        Ok(ops) => ops.refresh_now().await,
        Err(_) => AccountOps::cannot_confirm(),
    };
    Ok(wire(&view))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §8.41: the webview names one of two pages, and nothing else gets
    /// through — an address, a misspelling or an empty string is refused
    /// before the command runs.
    #[test]
    fn the_sign_in_choice_is_one_of_two_pages() {
        let choice = |text: &str| serde_json::from_str::<SignInChoice>(text).ok();
        assert_eq!(choice("\"sign_in\""), Some(SignInChoice::SignIn));
        assert_eq!(choice("\"sign_up\""), Some(SignInChoice::SignUp));
        for other in [
            "\"https://evil.test/\"",
            "\"SignUp\"",
            "\"sign-up\"",
            "\"\"",
            "null",
            "1",
        ] {
            assert_eq!(choice(other), None, "{other}");
        }
        assert_eq!(SignInEntry::from(SignInChoice::SignUp), SignInEntry::SignUp);
        assert_eq!(SignInEntry::from(SignInChoice::SignIn), SignInEntry::SignIn);
    }

    #[test]
    fn only_the_two_locked_plans_are_accepted() {
        assert_eq!(plan_of("monthly"), Some(Plan::Monthly));
        assert_eq!(plan_of("annual"), Some(Plan::Annual));
        for bad in [
            "",
            "Monthly",
            "ANNUAL",
            "yearly",
            "free",
            "monthly ",
            " annual",
            "monthly;annual",
        ] {
            assert!(plan_of(bad).is_none(), "{bad:?} was accepted as a plan");
        }
    }

    #[test]
    fn every_failure_maps_to_its_own_stable_code() {
        assert_eq!(code_for(OpsError::Busy), ERR_ACCOUNT_BUSY);
        assert_eq!(code_for(OpsError::Unreachable), ERR_ACCOUNT_UNREACHABLE);
        assert_eq!(
            code_for(OpsError::SignInDidNotFinish),
            ERR_SIGN_IN_DID_NOT_FINISH
        );
        assert_eq!(code_for(OpsError::Refused), ERR_ACCOUNT_REFUSED);
    }

    /// The wire shape is an allowlist: five named fields and nothing else, so
    /// a field added to `AccountView` tomorrow does not reach the webview
    /// until somebody adds it here on purpose. (Four until §8.38 added the
    /// clock notice, deliberately.)
    #[test]
    fn the_wire_shape_carries_exactly_five_fields() {
        let view = AccountView {
            signed_in: true,
            email: Some("someone@example.test".into()),
            state: "trial",
            days_left: Some(7),
            clock_wrong: true,
        };
        let json = wire(&view);
        let object = json.as_object().expect("the view serialises to an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["clock_wrong", "days_left", "email", "signed_in", "state"]
        );
        assert_eq!(object["clock_wrong"], serde_json::Value::Bool(true));
    }

    /// The lock-state answer is two fields, and `locked` is either `null` or
    /// the code a gated command would have been refused with.
    #[test]
    fn the_access_answer_carries_exactly_two_fields() {
        let open = access_wire(false, None);
        let object = open.as_object().expect("an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["locked", "sign_in"]);
        assert_eq!(object["locked"], serde_json::Value::Null);
        assert_eq!(object["sign_in"], serde_json::Value::Bool(false));

        let locked = access_wire(true, Some(crate::guard::ERR_LOCKED_TRIAL_ENDED));
        assert_eq!(locked["locked"], "locked_trial_ended");
        assert_eq!(locked["sign_in"], serde_json::Value::Bool(true));
    }

    /// An empty slot must answer with a stable code, never with Tauri's raw
    /// "state not managed" text (BRD §11.7.2), and never pretend to be
    /// signed out or ended.
    #[tokio::test]
    async fn an_empty_slot_answers_with_stable_words() {
        let slot = AccountSlot::new(None);
        assert_eq!(slot.ops().err().as_deref(), Some(ERR_ACCOUNT_UNAVAILABLE));
        let view = slot.view().await;
        assert_eq!(view.state, "cannot_confirm");
        assert!(!view.signed_in);
        assert!(slot.for_background_refresh().is_none());
        // Best effort, and nothing to do: must simply return.
        slot.sign_out_after_erasure().await;
    }

    /// A code with no arm in the frontend falls through to showing the raw
    /// code, so the plain-English line ships with the code — the lesson from
    /// step 4a, applied here rather than re-learned.
    #[test]
    fn every_account_code_has_a_plain_english_line_in_the_app() {
        const APP_JS: &str = include_str!("../../dist/app.js");
        for code in ALL_CODES {
            assert!(
                APP_JS.contains(&format!("case \"{code}\":")),
                "{code} has no plain-English line in the desktop bundle"
            );
        }
    }

    /// Likewise for the states a view can carry.
    #[test]
    fn every_account_state_has_a_plain_english_line_in_the_app() {
        const APP_JS: &str = include_str!("../../dist/app.js");
        for state in vault_app::account_ops::state::ALL {
            assert!(
                APP_JS.contains(&format!("case \"{state}\":")),
                "the account state {state} has no plain-English line in the desktop bundle"
            );
        }
    }
}
