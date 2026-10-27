//! One release number, everywhere it is written (s115 tech-debt item 1,
//! notes `AUTO-UPDATE-DESIGN.md` §7).
//!
//! The version lives in four places: the workspace `Cargo.toml` (the app's
//! own "is the offered update newer" check reads it), `tauri.conf.json` (the
//! updater and the installer read it), and the release build's
//! `RELEASE_VERSION` and Mac download names. The release build already refuses
//! a mismatch, but only hours into a run on GitHub's machines; this test fails
//! it on the pull request instead. A bump changes all four in one commit.

const TAURI_CONF: &str = include_str!("../tauri.conf.json");
const RELEASE_BUILD: &str = include_str!("../../../.github/workflows/release-build.yml");

/// The crate version, which is the workspace version (`version.workspace`).
const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[test]
fn the_app_and_its_installer_carry_the_same_version() {
    let conf: serde_json::Value = serde_json::from_str(TAURI_CONF)
        .unwrap_or_else(|e| panic!("tauri.conf.json is not JSON: {e}"));
    assert_eq!(
        conf["version"], CRATE_VERSION,
        "tauri.conf.json and the workspace Cargo.toml must name the same version"
    );
}

#[test]
fn the_release_build_builds_this_version() {
    let line = RELEASE_BUILD
        .lines()
        .find(|l| l.trim_start().starts_with("RELEASE_VERSION:"))
        .unwrap_or_else(|| panic!("release-build.yml has no RELEASE_VERSION"));
    assert_eq!(
        line.trim(),
        format!("RELEASE_VERSION: '{CRATE_VERSION}'"),
        "release-build.yml's RELEASE_VERSION must be the workspace version"
    );
}

#[test]
fn the_mac_download_names_carry_this_version() {
    let line = RELEASE_BUILD
        .lines()
        .find(|l| l.trim_start().starts_with("DMG:"))
        .unwrap_or_else(|| panic!("release-build.yml has no DMG name"));
    for name in [
        format!("'Zaaheen_{CRATE_VERSION}_aarch64.dmg'"),
        format!("'Zaaheen_{CRATE_VERSION}_aarch64_sandbox.dmg'"),
    ] {
        assert!(
            line.contains(&name),
            "the Mac download names must carry {CRATE_VERSION}: {line}"
        );
    }
}
