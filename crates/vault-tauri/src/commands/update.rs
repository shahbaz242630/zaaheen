//! Automatic updates: "Zaaheen 0.3.9 is ready. [Restart to update]"
//! (s115, ADR-116, ADR-SEC-051, notes `AUTO-UPDATE-DESIGN.md`).
//!
//! **What the user sees.** At open and once a day the app asks our one update
//! address (compiled into `tauri.conf.json`) whether a newer version exists.
//! If one does, it is downloaded in the background and the updater checks its
//! seal: our public key, and the version sealed inside the signature
//! (`requireSignedVersion`). Only then does [`update_status`] report it ready,
//! and the banner offers "Restart to update". Nothing installs until the user
//! clicks; a failed check or download says nothing and is tried again later.
//!
//! **What this module adds to the updater's own checks** (ADR-SEC-051):
//! - the offered version must be a plain `x.y.z` and higher than this one;
//! - the file must come from `https://dl.zaaheen.com/`, whatever the
//!   manifest says (its per-platform address is not sealed);
//! - the verified bytes stay in memory until the click: the updater does not
//!   verify again at install, so they are never parked in a file another
//!   program could swap;
//! - on a Mac, the new app must also satisfy the running app's own Apple
//!   signing requirement (a downloaded update is never checked by Gatekeeper).
//!
//! **The click.** The vault is taken exactly as "Delete everything" takes it
//! (`take_exclusive`): the keeper serving AI apps finishes what it is doing
//! and leaves, and no new one can start. That hold is kept through the
//! install. Both are operating-system locks, so they are released when this
//! process ends, including through the updater's own exit on Windows.
//!
//! Both commands are on the guard's OPEN list (founder s115): someone whose
//! trial or subscription has ended still receives security fixes. Neither
//! reads a memory.

use std::time::Duration;

use tauri::{AppHandle, State, Url};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;
use vault_app::keeper::exclusive::take_exclusive;
use vault_app::keeper::relay::KeychainKeySource;

use crate::link::KeeperLink;

/// Opaque error code: the vault is in use (a nightly tidy-up holds it), so
/// nothing was installed. The page says "try again in a few minutes".
pub const ERR_UPDATE_BUSY: &str = "update_busy";

/// Opaque error code: there is no verified update to install.
pub const ERR_UPDATE_NOT_READY: &str = "update_not_ready";

/// Opaque error code: the install did not start. Nothing was changed.
pub const ERR_UPDATE_FAILED: &str = "update_failed";

/// Where an update file may come from (ADR-SEC-051 control 2).
pub const UPDATE_ORIGIN: &str = "https://dl.zaaheen.com/";

/// How often the check repeats after the one at open.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The same waits as "Delete everything": a serving keeper lets go in well
/// under a second; a nightly tidy-up is not interrupted and runs this out.
const TAKE_WAIT: Duration = Duration::from_secs(20);
const TAKE_STEP: Duration = Duration::from_secs(5);

/// A downloaded, verified update waiting for the click.
struct Ready {
    version: String,
    update: Update,
    bytes: Vec<u8>,
}

/// The update waiting for the click, if any. Managed by the app.
#[derive(Default)]
pub struct UpdateSlot {
    ready: Mutex<Option<Ready>>,
}

/// A plain `x.y.z` version as three numbers. Anything else (a pre-release,
/// build metadata, a leading `v`, spaces, text) is refused: the banner shows
/// this string, and only our own release numbers belong there.
pub fn plain_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.');
    let mut next = || -> Option<u64> {
        let part = parts.next()?;
        let digits_only = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        let no_leading_zero = part == "0" || !part.starts_with('0');
        if digits_only && no_leading_zero && part.len() <= 9 {
            part.parse().ok()
        } else {
            None
        }
    };
    let version = (next()?, next()?, next()?);
    match parts.next() {
        None => Some(version),
        Some(_) => None,
    }
}

/// Offer `offered` only when it is a plain version higher than `running`.
pub fn worth_offering(offered: &str, running: &str) -> bool {
    match (plain_version(offered), plain_version(running)) {
        (Some(offered), Some(running)) => offered > running,
        _ => false,
    }
}

