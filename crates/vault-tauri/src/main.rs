//! `vault-tauri` binary entry point — Tauri shell that wraps the V0.1
//! composition root from vault-app. T0.1.11 Phase 4b.
//!
//! ## ADR cross-references
//!
//! - **ADR-003:** library→binary conversion lands at T0.1.11 Phase 3.
//!   Library target retained at `src/lib.rs` for testable utilities
//!   (env-var parsing, OS dispatch, integrity-failure formatting,
//!   resource-path env-var override checks); main.rs is thin Tauri
//!   Builder orchestration on top.
//! - **ADR-019:** bundled libonnxruntime dylib resolved via
//!   `app.path().resolve(filename, BaseDirectory::Resource)`. Production
//!   path. Dev-mode override via `VAULT_ORT_LIB_PATH` env var (testable
//!   via `vault_tauri::env_override_for`).
//! - **ADR-020:** model + tokenizer SHA-256 verification at
//!   `Application::new` — failure surfaces as fatal Tauri dialog via
//!   tauri-plugin-dialog, exits non-zero before any UI loads.
//! - **ADR-029:** branch (2) Windows-dogfood lock; founder runs
//!   `cargo run -p vault-tauri` on Windows 11 dev machine for V0.1
//!   founder-only dogfood (T0.1.12).
//! - **ADR-030:** vault-tauri spawns no external child MCP process — no
//!   user-controlled StdioServerParameters surface, no external-MCP-
//!   server-config UI in V0.1. Outcome shape (a) per ADR-026 forward-
//!   pointer. Phase 4b adds source-grep regression test in
//!   `lib.rs::tests::main_rs_does_not_register_external_mcp_spawn_command_per_adr_030`.
//! - **ADR-032:** SQLCipher passphrase sourced from `VAULT_KEY` env var
//!   for V0.1 founder-only dogfood. **Retired at T0.2.0 Phase 1
//!   (2026-05-09)** per ADR-040 + ADR-040 amendment: master_key now
//!   sourced from Windows Credential Manager via `vault_app::keychain::
//!   read_or_init_master_key`; SqlCipherKey + at-rest key derived as
//!   domain-separated BLAKE3 subkeys. Pre-Phase-1 callers reading
//!   VAULT_KEY env var are removed.
//! - **ADR-034 (Phase 5b fix-forward, 2026-05-05):** V0.1 vault-tauri is
//!   UI-only — no MCP server bound inside the Tauri process. Phase 5
//!   founder smoke surfaced that `Application::start_with_mcp` calls
//!   `rmcp::ServiceExt::serve(server, stdio()).await` which blocks on
//!   JSON-RPC `initialize` from a non-existent peer when launched as a
//!   Tauri UI app, hanging Tauri's setup() hook. Phase 5b replaces the
//!   call with `Application::spawn_retry_worker()` (worker-only, no MCP transport
//!   bind). AI-client MCP integration deferred to V0.2 alpha-distribution
//!   subcommand-split task. T0.1.12 founder dogfood is UI-only for V0.1.
//! - **ADR-038 (T0.2.0 Phase 0a fix-forward, 2026-05-07):** the binary's
//!   process environment MUST have `LANCE_MEM_POOL_SIZE=268435456` (256
//!   MiB) set BEFORE this binary launches, so lance/datafusion's
//!   `merge_insert` JOIN path is bounded. This cannot be set inside Rust
//!   code — ADR-002 forbids `unsafe_code` workspace-wide, and rustc 1.80+
//!   marks `std::env::set_var` as `unsafe`. The shell-level launcher is
//!   the correct semantic home: lance reads the var lazily on first
//!   datafusion-plan construction, so it must already be in the
//!   environment when the binary starts. Dev runs via `cargo` pick this
//!   up from `.cargo/config.toml`'s `[env]` block; CI runs pick it up
//!   from `.github/workflows/ci.yml`'s top-level `env:` block; V0.2
//!   alpha-distribution launchers (T0.2.14) MUST set it via WiX MSI
//!   pre-args (Windows), Info.plist `LSEnvironment` (macOS .app), or a
//!   `.desktop` `Exec` wrapper (Linux). See ADR-038 in HANDOFF.md and
//!   the struct-field doc on
//!   `vault_storage::vector_store::LanceVectorStore::upsert_lock`.
//! - **Phase 4a HIGH findings cleared at Phase 4b:** line 170
//!   `Boundary::default_name()` swap; line 191 `tauri::Builder::run`
//!   match + `eprintln!` + `std::process::exit`; lines 122-131
//!   `?`-propagation → match + `show_fatal_dialog_and_exit` routing;
//!   phantom `_force_sqlcipher_key_import_visible` deletion;
//!   `resolve_*` + `format_config_error_dialog` extracted to lib.rs
//!   for testability.

