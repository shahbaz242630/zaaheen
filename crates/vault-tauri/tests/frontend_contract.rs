//! Frontend↔backend contract guards for the desktop UI.
//!
//! The UI in `dist/` has no automated behaviour coverage — it is vanilla JS
//! with no toolchain (deliberate: no node build, CSP `default-src 'self'`).
//! These tests close the two gaps that actually bit us, using nothing but
//! string analysis of the checked-in sources, so they need no JS runtime, no
//! DOM, and no new dependencies.
//!
//! ## Why these specific guards (UI slice 2, 2026-07-20)
//!
//! **1. The dialog trap.** `forget` (permanent delete) and `revoke` gated
//! themselves on `window.confirm`, which this webview renders as an OK-only
//! message box: the user cannot decline, so the destructive action ran
//! whatever they clicked. Found by hand during founder live-verification —
//! and note that a DOM-simulation test would NOT have caught it, because a
//! simulated browser implements `confirm` correctly. A source-level ban is
//! the honest guard for this class of bug.
//!
//! **2. Command wiring.** Registering a Tauri command touches FOUR files, and
//! getting any one wrong fails late and cryptically. Both build failures in
//! this session were exactly this:
//!   - `permissions/default.toml` missing an `allow-*` definition →
//!     *"Permission allow-X not found, expected one of …"* from the build
//!     script, nowhere near the code at fault.
//!   - `generate_handler!` referencing a re-exported path instead of the
//!     defining module → 20 macro-resolution errors, including for commands
//!     that previously worked.
//!
//! These tests fail fast, in the crate under test, naming the exact command
//! and the exact file that needs the entry.

/// The frontend script that calls into the backend.
const APP_JS: &str = include_str!("../dist/app.js");

/// Command modules, in `generate_handler!` registration order.
const COMMAND_SOURCES: &[(&str, &str)] = &[
    ("memory.rs", include_str!("../src/commands/memory.rs")),
    ("boundary.rs", include_str!("../src/commands/boundary.rs")),
    ("agent.rs", include_str!("../src/commands/agent.rs")),
    ("settings.rs", include_str!("../src/commands/settings.rs")),
    ("engine.rs", include_str!("../src/commands/engine.rs")),
    (
        "maintenance.rs",
        include_str!("../src/commands/maintenance.rs"),
    ),
    ("erasure.rs", include_str!("../src/commands/erasure.rs")),
    ("logs.rs", include_str!("../src/commands/logs.rs")),
    ("account.rs", include_str!("../src/commands/account.rs")),
    ("export.rs", include_str!("../src/commands/export.rs")),
    ("location.rs", include_str!("../src/commands/location.rs")),
    ("connect.rs", include_str!("../src/commands/connect.rs")),
    ("startup.rs", include_str!("../src/commands/startup.rs")),
];

/// The command modules' list, to hold [`COMMAND_SOURCES`] to it.
const COMMANDS_MOD_RS: &str = include_str!("../src/commands/mod.rs");

/// The markup, for guards that pin UI structure rather than wiring.
const INDEX_HTML: &str = include_str!("../dist/index.html");

/// The stylesheet, for guards on motion (the welcome's entrance).
const STYLES_CSS: &str = include_str!("../dist/styles.css");

const MAIN_RS: &str = include_str!("../src/main.rs");
const PERMISSIONS_TOML: &str = include_str!("../permissions/default.toml");
const CAPABILITIES_JSON: &str = include_str!("../capabilities/default.json");

// ---------------------------------------------------------------------------
// Tiny extraction helpers (plain `std` — no regex dependency).
// ---------------------------------------------------------------------------

/// Collect every substring that appears between `open` and the next `close`.
fn collect_between(haystack: &str, open: &str, close: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = haystack;
    while let Some(start) = rest.find(open) {
        let after = &rest[start + open.len()..];
        match after.find(close) {
            Some(end) => {
                out.push(after[..end].to_string());
                rest = &after[end..];
            }
            None => break,
        }
    }
    out
}

/// Command names the frontend invokes: `invoke("name"` / `invoke('name'`.
fn frontend_invoked_commands() -> Vec<String> {
    let mut names = collect_between(APP_JS, "invoke(\"", "\"");
    names.extend(collect_between(APP_JS, "invoke('", "'"));
    names.sort();
    names.dedup();
    names
}