/// The update file must come from our download server over HTTPS, with no
/// other port and no user name in the address.
pub fn from_our_server(url: &Url) -> bool {
    url.as_str().starts_with(UPDATE_ORIGIN)
        && url.scheme() == "https"
        && url.host_str() == Some("dl.zaaheen.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

/// The check at open, then daily. Ends quietly when this build has no update
/// address (a sandbox build, ADR-116).
pub async fn check_at_open_then_daily(app: AppHandle, slot: std::sync::Arc<UpdateSlot>) {
    loop {
        if !check_once(&app, &slot).await {
            return;
        }
        tokio::time::sleep(CHECK_EVERY).await;
    }
}

/// One check. Returns false when updates are off in this build.
async fn check_once(app: &AppHandle, slot: &UpdateSlot) -> bool {
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(tauri_plugin_updater::Error::EmptyEndpoints) => {
            tracing::info!(target: "vault_tauri::update", "updates are off in this build");
            return false;
        }
        Err(e) => {
            tracing::warn!(target: "vault_tauri::update", error = %e, "update check could not start");
            return true;
        }
    };
    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => return true,
        Err(e) => {
            tracing::info!(target: "vault_tauri::update", error = %e, "update check failed; will retry");
            return true;
        }
    };
    let running = env!("CARGO_PKG_VERSION");
    if !worth_offering(&update.version, running) {
        tracing::warn!(target: "vault_tauri::update", "an offered version was refused");
        return true;
    }
    if !from_our_server(&update.download_url) {
        tracing::warn!(target: "vault_tauri::update", "an update from another address was refused");
        return true;
    }
    if slot
        .ready
        .lock()
        .await
        .as_ref()
        .is_some_and(|ready| ready.version == update.version)
    {
        return true;
    }
    // The updater verifies the seal and the sealed version before it returns
    // the bytes; a failure here is a refused or broken download.
    match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => {
            tracing::info!(target: "vault_tauri::update", version = %update.version, "update downloaded and verified");
            *slot.ready.lock().await = Some(Ready {
                version: update.version.clone(),
                update,
                bytes,
            });
        }
        Err(e) => {
            tracing::warn!(target: "vault_tauri::update", error = %e, "update download refused or failed; will retry");
        }
    }
    true
}

/// `{ "ready": bool, "version": "x.y.z" | null }` for the banner.
#[tauri::command]
pub async fn update_status(
    slot: State<'_, std::sync::Arc<UpdateSlot>>,
) -> Result<serde_json::Value, String> {
    let ready = slot.ready.lock().await;
    let version = ready.as_ref().map(|ready| ready.version.clone());
    Ok(serde_json::json!({ "ready": version.is_some(), "version": version }))
}

/// "Restart to update". Takes the vault from the keeper, then hands the
/// verified bytes to the updater. On Windows the updater ends this process
/// and the installer reopens Zaaheen; on a Mac the app restarts itself.
#[tauri::command]
pub async fn restart_to_update(
    app: AppHandle,
    link: State<'_, KeeperLink>,
    slot: State<'_, std::sync::Arc<UpdateSlot>>,
) -> Result<(), String> {
    let mut guard = slot.ready.lock().await;
    let Some(ready) = guard.take() else {
        return Err(ERR_UPDATE_NOT_READY.to_string());
    };

    #[cfg(target_os = "macos")]
    {
        if let Err(e) = mac_seal::check(&ready.bytes) {
            tracing::error!(target: "vault_tauri::update", error = %e, "update refused: the Apple signature check failed");
            return Err(ERR_UPDATE_FAILED.to_string());
        }
    }

    let Some(vault_root) = link.vault_root() else {
        tracing::warn!(target: "vault_tauri::update", "update not started: the vault folder could not be found");
        *guard = Some(ready);
        return Err(ERR_UPDATE_FAILED.to_string());
    };
    // The desktop's own link must not start a keeper while we wait.
    link.poison().await;
    let held = match take_exclusive(&vault_root, &KeychainKeySource, TAKE_WAIT, TAKE_STEP).await {
        Ok(held) => held,
        Err(e) => {
            tracing::info!(target: "vault_tauri::update", error = %e, "update not started: the vault is in use");
            link.clear_poison();
            *guard = Some(ready);
            return Err(ERR_UPDATE_BUSY.to_string());
        }
    };

    tracing::info!(target: "vault_tauri::update", version = %ready.version, "installing update");
    if let Err(e) = ready.update.install(&ready.bytes) {
        tracing::error!(target: "vault_tauri::update", error = %e, "update install did not start; nothing changed");
        drop(held);
        link.clear_poison();
        *guard = Some(ready);
        return Err(ERR_UPDATE_FAILED.to_string());
    }
    // Windows does not reach this line: the updater has ended the process.
    drop(held);
    app.restart()
}