#![forbid(unsafe_code)]
// Phase 5e fix-forward (T0.1.12 dogfood Finding #2): mark the binary as
// Windows GUI subsystem (not console subsystem) for release builds. Without
// this attribute, Windows allocates a console window alongside the Tauri
// UI on every launch — a stray "black terminal" window that looks broken to
// any user. Standard Tauri 2 starter-template line that was dropped during
// T0.1.11 Phase 3 lib→bin conversion. `cfg_attr(not(debug_assertions), ...)`
// preserves the console for debug builds (so println / tracing is visible
// during dev) while hiding it for release/MSI distribution.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use vault_app::keychain::{
    bridge_or_init_master_key, derive_at_rest_key, derive_sqlcipher_passphrase, with_key_init_lock,
    PRODUCTION_NAMESPACE, VAULT_ID,
};
use vault_app::{AppConfig, Application};
use vault_tauri::{
    dylib_filename_for_os, env_override_for, format_keychain_error_dialog,
    format_startup_failure_dialog, model_fetch,
};

/// Exit code for keychain provenance failures (ADR-040 + ADR-040 amendment;
/// retains the same numeric code that V0.1 used for VAULT_KEY config errors,
/// since wrapper scripts and CI keyed on `2 = config-class error` regardless
/// of the underlying provenance mechanism).
const EXIT_CONFIG_ERROR: i32 = 2;
/// Exit code for Application startup failures including ADR-020
/// ModelIntegrityFailed.
const EXIT_STARTUP_FAILURE: i32 = 1;

