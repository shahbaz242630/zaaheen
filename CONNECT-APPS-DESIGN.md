# Zaaheen — connecting an AI app for the person

Live design file for ADR-106 and ADR-SEC-031 (session 55, 2026-09-22). Quote it; don't paraphrase it.

## ADR-106 + ADR-SEC-031 — "Connect it for me": the app's own install route, never its settings file

### What changed

**Founder, in the session-55 walk-through of the setup's "connect" step:** shown that it asked people to paste JSON into a file they could not find, and that nothing confirmed it worked, and offered the words now and the button before launch: *"lets do both partner, build the button now too .. we are getting ourself publsih ready .."*. The step's words became plain in the same session (founder: *"yes wording is good partner"*): "Connect your first AI app.", tiles described for people, and where each app keeps its setting.

### Round 1: writing the apps' settings files — withdrawn

The first design had Zaaheen write a `zaaheen` entry into `%APPDATA%\Claude\claude_desktop_config.json` and `%USERPROFILE%\.cursor\mcp.json`, keeping the rest of each file. An adversarial review (read-only, with public issue reports and docs) found **1 BLOCKER and 6 MAJOR**, most of them outside anything Zaaheen controls:
- **BLOCKER:** some Windows-package (MSIX) Claude versions read a per-package copy under `%LOCALAPPDATA%\Packages\Claude_…\LocalCache\Roaming\Claude\` even when `%APPDATA%\Claude` exists (anthropics/claude-code #25579, #26073, #38830), so the file written could be one Claude never reads, with "connected" shown. The founder's install (1.52386) is not virtualised, but one machine is one sample.
- **MAJOR:** Claude rewrites its own file and drops `mcpServers` on launch, sign-in or opening Cowork (#85005, #56296, #32345); Claude's own atomic save uses `claude_desktop_config.json.tmp`, the very name the repo's `write_atomic` would use; Cursor leaves 0-byte `mcp.json` files; serde errors quote values (secrets) into logs; entries under other names or duplicate keys; development builds writing `target\` paths into real settings.

Its NOTE 16 was the way out: **each app has its own install route**, in which the app asks the person and writes its own settings. That removes every file-writing finding at once. The founder agreed: *"yes lets go"*.

### Decision

1. **Cursor: its documented install link** (cursor.com/docs/context/mcp/install-links): `cursor://anysphere.cursor-deeplink/mcp/install?name=<name>&config=<base64 of the server object>`. Cursor's own example decodes to a bare `{"command":…,"args":…}` (checked in session 55), so ours is `name=zaaheen` and `config` = base64 of `{"command":"zaaheen","args":["mcp","serve"]}`, **the manual snippet's server exactly**, which the installer contract already pins (bare `zaaheen`, on `PATH`). No absolute path: that answers the review's development-path and reinstall findings too. Cursor shows its own install prompt; the person clicks Install.
2. **The link is one fixed constant** (`vault_app::external_link::CURSOR_INSTALL_LINK`), built from no input. It cannot pass `ExternalLink::checked` (`https` only), so it has its own narrower gate, `ExternalLink::install_in_cursor`: that exact text, scheme, host and path, or refused. Its base64 happens to need no escaping (no `+` or `/`); a test holds that, and that it decodes to the snippet's server.
3. **Only when Cursor is there.** Windows would otherwise offer to find an app for the link in the Store. `connect_cursor` first asks whether `HKCR\cursor\shell\open\command` exists (`reg.exe query`, no window, no admin; measured on the founder's machine: Cursor's installer registers it); if not, nothing is opened: `app_not_found`.
4. **One gated command, `connect_app(app)`**, `app` a closed serde enum (`cursor` | `claude_desktop`): no path, link or program crosses the IPC boundary (BRD §11.7.1). Gated like every setup action after sign-in; no widening of §8.26 §6.4. Answers `{ "outcome": "asked" | "saved" | "app_not_found" | "could_not_save" | "could_not_open" }`; a reason goes to the log only. The words, founder-approved (*"wording is good go ahead"*), are in `dist/app.js` `CONNECT_WORDS`.
5. **Claude Desktop: its desktop-extension route** (a `.mcpb` bundle; Claude shows its own install screen). The MCPB manifest spec (manifest 0.3) neither allows nor forbids a binary server whose command is the Zaaheen already installed rather than a copy inside the bundle, and a second copy of `zaaheen.exe` would fight the installed one over the keeper (the review's finding 6). So it was **tested live on the founder's machine** (session 55; founder on his own connection: *"claude desktop is already connected with zaaheen so if you want to first remove it .. thats your choice"*: left untouched, the tests took their own names). Two hand-made 700-byte bundles, each carrying only a manifest and a note, installed through Settings, Extensions, Advanced settings, Install Extension:
   - **A**, `"command": "C:\\Program Files\\Zaaheen\\zaaheen.exe"`; **B**, `"command": "zaaheen"`.
   - Claude's log for **both**: *"Using basic execution for extension …: server.type is "binary""*, *"Using MCP server command: C:\Program Files\Zaaheen\zaaheen.exe"* (B's short name found on `PATH`), *"Server started and connected successfully"*, then `initialize` and `tools/list` answered. A text file as `entry_point` was accepted.
   - **Short name chosen**, as the manual snippet and the Cursor link: it survives a reinstall elsewhere and never names a development path.
   - **The Windows-package (Store) Claude registers no `.mcpb` file type** (its `AppxManifest.xml` lists 27 document and image types, none `.mcpb`; `HKCR\.mcpb` absent) and **no documented `claude://` install link exists**, so a double-click cannot reach it. Hence the button's two paths: the extension is saved as **"Zaaheen for Claude.mcpb"** in the person's Downloads folder (found from Windows by Tauri, `download_dir`, never from the page; written to `<name>.part` then renamed over any older copy); where Windows opens `.mcpb` files (`HKCR\.mcpb` exists: Claude's direct download) it is opened and Claude asks; otherwise the page gives the three clicks in Claude's settings and a **Show the file** button (a second gated command, `show_claude_extension`, that opens the Downloads folder).
   - The bundle: `manifest.json` (display name "Zaaheen", the app's version, Windows only, the app's icon), `icon.png`, and the entry-point note. Stored, not compressed, with a fixed date, so the same icon makes the same bytes. Built with `zip` 6.0.0, already in the lockfile at that exact version (libduckdb-sys), no features.
   - Extensions live apart from `claude_desktop_config.json`, so the file-rewriting bugs of round 1 do not reach them.
   - Seen during the test, for the record: Claude restarts every server when an extension is installed, and our relay logs Claude's early close as `error: MCP transport bind failed: connection closed: initialize request` (HANDOFF §4's "a relay closed before `initialize` logs a normal event as an error").
