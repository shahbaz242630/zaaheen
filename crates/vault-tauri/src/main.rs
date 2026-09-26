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
//!   (2026-05-09)** per ADR-040 + ADR-040 amendment: master_key sourced
//!   from Windows Credential Manager. **Since ADR-108 (session 60) the
//!   desktop never opens or creates the key at all**: the keeper does, and
//!   the V0.1 bridge is retired (D11). The desktop reads the key read-only,
//!   only to authenticate its admin connection.
//! - **ADR-108 (D4):** the desktop is a client of the keeper. It builds no
//!   `Application`, loads no model and holds no store; every command that
//!   needs the vault forwards over `link::KeeperLink`.
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
use std::sync::Arc;

use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use vault_app::keychain::KeyLocation;
use vault_app::location::missing::{self, Problem};
use vault_app::location::moving::{MoveOutcome, MoveProgress};
use vault_app::location::Homes;
use vault_core::VaultError;
use vault_tauri::commands::startup::Startup;
use vault_tauri::link::KeeperLink;
use vault_tauri::{
    dylib_filename_for_os, env_override_for, format_keychain_error_dialog,
    format_location_problem_dialog, format_start_again_refusal, model_fetch, CANCEL_BUTTON,
    CLOSE_BUTTON, KEY_ERROR_DIALOG_TITLE, START_AGAIN_BUTTON, START_AGAIN_CONFIRMATION,
    START_AGAIN_TITLE,
};

/// Exit code for startup failures.
const EXIT_STARTUP_FAILURE: i32 = 1;