fn main() {
    // Logging is initialised inside `.setup()` rather than here: the log
    // directory comes from `app.path().app_log_dir()`, which does not exist
    // until Tauri has built the app handle. `tracing_subscriber::fmt::init()`
    // used to run at this point and wrote to STDOUT -- which a GUI process
    // launched from Explorer does not have, so every event this application
    // emitted went nowhere (ADR-SEC-014).

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            // Commands are referenced by their DEFINING module, not through
            // the `commands` re-exports: `#[tauri::command]` emits hidden
            // companion items (`__cmd__<name>`) beside each function, and a
            // plain `pub use` does not carry those, so `generate_handler!`
            // cannot resolve a re-exported path.
            vault_tauri::commands::memory::add_memory,
            vault_tauri::commands::memory::search_memories,
            vault_tauri::commands::memory::update_memory,
            vault_tauri::commands::memory::delete_memory,
            vault_tauri::commands::memory::list_recent_memories,
            vault_tauri::commands::boundary::list_boundaries,
            vault_tauri::commands::boundary::create_boundary,
            vault_tauri::commands::agent::list_agents,
            vault_tauri::commands::agent::revoke_agent,
            vault_tauri::commands::settings::get_settings_info,
            vault_tauri::commands::engine::ensure_recall_engine,
            vault_tauri::commands::engine::recall_engine_state,
            vault_tauri::commands::engine::warm_recall_engine,
            vault_tauri::commands::maintenance::ensure_maintenance_engine,
            vault_tauri::commands::maintenance::get_maintenance_schedule,
            vault_tauri::commands::maintenance::set_maintenance_schedule,
            vault_tauri::commands::maintenance::run_maintenance_now,
            vault_tauri::commands::erasure::erase_everything,
            vault_tauri::commands::logs::export_logs,
        ])
        .setup(|app| {
            // 0. File logging FIRST, so every later step in this closure --
            //    including the fatal-dialog paths below -- is recorded. A
            //    startup failure is exactly the case a beta tester cannot
            //    describe and we cannot reproduce (ADR-SEC-014).
            //
            //    Non-fatal by design: an app that refuses to start because it
            //    could not open a log file has turned a diagnostic aid into an
            //    outage. On failure we carry on with no log rather than block
            //    the user from their own memories.
            //
            //    The directory is kept: the maintenance runner is handed it so
            //    a background run writes to the same file (ADR-SEC-015), and
            //    `export_logs` reads it back (ADR-SEC-017).
            let log_dir = match app.path().app_log_dir() {
                Ok(dir) => {
                    match vault_app::logging::init(&dir) {
                        Ok(path) => {
                            tracing::info!(
                                target: "vault_tauri::startup",
                                log_file = %path.display(),
                                "file logging started"
                            );
                        }
                        Err(e) => eprintln!("zaaheen: could not start file logging: {e}"),
                    }
                    dir
                }
                Err(e) => {
                    eprintln!("zaaheen: could not locate a log directory: {e}");
                    // Carry on with a path that simply holds no logs. Export
                    // then reports "nothing to send", which is honest, rather
                    // than the app refusing to start over a log directory.
                    PathBuf::new()
                }
            };

            // 1. Resolve libonnxruntime dylib path per ADR-019.
            let ort_lib_path = match resolve_ort_lib_path(app.handle()) {
                Ok(p) => p,
                Err(e) => {
                    show_fatal_dialog_and_exit(
                        app.handle(),
                        "Zaaheen — Resource Resolution Failed",
                        &format!(
                            "Could not locate libonnxruntime dylib.\n\n\
                             Details: {e}\n\n\
                             For dev runs, set VAULT_ORT_LIB_PATH to the path of \
                             the dylib (e.g. crates/vault-embedding/test-fixtures/\
                             bge-small-en-v1.5/libonnxruntime.{{dll,dylib,so}}).\n\
                             For installed builds, reinstall to recover."
                        ),
                        EXIT_STARTUP_FAILURE,
                    );
                }
            };

            // 2. Resolve bundled model + tokenizer paths per ADR-019/020.
            //    Phase 4b HIGH fix: ?-propagation → fatal-dialog routing for
            //    UX consistency with the surrounding setup() failure paths.
            let model_path = match resolve_model_path(app.handle()) {
                Ok(p) => p,
                Err(e) => show_fatal_dialog_and_exit(
                    app.handle(),
                    "Zaaheen — Model Resource Resolution Failed",
                    &format!(
                        "Could not locate model.onnx.\n\nDetails: {e}\n\n\
                         For dev runs, set VAULT_MODEL_PATH. For installed builds, reinstall."
                    ),
                    EXIT_STARTUP_FAILURE,
                ),
            };
            let tokenizer_path = match resolve_tokenizer_path(app.handle()) {
                Ok(p) => p,
                Err(e) => show_fatal_dialog_and_exit(
                    app.handle(),
                    "Zaaheen — Tokenizer Resource Resolution Failed",
                    &format!(
                        "Could not locate tokenizer.json.\n\nDetails: {e}\n\n\
                         For dev runs, set VAULT_TOKENIZER_PATH. For installed builds, reinstall."
                    ),
                    EXIT_STARTUP_FAILURE,
                ),
            };

            // 3. Per-user data directory.
            let data_dir = match app.path().app_data_dir() {
                Ok(p) => p,
                Err(e) => show_fatal_dialog_and_exit(
                    app.handle(),
                    "Zaaheen — Data Directory Unavailable",
                    &format!("Could not locate per-user data directory.\n\nDetails: {e}"),
                    EXIT_STARTUP_FAILURE,
                ),
            };
            if let Err(e) = std::fs::create_dir_all(&data_dir) {
                show_fatal_dialog_and_exit(
                    app.handle(),
                    "Zaaheen — Data Directory Creation Failed",
                    &format!(
                        "Could not create per-user data directory at {}.\n\nDetails: {e}",
                        data_dir.display()
                    ),
                    EXIT_STARTUP_FAILURE,
                );
            }
            let metadata_path = data_dir.join("vault.db");
            let vector_dir = data_dir.join("lance");
            let graph_path = data_dir.join("graph.duckdb");

            // 4. Source master_key per ADR-040 + ADR-041. The bridge
            //    composes ADR-040's keychain logic with the V0.1 → V0.2
            //    SQLCipher passphrase bridge (ADR-041 plan iteration 2):
            //    - Keychain entry present → return existing master_key
            //      (V0.2 second-launch path; identical to read_or_init).
            //    - Keychain absent + no V0.1 vault.db → fresh-init via
            //      read_or_init's first-run path.
            //    - Keychain absent + V0.1 vault.db present → V0.1 bridge:
            //      verify VAULT_KEY env var unlocks vault.db → generate
            //      new master_key → keychain write FIRST → snapshot
            //      vault.db → PRAGMA rekey to new keychain-derived
            //      passphrase → close+reopen+verify (post-write
            //      verification invariant per ADR-041 §10) → cleanup
            //      snapshot. Fail-closed with rollback at any step.
            //
            //    This step needs `data_dir` (to detect V0.1 vault.db)
            //    so it runs AFTER step 3 (data_dir resolution), unlike
            //    pre-ADR-041 ordering where keychain was step 1. Step
            //    renumbering 1-4 reflects the new ordering.
            //
            //    The master_key is then split into two domain-separated
            //    BLAKE3 subkeys per ADR-040 amendment option β:
            //    - `sqlcipher_passphrase` (hex-encoded → SqlCipherKey)
            //    - `at_rest_key` (32 bytes → AppConfig.at_rest_key,
            //      consumed by Application::new's
            //      StorageBackend::open_with_at_rest_key per Phase 2).
            //
            //    Replaces the V0.1 VAULT_KEY env var path (ADR-032
            //    retired in Phase 1; ADR-041 bridges existing V0.1
            //    vaults forward).
            //    VAULT_KEY env var is read here (once, at the call site)
            //    rather than inside the bridge so the bridge stays a pure
            //    fn of its inputs — avoids hidden env dependency + keeps
            //    tests pure. Empty string treated as unset (matches the
            //    bridge's Some-with-non-empty-content discipline).
            let vault_key_env = std::env::var("VAULT_KEY").ok();
            let v0_1_vault_key = vault_key_env.as_deref().filter(|s| !s.is_empty());
            // Under the key-creation lock: an AI app may have asked for a
            // keeper (ADR-102) that is creating the key at this very moment
            // on a fresh install; see `with_key_init_lock`.
            let master_key = match with_key_init_lock(&data_dir, || {
                bridge_or_init_master_key(&data_dir, PRODUCTION_NAMESPACE, VAULT_ID, v0_1_vault_key)
            }) {
                Ok(k) => k,
                Err(err) => {
                    show_fatal_dialog_and_exit(
                        app.handle(),
                        "Zaaheen — Keychain Access Failed",
                        &format_keychain_error_dialog(&err),
                        EXIT_CONFIG_ERROR,
                    );
                }
            };
            let key = derive_sqlcipher_passphrase(&master_key);
            let at_rest_key = derive_at_rest_key(&master_key);
            // master_key drops here — the two derived subkeys carry the
            // keying material forward; Zeroizing wipes the master_key
            // bytes on Drop per BRD §11.5.3.
            drop(master_key);

            // 5. Build AppConfig from resolved paths + the derived subkeys.
            //
            // T0.2.7 Phase 4 (2026-05-20): qwen_model_path is None for
            // the V0.1 Tauri UI — the read-pipeline + Qwen-7B path is
            // not yet wired into the Tauri shell (UI-only, no MCP
            // server bound per ADR-034). A future Tauri wiring task
            // will read the resolved model path and pass `Some(...)`
            // when the UI surfaces a read-tool affordance.
            //
            // T0.3.x Batch A (2026-05-26): phi4_model_path is also None
            // here — V0.1 Tauri shell does not host the consolidator.
            // The vault-cli `consolidate run` subcommand is the V0.2
            // entry point for nightly merge work; T0.2.6 will land
            // in-process scheduling that may re-evaluate this default.
            // ADR-087: resolve the reranker before building AppConfig. Never
            // fatal — absent files degrade ranking, they do not block startup.
            //
            // ADR-089: these are passed unconditionally, EVEN IF THE FILES ARE
            // NOT THERE YET, and that is deliberate. `LazyQwen3Reranker` binds
            // paths at construction but opens the model on first use, so a
            // download that lands after startup is picked up by the next query
            // with no relaunch and no re-init of `Application`. Gating on
            // `exists()` here would have frozen the decision at startup and
            // made a first-run download take effect only on the SECOND launch.
            //
            // This is only safe because an absent model now degrades instead
            // of failing (ADR-089): until the bytes arrive, search and read
            // return the retriever's own order rather than erroring.
            let (rerank_model, rerank_tokenizer) = resolve_reranker_paths(&data_dir);
            tracing::info!(
                model = %rerank_model.display(),
                present = rerank_model.exists() && rerank_tokenizer.exists(),
                "reranker paths bound (loaded lazily on first query)"
            );

            // ADR-092/093: assemble the context the automatic-maintenance
            // commands need to invoke the bundled `vault-cli consolidate run`
            // against THIS app's own vault. Built from clones before the paths
            // are moved into `AppConfig` below, and managed after `Application`.
            let models_dir = data_dir.join("models");
            let maintenance_ctx = vault_tauri::commands::maintenance::MaintenanceContext {
                vault_cli: resolve_vault_cli_path(),
                // ADR-SEC-015: the windowless runner both entry points go
                // through. Pointing the schedule at `vault-cli` is what showed
                // a console window on the founder's desktop at login.
                vault_maintenance: resolve_vault_maintenance_path(),
                log_dir: log_dir.clone(),
                vault_db: metadata_path.clone(),
                vector_dir: vector_dir.clone(),
                graph_db: graph_path.clone(),
                bge_model: model_path.clone(),
                bge_tokenizer: tokenizer_path.clone(),
                ort_lib: ort_lib_path.clone(),
                phi4_model: model_fetch::phi4_path_in(&models_dir),
                config_path: data_dir.join("maintenance.json"),
            };

            let config = AppConfig {
                metadata_path,
                vector_dir,
                graph_path,
                key,
                model_path,
                tokenizer_path,
                ort_lib_path,
                at_rest_key,
                qwen_model_path: None,
                phi4_model_path: None,
                // ADR-087 wired the reranker; ADR-089 makes the binding
                // unconditional so a first-run download takes effect in the
                // session that fetched it. Absent files degrade at query time.
                rerank_model_path: Some(rerank_model),
                rerank_tokenizer_path: Some(rerank_tokenizer),
            };

            // 6. Construct Application and spawn the cascading retry
            //    worker. Per ADR-034 (T0.1.11 Phase 5b): V0.1 vault-tauri
            //    is UI-only — no MCP server bound inside the Tauri
            //    process. `start_with_mcp` would call rmcp's
            //    `ServiceExt::serve(server, stdio()).await` which blocks
            //    on JSON-RPC `initialize` from a peer that doesn't exist
            //    when launched as a Tauri UI app, hanging Tauri's setup()
            //    hook indefinitely. `spawn_retry_worker()` spawns only the retry
            //    worker (no rmcp transport bind), keeping the UI
            //    responsive. AI-client MCP integration deferred to V0.2
            //    alpha-distribution task (subcommand-split design per
            //    ADR-034 cross-link).
            //
            //    `spawn_retry_worker()` is sync but spawns
            //    `tokio::spawn(worker.run)` which requires a tokio runtime in
            //    scope. Tauri provides one inside
            //    `tauri::async_runtime::block_on`, which we enter just to
            //    construct Application::new (async) and spawn the worker
            //    within the runtime context.
            let app_handle = app.handle().clone();
            let (application, _shutdown_sender) = tauri::async_runtime::block_on(async move {
                let application = match Application::new(&config).await {
                    Ok(a) => a,
                    Err(e) => show_fatal_dialog_and_exit(
                        &app_handle,
                        "Zaaheen — Fatal Error",
                        &format_startup_failure_dialog(&e),
                        EXIT_STARTUP_FAILURE,
                    ),
                };

                let shutdown_sender = application.spawn_retry_worker();

                // ADR-090: warm the ranking model NOW, in the background.
                //
                // `spawn_retry_worker()` starts the worker and nothing else.
                // The warm-up lives in `start_with_mcp` (step 3b), which a GUI
                // can never call because it blocks on an MCP handshake that
                // never arrives (ADR-034). So the desktop app had NO warm-up at
                // all and paid the full ~24 s model load on the user's FIRST
                // search — measured live 2026-07-22.
                //
                // ADR-095: this method was named `start()` and documented as a
                // "TEST-focused entry point" until 2026-07-25, which is what
                // made the gap above easy to miss for so long — the desktop
                // app's real production lifecycle ran through a method our own
                // docs said was for tests. Renamed to describe what it does.
                //
                // Must stay INSIDE this `block_on`: `spawn_reranker_warmup`
                // calls `tokio::spawn`, which panics outside a runtime
                // context. Same reason `spawn_retry_worker()` is called here.
                //
                // Fire-and-forget by design, and deliberately NOT a gate on
                // the window: founder decision 2026-07-22 chose "open
                // instantly and say honestly that the engine is still getting
                // ready" over blocking every launch for ~40 s. ADR-089's
                // degrade keeps search working throughout.
                //
                // A no-op when the first-run download has not landed (fails
                // fast with ModelUnavailable, leaving the cell cold); the
                // frontend calls `warm_recall_engine` again once acquisition
                // completes, which is what covers a genuine first run.
                let warming = application.spawn_reranker_warmup();
                tracing::info!(
                    warming,
                    state = application.reranker_state().as_wire_str(),
                    "ranking model warm-up requested at startup (ADR-090)"
                );

                (application, shutdown_sender)
            });

            // 7. Manage Application (for Tauri commands) + the worker
            //    shutdown Sender (held to keep the watch channel alive
            //    for the worker's lifetime; dropping it signals worker
            //    exit via the watch::changed() Err arm — which is fine
            //    on Tauri close, but holding it explicitly is the
            //    deliberate lifecycle).
            app.manage(application);
            app.manage(_shutdown_sender);

            // 8. First-run acquisition state (ADR-089). Bound to the same
            //    models directory `resolve_reranker_paths` resolves against,
            //    so what the download writes is exactly what the reranker
            //    later opens. Managed rather than created per-call so
            //    concurrent callers share one transfer.
            app.manage(vault_tauri::commands::engine::RecallEngineFetch::new(
                data_dir.join("models"),
            ));

            // 9. Automatic-maintenance state (ADR-092/093): the resolved
            //    context for building the `vault-cli` invocation, plus the
            //    first-run Phi-4 download deduper (bound to the same models
            //    dir so what onboarding fetches is what a run later loads).
            app.manage(vault_tauri::commands::logs::LogContext {
                log_dir: log_dir.clone(),
            });
            // ADR-SEC-015 amendment 1: an already-registered task still points
            // at whatever the build that registered it chose. Replacing the
            // files on disk does not change what Windows recorded, so an
            // upgrading user would keep the console window indefinitely. Refresh
            // it here, before the context is moved into managed state.
            //
            // Synchronous on the setup thread by choice: it is one `schtasks`
            // call, it runs before the window is shown, and doing it here means
            // it cannot race a user who opens the Maintenance tab. Never fatal —
            // a task we could not refresh is a console window at worst.
            vault_tauri::commands::maintenance::heal_registered_task(&maintenance_ctx);

            app.manage(maintenance_ctx);
            app.manage(vault_tauri::commands::maintenance::MaintenanceEngineFetch::new(models_dir));

            Ok(())
        });

    // Phase 4b HIGH fix: tauri::Builder::run().expect(...) → match Result.
    // Tauri Builder failure means the dialog plugin may not be available,
    // so we use eprintln + exit (degraded path) rather than the dialog
    // routing the rest of setup() uses.
    if let Err(e) = builder.run(tauri::generate_context!()) {
        eprintln!("Zaaheen failed to start: {e}");
        std::process::exit(EXIT_STARTUP_FAILURE);
    }
}

