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
//! - the verified file waits on disk in the app's own folder (not ~155 MB in
//!   memory all day: founder s115), and at the click its seal is checked
//!   AGAIN on the exact bytes about to be installed ([`seal`]), because the
//!   updater itself checks only once, at download;
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

mod seal;

use std::path::{Path, PathBuf};
use std::time::Duration;

use tauri::{AppHandle, Manager, State, Url};
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

/// The folder under the app's local data where a verified update waits.
const UPDATES_DIR: &str = "updates";
/// The waiting file, and the name it is written under before it is complete.
const PENDING: &str = "pending.update";
const PENDING_PART: &str = "pending.update.part";

/// A downloaded, verified update waiting for the click. The bytes are in
/// `file`; `update` carries the signature they are checked against again.
struct Ready {
    version: String,
    update: Update,
    file: PathBuf,
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
    clear_leftovers(&app);
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
    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(target: "vault_tauri::update", error = %e, "update download refused or failed; will retry");
            return true;
        }
    };
    let Some(dir) = updates_dir(app) else {
        tracing::warn!(target: "vault_tauri::update", "no folder to keep the update in; will retry");
        return true;
    };
    match park(&dir, &bytes) {
        Ok(file) => {
            tracing::info!(target: "vault_tauri::update", version = %update.version, "update downloaded and verified");
            *slot.ready.lock().await = Some(Ready {
                version: update.version.clone(),
                update,
                file,
            });
        }
        Err(e) => {
            tracing::warn!(target: "vault_tauri::update", error = %e, "update could not be kept; will retry");
        }
    }
    true
}

/// `<app local data>/updates`, made if missing.
fn updates_dir(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_local_data_dir().ok()?.join(UPDATES_DIR);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Write the verified bytes under a temporary name, then rename: a waiting
/// file is always complete. Returns where it waits.
fn park(dir: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let part = dir.join(PENDING_PART);
    let file = dir.join(PENDING);
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, &file)?;
    Ok(file)
}

/// At start: nothing is waiting (the wait lives in this process), so remove
/// what an earlier run left: our own waiting file, and on Windows the
/// updater's own installer folders, which it never deletes (~155 MB each).
fn clear_leftovers(app: &AppHandle) {
    if let Some(dir) = updates_dir(app) {
        for name in [PENDING, PENDING_PART] {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }
    #[cfg(windows)]
    {
        let product = app.package_info().name.clone();
        if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if is_updater_leftover(&name, &product) {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }
}

/// The updater's Windows folder: `%TEMP%\<product>-<version>-updater-<random>`.
pub fn is_updater_leftover(name: &str, product: &str) -> bool {
    name.strip_prefix(product)
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.split_once("-updater-"))
        .is_some_and(|(version, random)| plain_version(version).is_some() && !random.is_empty())
}

/// The public key compiled into this app (`plugins.updater.pubkey`).
fn compiled_pubkey(app: &AppHandle) -> Option<String> {
    app.config()
        .plugins
        .0
        .get("updater")?
        .get("pubkey")?
        .as_str()
        .map(str::to_string)
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

    // The seal again, on the exact bytes about to be installed: the file
    // waited on disk, and the updater will not check it a second time. A
    // refusal drops the update; the next daily check downloads it afresh.
    let bytes = match std::fs::read(&ready.file) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(target: "vault_tauri::update", error = %e, "the waiting update could not be read");
            return Err(ERR_UPDATE_FAILED.to_string());
        }
    };
    let Some(pubkey) = compiled_pubkey(&app) else {
        tracing::error!(target: "vault_tauri::update", "no update key compiled in");
        return Err(ERR_UPDATE_FAILED.to_string());
    };
    if let Err(why) = seal::verify(&bytes, &ready.update.signature, &pubkey, &ready.version) {
        tracing::error!(target: "vault_tauri::update", reason = ?why, "update refused at the click: the waiting file failed its seal");
        let _ = std::fs::remove_file(&ready.file);
        return Err(ERR_UPDATE_FAILED.to_string());
    }

    #[cfg(target_os = "macos")]
    {
        if let Err(e) = mac_seal::check(&bytes) {
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
    if let Err(e) = ready.update.install(&bytes) {
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
mod mac_seal;

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
    fn only_the_updaters_own_folders_are_cleared() {
        assert!(is_updater_leftover(
            "Zaaheen-0.3.9-updater-a1B2c3",
            "Zaaheen"
        ));
        for keep in [
            "Zaaheen",
            "Zaaheen-0.3.9-updater-",
            "Zaaheen-latest-updater-x",
            "Zaaheen-0.3.9-backup-x",
            "Other-0.3.9-updater-x",
            "ZaaheenX-0.3.9-updater-x",
            "zaaheen-update-check-1-2",
        ] {
            assert!(
                !is_updater_leftover(keep, "Zaaheen"),
                "{keep} must be left alone"
            );
        }
    }

    #[test]
    fn a_waiting_update_is_whole_or_absent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = park(dir.path(), b"verified bytes").expect("parked");
        assert_eq!(file, dir.path().join(PENDING));
        assert_eq!(std::fs::read(&file).expect("read"), b"verified bytes");
        assert!(!dir.path().join(PENDING_PART).exists());
        // A second update replaces the first.
        park(dir.path(), b"newer").expect("parked again");
        assert_eq!(std::fs::read(&file).expect("read"), b"newer");
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