6. **Confirming it worked** needs the keeper to know which apps are connected (HANDOFF §3 item 2, the Agents tab); that is the next piece.

### ADR-SEC-031 — asking another app to install Zaaheen

- **Input:** one closed enum. Nothing from the webview names a path, a link, or a program.
- **What is opened:** one fixed link, through `open::that` (never `open::with`, never the `insecure` feature), past a gate that accepts that text only. It names a program everyone's `PATH` already has from our installer, with two fixed arguments.
- **What Zaaheen touches:** nothing belonging to the other app. It reads registry keys (whether a `cursor:` or `claude:` link, or a `.mcpb` file, has a program), opens one fixed link, or writes one file of its own to Downloads and opens that file or that folder; the app asks the person and writes its own settings.
- **What is written:** "Zaaheen for Claude.mcpb", whose every byte is fixed by this build and its icon: a manifest naming `zaaheen mcp serve`, the icon and a note. No program.
- **Nothing to leak:** no other app's file is read, so no other app's secrets can reach a log, an error or the page.

### Tests, written first

vault-app: `external_link` — the fixed link passes its gate, survives parsing unchanged, carries `name=zaaheen` and a `config` that decodes to exactly the snippet's server, with no `+` or `/`; its debug line names where it points. `connect` — Cursor not there: nothing opened; there: exactly the fixed link handed to the opener; the opener failing: `could_not_open`. The Claude extension: runs `zaaheen mcp serve`, manifest 0.3, binary, Windows, the icon, three entries with forward slashes and no program, the same bytes every time, no stack name; Claude not there: nothing saved or opened; Store Claude: saved, not opened, no part-file left; `.mcpb` opens: the saved file handed to the opener; will not open: still saved; an older copy and a stray part-file replaced; no Downloads folder: `could_not_save`; every outcome's code; (Windows) an unregistered scheme or file type reads as not there. vault-tauri: `connect_app` accepts only the listed apps (not a link, a path or another name), both commands take nothing from the page but the app, both are gated (on `GATED_COMMANDS`, asking before they serve), have their permissions, and every answer has words on the page; the page sends only `{ app }`, shows the button for Claude Desktop and Cursor only, "Show the file" only after `saved`, and uses the approved words. Browser harness: both apps' flows.