/// Render a fatal dialog and terminate the process. **Diverges** —
/// the function never returns to the caller (`-> !`).
fn show_fatal_dialog_and_exit(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    exit_code: i32,
) -> ! {
    app.dialog().message(body).title(title).blocking_show();
    std::process::exit(exit_code);
}

/// Resolve libonnxruntime dylib path per ADR-019. Dev-mode override via
/// `VAULT_ORT_LIB_PATH` env var (testable via `env_override_for`);
/// production falls through to `app.path().resolve(BaseDirectory::Resource)`.
fn resolve_ort_lib_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = env_override_for("VAULT_ORT_LIB_PATH") {
        return Ok(p);
    }
    let filename =
        dylib_filename_for_os(std::env::consts::OS).map_err(|e| format!("OS dispatch: {e}"))?;
    app.path()
        .resolve(filename, tauri::path::BaseDirectory::Resource)
        .map_err(|e| format!("resolve {filename}: {e}"))
}

/// Resolve bundled model.onnx path. Dev-mode override via
/// `VAULT_MODEL_PATH` env var; production falls through to
/// `app.path().resolve("models/model.onnx", BaseDirectory::Resource)`.
fn resolve_model_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = env_override_for("VAULT_MODEL_PATH") {
        return Ok(p);
    }
    app.path()
        .resolve("models/model.onnx", tauri::path::BaseDirectory::Resource)
        .map_err(|e| format!("resolve model.onnx: {e}"))
}

