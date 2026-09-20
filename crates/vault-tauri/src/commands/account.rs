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

use tauri::State;
use vault_app::account_ops::{AccountOps, AccountView, OpsError, SubscriptionPlan as Plan};

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

/// Every code this module can return, for the test that pins each one to a
/// plain-English line in the desktop bundle.
pub const ALL_CODES: &[&str] = &[
    ERR_SIGN_IN_DID_NOT_FINISH,
    ERR_ACCOUNT_BUSY,
    ERR_ACCOUNT_UNREACHABLE,
    ERR_ACCOUNT_REFUSED,
    ERR_ACCOUNT_BAD_PLAN,
];

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

/// What this computer's account looks like now. No network, no writes.
#[tauri::command]
pub async fn account_status(account: State<'_, AccountOps>) -> Result<serde_json::Value, String> {
    Ok(wire(&account.status().await))
}

/// Open the browser and sign in.
///
/// Returns only the resulting view: the authorization code, the PKCE
/// verifier and the redirect never cross this boundary.
#[tauri::command]
pub async fn account_sign_in(account: State<'_, AccountOps>) -> Result<serde_json::Value, String> {
    account
        .sign_in()
        .await
        .map(|view| wire(&view))
        .map_err(code_for)
}

/// Sign out: revoke, delete the token, clear the folder.
#[tauri::command]
pub async fn account_sign_out(account: State<'_, AccountOps>) -> Result<serde_json::Value, String> {
    account
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
    account: State<'_, AccountOps>,
    plan: String,
) -> Result<serde_json::Value, String> {
    let Some(plan) = plan_of(&plan) else {
        return Err(ERR_ACCOUNT_BAD_PLAN.to_string());
    };
    account
        .subscribe(plan)
        .await
        .map(|view| wire(&view))
        .map_err(code_for)
}

/// "I've paid", and the poll that follows a checkout. One refresh, then the
/// fresh view. Never fails outright: refusing to answer would read as a lost
/// subscription.
#[tauri::command]
pub async fn account_refresh_now(
    account: State<'_, AccountOps>,
) -> Result<serde_json::Value, String> {
    Ok(wire(&account.refresh_now().await))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The wire shape is an allowlist: four named fields and nothing else, so
    /// a field added to `AccountView` tomorrow does not reach the webview
    /// until somebody adds it here on purpose.
    #[test]
    fn the_wire_shape_carries_exactly_four_fields() {
        let view = AccountView {
            signed_in: true,
            email: Some("someone@example.test".into()),
            state: "trial",
            days_left: Some(7),
        };
        let json = wire(&view);
        let object = json.as_object().expect("the view serialises to an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["days_left", "email", "signed_in", "state"]);
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