### Implementation record (session 56, 2026-09-22)

The first build was clean under `-D warnings`. Two test defects, no code defect. `the_commands_take_nothing_but_the_app` split the arguments at every comma, including the one inside `State<'_, Entitlement>`; it now splits at top-level commas only. And `frontend_contract.rs` never read `commands/connect.rs` or `commands/startup.rs` (its `COMMAND_SOURCES` list), so their three commands were outside every wiring guard there; both are listed, and a new guard, `every_command_module_is_read_by_the_guards`, holds that list to `commands/mod.rs`. **Independent code review** (read-only, against the zip 6.0.0, tauri 2.11 and open 5.4.4 sources): nothing at BLOCKER, MAJOR or MINOR; the coarse registry checks and the part-file's check-then-write were noted below its bar as this design's residuals. **Planted bugs, each caught:** another scheme through the link gate (with the constant edited to match); another server in `config`; the extension opened where Windows cannot open it; saved when Claude is not there; the extension running another program; `connect_app` not asking the lock; "Show the file" taking a path from the page; a command module the wiring guards never read.


---

# Session 59 (2026-09-24): the live connection test and what it fixed

The founder's live test, every app from a clean slate on the sandbox installer: Cursor (install link) and Claude Desktop (the extension; chat AND Cowork) connected and read correctly; ChatGPT desktop (Work and Codex, one "Add MCP Server" entry) too; the updated Antigravity could not connect. Results and fix list: `C:\Projects\MemoryVault-artifacts\ui-harness\WALKTHROUGH-S59-LIST.md`.

**Guided steps for the apps with no install route** (session 59, in `dist/app.js` `AGENTS`): ChatGPT, its own Settings → Integrations → Plugins → Add MCP Server form, with the command and its two arguments shown apart (the founder's first try put the whole line in Arguments, saved as `args = ["zaaheen mcp serve"]`, which fails); Antigravity, its file by full path, `%USERPROFILE%\.gemini\config\mcp_config.json` (the IDE: `.gemini\antigravity`), because its own assistant named the wrong file (`settings.json`) and the app's program text says `mcp_config.json`; Claude Code, its documented `claude mcp add --transport stdio --scope user zaaheen -- zaaheen mcp serve` (code.claude.com/docs/en/mcp). ChatGPT's Chat mode cannot connect (internet servers only). No button for these: ADR-106 still holds (never write an app's settings file).

## ADR-SEC-032 (session 59, 2026-09-24): rmcp 2.2.0 → 3.4.1, for the 2026-07-28 MCP spec