/// Resolve the Qwen3 reranker's model + tokenizer paths, returning `None` when
/// the files are not (yet) on disk (ADR-087).
///
/// **Why this exists at all.** Until now `main.rs` hardcoded
/// `rerank_model_path: None`, justified by a comment reading *"V0.1 Tauri shell
/// is UI-only (ADR-034, no MCP server bound): no read pipeline, so no
/// reranker."* That was true when written and stopped being true the moment UI
/// slice 1 wired a search box to the real retrieval pipeline. The stale `None`
/// silently put the desktop app on the cosine relevance gate — the very
/// mechanism the reranker was adopted to replace (ADR-059) — so search quality
/// in the GUI was quietly worse than over MCP.
///
/// **Why absent files yield `None` rather than an error.** `Application::new`
/// treats `None` as documented graceful degradation. A missing reranker must
/// leave the vault fully usable — recall is sacrosanct, and refusing to start
/// because an optional quality component is absent would be a far worse
/// failure than ranking with cosine.
///
/// **Why this does NOT download.** Fetching 1.15 GB unannounced during startup
/// would freeze first launch behind a silent transfer. The download belongs
/// behind the first-run progress UI; [`model_fetch::ensure_reranker`] is the
/// transport that flow will call. Until then this resolves what is already
/// present — which covers dev fixtures via the env overrides, and any install
/// where the files have been fetched or placed by hand.
fn resolve_reranker_paths(data_dir: &std::path::Path) -> (PathBuf, PathBuf) {
    // Dev override: both must be set together. A half-configured pair is
    // treated as unset rather than silently pairing an override with a
    // default — mismatched model/tokenizer is the insidious failure class
    // called out in `vault_embedding::integrity`.
    if let (Some(model), Some(tokenizer)) = (
        env_override_for("VAULT_RERANK_MODEL_PATH"),
        env_override_for("VAULT_RERANK_TOKENIZER_PATH"),
    ) {
        return (model, tokenizer);
    }

    let paths = model_fetch::reranker_paths_in(&data_dir.join("models"));
    (paths.model, paths.tokenizer)
}

