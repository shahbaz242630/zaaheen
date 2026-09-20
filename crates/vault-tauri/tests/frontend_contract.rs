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
];

/// The markup, for guards that pin UI structure rather than wiring.
const INDEX_HTML: &str = include_str!("../dist/index.html");

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
