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
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
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