/// How long closing the window waits to close the keeper link cleanly.
const EXIT_DISCONNECT: std::time::Duration = std::time::Duration::from_secs(2);

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
            vault_tauri::commands::agent::list_connected_apps,
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
            // Account (S3 step 4b). Ungated by design -- 8.26 6.4's
            // `account` slot -- and safe to be, because `AccountOps` holds
            // no vault. See `guard::no_account_command_receives_the_vault`.
            vault_tauri::commands::account::account_access,
            vault_tauri::commands::account::account_status,
            vault_tauri::commands::account::account_sign_in,
            vault_tauri::commands::account::account_sign_out,
            vault_tauri::commands::account::account_subscribe,
            vault_tauri::commands::account::account_refresh_now,
            // Download my memories (S4). Ungated: the promise it keeps is
            // that it works when everything else is refused.
            vault_tauri::commands::export::export_memories,
            // Where the memories live (ADR-105 L-e). Gated: see the module.
            vault_tauri::commands::location::location_status,
            vault_tauri::commands::location::location_check,
            vault_tauri::commands::location::location_move,
            vault_tauri::commands::location::location_forget_old_copy,
            // "Connect it for me" (ADR-106). Gated.
            vault_tauri::commands::connect::connect_app,
            vault_tauri::commands::connect::show_claude_extension,
            vault_tauri::commands::connect::server_command,
            // Where this start is (ADR-105 amendment 1, L-f). Open before the
            // lock, founder-approved: see the module.
            vault_tauri::commands::startup::startup_state,
            // Where the link to the keeper is (ADR-108 D6). Open before the
            // lock, approved with ADR-108: see the module.
            vault_tauri::commands::keeper::link_state,
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
                        "Zaaheen: resource resolution failed",
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
                    "Zaaheen: model resource resolution failed",
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
                    "Zaaheen: tokenizer resource resolution failed",
                    &format!(
                        "Could not locate tokenizer.json.\n\nDetails: {e}\n\n\
                         For dev runs, set VAULT_TOKENIZER_PATH. For installed builds, reinstall."
                    ),
                    EXIT_STARTUP_FAILURE,
                ),
            };

            // 3. The vault folder (ADR-105): the recorded location, set up
            //    once on this build's first run under the key lock — an
            //    existing install keeps its folder (Tauri's app_data_dir is
            //    one of the places looked at), a new install gets the local,
            //    never-roaming folder. Nothing here creates a folder for a
            //    location that is recorded but missing: that is how a second,
            //    empty vault would get made on an unplugged drive.
            let (homes, key_location) = match (
                vault_app::location::Homes::production(),
                KeyLocation::production(),
            ) {
                (Ok(homes), Ok(key)) => (homes, key),
                (Err(err), _) | (_, Err(err)) => {
                    tracing::error!(error = %err, "the app's folders could not be found");
                    show_fatal_dialog_and_exit(
                        app.handle(),
                        KEY_ERROR_DIALOG_TITLE,
                        &format_keychain_error_dialog(&err),
                        EXIT_STARTUP_FAILURE,
                    )
                }
            };
            // 3a. A move the person asked for (ADR-105 L5) is finished before
            //     anything opens the vault or the key, on both paths below.
            //     When one is waiting, the window comes first (ADR-105
            //     amendment 1, L-f): this returns so the page can draw
            //     "Moving your memories", and one thread makes the move and
            //     then the rest of this start, in the same order. The page
            //     asks nothing but `startup_state` until that thread says
            //     the start is ready.
            let start = Start {
                log_dir,
                ort_lib_path,
                model_path,
                tokenizer_path,
                homes,
                key_location,
            };
            if let Some(to) = vault_app::location::moving::waiting_move(&start.homes) {
                let progress = Arc::new(MoveProgress::default());
                let startup = Startup::moving(Arc::clone(&progress), to);
                app.manage(startup.clone());
                let (handle, on_thread, reporting, told) = (
                    app.handle().clone(),
                    start.clone(),
                    Arc::clone(&progress),
                    startup.clone(),
                );
                let spawned = std::thread::Builder::new()
                    .name(MOVE_THREAD.to_owned())
                    .spawn(move || {
                        // A panic here would leave the page waiting on a
                        // start that never ends (release builds abort; this
                        // is for the rest): end the app as a failed setup
                        // would. The move itself is crash-safe (L-d).
                        let run = std::panic::AssertUnwindSafe(|| {
                            move_then_open(&handle, on_thread, &reporting, &told);
                        });
                        if std::panic::catch_unwind(run).is_err() {
                            tracing::error!("the start stopped unexpectedly");
                            std::process::exit(EXIT_STARTUP_FAILURE);
                        }
                    });
                if let Err(e) = spawned {
                    // No thread: the same work here, as before L-f, under a
                    // window that draws once it is done.
                    tracing::warn!(error = %e, "the move could not run beside the window; it runs first");
                    move_then_open(app.handle(), start, &progress, &startup);
                }
                return Ok(());
            }

            // No move waiting: everything on this thread before the window
            // draws, exactly as before L-f (an old copy an earlier move left
            // is removed here too).
            let startup = Startup::opening();
            app.manage(startup.clone());
            let moved = finish_the_move(app.handle(), &start, &Arc::new(MoveProgress::default()));
            open_the_vault(app.handle(), start, moved, &startup);

            Ok(())
        });

    // Phase 4b HIGH fix: tauri::Builder::run().expect(...) → match Result.
    // Tauri Builder failure means the dialog plugin may not be available,
    // so we use eprintln + exit (degraded path) rather than the dialog
    // routing the rest of setup() uses.
    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("Zaaheen failed to start: {e}");
            std::process::exit(EXIT_STARTUP_FAILURE);
        }
    };
    // ADR-108 D6: the desktop holds no store, lock or discovery file, so
    // nothing needs draining on exit; the keeper link is closed cleanly so the
    // keeper sees the session end rather than a broken pipe. Bounded.
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(link) = handle.try_state::<KeeperLink>() {
                let _ = tauri::async_runtime::block_on(tokio::time::timeout(
                    EXIT_DISCONNECT,
                    link.disconnect(),
                ));
            }
        }
    });
}

/// What the start has resolved before the memories open (steps 0-3), handed
/// to whichever thread opens them (ADR-105 amendment 1, L-f).
#[derive(Clone)]
struct Start {
    log_dir: PathBuf,
    ort_lib_path: PathBuf,
    model_path: PathBuf,
    tokenizer_path: PathBuf,
    homes: Homes,
    key_location: KeyLocation,
}

/// ADR-105 amendment 1 (L-f): the move, then the rest of the start, on the
/// thread beside the window. The page shows the move's progress until
/// [`open_the_vault`] marks the start ready.
fn move_then_open(
    app: &tauri::AppHandle,
    start: Start,
    progress: &Arc<MoveProgress>,
    startup: &Startup,
) {
    let moved = finish_the_move(app, &start, progress);
    startup.move_finished();
    open_the_vault(app, start, moved, startup);
}