#[cfg(target_os = "macos")]
mod mac_seal {
    //! The Mac's second seal (ADR-SEC-051): the new app must satisfy the
    //! running app's own designated requirement (our Apple team), checked
    //! with the system's `codesign` on a private unpacked copy.

    use std::path::{Path, PathBuf};
    use std::process::Command;

    const CODESIGN: &str = "/usr/bin/codesign";
    const TAR: &str = "/usr/bin/tar";

    pub fn check(bytes: &[u8]) -> Result<(), String> {
        let running = running_bundle()?;
        let requirement = designated_requirement(&running)?;
        let dir = private_dir()?;
        let result = unpack_and_verify(bytes, &dir, &requirement);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    fn running_bundle() -> Result<PathBuf, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // <bundle>.app/Contents/MacOS/<program>
        exe.ancestors()
            .nth(3)
            .filter(|p| p.extension().is_some_and(|ext| ext == "app"))
            .map(Path::to_path_buf)
            .ok_or_else(|| "not running from an app bundle".to_string())
    }

    fn designated_requirement(bundle: &Path) -> Result<String, String> {
        let out = Command::new(CODESIGN)
            .args(["-d", "-r-"])
            .arg(bundle)
            .output()
            .map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        text.lines()
            .find_map(|line| line.strip_prefix("designated => "))
            .map(str::to_string)
            .filter(|req| req.contains("anchor apple generic"))
            .ok_or_else(|| "the running app is not signed by a Developer ID".to_string())
    }

    fn private_dir() -> Result<PathBuf, String> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zaaheen-update-check-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }

    fn unpack_and_verify(bytes: &[u8], dir: &Path, requirement: &str) -> Result<(), String> {
        let archive = dir.join("update.tar.gz");
        std::fs::write(&archive, bytes).map_err(|e| e.to_string())?;
        let unpacked = Command::new(TAR)
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(dir)
            .status()
            .map_err(|e| e.to_string())?;
        if !unpacked.success() {
            return Err("the update could not be unpacked".to_string());
        }
        let app = std::fs::read_dir(dir)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|p| p.extension().is_some_and(|ext| ext == "app"))
            .ok_or_else(|| "the update holds no app".to_string())?;
        let verified = Command::new(CODESIGN)
            .args(["--verify", "--deep", "--strict"])
            .arg(format!("--test-requirement=={requirement}"))
            .arg(&app)
            .status()
            .map_err(|e| e.to_string())?;
        if verified.success() {
            Ok(())
        } else {
            Err("the update is not signed by our Apple team".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_release_numbers_are_versions() {
        assert_eq!(plain_version("0.3.8"), Some((0, 3, 8)));
        assert_eq!(plain_version("10.0.12"), Some((10, 0, 12)));
        for bad in [
            "",
            "0.3",
            "0.3.8.1",
            "v0.3.8",
            "0.3.8-beta",
            "0.3.8+1",
            " 0.3.8",
            "0.3.08",
            "0..8",
            "0.3.x",
            "<b>0.3.9</b>",
            "1234567890.0.0",
        ] {
            assert_eq!(plain_version(bad), None, "{bad:?} must be refused");
        }
    }

    #[test]
    fn only_a_higher_version_is_offered() {
        assert!(worth_offering("0.3.9", "0.3.8"));
        assert!(worth_offering("0.10.0", "0.9.9"));
        assert!(worth_offering("1.0.0", "0.99.99"));
        assert!(!worth_offering("0.3.8", "0.3.8"));
        assert!(!worth_offering("0.3.7", "0.3.8"));
        assert!(!worth_offering("0.3.9-rc1", "0.3.8"));
        assert!(!worth_offering("0.3.9", "not a version"));
    }

    #[test]
    fn only_our_download_server_over_https() {
        let ok = |s: &str| from_our_server(&Url::parse(s).expect("url"));
        assert!(ok("https://dl.zaaheen.com/Zaaheen_0.3.9_x64.msi"));
        assert!(ok("https://dl.zaaheen.com/updates/Zaaheen.app.tar.gz"));
        for bad in [
            "http://dl.zaaheen.com/Zaaheen_0.3.9_x64.msi",
            "https://dl.zaaheen.com.evil.example/x.msi",
            "https://evil.example/dl.zaaheen.com/x.msi",
            "https://dl.zaaheen.com:8443/x.msi",
            "https://user@dl.zaaheen.com/x.msi",
            "https://zaaheen.com/x.msi",
            "file:///C:/x.msi",
        ] {
            assert!(!ok(bad), "{bad} must be refused");
        }
    }
}
