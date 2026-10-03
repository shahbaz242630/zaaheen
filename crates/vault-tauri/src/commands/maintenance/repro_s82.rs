//! THROWAWAY SPIKE (session 82, branch repro/s82-mac-delete; never merged).
//! Reproduce the Mac "Delete my account" hang on GitHub's macOS runner with
//! the production code paths: record the vault location, let a real keeper
//! create the key and serve, switch the nightly schedule on, then run the
//! delete steps in delete_account_start's order, each under a time limit.
//! On a hang: a native stack sample of this process and every keeper, and a
//! screenshot (a keychain question would be on screen).

use std::future::Future;
use std::time::{Duration, Instant};

use crate::commands::erasure::erase_everything_inner;
use crate::link::{KeeperLink, Kind};

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn say(line: &str) {
    eprintln!("[repro {:>7.1}s] {line}", START.get_or_init(Instant::now).elapsed().as_secs_f64());
}

fn evidence(tag: &str) {
    let pid = std::process::id().to_string();
    let out = std::env::var("REPRO_OUT").unwrap_or_else(|_| "/tmp/repro".into());
    let _ = std::fs::create_dir_all(&out);
    let _ = std::process::Command::new("screencapture")
        .args(["-x", &format!("{out}/{tag}-screen.png")])
        .status();
    let _ = std::process::Command::new("sample")
        .args([&pid, "3", "-file", &format!("{out}/{tag}-sample-self.txt")])
        .status();
    if let Ok(o) = std::process::Command::new("pgrep").arg("-l").arg("zaaheen").output() {
        let list = String::from_utf8_lossy(&o.stdout).to_string();
        say(&format!("zaaheen processes:\n{list}"));
        for line in list.lines() {
            if let Some(p) = line.split_whitespace().next() {
                let _ = std::process::Command::new("sample")
                    .args([p, "3", "-file", &format!("{out}/{tag}-sample-{p}.txt")])
                    .status();
            }
        }
    }
    if let Ok(o) = std::process::Command::new("pgrep").arg("-l").arg("SecurityAgent").output() {
        say(&format!("SecurityAgent: {}", String::from_utf8_lossy(&o.stdout).trim()));
    }
}