/// ADR-105 L5: a move the person asked for, or the old copy an earlier move
/// left, finished before anything opens the vault or the key. The keeper
/// hands the vault over; a window still closing is waited for. Another
/// window moving or erasing the memories means this one must not open them.
fn finish_the_move(
    app: &tauri::AppHandle,
    start: &Start,
    progress: &Arc<MoveProgress>,
) -> MoveOutcome {
    let moved = tauri::async_runtime::block_on(vault_app::location::moving::run_pending_reporting(
        &start.homes,
        &start.key_location,
        &vault_app::keeper::relay::KeychainKeySource,
        progress,
    ));
    tracing::info!(outcome = ?moved, "the vault's location at startup");
    if moved == MoveOutcome::Busy {
        show_fatal_dialog_and_exit(
            app,
            KEY_ERROR_DIALOG_TITLE,
            vault_tauri::MSG_MEMORIES_BUSY,
            EXIT_STARTUP_FAILURE,
        );
    }
    moved
}

/// Everything after the move (steps 3-9): the vault folder, the guard, the
/// link to the keeper, and every piece of state the commands need, in this
/// order on both paths. Its last act tells the page the start is ready
/// (ADR-105 L-f).
///
/// **ADR-108 (D4): the desktop never opens the vault or creates the key.**
/// The keeper does, when the first command needs it; the window opens without
/// waiting for it, and the home screen says "Opening your memories…" while it
/// starts (`link_state`).
fn open_the_vault(app: &tauri::AppHandle, start: Start, moved: MoveOutcome, startup: &Startup) {
    let Start {
        log_dir,
        ort_lib_path,
        model_path,
        tokenizer_path,
        homes,
        key_location,
    } = start;
    let also_existing: Vec<PathBuf> = app.path().app_data_dir().ok().into_iter().collect();
    let data_dir = match vault_app::location::prepare(&homes, &key_location, &also_existing) {
        Ok(dir) => dir.path().to_path_buf(),
        // ADR-105 L1/L6: not where the record says. The reason picks
        // the words, and a folder that is not there at all may be
        // swapped for a fresh start in the default place.
        Err(err @ VaultError::VaultLocation(_)) => {
            tracing::error!(error = %err, "the vault folder could not be found");
            when_the_memories_are_missing(app, &homes, &key_location, &err)
        }
        Err(err) => {
            tracing::error!(error = %err, "the vault folder could not be found");
            show_fatal_dialog_and_exit(
                app,
                KEY_ERROR_DIALOG_TITLE,
                &format_keychain_error_dialog(&err),
                EXIT_STARTUP_FAILURE,
            )
        }
    };
    // 3b. What this start did about a move, and what the location
    //     screens need to ask for the next one (ADR-105 L-e).
    app.manage(vault_tauri::commands::location::LocationContext::new(
        homes.clone(),
        key_location.clone(),
        moved,
    ));
    // ADR-105 L3: the models never follow the vault.
    let models_dir = vault_app::location::models_dir(&homes);

    // 4. The entitlement guard (SIGNIN-DESIGN.md §8.26 §6.4) and the account
    // the account commands use, from ONE account (ADR-SEC-028): two copies in
    // this process would each keep their own in-memory refresh token, and the
    // one that did not rotate would sign the person out.
    //
    // Built BEFORE the link to the keeper (ADR-108 D6, reviews A-B1 / B-B1):
    // on a fresh install nobody is signed in and there is no key yet, and the
    // desktop must reach its sign-in screen without any keeper at all.
    //
    // The decision itself lives in `guard::build`, not here, so that
    // `Entitlement`'s constructors can stay private. A build with no account
    // settings, or whose account could not be prepared, gets an empty slot:
    // the account commands then answer a stable code rather than the app
    // refusing to start -- a desktop that will not start is a desktop whose
    // export nobody can reach.
    let (entitlement, account) = vault_tauri::guard::build();

    // 4b. The lease refresh at desktop open (only when stale) and then daily
    // (§8.26 §4). Holds the account, never the vault.
    if let Some(ops) = account.for_background_refresh() {
        tauri::async_runtime::spawn(ops.refresh_at_open_then_daily());
    }
    app.manage(entitlement);
    app.manage(account);

    // 5. The link to the keeper (ADR-108 D5). The desktop never opens the
    // vault and never creates the key: the keeper does, when the first
    // command that needs it runs. Nothing here waits for a keeper, so the
    // window opens at once. Built inside the runtime: its idle reaper is a
    // task.
    let link_homes = homes.clone();
    let link = tauri::async_runtime::block_on(async move { KeeperLink::new(link_homes) });
    app.manage(link);

    // 6. Automatic-maintenance context (ADR-092/093): what the windowless
    // runner is started with. Paths only: the runner opens the vault itself.
    let maintenance_ctx = vault_tauri::commands::maintenance::MaintenanceContext {
        vault_cli: resolve_vault_cli_path(),
        // ADR-SEC-015: the windowless runner both entry points go
        // through. Pointing the schedule at `vault-cli` is what showed
        // a console window on the founder's desktop at login.
        vault_maintenance: resolve_vault_maintenance_path(),
        log_dir: log_dir.clone(),
        bge_model: model_path,
        bge_tokenizer: tokenizer_path,
        ort_lib: ort_lib_path,
        phi4_model: model_fetch::phi4_path_in(&models_dir),
        config_path: data_dir.join("maintenance.json"),
    };

    // 7. Automatic-maintenance state (ADR-092/093): the resolved
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
    // Synchronous by choice: it is one `schtasks` call, it runs before the
    // page can ask for anything (on the setup thread, or on the move's
    // thread before the start says ready, L-f), and doing it here means it
    // cannot race a user who opens the Maintenance tab. Never fatal — a
    // task we could not refresh is a console window at worst.
    vault_tauri::commands::maintenance::heal_registered_task(&maintenance_ctx);

    app.manage(maintenance_ctx);
    app.manage(vault_tauri::commands::maintenance::MaintenanceEngineFetch::new(models_dir));

    // Last: every piece of state is managed, so the page may go on (L-f).
    startup.ready();
}

