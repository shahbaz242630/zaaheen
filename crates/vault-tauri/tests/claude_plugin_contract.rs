//! The Claude plugin's contract (session 83).
//!
//! The Claude plugin (github.com/shahbaz242630/zaaheen-claude, its server
//! declared inline in `.claude-plugin/plugin.json` since v1.0.3) carries no
//! program of its own: it runs the Zaaheen app installed on this computer as
//! `zaaheen mcp serve`, found on the system PATH on Windows and through the
//! app bundle's folder, which the plugin adds to PATH, on a Mac. So the
//! app and the plugin can never speak different versions; the plugin breaks
//! only if one of the three facts below changes. The handshake's `WIRE`
//! version is NOT one of them: the relay the plugin starts is the installed
//! app's own.
//!
//! If this test fails, the change also needs a plugin release: update
//! `.claude-plugin/plugin.json` in zaaheen-claude (its command, arguments
//! or the PATH folder), push, and re-check the listing. Then update the
//! expected values here.

/// What the plugin runs today (zaaheen-claude `768043a`, v1.0.3): the Mac
/// path is the app bundle's folder on the plugin's PATH plus the program.
const PLUGIN_WINDOWS_COMMAND: &str = "zaaheen";
const PLUGIN_MAC_COMMAND: &str = "/Applications/Zaaheen.app/Contents/MacOS/zaaheen";
const PLUGIN_ARGS: [&str; 2] = ["mcp", "serve"];

const TAURI_CONF: &str = include_str!("../tauri.conf.json");
const MAC_CONF: &str = include_str!("../tauri.mac-test.conf.json");

const RELEASE_NOTE: &str = "The Claude plugin (zaaheen-claude, .claude-plugin/plugin.json) runs \
     this. Change it there too, push and re-check the listing, then update this test.";

#[test]
fn the_program_name_on_path_is_what_the_plugin_runs_on_windows() {
    assert_eq!(
        vault_app::server_command::SHORT_NAME,
        PLUGIN_WINDOWS_COMMAND,
        "{RELEASE_NOTE}"
    );
}

#[test]
fn the_mac_install_path_is_what_the_plugin_runs_on_a_mac() {
    // /Applications/<productName>.app/Contents/MacOS/<the bundled program>.
    let conf: serde_json::Value = serde_json::from_str(TAURI_CONF).expect("tauri.conf.json");
    let product = conf["productName"].as_str().expect("productName");
    assert!(
        MAC_CONF.contains("\"MacOS/zaaheen\": \"../../target/release/zaaheen\""),
        "the Mac bundle no longer puts the program at Contents/MacOS/zaaheen. {RELEASE_NOTE}"
    );
    assert_eq!(
        format!("/Applications/{product}.app/Contents/MacOS/zaaheen"),
        PLUGIN_MAC_COMMAND,
        "{RELEASE_NOTE}"
    );
}

#[test]
fn the_arguments_are_what_the_plugin_passes() {
    let json = vault_app::server_command::ServerCommand::short_name().server_json();
    assert_eq!(
        json["args"],
        serde_json::json!(PLUGIN_ARGS),
        "{RELEASE_NOTE}"
    );
}