/// Command names defined with `#[tauri::command]` across the command modules.
///
/// The attribute is always followed by the `pub async fn <name>(` wrapper, so
/// we take the first `fn ` after each attribute occurrence.
fn defined_commands() -> Vec<String> {
    let mut out = Vec::new();
    for (_file, src) in COMMAND_SOURCES {
        let mut rest = *src;
        while let Some(at) = rest.find("#[tauri::command]") {
            let after = &rest[at + "#[tauri::command]".len()..];
            if let Some(fn_at) = after.find("fn ") {
                let sig = &after[fn_at + 3..];
                let name: String = sig
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.push(name);
                }
            }
            rest = after;
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Command names registered in `main.rs`'s `generate_handler!` block, taken
/// from the `commands::<module>::<name>,` lines.
fn registered_commands() -> Vec<String> {
    let block_start = MAIN_RS
        .find("generate_handler![")
        .expect("main.rs must contain a generate_handler! block");
    let block = &MAIN_RS[block_start..];
    let block_end = block
        .find("])")
        .expect("generate_handler! block must close");
    let block = &block[..block_end];

    let mut out = Vec::new();
    for line in block.lines() {
        let line = line.trim();
        if line.starts_with("//") || !line.contains("commands::") {
            continue;
        }
        // Take the final `::`-separated segment, minus the trailing comma.
        let name = line.trim_end_matches(',').rsplit("::").next().unwrap_or("");
        let name: String = name
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.push(name);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Command names granted by a `commands.allow = ["name"]` entry in
/// `permissions/default.toml`.
fn permitted_commands() -> Vec<String> {
    let mut out = Vec::new();
    for entry in collect_between(PERMISSIONS_TOML, "commands.allow = [", "]") {
        out.extend(collect_between(&entry, "\"", "\""));
    }
    out.sort();
    out.dedup();
    out
}

/// Convert a snake_case command name to its Tauri permission identifier.
fn permission_identifier(command: &str) -> String {
    format!("allow-{}", command.replace('_', "-"))
}

// ---------------------------------------------------------------------------
// Guard 1 — no native dialog may gate an action
// ---------------------------------------------------------------------------

#[test]
fn frontend_never_gates_an_action_on_a_native_dialog() {
    // `confirmAction(` is OUR promise-based in-app dialog and is the intended
    // replacement; strip it before looking for the native call so the check
    // is a plain substring search with no false positive on our own helper.
    let scrubbed = APP_JS.replace("confirmAction(", "");

    for banned in ["confirm(", "prompt("] {
        assert!(
            !scrubbed.contains(banned),
            "dist/app.js uses the native `{banned}` dialog.\n\
             This webview renders it as an OK-only message box: the user \
             CANNOT decline, so the guarded action runs no matter what they \
             click. This shipped in UI slice 1 on `forget` (PERMANENT delete) \
             and was only caught by hand.\n\
             Use `confirmAction({{ title, body, confirmLabel }})` instead — it \
             resolves true only on a real choice."
        );
    }
}

#[test]
fn frontend_confirm_helper_is_present_and_used_by_destructive_actions() {
    // The ban above is only meaningful while the replacement exists and the
    // two destructive surfaces actually route through it.
    assert!(
        APP_JS.contains("function confirmAction("),
        "dist/app.js must define confirmAction() — the native-dialog ban \
         assumes a working in-app replacement exists"
    );
    let uses = APP_JS.matches("await confirmAction(").count();
    assert!(
        uses >= 2,
        "expected both destructive actions (forget, revoke) to await \
         confirmAction(); found {uses} call site(s)"
    );
}

// ---------------------------------------------------------------------------
// Guard 1b — the irreversible action keeps its type-to-confirm gate
// (ADR-SEC-008)
// ---------------------------------------------------------------------------

/// `erase_everything` destroys the master key. It cannot be undone, and it
/// cannot be recovered from a copy of the data folder. A yes/no dialog is
/// too easy to click through for that, so the UI requires the user to type
/// a phrase.
///
/// This guard exists because the gate is pure convention: nothing in the
/// Rust command enforces it, so a UI refactor could quietly reduce the most
/// destructive action in the product to a single click and every other test
/// would still pass.
#[test]
fn erase_everything_is_gated_behind_a_typed_confirmation() {
    assert!(
        APP_JS.contains("ERASE_PHRASE"),
        "ADR-SEC-008: the typed-confirmation constant is gone from dist/app.js. \
         erase_everything permanently destroys the vault key and MUST NOT be \
         reachable from a single click."
    );
    assert!(
        INDEX_HTML.contains("id=\"erase-phrase\""),
        "ADR-SEC-008: the type-to-confirm input is missing from index.html"
    );

    // The invoke must be downstream of the phrase check, not merely present
    // somewhere in the file.
    let guard = APP_JS
        .find("if ($(\"erase-phrase\").value !== ERASE_PHRASE) return;")
        .expect(
            "ADR-SEC-008: eraseEverything() no longer early-returns on a phrase \
             mismatch, so the typed confirmation is decorative",
        );
    let call = APP_JS
        .find("invoke(\"erase_everything\")")
        .expect("dist/app.js must invoke erase_everything");
    assert!(
        guard < call,
        "ADR-SEC-008: the phrase check must run BEFORE invoke(\"erase_everything\"); \
         found the guard at {guard} and the call at {call}"
    );
}

/// The user must be told that uninstalling leaves their memories on disk.
/// Silence here is the actual product problem ADR-SEC-008 set out to fix —
/// a user who uninstalls a privacy product reasonably assumes it took its
/// data with it.
#[test]
fn settings_discloses_that_uninstalling_keeps_memories() {
    let html = INDEX_HTML.to_lowercase();
    assert!(
        html.contains("uninstall"),
        "ADR-SEC-008: the Settings tab no longer tells the user what happens to \
         their memories when they uninstall. Keeping data on uninstall is the \
         right default; keeping it SILENTLY is not."
    );
    assert!(
        INDEX_HTML.contains("id=\"data-location\""),
        "ADR-SEC-008: the vault location is no longer shown, so the disclosure \
         is not checkable by the user"
    );
}

#[test]
fn settings_offers_a_way_to_send_us_the_activity_record() {
    // ADR-SEC-017. Without a visible affordance the log exists but sits in a
    // hidden system folder, and "it broke" from a beta tester yields nothing —
    // which is the state ADR-SEC-014 was written to end.
    assert!(
        INDEX_HTML.contains("id=\"export-logs\""),
        "ADR-SEC-017: the Settings tab no longer offers a log export. The log \
         then only exists somewhere a non-technical tester cannot reach."
    );
    assert!(
        APP_JS.contains("invoke(\"export_logs\""),
        "the export button must actually call the command"
    );
}

#[test]
fn the_export_copy_promises_that_memories_are_not_included() {
    // The user is about to email this file to strangers. If the UI does not
    // say plainly that their memories are not in it, the honest answer for a
    // cautious person is "don't send it" -- and we lose the diagnostic.
    let html = INDEX_HTML.to_lowercase();
    let claim = html
        .find("none of your memories")
        .expect("ADR-SEC-017: the export section must state that memories are not included");
    let button = html
        .find("id=\"export-logs\"")
        .expect("export button must exist");
    assert!(
        claim < button,
        "the promise must be made BEFORE the button, not after it"
    );
}

#[test]
fn the_export_never_names_the_stack() {
    // ADR-086 white-label: user-visible copy describes capability and trust,
    // never the implementation.
    let html = INDEX_HTML.to_lowercase();
    // Deliberately NOT "rust": the white-label guidance asks for trust
    // language, and "trust" contains it. A guard that fires on the wording we
    // want people to use gets deleted rather than obeyed.
    for banned in ["tracing", "log file", "stdout", "subscriber", "tauri"] {
        assert!(
            !html.contains(banned),
            "the Settings copy names an implementation detail: {banned:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Guard 2 — the four-file command wiring must agree
// ---------------------------------------------------------------------------

#[test]
fn every_frontend_invoked_command_is_defined_in_rust() {
    let defined = defined_commands();
    for cmd in frontend_invoked_commands() {
        assert!(
            defined.contains(&cmd),
            "dist/app.js calls invoke(\"{cmd}\") but no `#[tauri::command] \
             fn {cmd}` exists in src/commands/.\nDefined commands: {defined:?}"
        );
    }
}

#[test]
fn every_frontend_invoked_command_is_registered_in_generate_handler() {
    let registered = registered_commands();
    for cmd in frontend_invoked_commands() {
        assert!(
            registered.contains(&cmd),
            "dist/app.js calls invoke(\"{cmd}\") but it is not registered in \
             main.rs's generate_handler!.\nAn unregistered command fails at \
             RUNTIME with a confusing error, not at build time.\nRegistered: \
             {registered:?}"
        );
    }
}

#[test]
fn every_registered_command_has_a_permission_definition() {
    // The failure this prevents: session 21's first build failure, where the
    // capability referenced `allow-list-recent-memories` but no such
    // permission was DEFINED. The build script's error names the permission,
    // not the file that should have declared it.
    let permitted = permitted_commands();
    for cmd in registered_commands() {
        assert!(
            permitted.contains(&cmd),
            "command `{cmd}` is registered in generate_handler! but has no \
             `commands.allow = [\"{cmd}\"]` entry in \
             permissions/default.toml.\nAdding a command is a TWO-file \
             permission change: permissions/default.toml DEFINES the \
             allow-* permission, capabilities/default.json only REFERENCES \
             it.\nPermitted: {permitted:?}"
        );
    }
}

#[test]
fn every_registered_command_is_referenced_by_the_default_capability() {
    // A permission that exists but is not granted to the window means the
    // command is silently unreachable from the UI at runtime.
    for cmd in registered_commands() {
        let identifier = permission_identifier(&cmd);
        assert!(
            CAPABILITIES_JSON.contains(&identifier),
            "command `{cmd}` has no `\"{identifier}\"` entry in \
             capabilities/default.json, so the webview cannot invoke it at \
             runtime even though it compiles."
        );
    }
}

#[test]
fn every_defined_command_is_registered() {
    // A defined-but-unregistered command is dead code that still carries a
    // permission surface. If one is ever intentionally withheld, this test is
    // the right place to record why.
    let registered = registered_commands();
    for cmd in defined_commands() {
        assert!(
            registered.contains(&cmd),
            "`#[tauri::command] fn {cmd}` is defined but never registered in \
             generate_handler! — either register it or delete it"
        );
    }
}

// ---------------------------------------------------------------------------
// Meta — the extractors themselves must not silently return nothing
// ---------------------------------------------------------------------------

#[test]
fn extractors_find_the_expected_command_surface() {
    // Without this, a parsing regression would turn every guard above into a
    // vacuous pass over an empty set — the classic "green because it tested
    // nothing" failure.
    let invoked = frontend_invoked_commands();
    let defined = defined_commands();
    let registered = registered_commands();
    let permitted = permitted_commands();

    assert!(
        invoked.len() >= 6,
        "expected the UI to invoke at least the 6 slice-1/2 commands, \
         found {invoked:?}"
    );
    assert!(
        defined.len() >= 10,
        "expected at least 10 defined commands after slice 2, found {defined:?}"
    );
    assert_eq!(
        defined, registered,
        "defined and registered command sets must match exactly"
    );
    assert_eq!(
        registered, permitted,
        "registered and permitted command sets must match exactly"
    );
}

/// Every command module is read by the guards above. Session 55 added
/// `connect.rs` and `startup.rs` without listing them, so their three
/// commands were invisible to every wiring check here (found by session 56's
/// first build).
#[test]
fn every_command_module_is_read_by_the_guards() {
    let listed: Vec<&str> = COMMAND_SOURCES.iter().map(|(file, _)| *file).collect();
    let modules: Vec<String> = COMMANDS_MOD_RS
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub mod "))
        .filter_map(|rest| rest.strip_suffix(';'))
        .map(|name| format!("{name}.rs"))
        .collect();
    assert!(modules.len() >= 13, "found {modules:?}");
    for module in &modules {
        assert!(
            listed.contains(&module.as_str()),
            "src/commands/{module} is not in COMMAND_SOURCES, so no guard here reads its commands"
        );
    }
}

// ---------------------------------------------------------------------------
// Guard 3 — the progress event name must agree across the language boundary
// ---------------------------------------------------------------------------

/// The event channel is a bare string declared independently in Rust
/// (`engine.rs::PROGRESS_EVENT`) and in JS (`app.js`'s listener). Nothing at
/// compile time connects them: rename one and the download still runs, the
/// UI still loads, and the progress bar simply never moves — a silent
/// failure of exactly the kind the command-wiring guard exists to prevent.
#[test]
fn progress_event_name_matches_between_rust_and_the_frontend() {
    const ENGINE_RS: &str = include_str!("../src/commands/engine.rs");

    let declared = collect_between(ENGINE_RS, "pub const PROGRESS_EVENT: &str = \"", "\"")
        .into_iter()
        .next()
        .expect("engine.rs must declare PROGRESS_EVENT as a string literal");

    assert!(
        APP_JS.contains(&format!("listenEvent(\"{declared}\"")),
        "dist/app.js must listen on the event name Rust emits.\n\
         Rust declares PROGRESS_EVENT = \"{declared}\", but app.js does not \
         listen for it. First-run progress would silently never appear."
    );
}

/// ADR-086 white-label: the event name crosses into frontend source, so it is
/// one rename away from putting a model name in the UI layer.
#[test]
fn progress_event_name_does_not_leak_the_stack() {
    const ENGINE_RS: &str = include_str!("../src/commands/engine.rs");
    let declared = collect_between(ENGINE_RS, "pub const PROGRESS_EVENT: &str = \"", "\"")
        .into_iter()
        .next()
        .expect("engine.rs must declare PROGRESS_EVENT");
    let lowered = declared.to_lowercase();

    for banned in ["qwen", "onnx", "bge", "phi", "gguf", "llama", "rerank"] {
        assert!(
            !lowered.contains(banned),
            "ADR-086: the progress event name must not name the stack \
             ('{banned}'); got \"{declared}\""
        );
    }
}

/// The maintenance-engine (Phi-4) download progress event carries the same
/// silent-failure and white-label risks as the recall one: rename it in Rust
/// and the Maintenance tab's progress line quietly stops moving; name the stack
/// in it and a model name leaks into the UI. Guard both (ADR-092/086).
#[test]
fn maintenance_progress_event_matches_frontend_and_does_not_leak() {
    const MAINTENANCE_RS: &str = include_str!("../src/commands/maintenance.rs");
    let declared = collect_between(
        MAINTENANCE_RS,
        "pub const MAINTENANCE_PROGRESS_EVENT: &str = \"",
        "\"",
    )
    .into_iter()
    .next()
    .expect("maintenance.rs must declare MAINTENANCE_PROGRESS_EVENT as a string literal");

    assert!(
        APP_JS.contains(&format!("listenEvent(\"{declared}\"")),
        "dist/app.js must listen on the maintenance event name Rust emits.\n\
         Rust declares MAINTENANCE_PROGRESS_EVENT = \"{declared}\", but app.js \
         does not listen for it — the tab's progress would silently never move."
    );

    let lowered = declared.to_lowercase();
    for banned in ["qwen", "onnx", "bge", "phi", "gguf", "llama", "rerank"] {
        assert!(
            !lowered.contains(banned),
            "ADR-086: the maintenance event name must not name the stack \
             ('{banned}'); got \"{declared}\""
        );
    }
}

#[test]
fn permission_identifier_maps_snake_case_to_kebab_case() {
    assert_eq!(
        permission_identifier("list_recent_memories"),
        "allow-list-recent-memories"
    );
    assert_eq!(permission_identifier("add_memory"), "allow-add-memory");
}

// ---------------------------------------------------------------------------
// Guard 4 — the engine-readiness states must agree across the language
// boundary (ADR-090)
// ---------------------------------------------------------------------------

/// `RerankerState::as_wire_str` in `vault-app` and the string the frontend
/// compares against in `app.js` are connected by nothing at compile time.
///
/// The failure this prevents is specifically nasty: rename the Rust side and
/// the engine still loads, search still works, and the "getting your recall
/// engine ready" line simply **never clears** — the app sits there claiming a
/// wait that finished minutes ago. That is worse than the bug ADR-090 set out
/// to fix, because it is a permanent lie rather than a temporary silence.
#[test]
fn engine_ready_state_matches_between_rust_and_the_frontend() {
    const APPLICATION_RS: &str = include_str!("../../vault-app/src/application.rs");
    const APP_JS: &str = include_str!("../dist/app.js");

    // The one state the UI branches on. If Rust stops emitting it, the note
    // can never be shown; if the frontend stops checking it, it can never be
    // hidden.
    assert!(
        APPLICATION_RS.contains(r#"Self::Preparing => "preparing""#),
        "vault-app must map RerankerState::Preparing to the wire string \
         \"preparing\" — the frontend branches on exactly that value"
    );
    assert!(
        APP_JS.contains(r#"engineReady.state === "preparing""#),
        "app.js must branch on the \"preparing\" wire string; without it the \
         readiness note can never be shown or cleared"
    );

    // The two terminal states must NOT be treated as a wait. A spinner shown
    // for these would never resolve — the dishonesty ADR-090 exists to remove.
    for terminal in ["not_configured", "unavailable"] {
        assert!(
            APPLICATION_RS.contains(&format!(r#"=> "{terminal}""#)),
            "vault-app must declare the \"{terminal}\" wire string"
        );
    }
}

/// The readiness states cross into user-visible logic, so they fall under the
/// same white-label rule as every other string the UI can branch on.
#[test]
fn engine_ready_states_do_not_leak_the_stack() {
    const APPLICATION_RS: &str = include_str!("../../vault-app/src/application.rs");

    for state in ["not_configured", "unavailable", "preparing", "ready"] {
        assert!(
            APPLICATION_RS.contains(&format!(r#"=> "{state}""#)),
            "expected wire string \"{state}\" to be declared"
        );
        for banned in ["qwen", "onnx", "bge", "phi", "gguf", "llama", "rerank"] {
            assert!(
                !state.contains(banned),
                "ADR-086: readiness state must not name the stack ('{banned}'); \
                 got \"{state}\""
            );
        }
    }
}

/// The user-facing readiness copy must not name the stack either (ADR-086),
/// and must not promise that recall is unavailable — it is not. Search works
/// throughout, on the retriever's own order (ADR-089).
#[test]
fn engine_ready_note_is_white_label_and_does_not_claim_search_is_broken() {
    const APP_JS: &str = include_str!("../dist/app.js");

    let note = APP_JS
        .split("Getting your recall engine ready")
        .nth(1)
        .expect("app.js must contain the readiness note copy");
    let sentence = note.split('"').next().unwrap_or("").to_lowercase();

    for banned in [
        "qwen", "onnx", "bge", "phi", "gguf", "llama", "rerank", "model",
    ] {
        assert!(
            !sentence.contains(banned),
            "ADR-086: readiness copy must not name the stack ('{banned}')"
        );
    }
    assert!(
        sentence.contains("you can search now"),
        "the readiness note must tell the user search still works — ADR-089 \
         degrades rather than failing, and the copy must not imply otherwise"
    );
}

// ---------------------------------------------------------------------------
// Guard 5 — the lock screen and the account (S3 step 4d-2,
// SIGNIN-DESIGN.md §8.38 and §8.39)
//
// The same honesty as every guard above: these read the shipped source, so
// they pin structure and wording, not behaviour in a running webview. That
// is weaker than a behaviour test and is said here rather than implied. Each
// was run failing first, and the key ones were planted (§8.39's evidence).
// ---------------------------------------------------------------------------

/// `app.js` as code: line endings normalised (this checkout is CRLF, CI's is
/// LF — §8.38's lesson) and comments removed, whole-line and trailing, so a
/// guard never fires on a sentence *describing* the rule it checks (§8.37: a
/// source test reads prose as code otherwise).
fn js_code() -> String {
    APP_JS
        .replace("\r\n", "\n")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .map(strip_trailing_comment)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drop a trailing `// …` comment, but only where the `//` is outside a
/// string: an even number of `"` and backticks before it. No string in
/// `app.js` contains `//`; `extractors_for_the_lock_guards_find_real_code`
/// would notice a cut that ate code.
fn strip_trailing_comment(line: &str) -> &str {
    let mut search = 0;
    while let Some(found) = line[search..].find("//") {
        let at = search + found;
        let before = &line[..at];
        if (before.matches('"').count() + before.matches('`').count()) % 2 == 0 {
            return line[..at].trim_end();
        }
        search = at + 2;
    }
    line
}

/// The markup, line endings normalised.
fn html() -> String {
    INDEX_HTML.replace("\r\n", "\n")
}

/// One top-level function of `app.js`. By the file's own style it opens at
/// column 0 (`function name(` or `async function name(`) and closes at the
/// next line that is exactly `}`, or on its own line for a one-liner. The
/// split is exact for that style and panics, rather than guessing, if the
/// style changes.
struct JsFunction {
    name: String,
    body: String,
    start: usize,
    end: usize,
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn top_level_functions(code: &str) -> Vec<JsFunction> {
    let mut out = Vec::new();
    let mut open: Option<(String, usize)> = None;
    let mut offset = 0;
    for line in code.split_inclusive('\n') {
        let text = line.trim_end_matches('\n');
        let declared = text
            .strip_prefix("async function ")
            .or_else(|| text.strip_prefix("function "))
            .map(|rest| {
                rest.chars()
                    .take_while(|c| is_ident_char(*c))
                    .collect::<String>()
            });
        if let Some(name) = declared {
            assert!(
                open.is_none(),
                "app.js: `{name}` opens before `{}` has closed. The column-0 style these \
                 guards read has changed; fix the file or the reader, never both at once.",
                open.as_ref().map_or("", |(n, _)| n.as_str())
            );
            let one_liner = text.trim_end().ends_with('}')
                && text.matches('{').count() == text.matches('}').count();
            if one_liner {
                out.push(JsFunction {
                    name,
                    body: text.to_string(),
                    start: offset,
                    end: offset + line.len(),
                });
            } else {
                open = Some((name, offset));
            }
        } else if text == "}" {
            if let Some((name, start)) = open.take() {
                let end = offset + line.len();
                out.push(JsFunction {
                    name,
                    body: code[start..end].to_string(),
                    start,
                    end,
                });
            }
        }
        offset += line.len();
    }
    assert!(open.is_none(), "app.js: a top-level function never closes");
    out
}

fn js_function<'a>(functions: &'a [JsFunction], name: &str) -> &'a JsFunction {
    functions
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("app.js defines no top-level function `{name}`"))
}

/// Does `text` call `name(` itself — not `x.name(`, not `somename(`?
fn calls(text: &str, name: &str) -> bool {
    let needle = format!("{name}(");
    text.match_indices(&needle).any(|(at, _)| {
        !matches!(text[..at].chars().next_back(), Some(c) if is_ident_char(c) || c == '.')
    })
}

/// Does `text` mention `name` as a whole word?
fn mentions(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        let before = !matches!(text[..at].chars().next_back(), Some(c) if is_ident_char(c));
        let after = !matches!(text[at + name.len()..].chars().next(), Some(c) if is_ident_char(c));
        before && after
    })
}

/// The argument text of every `setTimeout(` / `setInterval(` /
/// `requestAnimationFrame(` call, up to its matching `)`.
fn timer_arguments(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    for timer in ["setTimeout(", "setInterval(", "requestAnimationFrame("] {
        for (at, _) in code.match_indices(timer) {
            if code[..at].chars().next_back().is_some_and(is_ident_char) {
                continue;
            }
            let args = &code[at + timer.len()..];
            let mut depth = 1usize;
            let mut end = args.len();
            for (i, c) in args.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            out.push(args[..end].to_string());
        }
    }
    out
}

/// The commands one piece of code invokes.
fn invoked_in(text: &str) -> Vec<String> {
    let mut names = collect_between(text, "invoke(\"", "\"");
    names.sort();
    names.dedup();
    names
}

#[test]
fn extractors_for_the_lock_guards_find_real_code() {
    // Without this, a reader that silently found nothing would turn every
    // guard below into a vacuous pass.
    let code = js_code();
    let functions = top_level_functions(&code);
    assert!(
        functions.len() >= 60,
        "found only {} top-level functions in app.js",
        functions.len()
    );
    for name in ["invoke", "esc", "$", "init", "askAccess", "renderLock"] {
        js_function(&functions, name);
    }
    for f in &functions {
        assert!(
            !f.body.contains("\nfunction ") && !f.body.contains("\nasync function "),
            "`{}` swallowed the function after it",
            f.name
        );
    }
    assert!(
        code.contains("document.addEventListener(\"DOMContentLoaded\""),
        "stripping comments ate code"
    );

    assert!(calls("a(); x.b(1);", "a"));
    assert!(!calls("x.a(1);", "a"));
    assert!(!calls("ba(1);", "a"));
    assert!(mentions("f(onPaid, 3)", "onPaid"));
    assert!(!mentions("onPaidLater()", "onPaid"));
    assert_eq!(
        timer_arguments("setTimeout(() => go(1), 5); clearTimeout(t);"),
        vec!["() => go(1), 5".to_string()]
    );
    assert_eq!(strip_trailing_comment("x = 1; // note"), "x = 1;");
    assert_eq!(strip_trailing_comment("s = \"a//b\";"), "s = \"a//b\";");
}

/// The functions allowed to ask `account_access`, each with its §8.38
/// reason: *"It is asked when the app opens and after an account action,
/// never on a timer."* A new asker is a decision, made here where a reviewer
/// sees it.
const ACCESS_ASKED_FROM: &[(&str, &str)] = &[
    ("enterApp", "the app opens"),
    (
        "beginAccount",
        "the person signed in or created an account (both buttons, §8.41)",
    ),
    ("onSignOut", "the person signed out"),
    ("onPaid", "the person pressed \"I've paid\""),
    ("onRetry", "the person pressed \"Try again\""),
    (
        "afterCheckout",
        "a checkout's wait ended: once, after the wait, never inside it",
    ),
];

/// What nothing that reaches `account_access` may contain: anything that
/// repeats on its own.
const REPEATS: &[&str] = &[
    "setTimeout(",
    "setInterval(",
    "requestAnimationFrame(",
    "while (",
    "while(",
    "for (",
    "for(",
    "sleep(",
];

/// **§8.38's obligation on this step: `account_access` is never asked on a
/// timer.** A served ask records a use (§4), so a window that polled it
/// would keep `last_active_anchor` current for as long as it stayed open,
/// and the 30-days-unused sign-out would never fire. Nothing in 4d-1
/// enforced it. This does, in four parts:
///
/// 1. exactly one `invoke("account_access")`, inside `askAccess`;
/// 2. `askAccess` is called only by [`ACCESS_ASKED_FROM`];
/// 3. neither it nor anything that reaches it — directly or through other
///    functions — contains a timer or a loop;
/// 4. no timer is handed any of those functions;
/// 5. anywhere one of them is handed over as a callback rather than called,
///    it is to a button's `click`, and nothing that reaches the lock
///    registers a listener for anything else. (The independent review found
///    parts 1–4 blind to `window.addEventListener("focus", askAccess)`,
///    which would ask the lock on every focus; §8.39.)
///
/// Part 3 is why the checkout wait (`waitForPayment`) only refreshes and
/// returns, and `afterCheckout` asks once, after it.
#[test]
fn account_access_is_never_asked_on_a_timer() {
    let code = js_code();
    let functions = top_level_functions(&code);

    // 1
    let asks: Vec<usize> = code
        .match_indices("invoke(\"account_access\"")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        asks.len(),
        1,
        "account_access must be asked in exactly one place, askAccess; found {}",
        asks.len()
    );
    let ask = js_function(&functions, "askAccess");
    assert!(
        ask.start <= asks[0] && asks[0] < ask.end,
        "the one invoke(\"account_access\") must be inside askAccess"
    );
    for (at, _) in code.match_indices("askAccess(") {
        assert!(
            functions.iter().any(|f| f.start <= at && at < f.end),
            "askAccess is called outside a named top-level function (byte {at}), \
             where this guard cannot see what repeats it"
        );
    }

    // 2
    let mut direct: Vec<&str> = functions
        .iter()
        .filter(|f| f.name != "askAccess" && calls(&f.body, "askAccess"))
        .map(|f| f.name.as_str())
        .collect();
    direct.sort_unstable();
    let mut allowed: Vec<&str> = ACCESS_ASKED_FROM.iter().map(|(n, _)| *n).collect();
    allowed.sort_unstable();
    assert_eq!(
        direct, allowed,
        "the functions that ask account_access changed. Each asker is a decision \
         (§8.38: at open and after an account action, never on a timer): add it to \
         ACCESS_ASKED_FROM with its reason, or don't ask there."
    );

    // 3
    let mut reaching: Vec<String> = vec!["askAccess".to_string()];
    loop {
        let before = reaching.len();
        for f in &functions {
            if !reaching.contains(&f.name) && reaching.iter().any(|r| calls(&f.body, r)) {
                reaching.push(f.name.clone());
            }
        }
        if reaching.len() == before {
            break;
        }
    }
    for name in &reaching {
        let f = js_function(&functions, name);
        for repeat in REPEATS {
            assert!(
                !f.body.contains(repeat),
                "`{name}` reaches account_access and contains `{repeat}`: it could ask \
                 the lock again and again while the window stays open (§8.38)"
            );
        }
    }

    // 4
    let timers = timer_arguments(&code);
    assert!(
        timers.len() >= 5,
        "found only {} timer calls in app.js, so this part checks nothing",
        timers.len()
    );
    for args in &timers {
        for name in &reaching {
            assert!(
                !mentions(args, name),
                "a timer is handed `{name}`, which reaches account_access: `{args}`"
            );
        }
    }

    // 5
    let mut handed_to_a_click = 0;
    for name in &reaching {
        for (at, _) in code.match_indices(name.as_str()) {
            let before = code[..at].chars().next_back();
            let after = code[at + name.len()..].chars().next();
            let whole_word = !matches!(before, Some(c) if is_ident_char(c))
                && !matches!(after, Some(c) if is_ident_char(c));
            if !whole_word || after == Some('(') {
                continue; // a call, or a longer name: parts 2 and 3 see those
            }
            let line_start = code[..at].rfind('\n').map_or(0, |n| n + 1);
            assert!(
                code[line_start..at].ends_with("addEventListener(\"click\", "),
                "`{name}` reaches account_access and is handed over as a callback \
                 other than a button's click: `{}`. Only a person's click may lead to \
                 the lock being asked (§8.38).",
                code[line_start..].lines().next().unwrap_or("").trim()
            );
            handed_to_a_click += 1;
        }
    }
    assert!(
        handed_to_a_click >= 5,
        "found only {handed_to_a_click} buttons wired to functions that ask the lock, \
         so part 5 checks nothing"
    );
    for name in &reaching {
        let f = js_function(&functions, name);
        for (at, _) in f.body.match_indices("addEventListener(") {
            assert!(
                f.body[at..].starts_with("addEventListener(\"click\""),
                "`{name}` reaches account_access and registers a listener for something \
                 other than a click"
            );
        }
        assert!(
            !f.body.contains("listenEvent("),
            "`{name}` reaches account_access and listens for backend events, which \
             arrive on their own"
        );
    }
}

/// §8.26 §4: *"After checkout: every 10 s for 10 min, plus an 'I've paid'
/// button."* The wait refreshes and does nothing else — it never asks the
/// lock (`afterCheckout` does that once, after it) — and it ends.
#[test]
fn the_checkout_wait_refreshes_every_ten_seconds_for_ten_minutes_and_ends() {
    let code = js_code();
    assert!(
        code.contains("const CHECKOUT_POLL_MS = 10 * 1000;"),
        "the checkout poll is every 10 s (§8.26 §4)"
    );
    assert!(
        code.contains("const CHECKOUT_POLL_FOR_MS = 10 * 60 * 1000;"),
        "the checkout poll lasts 10 min (§8.26 §4)"
    );
    let functions = top_level_functions(&code);
    let wait = js_function(&functions, "waitForPayment");
    for needle in [
        "Date.now() + CHECKOUT_POLL_FOR_MS",
        "while (Date.now() < until",
        "sleep(CHECKOUT_POLL_MS)",
    ] {
        assert!(
            wait.body.contains(needle),
            "waitForPayment no longer contains `{needle}`: the wait must be every \
             10 s and must end after 10 min"
        );
    }
    assert_eq!(
        invoked_in(&wait.body),
        vec!["account_refresh_now".to_string()],
        "the checkout wait may only refresh"
    );
}

/// Every code a gated command can be refused with has a screen, and every
/// screen has words. A code with no screen would leave somebody looking at
/// a lock with no way out.
#[test]
fn every_lock_code_opens_its_own_lock_screen() {
    use vault_tauri::guard::{
        ERR_LOCKED_CANNOT_CONFIRM, ERR_LOCKED_SIGNED_OUT, ERR_LOCKED_SUBSCRIPTION_ENDED,
        ERR_LOCKED_TRIAL_ENDED, ERR_LOCKED_UNLOCKING,
    };

    let code = js_code();
    let map = collect_between(&code, "const LOCK_VARIANT = {", "};")
        .into_iter()
        .next()
        .expect("app.js maps each lock code to its screen in LOCK_VARIANT");
    let copy = collect_between(&code, "const LOCK_COPY = {", "\n};")
        .into_iter()
        .next()
        .expect("app.js holds each lock screen's words in LOCK_COPY");

    for lock_code in [
        ERR_LOCKED_SIGNED_OUT,
        ERR_LOCKED_CANNOT_CONFIRM,
        ERR_LOCKED_TRIAL_ENDED,
        ERR_LOCKED_SUBSCRIPTION_ENDED,
        ERR_LOCKED_UNLOCKING,
    ] {
        let variant = collect_between(&map, &format!("{lock_code}: \""), "\"")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{lock_code} has no lock screen in LOCK_VARIANT"));
        assert!(
            copy.contains(&format!("{variant}: {{")),
            "LOCK_VARIANT sends {lock_code} to `{variant}`, which LOCK_COPY has no words for"
        );
    }

    // §8.38's "close and reopen" screen: told apart from "check your
    // connection" by the account view, never by guessing.
    assert!(copy.contains("reopen: {"), "LOCK_COPY has no reopen screen");
    let functions = top_level_functions(&code);
    let pick = js_function(&functions, "lockVariant");
    assert!(
        pick.body
            .contains("!view.signed_in && view.state === \"cannot_confirm\""),
        "the reopen screen is for an account this computer could not read at all"
    );
    assert!(
        pick.body.contains("|| \"cannot_confirm\""),
        "an unknown lock code must fall back to the try-again screen"
    );
}

/// BRD §1.6 amendment 1 and §8.26 §6: whatever the reason for the lock, the
/// person can always take their memories and can always reach us. So the
/// export and the address live in the lock screen's foot, outside every
/// reason's own buttons, and the code that picks a reason never touches them.
#[test]
fn the_lock_screen_always_offers_the_export_and_the_support_email() {
    let html = html();
    let lock = collect_between(&html, "id=\"screen-lock\"", "<!-- =====")
        .into_iter()
        .next()
        .expect("index.html has a lock screen");
    let foot = lock
        .split_once("class=\"lock-foot\"")
        .expect("the lock screen has a foot, outside every reason's own buttons")
        .1;
    assert!(
        foot.contains("id=\"lock-export\""),
        "Download my memories is not in the lock screen's foot"
    );
    assert!(
        foot.contains("customerservice@zaaheen.com"),
        "the support address is not in the lock screen's foot"
    );

    let code = js_code();
    let functions = top_level_functions(&code);
    let render = js_function(&functions, "renderLock");
    for part in ["lock-export", "lock-foot", "lock-help"] {
        assert!(
            !render.body.contains(part),
            "renderLock touches `{part}`: the export and the address must stay \
             whatever the reason for the lock"
        );
    }
    assert!(
        code.contains("$(\"lock-export\").addEventListener(\"click\", onLockExport)"),
        "the lock screen's Download button is not wired"
    );
    assert!(calls(
        &js_function(&functions, "onLockExport").body,
        "exportMemories"
    ));
    assert!(js_function(&functions, "exportMemories")
        .body
        .contains("invoke(\"export_memories\""));
}

/// §8.42 (founder, session 55): the lock screen's download is offered
/// whenever this computer has memories, and hidden only where there is
/// nothing to take. The rule is the count, never the reason for the lock
/// (the test above still holds: `renderLock` does not touch it). Hidden only
/// on a count that reads exactly zero; a count that cannot be read shows it,
/// because hiding it from somebody with memories is the failure that
/// matters. It starts shown, so nothing has to go right for it to appear.
#[test]
fn the_lock_screen_offers_the_export_whenever_there_are_memories() {
    let html = html();
    let foot = html
        .split_once("class=\"lock-foot\"")
        .expect("the lock screen has a foot")
        .1;
    let box_at = foot
        .find("<div id=\"lock-export-box\">")
        .expect("the download sits in its own box in the foot, and starts shown");
    let button_at = foot
        .find("id=\"lock-export\"")
        .expect("the download button is in the foot");
    assert!(box_at < button_at, "the download button is outside its box");

    let code = js_code();
    let functions = top_level_functions(&code);
    let render = &js_function(&functions, "renderLockExport").body;
    assert!(
        render.contains("invoke(\"get_settings_info\")"),
        "the count comes from the command already on the locked allowlist"
    );
    assert!(
        render.contains(
            "none = info !== null && typeof info === \"object\" && info.memory_count === 0;"
        ),
        "hidden on exactly zero, nothing looser"
    );
    assert_eq!(
        render.matches("none =").count(),
        2,
        "`none` starts false and is set once, from the count"
    );
    let unsure = render
        .split_once("} catch {")
        .expect("a count that cannot be read is caught")
        .1
        .split_once('}')
        .expect("the catch closes")
        .0;
    assert!(
        render.contains("let none = false;") && !unsure.contains("none"),
        "a count that cannot be read must leave the download shown"
    );
    assert!(render.contains("$(\"lock-export-box\").classList.toggle(\"hidden\", none)"));
    assert!(
        js_function(&functions, "showScreen")
            .body
            .contains("if (name === \"lock\") renderLockExport();"),
        "asked each time the lock screen is entered"
    );
}

/// The lock screen's words, founder-approved 2026-09-21 (session 51: *"yes
/// partner go with that wording"*; `SIGNIN-DESIGN.md` §8.39), plus the
/// support address (founder, same day). Pinned so an edit is a decision, not
/// a drift.
const APPROVED_LOCK_WORDING: &[&str] = &[
    // every version
    "Download my memories",
    "Save everything you've kept as one file. It's yours, whether or not you subscribe.",
    "Need help?",
    "customerservice@zaaheen.com",
    // 1. not signed in
    "Sign in to open your memories.",
    // "on the next page" dropped with the Create an account button (founder,
    // session 54, SIGNIN-DESIGN.md §8.41: "wording is good").
    "Your memories are on this computer, encrypted, just as you left them. Sign in to your \
     Zaaheen account to open them. New to Zaaheen? Create an account, and your 30-day free \
     trial starts straight away. No card is needed.",
    "This opens your web browser. Signing in never shares your memories with us. They stay \
     locked on this computer.",
    "Finish signing in in your browser, then come back here.",
    // the setup's step and both buttons (§8.41)
    "Create your Zaaheen account, or sign in.",
    "Your 30-day free trial starts when you create your account, and no card is needed.",
    "Create an account",
    "Finish creating your account in your browser, then come back here.",
    // 2. trial ended
    "Your free trial has ended.",
    "Subscribe to keep using your memories. Nothing has been deleted. Everything you kept is \
     still here on this computer.",
    "$5 a month",
    "$48 a year (save $12)",
    "Payment opens in your browser. Tax is added at checkout where it applies. Cancel anytime.",
    "Finish paying in your browser. Zaaheen unlocks by itself as soon as your payment comes \
     through.",
    "I've paid",
    "We haven't seen your payment yet. It can take a minute, so try again shortly.",
    "Not you?",
    // 3. subscription ended
    "Your subscription has ended.",
    "Subscribe again to keep using your memories. Nothing has been deleted. Everything you \
     kept is still here on this computer.",
    // 4. could not check
    "We couldn't confirm your subscription.",
    "Zaaheen couldn't reach us to check your subscription. Check your internet connection, \
     then try again. Only your subscription is checked, never your memories.",
    "Try again",
    "Still couldn't connect. Check your internet connection and try again in a moment.",
    // 5. needs reopening
    "Zaaheen couldn't open your account on this computer.",
    "Closing Zaaheen and opening it again usually fixes this. If it keeps happening, email us \
     at the address below.",
    "Close Zaaheen",
];

#[test]
fn the_lock_screen_says_what_the_founder_approved() {
    let shipped = format!("{APP_JS}\n{INDEX_HTML}").replace("\r\n", "\n");
    for line in APPROVED_LOCK_WORDING {
        assert!(
            shipped.contains(line),
            "a founder-approved line is gone or has changed: {line:?}. Changing the \
             lock screen's words is the founder's decision (§8.39)."
        );
    }
}

/// §8.38: *"make the frontend treat any `locked_*` rejection from a gated
/// command as the same signal, so that a subscription that lapses while the
/// app is open reaches the lock screen on the next action."* One place, the
/// central `invoke` wrapper, rather than fifteen call sites.
#[test]
fn a_refused_command_takes_the_person_to_the_lock_screen() {
    let code = js_code();
    let functions = top_level_functions(&code);
    let wrapper = js_function(&functions, "invoke");
    let check = wrapper
        .body
        .find("friendlyLockError(raw)")
        .expect("the invoke wrapper no longer recognises a lock rejection");
    let show = wrapper.body.find("showLock(raw)").expect(
        "the invoke wrapper no longer sends a lock rejection to the lock screen, so a \
         subscription that lapses while the app is open never reaches it",
    );
    assert!(
        check < show,
        "showLock must follow the lock-code check, or every error opens the lock screen"
    );
}

/// The handoff's finding: `ensure_recall_engine`, `recall_engine_state`,
/// `warm_recall_engine` and the maintenance calls are gated, so starting them
/// when the window opens gets a fresh install's first download refused before
/// anybody could sign in. They start only once the lock has said yes.
#[test]
fn nothing_gated_starts_before_the_lock_has_answered() {
    let code = js_code();
    let functions = top_level_functions(&code);

    let init = js_function(&functions, "init");
    for starter in [
        "startEngineFetch",
        "pollEngineReady",
        "warmEngine",
        "startMaintenanceFetch",
        "catchUpMaintenanceIfDue",
        // ADR-105 L-e: the location commands are gated too.
        "refreshLocation",
        "renderLocationStep",
        "refreshLocationNotice",
        "renderMoveSection",
    ] {
        assert!(
            !calls(&init.body, starter),
            "init() calls `{starter}`, a gated command, before the lock has answered"
        );
    }

    let work = js_function(&functions, "startEntitledWork");
    for starter in [
        "startEngineFetch",
        "pollEngineReady",
        "startMaintenanceFetch",
        "catchUpMaintenanceIfDue",
    ] {
        assert!(
            calls(&work.body, starter),
            "startEntitledWork no longer starts `{starter}`"
        );
    }

    let callers: Vec<&str> = functions
        .iter()
        .filter(|f| f.name != "startEntitledWork" && calls(&f.body, "startEntitledWork"))
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(
        callers,
        ["routeAfterAccess"],
        "the gated background work must start from one place, after the lock answers"
    );
    let route = js_function(&functions, "routeAfterAccess");
    let open_branch = route
        .body
        .split_once("if (locked === null) {")
        .expect("routeAfterAccess has an open branch")
        .1
        .split_once("return;")
        .expect("the open branch returns")
        .0;
    assert!(
        calls(open_branch, "startEntitledWork"),
        "startEntitledWork must run on the branch where the lock said yes"
    );
    // ... and only there: a second call on the locked branch would start
    // gated work that is refused, and show a locked person half an app.
    assert_eq!(
        route.body.matches("startEntitledWork(").count(),
        1,
        "routeAfterAccess must start the gated work once, on the open branch only"
    );
}

/// The handoff: the onboarding sign-in step is placed **before** the
/// recall-engine download and the first memory, because both are gated.
/// Since ADR-105 L-e the location step (gated too) comes next.
#[test]
fn a_new_install_signs_in_before_its_first_gated_step() {
    let code = js_code();
    assert!(
        code.contains(
            "const ONBOARDING_WITH_SIGN_IN = [\"welcome\", \"signin\", \"location\", \"connect\", \"memory\", \"maintenance\"];"
        ),
        "the sign-in step must come straight after the welcome, before anything gated"
    );
    assert!(
        code.contains(
            "const ONBOARDING = [\"welcome\", \"location\", \"connect\", \"memory\", \"maintenance\"];"
        ),
        "a build without sign-in starts at the location step"
    );
    assert!(
        html().contains("id=\"screen-signin\""),
        "index.html has no sign-in step"
    );

    let functions = top_level_functions(&code);
    let begin = js_function(&functions, "beginSetup");
    let signed_out = begin
        .body
        .find("locked_signed_out")
        .expect("beginSetup does not look for a signed-out computer");
    let to_signin = begin
        .body
        .find("showScreen(\"signin\")")
        .expect("beginSetup never goes to the sign-in step");
    assert!(signed_out < to_signin);
}

/// ADR-SEC-027's address came from the account service; it is shown as text
/// and never as markup (BRD §11.12's webview checklist).
#[test]
fn the_signed_in_address_is_only_ever_text() {
    let code = js_code();
    let functions = top_level_functions(&code);
    let readers: Vec<&JsFunction> = functions
        .iter()
        .filter(|f| f.body.contains(".email"))
        .collect();
    assert!(
        !readers.is_empty(),
        "no function shows the signed-in address, so this checks nothing"
    );
    for f in readers {
        for markup in ["innerHTML", "outerHTML", "insertAdjacentHTML"] {
            assert!(
                !f.body.contains(markup),
                "`{}` reads the signed-in address and writes `{markup}`",
                f.name
            );
        }
        assert!(
            f.body.contains("textContent"),
            "`{}` reads the signed-in address but never shows it with textContent",
            f.name
        );
    }
}

/// §8.26 §6: *"Trial reminders: desktop banner from day 23"* — the last
/// seven days of thirty — plus the payment-failed banner and §4's clock
/// notice, in their locked words.
#[test]
fn the_banners_are_the_ones_the_design_asks_for() {
    let code = js_code();
    assert!(
        code.contains("const TRIAL_BANNER_DAYS = 7;"),
        "the trial banner starts on day 23 of 30"
    );
    let functions = top_level_functions(&code);
    let banners = js_function(&functions, "bannerLines");
    for needle in [
        "view.state === \"trial\" && view.days_left !== null && view.days_left <= TRIAL_BANNER_DAYS",
        "view.state === \"payment_failed\"",
        "view.clock_wrong",
        // §8.26 §4, word for word.
        "Your computer's clock is wrong. Set it to update automatically.",
    ] {
        assert!(
            banners.body.contains(needle),
            "bannerLines no longer contains `{needle}`"
        );
    }
}

/// §8.26 §7: *"dialog says the subscription continues until cancelled, with
/// the portal link"*. The link is a button: no URL crosses into the webview
/// (ADR-SEC-026).
#[test]
fn delete_everything_says_the_subscription_continues() {
    let html = html();
    let confirm = collect_between(&html, "id=\"erase-confirm\"", "id=\"erase-phrase\"")
        .into_iter()
        .next()
        .expect("index.html has the erase confirmation");
    assert!(
        confirm.contains("does not cancel your subscription"),
        "the Delete everything dialog no longer says the subscription continues"
    );
    assert!(
        confirm.contains("id=\"erase-manage\""),
        "the Delete everything dialog has no way to manage the subscription"
    );
    assert!(
        js_code().contains("$(\"erase-manage\").addEventListener(\"click\", onManage)"),
        "the dialog's Manage subscription button is not wired"
    );
}

/// ADR-086 white label: no vendor behind the account appears in anything the
/// desktop shows. Passes today; planted in §8.39 to prove it can fail.
#[test]
fn nothing_the_desktop_shows_names_a_vendor() {
    let shipped = format!("{APP_JS}\n{INDEX_HTML}").to_lowercase();
    for vendor in ["paddle", "clerk", "cloudflare", "stripe"] {
        assert!(
            !shipped.contains(vendor),
            "the desktop bundle names `{vendor}` (ADR-086)"
        );
    }
}

/// §8.38: *"a developer build never shows a sign-in screen."* A build with no
/// account settings answers `sign_in: false, locked: null`, so the lock never
/// shows; the panel and the banners must stay hidden too.
#[test]
fn a_build_without_sign_in_shows_no_account_screens() {
    let code = js_code();
    let functions = top_level_functions(&code);
    for name in ["renderAccountPanel", "renderBanners"] {
        assert!(
            js_function(&functions, name)
                .body
                .contains("if (!account.signIn"),
            "`{name}` shows account surfaces without checking the build has sign-in"
        );
    }
}

// ---------------------------------------------------------------------------
// Where the memories live (ADR-105 L-e)
//
// Structure and wording, like every guard in this file; the behaviour is
// driven in the browser harness (`MemoryVault-artifacts\ui-harness`).
// ---------------------------------------------------------------------------

/// L5's onboarding: the location step after the sign-in step (it is gated),
/// before the first gated work of the setup; "Continue" keeps the folder.
#[test]
fn the_location_step_comes_after_sign_in_and_before_the_rest_of_the_setup() {
    let code = js_code();
    let functions = top_level_functions(&code);
    assert!(
        html().contains("id=\"screen-location\""),
        "index.html has no location step"
    );
    assert!(
        js_function(&functions, "beginSetup")
            .body
            .contains("showScreen(\"location\")"),
        "an entitled computer's setup starts at the location step"
    );
    assert!(
        js_function(&functions, "routeAfterAccess")
            .body
            .contains("showScreen(store.get(\"mv_onboarded\", false) ? \"home\" : \"location\")"),
        "after signing in, the setup goes on at the location step"
    );
    assert!(
        js_function(&functions, "onLocationContinue")
            .body
            .contains("showScreen(\"connect\")"),
        "Continue keeps the folder and goes on"
    );
    assert!(code.contains("$(\"location-cta\").addEventListener(\"click\", onLocationContinue)"));
}

/// ADR-105 amendment 1 (L-f): a start that moves the memories serves the page
/// before it opens them, so the page asks `startup_state` and nothing else
/// until the start says ready, and only then asks the lock. If the question
/// itself fails it carries on as before; every gated command still asks.
#[test]
fn the_page_waits_for_the_start_before_it_asks_anything() {
    let code = js_code();
    let boot = code
        .split_once("document.addEventListener(\"DOMContentLoaded\", async () => {")
        .expect("the page boots from DOMContentLoaded, waiting")
        .1
        .split_once("});")
        .expect("the boot handler closes")
        .0;
    let init = boot.find("init();").expect("wires the page first");
    let waited = boot
        .find("await untilStarted();")
        .expect("waits for the start");
    let asked = boot.find("enterApp();").expect("then asks the lock");
    assert!(init < waited && waited < asked);

    let functions = top_level_functions(&code);
    let wait = &js_function(&functions, "untilStarted").body;
    assert!(wait.contains("await invoke(\"startup_state\")"));
    assert_eq!(
        wait.matches("invoke(").count(),
        1,
        "nothing but startup_state is asked while the start runs"
    );
    assert!(
        wait.contains("failed += 1;\n      if (failed >= STARTUP_TRIES) break;"),
        "a failed question is asked again before the page carries on"
    );
    assert!(code.contains("const STARTUP_TRIES = 20;"));
    assert!(wait.contains("answer.stage === \"ready\") break;"));
    assert!(wait.contains("showScreen(\"moving\")"));
    assert!(
        js_function(&functions, "showScreen")
            .body
            .contains("\"lock\", \"moving\"]"),
        "the moving screen is one of the screens"
    );
}

/// L-f: the screen shows the move's own progress and nothing made up: the
/// phase in words, the bar from the bytes (copying the first half, checking
/// the second), the sizes, and the folder, all set as text.
#[test]
fn the_moving_screen_shows_the_moves_own_progress() {
    let html = html();
    let screen = collect_between(&html, "id=\"screen-moving\"", "<!-- =====")
        .into_iter()
        .next()
        .expect("index.html has the moving screen");
    for id in ["moving-phase", "moving-fill", "moving-amount"] {
        assert!(screen.contains(&format!("id=\"{id}\"")), "{id}");
    }
    assert!(screen.contains("role=\"progressbar\""));

    let code = js_code();
    let functions = top_level_functions(&code);
    let render = &js_function(&functions, "renderMoving").body;
    for fact in [
        "answer.total",
        "answer.done",
        "answer.phase === \"copying\"",
        "answer.phase === \"checking\"",
        "answer.phase === \"finishing\"",
        "answer.stage === \"opening\"",
        "answer.to",
    ] {
        assert!(render.contains(fact), "renderMoving does not read {fact}");
    }
    assert!(render.contains("percent = part * 50;"));
    assert!(render.contains("percent = 50 + part * 50;"));
    assert!(
        render.contains("Math.min(Number(answer.done) || 0, total)"),
        "the bar never runs past the total"
    );
    for el in ["moving-phase", "moving-amount"] {
        assert!(render.contains(&format!("$(\"{el}\").textContent =")));
    }
    assert!(
        !render.contains("innerHTML"),
        "the folder is the person's, shown as text"
    );
}

/// L-f's words, founder-approved (session 55: *"yes all good"*, shown
/// running in the browser preview). Pinned so an edit is a decision.
#[test]
fn the_moving_words_are_the_founders() {
    let both = format!("{}\n{}", html(), js_code());
    for words in [
        ">Moving your memories</h1>",
        "Getting ready to move them.",
        "`Copying them to ${answer.to || \"the new folder\"}.`",
        "Checking that every memory arrived safely.",
        "Almost done.",
        "Opening your memories.",
        "`${formatBytes(done)} of ${formatBytes(total)} copied`",
        "`${formatBytes(done)} of ${formatBytes(total)} checked`",
        // Shortened after L-f's review ("tries again" is not true of a
        // second attempt), and shown only before the switch.
        "Please keep Zaaheen open until this finishes. If it closes, your memories stay safe \
         where they were.</p>",
    ] {
        assert!(both.contains(words), "{words}");
    }
    let functions = top_level_functions(&js_code());
    let render = &js_function(&functions, "renderMoving").body;
    assert!(render.contains("[\"waiting\", \"copying\", \"checking\"].includes(answer.phase)"));
    assert!(render.contains("$(\"moving-note\").classList.toggle(\"hidden\", !beforeTheSwitch);"));
}

/// L-f: the bar's soft light and its easing move only a background and a
/// width, and less motion asked for turns both off.
#[test]
fn the_moving_bar_respects_reduced_motion() {
    let css = STYLES_CSS.replace("\r\n", "\n");
    let fill = collect_between(&css, ".moving-fill {", "\n}")
        .into_iter()
        .next()
        .expect("styles.css draws the bar");
    assert!(fill.contains("animation: mvShimmer"));
    assert!(fill.contains("transition: width"));
    assert!(
        css.contains(".moving-fill { transition: none; animation: none;"),
        "no reduced-motion form for the bar"
    );
}

/// The setup's "connect" step in plain words (founder's walk-through, session
/// 55: *"yes wording is good partner"*): "AI app" as the welcome says, the
/// tiles described for people, and where each app keeps its setting.
#[test]
fn the_connect_step_speaks_plainly() {
    let html = html();
    let step = collect_between(&html, "id=\"screen-connect\"", "<!-- =====")
        .into_iter()
        .next()
        .expect("index.html has the connect step");
    assert!(step.contains("<h1>Connect your first AI app.</h1>"));
    assert!(step.contains("Pick the AI app you use, and we'll show you how to connect it."));
    assert!(step.contains(">I'll connect one later</button>"));
    let code = js_code();
    for words in [
        "name: \"Claude Desktop\", desc: \"The Claude app for your computer\"",
        "In Claude, open Settings, then Developer, then Edit Config. Add this to the file it \
         shows you, save it, then quit Claude and open it again:",
        "name: \"Cursor\", desc: \"AI code editor\"",
        "name: \"Claude Code\", desc: \"Claude in your terminal\"",
        "name: \"Codex\", desc: \"OpenAI's coding assistant\"",
        "name: \"Antigravity\", desc: \"Google's AI code editor\"",
        "name: \"Another app\", desc: \"Any AI app that can connect to Zaaheen\"",
    ] {
        assert!(code.contains(words), "{words}");
    }
    let tiles = collect_between(&code, "const AGENTS = [", "];")
        .into_iter()
        .next()
        .expect("app.js lists the tiles");
    for jargon in [
        "stdio",
        "MCP-compatible",
        "agentic",
        "CLI agent",
        "Introduce your first agent",
    ] {
        assert!(
            !step.contains(jargon) && !tiles.contains(jargon),
            "the connect step still says {jargon}"
        );
    }
}

/// ADR-106: "Connect it for me" only for an app with its own install route
/// (Claude Desktop and Cursor), and the page sends nothing but that app's
/// name. Each answer has its line (founder: *"wording is good go ahead"*).
#[test]
fn connect_it_for_me_names_only_the_app() {
    let html = html();
    assert!(html
        .contains("<button id=\"connect-auto-btn\" class=\"btn-pill\">Connect it for me</button>"));
    assert!(html.contains(
        "<button id=\"connect-auto-show\" class=\"link-underline hidden\">Show the file</button>"
    ));
    let code = js_code();
    assert_eq!(
        code.matches(", connect: \"").count(),
        2,
        "Claude Desktop and Cursor have the button"
    );
    assert!(code.contains("snippet: SNIPPET_JSON, connect: \"cursor\" }"));
    assert!(code.contains("snippet: SNIPPET_JSON, connect: \"claude_desktop\" }"));
    assert!(code.contains("invoke(\"show_claude_extension\")"));
    for words in [
        "note: \"Zaaheen saves a small Claude extension, and Claude asks you to install it.\"",
        "asked: \"Claude should now be asking you to install Zaaheen. Click Install, and it's \
         connected.\"",
        "saved: \"Zaaheen saved the extension to your Downloads folder. In Claude, open Settings, \
         then Extensions, then Advanced settings, then Install Extension, and choose \
         \\\"Zaaheen for Claude.mcpb\\\".\"",
        "app_not_found: \"Zaaheen couldn't find Claude on this computer. If it's installed, you \
         can add the setting yourself below.\"",
    ] {
        assert!(code.contains(words), "{words}");
    }
    let functions = top_level_functions(&code);
    let ask = &js_function(&functions, "onConnectAuto").body;
    assert!(ask.contains("await invoke(\"connect_app\", { app: agent.connect })"));
    assert_eq!(ask.matches("invoke(").count(), 1);
    assert!(code.contains("$(\"connect-auto-btn\").addEventListener(\"click\", onConnectAuto)"));
    for words in [
        "note: \"Cursor will ask you to install Zaaheen. Click Install there.\"",
        "asked: \"Cursor should now be asking you to install Zaaheen. Click Install, and it's \
         connected.\"",
        "app_not_found: \"Zaaheen couldn't find Cursor on this computer. If it's installed, you \
         can add the setting yourself below.\"",
        "could_not_open: \"Zaaheen couldn't open Cursor. You can add the setting yourself \
         below.\"",
    ] {
        assert!(code.contains(words), "{words}");
    }
}

/// Settings laid out like Claude's own (founder, session 55: *"our settings
/// page should be organized like this"*): the sections down the left, the
/// chosen one on the right, one row per setting. Every control the page had
/// is still there, each inside a section, so nothing was lost in the move.
#[test]
fn settings_has_sections_down_the_left_and_keeps_every_control() {
    let html = html();
    let settings = collect_between(&html, "id=\"tab-settings\"", "<div class=\"statusbar\">")
        .into_iter()
        .next()
        .expect("index.html has the Settings tab");
    let nav = collect_between(&settings, "<nav id=\"settings-nav\"", "</nav>")
        .into_iter()
        .next()
        .expect("Settings lists its sections");
    let pane = settings
        .split_once("</nav>")
        .expect("the list closes before the sections")
        .1;
    let sections = ["account", "memories", "about", "help"];
    for (section, label) in
        sections
            .iter()
            .zip(["Account", "Your memories", "About Zaaheen", "Help"])
    {
        assert!(
            nav.contains(&format!(
                "<button data-section=\"{section}\">{label}</button>"
            )),
            "the list has no {label}"
        );
        assert!(
            pane.contains(&format!("data-section=\"{section}\">")),
            "no {section} section on the right"
        );
    }
    for id in [
        "account-signout",
        "account-plans",
        "account-manage-btn",
        "account-paid",
        "settings-rows",
        "data-location",
        "move-reveal",
        "settings-move-host",
        "old-copy-forget",
        "export-memories",
        "erase-reveal",
        "erase-confirm-btn",
        "export-logs",
        "replay-welcome",
    ] {
        let at = settings
            .find(&format!("id=\"{id}\""))
            .unwrap_or_else(|| panic!("Settings lost {id}"));
        let opened = settings[..at].matches("<section ").count();
        let closed = settings[..at].matches("</section>").count();
        assert_eq!(opened, closed + 1, "{id} is not inside a section");
    }
    let code = js_code();
    let functions = top_level_functions(&code);
    let draw = &js_function(&functions, "renderSettingsNav").body;
    assert!(draw.contains("state.settingsSection = account.signIn ? \"account\" : \"memories\""));
    assert!(
        js_function(&functions, "renderSettings")
            .body
            .contains("renderSettingsNav();"),
        "the sections are drawn when Settings opens"
    );
    assert!(
        js_function(&functions, "onBannerClick")
            .body
            .contains("state.settingsSection = \"account\";"),
        "Subscribe from a banner opens Settings on the account"
    );
    assert!(code.contains("$(\"settings-nav\").addEventListener(\"click\", onSettingsNav)"));
}

/// "Choose another folder…" sits right under the folder it would change,
/// inside the box, as a button, not a faint link at the foot of the page
/// (founder, session 55: *"it should be right below where we show the
/// current location so its easily located by user"*).
#[test]
fn choose_another_folder_is_right_under_the_folder() {
    let html = html();
    let step = collect_between(&html, "id=\"screen-location\"", "<!-- =====")
        .into_iter()
        .next()
        .expect("index.html has the location step");
    let boxed = collect_between(&step, "<div class=\"location-box\">", "</div>")
        .into_iter()
        .next()
        .expect("the step shows the folder in a box");
    let folder_at = boxed
        .find("id=\"location-folder\"")
        .expect("the box shows the folder");
    let change_at = boxed
        .find("<button id=\"location-change\" class=\"btn-quiet\">Choose another folder…</button>")
        .expect("Choose another folder… is a button inside the box");
    assert!(folder_at < change_at, "the button comes under the folder");
    let foot = step
        .split_once("class=\"onboard-foot\"")
        .expect("the step has its foot")
        .1;
    assert!(
        !foot.contains("location-change"),
        "Choose another folder… is still at the foot of the page"
    );
    assert!(
        STYLES_CSS.contains(".location-screen.choosing #location-change { display: none; }"),
        "while a move is being confirmed, the button steps aside with Continue"
    );
}

/// A move is recorded only from the confirmation's own button — never from
/// the picker, and never without the folder having been checked first.
#[test]
fn a_move_is_asked_for_only_from_the_confirmation() {
    let code = js_code();
    let functions = top_level_functions(&code);
    let moves: Vec<usize> = code
        .match_indices("invoke(\"location_move\"")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(moves.len(), 1, "one place asks for a move");
    let go = js_function(&functions, "onMoveGo");
    assert!(go.start <= moves[0] && moves[0] < go.end);
    assert!(code.contains("$(\"move-go\").addEventListener(\"click\", onMoveGo)"));
    assert_eq!(
        code.matches("onMoveGo").count(),
        2,
        "onMoveGo is defined once and wired to its button once"
    );

    let choose = js_function(&functions, "chooseFolder");
    assert!(
        choose.body.contains("directory: true"),
        "the picker picks a folder"
    );
    assert_eq!(invoked_in(&choose.body), vec!["location_check".to_string()]);
    assert!(
        calls(&choose.body, "showMovePanel"),
        "a checked folder goes to the confirmation"
    );
}

/// ADR-105 L4.3: when the cloud check could not run, the person confirms
/// that no cloud app syncs the folder before the move can be asked for.
#[test]
fn a_folder_the_cloud_check_could_not_clear_needs_the_persons_word() {
    let code = js_code();
    let functions = top_level_functions(&code);
    let show = js_function(&functions, "showMovePanel");
    assert!(show
        .body
        .contains("pending.notes.includes(\"cloud_unchecked\")"));
    assert!(show.body.contains("$(\"move-go\").disabled = unchecked;"));
    let go = js_function(&functions, "onMoveGo");
    let refused = go
        .body
        .find("pending.notes.includes(\"cloud_unchecked\") && !$(\"move-cloud-ok\").checked")
        .expect("onMoveGo checks the person's word");
    let asked = go.body.find("invoke(\"location_move\"").unwrap();
    assert!(refused < asked);
    assert!(html().contains("id=\"move-cloud-ok\""));
}

/// The restart a move needs does not lose the setup: it resumes at the
/// location step, which says what happened, and forgets the resume then.
#[test]
fn a_setup_interrupted_by_a_move_resumes_at_the_location_step() {
    let code = js_code();
    let functions = top_level_functions(&code);
    assert!(code.contains(
        "screen: store.get(\"mv_onboarded\", false) || store.get(RESUME_KEY, null) ? \"boot\" : \"welcome\","
    ));
    assert!(code.contains(
        "onboardSteps: store.get(RESUME_KEY, null) === \"with_sign_in\" ? ONBOARDING_WITH_SIGN_IN : ONBOARDING,"
    ));
    let go = js_function(&functions, "onMoveGo");
    let resume = go
        .body
        .find("store.set(RESUME_KEY,")
        .expect("a move during setup marks the resume");
    let asked = go.body.find("invoke(\"location_move\"").unwrap();
    assert!(resume < asked, "marked before the app restarts");
    assert!(
        js_function(&functions, "renderLocationStep")
            .body
            .contains("store.set(RESUME_KEY, null)"),
        "the location step clears the resume once it is shown"
    );
    assert!(
        calls(
            &js_function(&functions, "renderLocationStep").body,
            "outcomeLines"
        ),
        "the location step says what the restart did"
    );
}

/// Folders come from the file system and from the person: always text,
/// never markup (BRD §11.12's webview checklist).
#[test]
fn every_folder_the_location_screens_show_is_text() {
    let code = js_code();
    let functions = top_level_functions(&code);
    let mut readers = 0;
    for f in &functions {
        let shows_a_folder = [".folder", ".target", ".from", "atStart.to", "move_waiting"]
            .iter()
            .any(|field| f.body.contains(field));
        if !shows_a_folder {
            continue;
        }
        readers += 1;
        for markup in ["innerHTML", "outerHTML", "insertAdjacentHTML"] {
            assert!(
                !f.body.contains(markup),
                "`{}` shows a folder and writes `{markup}`",
                f.name
            );
        }
    }
    assert!(
        readers >= 5,
        "found only {readers} functions showing a folder"
    );
}

/// Stopping the wait for an old copy asks first, in the app's own dialog.
#[test]
fn stopping_the_wait_for_an_old_copy_asks_first() {
    let code = js_code();
    let functions = top_level_functions(&code);
    let forget = js_function(&functions, "onForgetOldCopy");
    let asked = forget
        .body
        .find("confirmAction(")
        .expect("the person is asked first");
    let done = forget
        .body
        .find("invoke(\"location_forget_old_copy\"")
        .expect("the wait is stopped here");
    assert!(asked < done);
    assert!(forget.body.contains("if (!ok) return;"));
    assert!(
        forget.body.contains("old.connected"),
        "only a copy whose drive can't be seen"
    );
}

/// The folder picker's permission is named, like the save dialog's, so a
/// narrower `dialog:default` upstream cannot silently break the screens.
#[test]
fn the_folder_picker_is_permitted_by_name() {
    assert!(CAPABILITIES_JSON.contains("\"dialog:allow-open\""));
}

/// ADR-105's own words for the notes (L4.3, L4.7, the USB note), as the
/// design wrote them, with the long dash made a full stop (founder, session
/// 54: *"remove the dashes from all pages"*).
#[test]
fn the_location_notes_are_the_designs_words() {
    for words in [
        "Memories on another drive only open on this computer, and only while that drive is \
         connected. AI apps can't reach them while it's out.",
        "Zaaheen couldn't check whether a cloud app syncs this folder. Make sure none does.",
        "This drive can't lock the folder to your Windows account. Your memories stay encrypted.",
    ] {
        assert!(APP_JS.contains(words), "{words}");
    }
}

/// The location screens' words, founder-approved 2026-09-22 (session 54:
/// *"yes all good"*; `VAULT-KEY-AND-LOCATION.md`, ADR-105 implementation
/// record, L-e). Pinned so an edit is a decision, not a drift.
const APPROVED_LOCATION_WORDING: &[&str] = &[
    // the setup's step
    "Where should Zaaheen keep your memories?",
    "They stay on this computer, encrypted. The usual place suits most people. You can change \
     this later in Settings.",
    "Choose another folder…",
    // the dashes removed with the new headline (founder, session 54), then
    // the step count dropped (founder, session 55: "yes drop the number")
    "A few short steps, about three minutes.",
    "A few short steps, about two minutes.",
    // before a move
    "Your memories will move to",
    "Zaaheen will close, move them, and open again by itself. With many memories this can take \
     a few minutes.",
    "Move them here",
    "No cloud app syncs this folder.",
    "Zaaheen is closing to move your memories. It opens again by itself once they're moved.",
    // refused folders (examples shown to the founder)
    "A cloud storage app syncs that folder, so your memories would leave this computer, and \
     syncing can damage them. Choose a folder that isn't synced.",
    "That folder belongs to Windows or to a program. Choose one of your own folders.",
    "There isn't enough free space there for your memories. Free up some space, or choose \
     another drive.",
    // after the restart
    "Your memories are now kept in ${atStart.to}.",
    "They're still where they were, safe and unchanged.",
    // Settings
    "Move my memories…",
    "Stop waiting for the old copy",
    "which isn't connected. Zaaheen removes it once that drive is back, and until then your \
     memories can't be moved again.",
    "If that drive is gone for good, you can stop waiting. If it turns up later, you can delete \
     that folder yourself. It's encrypted, and it only opens on this computer.",
];

#[test]
fn the_location_screens_say_what_the_founder_approved() {
    let shipped = format!("{APP_JS}\n{INDEX_HTML}").replace("\r\n", "\n");
    for line in APPROVED_LOCATION_WORDING {
        assert!(
            shipped.contains(line),
            "a founder-approved line is gone or has changed: {line:?}. Changing the \
             location screens' words is the founder's decision (ADR-105 L-e)."
        );
    }
}

/// The welcome screen's headline and subheading, founder-chosen 2026-09-22
/// (session 54: *"yes go with 1 and remove the dashes too"*), and no long
/// dash anywhere the welcome screen can show text: the markup, the checks it
/// replays, the recall-engine row and the hint under "Begin set up".
#[test]
fn the_welcome_screen_says_what_the_founder_chose_with_no_long_dash() {
    let html = html();
    let welcome = collect_between(&html, "id=\"screen-welcome\"", "<!-- =====")
        .into_iter()
        .next()
        .expect("index.html has a welcome screen");
    assert!(welcome.contains(
        "<h1><span class=\"rise rise-1\">One memory for all your AI apps,</span> \
         <span class=\"rise rise-2\">kept safe on your computer.</span></h1>"
    ));
    assert!(welcome.contains(
        "Tell it something once, and every AI app you connect will remember it. It's \
         encrypted, and you decide who can read it."
    ));
    assert!(
        !welcome.contains("class=\"orb\""),
        "the black circle above the headline was removed"
    );
    assert!(
        welcome.contains("zaaheen-icon.png"),
        "the name with its icon"
    );

    let code = js_code();
    let checks = collect_between(&code, "const CHECK_DEFS = [", "];")
        .into_iter()
        .next()
        .expect("app.js lists the welcome checks in CHECK_DEFS");
    let functions = top_level_functions(&code);
    let mut shown = vec![welcome, checks];
    for name in ["renderEngineRow", "renderBeginState"] {
        shown.push(js_function(&functions, name).body.clone());
    }
    for text in &shown {
        assert!(
            !text.contains('\u{2014}') && !text.contains('\u{2013}'),
            "the welcome screen shows a long dash: {text}"
        );
    }
}

/// The hint under "Begin set up" names no number of steps (founder, session
/// 55: *"yes drop the number"*). The count depends on whether this computer
/// needs the sign-in step, which is known only after Begin; the number it
/// named went stale when the location step was added.
#[test]
fn the_welcome_hint_names_no_step_count() {
    let code = js_code();
    let functions = top_level_functions(&code);
    let hint = js_function(&functions, "renderBeginState")
        .body
        .to_lowercase();
    assert!(hint.contains("a few short steps"), "{hint}");
    for n in [
        "two", "three", "four", "five", "six", "seven", "2", "3", "4", "5", "6", "7",
    ] {
        for form in [
            format!("{n} steps"),
            format!("{n} gentle steps"),
            format!("{n} short steps"),
        ] {
            assert!(!hint.contains(&form), "the hint counts the steps: {form}");
        }
    }
}

/// The welcome's entrance (founder, session 54) moves only transform and
/// opacity, and always ships with its reduced-motion form: somebody who has
/// asked their computer for less motion gets a fade with no movement.
#[test]
fn the_welcome_entrance_respects_reduced_motion() {
    let css = STYLES_CSS.replace("\r\n", "\n");
    let rise = collect_between(&css, "@keyframes mvRise {", "\n}")
        .into_iter()
        .next()
        .expect("styles.css defines the welcome's rise");
    for property in ["top:", "margin", "height", "width", "left:"] {
        assert!(!rise.contains(property), "mvRise animates {property}");
    }
    let reduced = collect_between(&css, "@media (prefers-reduced-motion: reduce) {", "\n}")
        .into_iter()
        .find(|block| block.contains(".welcome .rise"))
        .expect("the welcome's entrance has a reduced-motion form");
    assert!(reduced.contains("mvFadeOnly"));
    let fade = collect_between(&css, "@keyframes mvFadeOnly {", "\n}")
        .into_iter()
        .next()
        .expect("styles.css defines a fade with no movement");
    assert!(
        !fade.contains("transform"),
        "the reduced form must not move"
    );
    assert!(css.contains("--ease-out: cubic-bezier(0.23, 1, 0.32, 1);"));
}

/// Every screen carries the name with the app's own icon at the top (founder,
/// session 54: *"all pages should have zaaheen icon on top"*): the welcome,
/// every setup step and the lock screen, in the shared header; the home
/// screen, in its top bar.
#[test]
fn every_screen_shows_the_name_with_its_icon() {
    let html = html();
    for screen in [
        "welcome",
        "signin",
        "location",
        "connect",
        "maintenance",
        "memory",
        "lock",
    ] {
        let section = html
            .split_once(&format!("id=\"screen-{screen}\""))
            .unwrap_or_else(|| panic!("index.html has no {screen} screen"))
            .1
            .split("id=\"screen-")
            .next()
            .unwrap_or("");
        assert!(
            section.contains("class=\"screen-brand\"")
                && section.contains("src=\"zaaheen-icon.png\""),
            "the {screen} screen has no name and icon at the top"
        );
    }
    let home = html
        .split_once("id=\"screen-home\"")
        .expect("index.html has a home screen")
        .1;
    assert!(
        home.contains("class=\"brand-icon\""),
        "the home screen's top bar has no icon"
    );
    assert!(
        !html.contains("class=\"orb\"") && !html.contains("class=\"dot\""),
        "the old black marks are back"
    );
}

/// §8.41: the setup's sign-in step and the signed-out lock screen both offer
/// "Create an account" and "Sign in"; both run the one sign-in, naming only
/// which page opens first, and neither passes an address.
#[test]
fn both_pages_are_offered_and_run_the_one_sign_in() {
    let html = html();
    let step = html
        .split_once("id=\"screen-signin\"")
        .expect("index.html has the sign-in step")
        .1
        .split("id=\"screen-")
        .next()
        .unwrap_or("");
    let lock = html
        .split_once("id=\"lock-signin\"")
        .expect("the lock screen has its sign-in actions")
        .1
        .split("id=\"lock-subscribe\"")
        .next()
        .unwrap_or("");
    for (place, section, up, inn) in [
        ("the setup's step", step, "signup-btn", "signin-btn"),
        (
            "the lock screen",
            lock,
            "lock-signup-btn",
            "lock-signin-btn",
        ),
    ] {
        assert!(
            section.contains(&format!("id=\"{up}\""))
                && section.contains(">Create an account</button>"),
            "{place} has no Create an account button"
        );
        assert!(
            section.contains(&format!("id=\"{inn}\"")) && section.contains(">Sign in</button>"),
            "{place} has no Sign in button"
        );
        // Same colour, same width (founder, session 55: "both in same
        // color like black .. this will keep them same size").
        for id in [up, inn] {
            assert!(
                section.contains(&format!("id=\"{id}\" class=\"btn-pill\"")),
                "{place}: {id} does not look like its partner"
            );
        }
    }
    let css = STYLES_CSS.replace("\r\n", "\n");
    let choices = collect_between(&css, ".signin-choices {", "}")
        .into_iter()
        .next()
        .expect("styles.css lays out the two sign-in buttons");
    assert!(
        choices.contains("grid-auto-columns: 1fr"),
        "the two sign-in buttons are not the same width: {choices}"
    );

    let code = js_code();
    for wiring in [
        "$(\"signin-btn\").addEventListener(\"click\", onSignIn)",
        "$(\"signup-btn\").addEventListener(\"click\", onSignUp)",
        "$(\"lock-signin-btn\").addEventListener(\"click\", onSignIn)",
        "$(\"lock-signup-btn\").addEventListener(\"click\", onSignUp)",
    ] {
        assert!(code.contains(wiring), "not wired: {wiring}");
    }
    let functions = top_level_functions(&code);
    assert!(js_function(&functions, "onSignIn")
        .body
        .contains("beginAccount(\"sign_in\")"));
    assert!(js_function(&functions, "onSignUp")
        .body
        .contains("beginAccount(\"sign_up\")"));
    let begin = js_function(&functions, "beginAccount");
    assert!(
        begin
            .body
            .contains("invoke(\"account_sign_in\", { entry })"),
        "the choice is the only thing sent"
    );
    assert_eq!(
        code.matches("invoke(\"account_sign_in\"").count(),
        1,
        "one place starts a sign-in"
    );
}

/// No screen shows a long dash (founder, session 54: *"remove the dashes
/// from all pages"*, so the app does not read as machine-written): not the
/// markup outside its comments, not any string the screen code can show, and
/// not the titles of the startup dialogs.
#[test]
fn no_screen_shows_a_long_dash() {
    let dashes = ['\u{2014}', '\u{2013}'];
    // The markup, without its `<!-- -->` notes.
    let mut shown = String::new();
    for (i, part) in html().split("<!--").enumerate() {
        let text = if i == 0 {
            part
        } else {
            part.split_once("-->").map_or("", |(_, rest)| rest)
        };
        shown.push_str(text);
    }
    for entity in ["&mdash;", "&ndash;"] {
        assert!(!shown.contains(entity), "index.html shows {entity}");
    }
    for (n, line) in shown.lines().enumerate() {
        assert!(
            !line.contains(&dashes[..]),
            "index.html shows a long dash (markup line {n}): {}",
            line.trim()
        );
    }
    // The screen code without its `//` notes; no `/* */` note may hold one either.
    for line in js_code().lines() {
        assert!(
            !line.contains(&dashes[..]),
            "app.js can show a long dash: {}",
            line.trim()
        );
    }
    for title in MAIN_RS
        .lines()
        .filter(|l| l.trim_start().starts_with("\"Zaaheen"))
    {
        assert!(
            !title.contains(&dashes[..]),
            "a startup dialog title: {}",
            title.trim()
        );
    }
}