async fn step<T: std::fmt::Debug>(
    name: &str,
    limit: Duration,
    fut: impl Future<Output = T>,
) -> Option<T> {
    say(&format!("STEP START {name}"));
    let t = Instant::now();
    match tokio::time::timeout(limit, fut).await {
        Ok(v) => {
            say(&format!(
                "STEP DONE  {name} in {:.2}s -> {v:?}",
                t.elapsed().as_secs_f64()
            ));
            Some(v)
        }
        Err(_) => {
            say(&format!("STEP HANG  {name}: no answer after {limit:?}"));
            evidence(&name.replace(' ', "_"));
            None
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spike: run on a throwaway macOS runner only"]
async fn repro_s82_mac_delete() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("debug,hyper=info,rustls=info,reqwest=info,h2=info")
        .with_writer(std::io::stderr)
        .try_init();
    let mut hung = Vec::new();

    let homes = vault_app::location::Homes::production().expect("homes");
    let key = vault_app::keychain::KeyLocation::production().expect("key location");
    say(&format!("exe dir {:?}", vault_app::install_paths::resource_dir()));

    // 1. The first-run setup's location step.
    let (h, k) = (homes.clone(), key.clone());
    let r = step("prepare location", Duration::from_secs(30), tokio::task::spawn_blocking(move || {
        vault_app::location::prepare(&h, &k, &[]).map(|d| d.path().to_path_buf())
    }))
    .await;
    say(&format!("location: {r:?}"));

    // 2. The desktop's link; its first call starts a keeper, which makes the key.
    let link = KeeperLink::new(homes.clone());
    if step("first keeper call", Duration::from_secs(120), link.call("admin_engine_status", serde_json::json!({}), Kind::Read)).await.is_none() {
        hung.push("first keeper call");
    }
    say(&format!("key state after start: {:?}", link.key_state()));
    // A second call, as the home screen makes, so the link is connected.
    let _ = step("second keeper call", Duration::from_secs(60), link.call("admin_agent_list", serde_json::json!({}), Kind::Read)).await;

    // 3. Consolidation switched on (daily 03:00), as the friend did.
    let exe_dir = vault_app::install_paths::resource_dir().expect("exe dir");
    let data = std::env::var("HOME").map(std::path::PathBuf::from).expect("HOME").join("Library/Application Support/com.zaaheen.repro");
    let _ = std::fs::create_dir_all(&data);
    let ctx = super::MaintenanceContext {
        vault_cli: exe_dir.join("zaaheen"),
        vault_maintenance: exe_dir.join("zaaheen-maintenance"),
        log_dir: vault_app::install_paths::log_dir().expect("log dir"),
        bge_model: exe_dir.join("models/model.onnx"),
        bge_tokenizer: exe_dir.join("models/tokenizer.json"),
        ort_lib: exe_dir.join("libs/libonnxruntime.dylib"),
        phi4_model: data.join("models/phi4.gguf"),
        config_path: data.join("maintenance.json"),
    };
    let config = super::MaintenanceConfig { enabled: true, hour: 3, minute: 0, ..Default::default() };
    let spec = super::build_spec(&ctx, &config).expect("spec");
    let _ = step("register schedule", Duration::from_secs(30), tokio::task::spawn_blocking(move || {
        vault_scheduler::platform_scheduler().and_then(|s| s.register(&spec)).map_err(|e| e.to_string())
    }))
    .await;
    let _ = super::save_config(&ctx.config_path, &config);
    if let Ok(o) = std::process::Command::new("launchctl").arg("list").output() {
        say(&format!("launchctl zaaheen: {}", String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.contains("zaaheen")).collect::<Vec<_>>().join(" | ")));
    }

    // 4. Delete my account, in delete_account_start's order.
    let (_entitlement, account) = crate::guard::build();
    let page = account.delete_account_link();
    say(&format!("delete page: {:?}", page.as_ref().map(|p| p.as_str().to_string())));
    say(&format!("vault recorded: {}", link.vault_recorded()));

    let mut failed: Vec<&str> = Vec::new();
    match step("erase_everything_inner", Duration::from_secs(120), erase_everything_inner(&link)).await {
        None => hung.push("erase_everything_inner"),
        Some(r) => {
            say(&format!("erase result: {r:?}"));
            match &r {
                Ok(v) if v["key_destroyed"] == serde_json::Value::Bool(true) => say("PROOF erase reported key_destroyed=true"),
                _ => failed.push("erase did not report key_destroyed=true"),
            }
        }
    }
    // The key must be gone from the login keychain (attributes only).
    let found = std::process::Command::new("security")
        .args(["find-generic-password", "-s", vault_app::keychain::PRODUCTION_NAMESPACE, "-a", vault_app::keychain::VAULT_ID])
        .output();
    match found {
        Ok(o) if !o.status.success() => say(&format!("PROOF key absent from the keychain (security exit {:?})", o.status.code())),
        Ok(_) => failed.push("the key is STILL in the keychain"),
        Err(e) => failed.push(Box::leak(format!("security could not run: {e}").into_boxed_str())),
    }
    if step("sign_out_after_erasure", Duration::from_secs(90), account.sign_out_after_erasure()).await.is_none() {
        hung.push("sign_out_after_erasure");
    }
    if let Ok(page) = page {
        if step("page.open", Duration::from_secs(30), tokio::task::spawn_blocking(move || page.open().is_ok())).await.is_none() {
            hung.push("page.open");
        }
    }
    say(&format!("key state after erase: {:?}", link.key_state()));
    // Start again: a new window (the old one closes after a delete) makes a fresh key.
    let again = KeeperLink::new(homes.clone());
    match step("start again", Duration::from_secs(120), again.call("admin_engine_status", serde_json::json!({}), Kind::Read)).await {
        None => hung.push("start again"),
        Some(r) => {
            say(&format!("start again: ok={}", r.is_ok()));
            if r.is_err() { failed.push("could not start again after the delete"); }
        }
    }
    say(&format!("key state at end: {:?}", again.key_state()));
    evidence("end");
    say(&format!("SUMMARY hung steps: {hung:?} failed: {failed:?}"));
    // Exit now: a blocking task stuck in a hang would hold the runtime's
    // shutdown forever.
    std::process::exit(if !hung.is_empty() { 3 } else if !failed.is_empty() { 4 } else { 0 });
}