/// Resolve the bundled `vault-cli` executable that automatic maintenance runs
/// (ADR-091 ships it into the install dir; ADR-093 spawns it for consolidation).
///
/// Dev-mode override via `VAULT_CLI_PATH`. Otherwise it sits beside the running
/// executable — the install directory on an installed build, `target/<profile>`
/// during development. Falls back to the bare name (resolved via PATH, which the
/// installer also puts the install dir on) if the current-exe lookup fails.
fn resolve_vault_cli_path() -> PathBuf {
    resolve_sibling_binary("VAULT_CLI_PATH", "zaaheen")
}

/// Resolve the bundled `vault-maintenance` runner (ADR-SEC-015).
///
/// The windowless binary the OS task and "Run now" both invoke. Dev-mode
/// override via `VAULT_MAINTENANCE_PATH`.
fn resolve_vault_maintenance_path() -> PathBuf {
    resolve_sibling_binary("VAULT_MAINTENANCE_PATH", "zaaheen-maintenance")
}

/// Resolve a bundled executable that ships beside this one.
///
/// Honours an env override first (dev trees keep the binaries elsewhere), then
/// falls back to a sibling of our own executable — which is where the
/// installer puts them, and which cannot be redirected by the user's `PATH`.
fn resolve_sibling_binary(env_var: &str, stem: &str) -> PathBuf {
    if let Some(p) = env_override_for(env_var) {
        return p;
    }
    let exe_name = if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&exe_name)))
        .unwrap_or_else(|| PathBuf::from(exe_name))
}

/// Resolve bundled tokenizer.json path. Dev-mode override via
/// `VAULT_TOKENIZER_PATH` env var; production falls through to
/// `app.path().resolve("models/tokenizer.json", BaseDirectory::Resource)`.
fn resolve_tokenizer_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = env_override_for("VAULT_TOKENIZER_PATH") {
        return Ok(p);
    }
    app.path()
        .resolve(
            "models/tokenizer.json",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|e| format!("resolve tokenizer.json: {e}"))
}
