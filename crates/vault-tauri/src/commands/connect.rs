//! "Connect it for me" (ADR-106 + ADR-SEC-031, `CONNECT-APPS-DESIGN.md`).
//!
//! Zaaheen asks the app to install it through the app's own route; it never
//! writes the app's settings. Gated, like every setup action after sign-in.
//! The page names only **which** app, from a closed list: no path, no link
//! and no command crosses the IPC boundary. Cursor's link is fixed in
//! `vault_app::external_link`; Claude's extension is built in
//! `vault_app::connect` and saved to the person's Downloads folder, found
//! here from Windows, never from the page.

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};
use vault_app::connect::{self, ConnectOutcome};

use crate::guard::Entitlement;

/// The app's icon, carried by the Claude extension so Claude shows it.
const ICON_PNG: &[u8] = include_bytes!("../../icons/icon.png");

/// The apps Zaaheen can ask to install it. Anything else the page sends is
/// refused before the command runs.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectableApp {
    Cursor,
    ClaudeDesktop,
}

/// Ask `app` to install Zaaheen. Answers `{ "outcome": "asked" | "saved" |
/// "app_not_found" | "could_not_save" | "could_not_open" }`; the reason
/// behind a failure goes to the log only.
#[tauri::command]
pub async fn connect_app(
    entitlement: State<'_, Entitlement>,
    handle: AppHandle,
    app: ConnectableApp,
) -> Result<Value, String> {
    let _entitled = entitlement.require().await?;
    let downloads = handle.path().download_dir().ok();
    let outcome = tokio::task::spawn_blocking(move || match app {
        ConnectableApp::Cursor => connect::connect_cursor(),
        ConnectableApp::ClaudeDesktop => connect::connect_claude(downloads.as_deref(), ICON_PNG),
    })
    .await
    .unwrap_or_else(|e| {
        tracing::error!(target: "vault_tauri::connect", error = %e, "asking an app to install Zaaheen stopped");
        ConnectOutcome::CouldNotOpen
    });
    tracing::info!(target: "vault_tauri::connect", ?app, outcome = outcome.code(), "asked an app to install Zaaheen");
    Ok(json!({ "outcome": outcome.code() }))
}

/// "Show the file": open the Downloads folder the Claude extension was saved
/// in. Answers `{ "shown": bool }`.
#[tauri::command]
pub async fn show_claude_extension(
    entitlement: State<'_, Entitlement>,
    handle: AppHandle,
) -> Result<Value, String> {
    let _entitled = entitlement.require().await?;
    let Ok(downloads) = handle.path().download_dir() else {
        return Ok(json!({ "shown": false }));
    };
    let shown = tokio::task::spawn_blocking(move || connect::show_downloads(&downloads))
        .await
        .unwrap_or(false);
    Ok(json!({ "shown": shown }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_listed_app_can_be_asked() {
        for (wire, app) in [
            ("cursor", ConnectableApp::Cursor),
            ("claude_desktop", ConnectableApp::ClaudeDesktop),
        ] {
            let got: ConnectableApp = serde_json::from_value(json!(wire)).unwrap();
            assert_eq!(got, app);
        }
        for refused in [
            json!("Cursor"),
            json!("claude"),
            json!("codex"),
            json!("cursor://anysphere.cursor-deeplink/mcp/install"),
            json!(r"C:\Windows\System32\cmd.exe"),
            json!({ "cursor": true }),
        ] {
            assert!(
                serde_json::from_value::<ConnectableApp>(refused.clone()).is_err(),
                "accepted {refused}"
            );
        }
    }

    /// The page has words for every answer (dist/app.js).
    #[test]
    fn every_answer_has_words_on_the_page() {
        let app_js = include_str!("../../dist/app.js");
        for outcome in [
            ConnectOutcome::Asked,
            ConnectOutcome::Saved,
            ConnectOutcome::AppNotFound,
            ConnectOutcome::CouldNotSave,
            ConnectOutcome::CouldNotOpen,
        ] {
            let code = format!("{}:", outcome.code());
            assert!(app_js.contains(&code), "the page has no words for {code}");
        }
    }

    /// ADR-SEC-031: nothing from the page names a path, a link or a program.
    /// `AppHandle` is Tauri's own, never the page's.
    #[test]
    fn the_commands_take_nothing_but_the_app() {
        let code: String = include_str!("connect.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        // Split at the commas between arguments, not the one inside
        // `State<'_, Entitlement>`.
        let args_of = |name: &str| -> Vec<String> {
            let list = code
                .split_once(&format!("pub async fn {name}("))
                .unwrap_or_else(|| panic!("{name} is defined here"))
                .1
                .split_once(") -> ")
                .expect("its arguments close")
                .0;
            let (mut args, mut arg, mut depth) = (Vec::new(), String::new(), 0_u32);
            for c in list.chars() {
                match c {
                    '<' => depth += 1,
                    '>' => depth = depth.saturating_sub(1),
                    ',' if depth == 0 => {
                        args.push(std::mem::take(&mut arg));
                        continue;
                    }
                    _ => {}
                }
                arg.push(c);
            }
            args.push(arg);
            args.into_iter()
                .map(|a| a.trim().to_owned())
                .filter(|a| !a.is_empty())
                .collect()
        };
        assert_eq!(
            args_of("connect_app"),
            [
                "entitlement: State<'_, Entitlement>",
                "handle: AppHandle",
                "app: ConnectableApp"
            ]
        );
        assert_eq!(
            args_of("show_claude_extension"),
            ["entitlement: State<'_, Entitlement>", "handle: AppHandle"]
        );
    }
}
