//! Automatic updates: the settings that make an update trustworthy (s115,
//! ADR-116, ADR-SEC-051, notes `AUTO-UPDATE-DESIGN.md` §3).
//!
//! The updater accepts an update only when its file carries our seal
//! (`pubkey`), it came from our one address (`endpoints`), and the version
//! sealed inside the signature is the version offered (`requireSignedVersion`,
//! which stops an older, genuinely sealed build being offered under a newer
//! number). Each of those lives in a config file, where a one-line edit would
//! quietly switch it off; these tests make that edit fail instead.

const TAURI_CONF: &str = include_str!("../tauri.conf.json");
const MAC_CONF: &str = include_str!("../tauri.mac-test.conf.json");
const SANDBOX_CONF: &str = include_str!("../tauri.sandbox.conf.json");
const CAPABILITIES: &str = include_str!("../capabilities/default.json");

/// The one address the app asks. Compiled in; nothing else may name another.
const ENDPOINT: &str = "https://dl.zaaheen.com/updates/latest.json";

/// The public half of the update signing key compiled into the app.
///
/// The real key (key ID A6B6220B582FCA5D), made by the founder in s116 with no
/// AI open; its private half lives only in his password manager and the
/// `update-signing` environment (ADR-SEC-051 key custody). The s115 rehearsal
/// key (790322A4BDE7902B) is refused by the release build.
const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEE2QjYyMjBCNTgyRkNBNUQKUldSZHlpOVlDeUsycGdWYkF1V0Q0OXh6MWUvM3FBeHV3bFZxUTQ2UG9OOWJ1REtTWXcyUlpTaGgK";

fn json(text: &str, name: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{name} is not JSON: {e}"))
}

fn updater(conf: &serde_json::Value) -> &serde_json::Value {
    &conf["plugins"]["updater"]
}

#[test]
fn the_app_asks_one_fixed_https_address() {
    let conf = json(TAURI_CONF, "tauri.conf.json");
    assert_eq!(
        updater(&conf)["endpoints"],
        serde_json::json!([ENDPOINT]),
        "the update address changed; ADR-SEC-051 control 2 names exactly one"
    );
}

#[test]
fn the_seal_is_the_pinned_public_key() {
    let conf = json(TAURI_CONF, "tauri.conf.json");
    assert_eq!(updater(&conf)["pubkey"], serde_json::json!(PUBKEY));
}

#[test]
fn the_signed_version_must_match_the_offered_one() {
    // ADR-SEC-051 control 3: without it, whoever can write to the download
    // server can offer an old sealed build under a new number.
    let conf = json(TAURI_CONF, "tauri.conf.json");
    assert_eq!(
        updater(&conf)["requireSignedVersion"],
        serde_json::json!(true)
    );
}

#[test]
fn no_safety_check_is_switched_off() {
    let conf = json(TAURI_CONF, "tauri.conf.json");
    let updater = updater(&conf)
        .as_object()
        .expect("plugins.updater is an object");
    for key in updater.keys() {
        assert!(
            !key.starts_with("dangerous") && key != "allowDowngrades",
            "plugins.updater.{key} weakens the update checks (ADR-SEC-051)"
        );
    }
    // The Windows installer must refuse an older version too.
    assert_eq!(
        conf["bundle"]["windows"]["allowDowngrades"],
        serde_json::json!(false)
    );
}

#[test]
fn windows_installs_with_a_progress_bar_only() {
    let conf = json(TAURI_CONF, "tauri.conf.json");
    assert_eq!(
        updater(&conf)["windows"]["installMode"],
        serde_json::json!("passive")
    );
}

#[test]
fn the_mac_build_inherits_the_update_settings_unchanged() {
    // The Mac build merges tauri.mac-test.conf.json over the base file. It
    // must not carry its own updater block, or the two could drift apart.
    let mac = json(MAC_CONF, "tauri.mac-test.conf.json");
    assert!(
        mac.get("plugins").is_none(),
        "the Mac overlay must not override the updater"
    );
}

#[test]
fn a_sandbox_build_has_no_update_address_and_nothing_else_changed() {
    // ADR-116: a test install can never pull a production build. The overlay
    // empties the address list and touches nothing else: the key stays,
    // because without one the plugin refuses to start the app.
    let sandbox = json(SANDBOX_CONF, "tauri.sandbox.conf.json");
    assert_eq!(
        sandbox,
        serde_json::json!({ "plugins": { "updater": { "endpoints": [] } } })
    );
}

#[test]
fn the_page_is_given_no_updater_permission() {
    // ADR-SEC-051: the plugin is driven from Rust only. Without an `updater:`
    // permission the page cannot call the plugin's own commands.
    assert!(
        !CAPABILITIES.contains("updater:"),
        "capabilities/default.json grants an updater permission to the page"
    );
    // And no other capability file has appeared that could grant one.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
    let files: Vec<String> = std::fs::read_dir(&dir)
        .expect("capabilities folder")
        .map(|entry| {
            entry
                .expect("capabilities entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(files, vec!["default.json".to_string()]);
}