/// ADR-105 L1/L6: the memories are not where the record says. The reason
/// picks the message. Only a folder that is not there at all is offered
/// "Start again in the default place", and only once the person confirms
/// that the memories on that drive can't be opened from here. Returns the
/// new vault folder, or does not return.
fn when_the_memories_are_missing(
    app: &tauri::AppHandle,
    homes: &Homes,
    key: &KeyLocation,
    err: &VaultError,
) -> PathBuf {
    let problem = missing::diagnose(homes);
    let Some(absent @ Problem::FolderAbsent(_)) = &problem else {
        let body = problem.as_ref().map_or_else(
            || format_keychain_error_dialog(err),
            format_location_problem_dialog,
        );
        show_fatal_dialog_and_exit(app, KEY_ERROR_DIALOG_TITLE, &body, EXIT_STARTUP_FAILURE);
    };
    let wants_to = startup_dialog(app, format_location_problem_dialog(absent))
        .title(KEY_ERROR_DIALOG_TITLE)
        .buttons(MessageDialogButtons::OkCancelCustom(
            START_AGAIN_BUTTON.to_owned(),
            CLOSE_BUTTON.to_owned(),
        ))
        .blocking_show();
    if !wants_to {
        std::process::exit(EXIT_STARTUP_FAILURE);
    }
    let confirmed = startup_dialog(app, START_AGAIN_CONFIRMATION)
        .title(START_AGAIN_TITLE)
        .buttons(MessageDialogButtons::OkCancelCustom(
            START_AGAIN_BUTTON.to_owned(),
            CANCEL_BUTTON.to_owned(),
        ))
        .blocking_show();
    if !confirmed {
        std::process::exit(EXIT_STARTUP_FAILURE);
    }
    match missing::start_again(homes, key) {
        Ok(dir) => dir.path().to_path_buf(),
        Err(refusal) => {
            tracing::warn!(?refusal, "starting again did not happen");
            show_fatal_dialog_and_exit(
                app,
                KEY_ERROR_DIALOG_TITLE,
                &format_start_again_refusal(refusal),
                EXIT_STARTUP_FAILURE,
            )
        }
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
    startup_dialog(app, body).title(title).blocking_show();
    std::process::exit(exit_code);
}

/// The name of the thread that makes a move beside the window (L-f).
const MOVE_THREAD: &str = "zaaheen-move";

/// A startup dialog. From the move's thread (L-f) it is set in front of the
/// app's window, which is drawn and focused by then and could otherwise hide
/// it. Never from the setup thread: there the window's own thread is the one
/// waiting for the answer, and a dialog owned by that window would wait on
/// it in turn.
fn startup_dialog(
    app: &tauri::AppHandle,
    body: impl Into<String>,
) -> tauri_plugin_dialog::MessageDialogBuilder<tauri::Wry> {
    let dialog = app.dialog().message(body);
    let on_move_thread = std::thread::current().name() == Some(MOVE_THREAD);
    match app.get_webview_window("main") {
        Some(window) if on_move_thread => dialog.parent(&window),
        _ => dialog,
    }
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