**Found live.** Antigravity, updated on 2026-09-24, could not connect: "MCP transport bind failed:
expect initialized request, but received: Some(Request(JsonRpcRequest { ... id: Number(1), request:
CustomRequest(CustomRequest { method: "server/discover", params: Some(Object {}) ... })) : connection
closed: calling "initialize": client is closing: EOF". It speaks the 2026-07-28 spec: after
`initialize` it sends `server/discover` (SEP-2575, which servers MUST answer) with no
`notifications/initialized`, and rmcp 2.2.0 closed the connection. The other apps will move to that
spec too.

**Decision.** rmcp `=3.4.1` (workspace pin; ADR-SEC-021's note already said "3.x is the 2026-07-28
spec and needs its own live test"). Checked before the move: 3.x answers `server/discover` from the
same server info and no longer gates on `initialized` (rmcp `service/server.rs`); 3.2.0 keeps the
`initialize` handshake for every legacy client; no GitHub advisory affects 3.x (the four rmcp
advisories are all < 2.1.0); 3.4.1 still carries the #947 line-reader fix ADR-SEC-021 required; the
lockfile gains only base64 0.23, darling 0.24 and a syn version. `tokio-util =0.7.18` becomes a
direct dependency (already in the lockfile through rmcp) for `CancellationToken`.

**What changed with it (security-relevant).**
1. **The subscription gate lost its compile-time alarm.** rmcp 3 made `ClientRequest`
   `#[non_exhaustive]`, so §6.1's "a future rmcp variant fails to compile" is no longer possible:
   the compiler requires a catch-all. It fails CLOSED: `_ => Kind::Other` (asks the check, refused
   while locked). Every variant rmcp 3.4.1 has is still named; `server/discover` is Open like
   `initialize` (no user data; startup waits on it); `subscriptions/listen` and `tasks/update` are
   Other. `tests/entitlement_gate.rs` pins exactly one catch-all mapping to `Kind::Other`, every
   variant named, and the `=3.4.1` pin (a new variant can only arrive through an upgrade that
   re-reads the list).
2. **No task-style tool calls any more.** rmcp 2.2 let a client ask for one (the gate answered with
   rmcp's own invalid-params refusal). In rmcp 3 (SEP-2663) the SERVER creates tasks and ours never
   does, so `CallToolRequestParams.task` is gone; a locked call carrying a `task` field gets the
   ordinary locked words and still never reaches the server (test).
3. Mechanical: `ServerInfo` → `ServerConfig` (deprecated alias), `call_tool` returns
   `CallToolResponse`, a client's view of `server_info` is optional.

**Tests.** `vault-mcp/tests/new_protocol_handshake.rs` replays Antigravity's exact sequence against
the relay and the keeper's server, for protocol versions 2025-11-25 and 2026-07-28, and keeps the
old handshake working; `entitlement_gate.rs`: `server_discover_is_answered_even_while_locked_without_asking`,
`a_locked_call_carrying_a_task_field_is_refused_like_any_other`,
`the_gate_names_every_request_kind_and_fails_closed_on_new_ones`.

**`WIRE` 3 → 4** (`keeper/handshake.rs`): rmcp 3 carries the pipe and the tool descriptions changed (ADR-107), so a keeper and a relay from different installs refuse each other with "restart every app" instead of disagreeing silently; the contract hash is re-pinned (`the_tool_contract_is_pinned_to_the_wire_version`).

**One thing only the live test can settle.** rmcp 3 answers a `server/discover` whose `_meta` carries the 2026-07-28 fields (protocol version, client capabilities), and answers one without them with an invalid-params ERROR while keeping the session open. Antigravity's logged line showed `params: Some(Object {})`, but rmcp moves `_meta` out of the params before printing, so which it sent is unknown. Both cases are tested (`new_protocol_handshake.rs`); whether Antigravity then proceeds is for the live test.

**Live test still owed** (ADR-SEC-021's condition): the next installer, with Claude, Cursor,
ChatGPT and the updated Antigravity.

## ADR-107 (session 59): the read desk, cancelled calls dropped, the keeper started on connect

**Found live (2026-09-24, keeper log).** Cursor's model sent six `memory_read` at once; the reranker
takes one question at a time (~25-30 s each on the founder's laptop), so reads 2-6 waited 55-200 s
and came back "the vault took too long to answer" although the keeper answered every one. The relay
only stopped listening when its 55 s ran out, and the keeper went on answering reads nobody wanted,
so a new chat's single question queued behind them and failed too (08:05-08:07 UTC). And the first
question after a quiet spell started the keeper itself: 48.2 s of the 55 s, ~19 s of it loading the
engine. The AI apps stop waiting at 60 s, so the budget cannot grow.

**Decision.**
- **Cancel what nobody waits for.** The relay sends its call as a cancellable request and, when its
  deadline passes or the AI app cancels, sends MCP `notifications/cancelled` to the keeper.
- **One desk** (`vault-mcp/src/desk.rs`), shared by every connection a keeper serves: reads and
  searches take turns; waiting is an async wait, so a cancelled question leaves the line without a
  turn; from the median of the last three finished turns, a question that could not be answered
  within 50 s (`DESK_BUDGET`) hears `MSG_BUSY` at once ("ask again in a moment, one question at a
  time") instead of failing after a minute. It never says busy before a turn has finished.
- **Start the keeper when an app connects.** The relay's `initialize` records the app and starts
  finding (or starting) the keeper in the background, so the engine loads while the person types.
- **Ask agents for one question at a time** in the tool descriptions (the cross-platform lever).

## The Agents tab lists the apps actually connected (session 36's design, built session 59)
The relay introduces itself to the keeper under its AI app's own MCP `clientInfo` name; the keeper
keeps `<vault>/.vault-clients.json` (names and times only, atomic writes, removed on exit, in
`VAULT_ENTRIES`, operational not data); the desktop's `list_connected_apps` reads it only while the
discovery file names the same keeper. The tab and footer show connected apps with friendly names;
the daemon's access keys keep their own list, shown only when one exists.

# Session 64 (2026-09-26): the live test on a fresh install

## ADR-111 + ADR-SEC-031 amendment 1 — the apps run Zaaheen by its full path, not its short name

**Found live (session 64, fresh install of `s62-d4-sandbox`).** "Connect it for me" opened Cursor,
the founder clicked Install, and Cursor's log said *"'zaaheen' is not recognized as an internal or
external command"*. The installer adds its folder to the **user** `PATH`, but a program only reads
`PATH` when it starts. The app was started by the installer (before the change), and it opened
Cursor, so Cursor inherited the old `PATH`; any AI app already open before the install is in the
same position (ChatGPT and Antigravity both needed a restart in the same test). Claude worked only
because the founder had started Claude Desktop fresh from Start. So the short name, chosen in
session 55 (item 5, "Short name chosen"), fails on exactly the first connection, the one moment a
new customer judges the product. Founder: *"ko agreed with you"* (fix it properly; a "restart the
app" note only as the fallback).

**The installer lets the person choose the folder** (Tauri's WiX UI), so no fixed path is right
either.

**Decision.**
1. **One source of the command:** `vault_app::server_command::ServerCommand`. `installed()` is
   `zaaheen.exe` beside the running program (`install_paths::resource_dir()`, i.e.
   `current_exe().parent()`, the same resolution the keeper launcher and ADR-101 already use), when
   that file exists and its path is valid Unicode; otherwise the short name `zaaheen` (a developer
   build, or something unexpected: the old behaviour, never worse). The page never supplies it.
2. **Every route uses it:** the Cursor install link, the Claude extension's `mcp_config.command`
   (session 55 tested the full path in a bundle, variant A: it runs), and the copy-paste steps
   (JSON, TOML, the ChatGPT form, the Claude Code command), which the page fetches from a new
   read-only command, `server_command`, gated like `connect_app` (the same setup step), that
   returns only this text.
3. **The installer still adds its folder to `PATH`,** so a terminal user can type `zaaheen`.

**Runtime spike (session 64, founder's machine, before any code).** Cursor's docs say only
"base64 encode" and are silent on escaping. A link whose `config` was the base64 of
`{"command":"C:\Program Files\Zaaheen\zaaheen.exe","args":["mcp","serve"]}` with its `=`
padding percent-encoded (`%3D%3D`) was opened; the founder clicked Install; Cursor wrote
`"command": "C:\Program Files\Zaaheen\zaaheen.exe"` to `~/.cursor/mcp.json`. So Cursor
percent-decodes `config`, and the link is built with `url`'s query encoder (a base64 `+` would
otherwise read as a space; `/` and `=` are encoded too).

**ADR-SEC-031 amendment 1.** The Cursor link is no longer one fixed constant: it carries the
install path, which comes from the operating system (`current_exe`), never from the page, the
network or a file. Its gate, `ExternalLink::install_in_cursor(&ServerCommand)`, still pins the
scheme, host and path, exactly two query pairs (`name=zaaheen`, `config`), and a `config` that
decodes to exactly `{"command": <that command>, "args": ["mcp","serve"]}`. A `ServerCommand` can
only be the short name or an absolute path whose file name is `zaaheen.exe`, with no control
characters and not a network share or verbatim `\\?\` path (added after the independent review);
nothing else can be constructed. The Claude extension still carries no program.
Returning the install path to our own page reveals nothing the page could not already infer.

**Residual.** Reinstalling Zaaheen into a *different* folder leaves the old path in each app's
settings; the person connects again (the Agents tab's "Connect it for me" rewrites it). Rare, and
it fails visibly.

**Tests, written first.** `server_command`: the short name; an absolute `zaaheen.exe` path is
accepted; a relative path, another program (`cmd.exe`, `zaaheen.exe.bat`), an empty string and a
path with a newline are refused; `installed()` falls back to the short name where no sibling
exists (the test binary's folder). `external_link`: the link for the short name and for a full
path decodes to exactly that server; a path whose base64 contains `+` and `/` survives parsing
unchanged; the gate refuses a link rebuilt with another host or a third pair. `connect`: Cursor is
handed the link for the command given; the Claude extension's `mcp_config.command` is the command
given. `vault-tauri`: `server_command` is registered and classified gated; the page builds every
snippet from the fetched command and falls back to the short name.

**Independent review (session 64, read-only):** SAFE TO COMMIT; no BLOCKER or MAJOR. MINOR: this
record said the command was "ungated" while the code gates it (the record corrected). Noted below
its bar and fixed anyway: `parse` accepted a UNC or verbatim `\\?\` path (unreachable, the source
is `current_exe`), now refused, with tests.
