// Zaaheen — V0.2 beta UI logic ("Quiet" direction).
// Vanilla JS, no framework, no eval, no remote resources (CSP: 'self').
// All user/vault content rendered through esc() — BRD §11.12 vault-tauri
// checklist (XSS prevention in webview).

// Guard the bridge so the UI still renders (with failing commands) when
// opened outside Tauri — e.g. design review in a plain browser.
const rawInvoke = window.__TAURI__ && window.__TAURI__.core
  ? window.__TAURI__.core.invoke
  : async () => { throw new Error("vault engine not connected (running outside Tauri)"); };

// Every gated command can reject with one of the `locked_*` codes, and there
// are ~15 places that render a caught error straight into the page. Rather
// than teach each of them about entitlement, handle it once here:
//
// * a lock code sends the person to the lock screen, whichever button they
//   pressed — so a subscription that lapses while the app is open reaches
//   the lock screen on the next action (SIGNIN-DESIGN.md §8.38) — and the
//   call site still gets a plain line to show;
// * an account or export code becomes its plain line;
// * anything else is re-thrown untouched.
//
// Lines are thrown as strings, the way Tauri itself rejects, so a call site's
// `${err}` reads the line rather than "Error: …".
async function invoke(...args) {
  try {
    return await rawInvoke(...args);
  } catch (err) {
    const raw = String(err && err.message ? err.message : err);
    const lockLine = friendlyLockError(raw);
    if (lockLine !== null) {
      showLock(raw);
      throw lockLine;
    }
    const line = friendlyAccountError(raw);
    throw line === null ? err : line;
  }
}

// Same guard for the event bridge. Used only for first-run acquisition
// progress; outside Tauri this is a no-op so the UI still renders.
const listenEvent = window.__TAURI__ && window.__TAURI__.event
  ? window.__TAURI__.event.listen
  : async () => () => {};

// ---------------------------------------------------------------- utilities

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

function $(id) { return document.getElementById(id); }

function relTime(iso) {
  const t = typeof iso === "number" ? iso : Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 604800) return `${Math.floor(s / 86400)}d ago`;
  return new Date(t).toLocaleDateString();
}

const store = {
  get(key, fallback) {
    try {
      const v = localStorage.getItem(key);
      return v === null ? fallback : JSON.parse(v);
    } catch { return fallback; }
  },
  set(key, val) {
    try { localStorage.setItem(key, JSON.stringify(val)); } catch { /* non-fatal */ }
  },
};

async function copyText(text, btn) {
  let ok = false;
  try {
    await navigator.clipboard.writeText(text);
    ok = true;
  } catch {
    // WebView2 fallback: hidden textarea + execCommand.
    try {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      ok = document.execCommand("copy");
      ta.remove();
    } catch { ok = false; }
  }
  if (btn) {
    btn.textContent = ok ? "copied ✓" : "select + copy manually";
    setTimeout(() => { btn.textContent = "copy"; }, 1600);
  }
}

// -- confirmation ------------------------------------------------------------

// Ask the user to confirm a destructive action; resolves true only if they
// actively choose to proceed.
//
// Why this exists: the webview renders the native `window.confirm` dialog as
// an OK-only message box — there is no Cancel, so the user CANNOT decline and
// the action goes ahead whatever they click. Found in live verification
// 2026-07-20 on both `forget` (permanent delete) and `revoke`. A gate that
// cannot say no is not a gate. `tests/frontend_contract.rs` now fails the
// build if a native gating dialog is reintroduced anywhere in this file.
//
// Cancel is the visually dominant button and takes initial focus, so Enter
// and Esc both mean "don't". Text is set via textContent, never innerHTML —
// the agent name interpolated into the revoke prompt is untrusted input.
function confirmAction({ title, body, confirmLabel }) {
  return new Promise((resolve) => {
    const overlay = $("confirm-overlay");
    const go = $("confirm-go");
    const cancel = $("confirm-cancel");

    $("confirm-title").textContent = title;
    $("confirm-body").textContent = body;
    go.textContent = confirmLabel;
    overlay.classList.remove("hidden");
    cancel.focus();

    function settle(answer) {
      overlay.classList.add("hidden");
      go.removeEventListener("click", onGo);
      cancel.removeEventListener("click", onCancel);
      overlay.removeEventListener("mousedown", onBackdrop);
      document.removeEventListener("keydown", onKey);
      resolve(answer);
    }
    function onGo() { settle(true); }
    function onCancel() { settle(false); }
    // Backdrop click dismisses — but only when the press STARTED on the
    // backdrop, so a drag that ends outside the box doesn't cancel.
    function onBackdrop(e) { if (e.target === overlay) settle(false); }
    function onKey(e) {
      if (e.key === "Escape") { e.preventDefault(); settle(false); }
    }

    go.addEventListener("click", onGo);
    cancel.addEventListener("click", onCancel);
    overlay.addEventListener("mousedown", onBackdrop);
    document.addEventListener("keydown", onKey);
  });
}

// ---------------------------------------------------------------- app state

// Every fact shown in onboarding is real: the engine performed these steps
// during Tauri setup() before this webview loaded (main.rs steps 1-7) — the
// welcome animation replays them, it does not fake them.
const CHECK_DEFS = [
  {
    phases: ["locating vault store", "deriving key from Credential Manager", "unsealing store with AES-256"],
    done: "vault unsealed with AES-256, key in Windows Credential Manager",
  },
  // White-label rule (founder, 2026-07-11): never name the underlying
  // models or stack in the UI — the user-facing promise is "on-device".
  {
    phases: ["waking the recall engine", "loading on-device intelligence", "indexing your memory space"],
    done: "recall engine ready, runs entirely on this device",
  },
  {
    phases: ["attaching audit log", "opening default boundary"],
    done: "audit log active, every read and write recorded",
  },
];
const SPIN = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const CHECK_MS = 2800; // per-check animation duration — one line completes fully before the next appears

// ------------------------------------------------- first-run acquisition
//
// On a fresh install the recall engine's files are not on disk yet. They are
// fetched in the background while the user works through onboarding (founder
// decision 2026-07-20: the download OVERLAPS setup rather than blocking it),
// and onboarding completion GATES on the fetch so nobody reaches search
// before the engine is fully prepared.
//
// Until it completes, search still works — it just ranks less sharply
// (ADR-089). Nothing here is load-bearing for correctness; it is about not
// handing someone a half-prepared engine as their first impression.
//
// White-label (ADR-086): "recall engine", never a model or runtime name.
const engineFetch = {
  percent: 0,
  active: false,   // a transfer has actually reported bytes
  done: false,
  failed: false,
  waiting: false,  // onboarding is currently blocked on the fetch
  tick: 0,
  promise: null,
};

// Kick off (or join) acquisition. Idempotent on both sides: the Rust command
// shares one transfer between concurrent callers, and `promise` keeps the JS
// side from stacking invocations. On failure `promise` is cleared so a retry
// genuinely re-attempts instead of re-awaiting the settled rejection.
function startEngineFetch() {
  if (engineFetch.done) return Promise.resolve();
  if (engineFetch.promise) return engineFetch.promise;
  engineFetch.failed = false;
  engineFetch.promise = invoke("ensure_recall_engine")
    .then(() => {
      engineFetch.done = true;
      engineFetch.percent = 100;
      renderEngineRow();
    })
    .catch((err) => {
      engineFetch.failed = true;
      engineFetch.promise = null;
      renderEngineRow();
      throw err;
    });
  return engineFetch.promise;
}

// ── Engine readiness (ADR-090) ───────────────────────────────────────────
//
// Acquisition (bytes on disk) and readiness (model loaded and serving) are
// DIFFERENT things, and conflating them is what made the first search after a
// download take ~24 s with no explanation. `ensure_recall_engine` finishing
// means the files arrived; the model still has to be read and prepared.
//
// The founder's call (2026-07-22) was: open the window instantly and say
// honestly that the engine is still getting ready — rather than gate the
// window for ~40 s on every launch. Recall keeps working throughout, on the
// retriever's own order (ADR-089).
//
// `preparing` is the ONLY state that represents a wait that ends. `unavailable`
// (files not fetched) and `not_configured` (no engine at all) must never show
// a spinner: promising a wait that will not end is exactly the dishonesty this
// is meant to avoid.
const engineReady = {
  state: "preparing", // pessimistic until the backend says otherwise
  polling: false,
};

function engineIsPreparing() {
  return engineReady.state === "preparing";
}

async function refreshEngineState() {
  try {
    engineReady.state = await invoke("recall_engine_state");
  } catch {
    // A failed status read must not invent a wait. Treat it as "nothing to
    // report" so the UI stays quiet rather than spinning forever.
    engineReady.state = "unavailable";
  }
  renderEngineReady();
  return engineReady.state;
}

// Poll only while there is a genuine wait in progress. Stops on `ready` and on
// any terminal state, so an idle app is not calling into Rust forever.
function pollEngineReady() {
  if (engineReady.polling) return;
  engineReady.polling = true;
  const tick = async () => {
    const s = await refreshEngineState();
    if (s === "preparing") {
      setTimeout(tick, 1500);
    } else {
      engineReady.polling = false;
    }
  };
  tick();
}

// Ask the backend to start preparing the engine, then watch for it to finish.
// Called after acquisition completes — the case `main.rs`'s startup warm-up
// cannot cover, because on a real first run the files are still downloading
// when the app boots.
function warmEngine() {
  return invoke("warm_recall_engine")
    .catch(() => false)
    .then(() => pollEngineReady());
}

// ------------------------------------------------- maintenance engine (Phi-4)
//
// "Automatic maintenance" (the nightly consolidator) runs on a second, larger
// on-device model. Per the founder decision (2026-07-24) it downloads for
// EVERY user during onboarding so the real product is one toggle away — but,
// unlike the recall engine, it NEVER gates onboarding: recall does not need it,
// only maintenance does, so it streams quietly in the background.
//
// White-label (ADR-086): "consolidation engine", never a model name. The
// user-facing surface is named "Consolidation"; the internal keys, command
// names, and event names stay `maintenance*` (renaming those buys nothing and
// would churn the Rust contract in tests/frontend_contract.rs).
const maintFetch = { percent: 0, active: false, done: false, failed: false, promise: null };
const maintState = { view: null }; // last ScheduleView from get_maintenance_schedule

function startMaintenanceFetch() {
  if (maintFetch.done) return Promise.resolve();
  if (maintFetch.promise) return maintFetch.promise;
  maintFetch.failed = false;
  maintFetch.promise = invoke("ensure_maintenance_engine")
    .then(() => {
      maintFetch.done = true;
      maintFetch.percent = 100;
      renderMaintEngineStatus();
      // Engine is now ready — a launch-time catch-up that deferred because the
      // download was still in flight can proceed.
      catchUpMaintenanceIfDue();
    })
    .catch(() => {
      // Non-gating: a failed fetch must never block the user. Surfaced quietly
      // on the Maintenance tab; retried on the next launch.
      maintFetch.failed = true;
      maintFetch.promise = null;
      renderMaintEngineStatus();
    });
  return maintFetch.promise;
}

// Is the maintenance engine present and ready to run a manual pass?
function maintEngineReady() {
  return maintFetch.done || !!(maintState.view && maintState.view.engine_ready);
}

// Update whichever maintenance surface is visible with the engine's download
// state. Called on progress events and on screen/tab entry.
function renderMaintEngineStatus() {
  if (state.screen === "maintenance") {
    const note = $("maint-onboard-note");
    if (note && !maintFetch.done && !note.textContent.startsWith("Finishing")) {
      note.textContent = maintFetch.failed
        ? "The consolidation engine will finish downloading later, and you can still turn this on."
        : (maintFetch.active
            ? `Preparing the consolidation engine in the background (${maintFetch.percent}%). You can finish now either way.`
            : "");
    }
  }
  if (state.screen === "home" && state.tab === "maintenance") {
    const note = $("maint-engine-note");
    if (note) {
      if (maintEngineReady()) {
        note.classList.add("hidden");
      } else {
        note.classList.remove("hidden");
        if (maintFetch.failed) {
          note.textContent = "The consolidation engine didn't finish downloading. It will retry when you turn consolidation on, or on the next scheduled run.";
        } else if (maintFetch.active) {
          note.textContent = `Preparing the consolidation engine (${maintFetch.percent}%). Step 3 becomes available once it is ready.`;
        } else {
          note.textContent = "Consolidation uses a one-time on-device engine (~2.5 GB). It downloads when you turn consolidation on.";
        }
      }
    }
    const runBtn = $("maint-run");
    if (runBtn) runBtn.classList.toggle("disabled", !maintEngineReady() || state.maintRunning);
  }
}

// MCP connection snippets run `zaaheen`, the binary the installer actually
// lays down (ADR-SEC-018), invoked with no paths because ADR-101 taught it to
// find its own vault and models. This pairing is pinned by
// installer_contract.rs: a comment claiming the snippet works is worth nothing,
// which is exactly how `vault-cli` survived the rename here and would have sent
// every tester a config for a program that does not exist.
//
// ADR-111 (session 64): by its FULL path, which `server_command` reads from
// Windows. An AI app open before the install never sees the new PATH, so the
// short name failed on the first connection ("'zaaheen' is not recognized").
// The short name stays the fallback until the answer arrives, or if it can't.
const SHORT_NAME = "zaaheen";
// JSON.stringify quotes the path for JSON and, identically, for a TOML
// basic string (backslashes doubled).
const SNIPPET_JSON = (command) => `{
  "mcpServers": {
    "zaaheen": {
      "command": ${JSON.stringify(command)},
      "args": ["mcp", "serve"]
    }
  }
}`;
const SNIPPET_TOML = (command) => `[mcp_servers.zaaheen]
command = ${JSON.stringify(command)}
args = ["mcp", "serve"]`;

// Plain words for people, not developers (founder's walk-through, session 55):
// "AI app", as the welcome says, and where each app keeps the setting.
// Session 59, from the live test of every app: ChatGPT and Antigravity have
// no install route, so their steps name the exact screen or file, and
// ChatGPT's form keeps the command and its two arguments apart (the whole
// line typed into "Arguments" was the founder's first attempt, and it fails).
const SNIPPET_FORM = (command) => `Name:       Zaaheen
Command:    ${command}
Arguments:  mcp
            serve`;
// As documented at code.claude.com/docs/en/mcp: user scope, so every project
// has it; everything after `--` is the server's own command line. A full path
// has a space ("Program Files"), so it is quoted for the terminal.
const SNIPPET_CLAUDE_CODE = (command) =>
  `claude mcp add --transport stdio --scope user zaaheen -- ${command === SHORT_NAME ? command : `"${command}"`} mcp serve`;
const AGENTS = [
  { name: "Claude Desktop", desc: "The Claude app for your computer, in Chat and Cowork", hint: "In Claude, open Settings, then Developer, then Edit Config. Add this to the file it shows you, save it, then quit Claude and open it again:", snippet: SNIPPET_JSON, connect: "claude_desktop" },
  { name: "Cursor", desc: "AI code editor", hint: "Add this to Cursor's settings file, mcp.json, in the .cursor folder in your home folder. Save it, then restart Cursor:", snippet: SNIPPET_JSON, connect: "cursor" },
  { name: "ChatGPT", desc: "The ChatGPT app for your computer, in Work and Codex", hint: "In ChatGPT, open Settings, then Integrations, then Plugins. Choose Add, then Add MCP Server, and fill it in as below. Put mcp and serve in Arguments as two separate items. Save it, then use Zaaheen in ChatGPT's Work or Codex mode (its Chat mode can't connect to apps on your computer):", snippet: SNIPPET_FORM },
  // Broken into steps, each line to type in its own copy box (founder,
  // session 64: "break it down .. and make type notepad commands copy able").
  { name: "Antigravity", desc: "Google's AI app and code editor", hint: "First open its settings file in Notepad: press Windows and R together, paste the line for your Antigravity, and press Enter.", openers: [
    { label: "Antigravity app", command: "notepad %USERPROFILE%\\.gemini\\config\\mcp_config.json" },
    { label: "Antigravity IDE", command: "notepad %USERPROFILE%\\.gemini\\antigravity\\mcp_config.json" },
  ], pasteHint: "Then paste this in. If the file already lists other apps, add the zaaheen part next to them. Save it, then close Antigravity fully and open it again:", snippet: SNIPPET_JSON },
  { name: "Claude Code", desc: "Claude in your terminal", hint: "In a terminal, run this once. Zaaheen is then available in every Claude Code project:", snippet: SNIPPET_CLAUDE_CODE },
  { name: "Codex", desc: "OpenAI's coding assistant in your terminal", hint: "Add this to Codex's settings file, config.toml, in the .codex folder in your home folder. Save it, then restart Codex:", snippet: SNIPPET_TOML },
  { name: "Another app", desc: "Any AI app that can connect to Zaaheen", hint: "Give your app this setting:", snippet: SNIPPET_JSON },
];

// How to see it worked, in each app's own screens (session 64's live test;
// Antigravity's in the founder's words: "go to settings .. customization
// under token usage mcp tools click on show breakdown .. see zaaheen shows
// up and has green circle").
const AGENT_CHECKS = {
  "Claude Desktop": "In Claude, open Settings, then Extensions. Zaaheen is listed there and switched on.",
  "Cursor": "In Cursor, open Settings, then MCP (Tools & MCP in newer versions). Zaaheen has a green dot.",
  "ChatGPT": "In ChatGPT, open Settings, then Integrations, then Plugins. Zaaheen is listed there.",
  "Antigravity": "In Antigravity, open Settings, then Customization. Under Token usage, next to MCP tools, click Show breakdown. Zaaheen is listed with a green circle.",
  "Claude Code": "In a terminal, run claude mcp list. Zaaheen shows as connected.",
  "Codex": "In a terminal, run codex mcp list. Zaaheen is listed.",
  "Another app": "Your app's list of tools or MCP servers shows Zaaheen.",
};

// Session 64: Claude and ChatGPT look in their own memory first and may stop
// there. One line in their own instructions makes them check Zaaheen too.
const TIP_LINE = "Before answering anything about me, my preferences, my work or my plans, also check my Zaaheen memory, even when your own memory has nothing.";
const AGENT_TIPS = {
  "Claude Desktop": "Claude looks in its own memory first. In Claude, open Settings, then Profile, and add this line to your personal preferences:",
  "ChatGPT": "ChatGPT looks in its own memory first. In ChatGPT, open Settings, then Personalization, then Custom instructions, and add this line:",
};

// "Connect it for me" (ADR-106): Zaaheen asks the app through its own
// install route and never writes the app's settings, so the app asks the
// person. One entry per `connect` value above; one line per answer of
// `connect_app`.
const CONNECT_WORDS = {
  claude_desktop: {
    note: "Zaaheen saves a small Claude extension, and Claude asks you to install it.",
    asked: "Claude should now be asking you to install Zaaheen. Click Install, and it's connected.",
    saved: "Zaaheen saved the extension to your Downloads folder. In Claude, open Settings, then Extensions, then Advanced settings, then Install Extension, and choose \"Zaaheen for Claude.mcpb\".",
    app_not_found: "Zaaheen couldn't find Claude on this computer. If it's installed, you can add the setting yourself below.",
    could_not_save: "Zaaheen couldn't save the Claude extension. You can add the setting yourself below.",
    could_not_open: "Zaaheen couldn't open Claude. You can add the setting yourself below.",
  },
  cursor: {
    note: "Cursor will ask you to install Zaaheen. Click Install there.",
    asked: "Cursor should now be asking you to install Zaaheen. Click Install, and it's connected.",
    app_not_found: "Zaaheen couldn't find Cursor on this computer. If it's installed, you can add the setting yourself below.",
    could_not_open: "Zaaheen couldn't open Cursor. You can add the setting yourself below.",
  },
};

// ------------------------------------------------------ account and the lock
//
// Sign-in, the lock screen, the account panel and the banners (S3 step 4d-2,
// SIGNIN-DESIGN.md §8.38 and §8.39). The Rust side decides and this side
// shows: every gated command asks the lock itself, so the worst a mistake in
// this file can do is show the wrong screen, never open a locked vault.

// Onboarding. Sign-in comes straight after the welcome, before anything
// gated: the recall-engine download and the first memory both need the lock
// to say yes. Where the memories live comes next (ADR-105 L-e): choosing a
// folder is gated too. A build with no sign-in starts at that step.
const ONBOARDING = ["welcome", "location", "connect", "memory", "maintenance"];
const ONBOARDING_WITH_SIGN_IN = ["welcome", "signin", "location", "connect", "memory", "maintenance"];

// Set just before a move asked for during setup: Zaaheen restarts to make
// it, and the setup carries on from the location step afterwards, saying
// what happened. Holds which of the two step lists the setup was on.
const RESUME_KEY = "mv_resume_location";

// §8.26 §6: the trial banner shows from day 23 of 30.
const TRIAL_BANNER_DAYS = 7;
// §8.26 §4: after a checkout, refresh every 10 s for 10 min.
const CHECKOUT_POLL_MS = 10 * 1000;
const CHECKOUT_POLL_FOR_MS = 10 * 60 * 1000;
// A returning computer shows nothing until the lock answers, so a locked one
// never flashes its memories. The answer is a file read when all is well; a
// refusal refreshes first, which can take a few seconds (§8.26 §4), so after
// this long the lock screen says what it is doing.
const CHECKING_AFTER_MS = 400;

const account = {
  signIn: false,       // does this build have sign-in at all (account_access)
  locked: null,        // the last lock code, or null while the lock is open
  view: null,          // the last account view (account_status and friends)
  ready: null,         // the first answer at open, for "Begin set up"
  signingIn: false,
  subscribing: false,
  checkingPaid: false,
  checkout: null,      // { startState } while waiting for a payment to arrive
  lockCode: null,      // what the lock screen is showing, and why
  lockVariant: null,
};

// Which lock screen each refusal shows. The code comes from the lock itself —
// account_access, or a gated command's refusal — and nothing here infers it.
const LOCK_VARIANT = {
  locked_signed_out: "signed_out",
  locked_trial_ended: "trial_ended",
  locked_subscription_ended: "subscription_ended",
  locked_cannot_confirm: "cannot_confirm",
  locked_unlocking: "unlocking",
};

// The lock screen's words, founder-approved 2026-09-21 (§8.39). `action`
// picks the buttons; `who` adds "Signed in as …" where somebody may have
// paid under another account.
const LOCK_COPY = {
  checking: {
    heading: "",
    text: "Checking your subscription…",
    action: null,
  },
  signed_out: {
    heading: "Sign in to open your memories.",
    // "on the next page" dropped with the Create an account button (§8.41).
    text: "Your memories are on this computer, encrypted, just as you left them. Sign in to your Zaaheen account to open them. New to Zaaheen? Create an account, and your 30-day free trial starts straight away. No card is needed.",
    action: "sign_in",
  },
  trial_ended: {
    heading: "Your free trial has ended.",
    text: "Subscribe to keep using your memories. Nothing has been deleted. Everything you kept is still here on this computer.",
    action: "subscribe",
    who: true,
  },
  subscription_ended: {
    heading: "Your subscription has ended.",
    text: "Subscribe again to keep using your memories. Nothing has been deleted. Everything you kept is still here on this computer.",
    action: "subscribe",
    who: true,
  },
  cannot_confirm: {
    heading: "We couldn't confirm your subscription.",
    text: "Zaaheen couldn't reach us to check your subscription. Check your internet connection, then try again. Only your subscription is checked, never your memories.",
    action: "retry",
  },
  reopen: {
    heading: "Zaaheen couldn't open your account on this computer.",
    text: "Closing Zaaheen and opening it again usually fixes this. If it keeps happening, email us at the address below.",
    action: "close",
  },
  // Reachable only from a lock-mode keeper, never from this app's own check.
  // The words are §8.26 §6's.
  unlocking: {
    heading: "Zaaheen is unlocking.",
    text: "Try again in a moment.",
    action: "retry",
  },
};

// The lock screen's passing lines, approved with the rest (§8.39).
const LOCK_LINES = {
  signingIn: "Finish signing in in your browser, then come back here.",
  signingUp: "Finish creating your account in your browser, then come back here.",
  notPaidYet: "We haven't seen your payment yet. It can take a minute, so try again shortly.",
  stillOffline: "Still couldn't connect. Check your internet connection and try again in a moment.",
};

const state = {
  // "boot": a returning computer waiting for the lock's first answer, with
  // nothing on screen yet (see CHECKING_AFTER_MS) — and a setup resuming
  // after the restart a move needs, which goes on at the location step.
  screen: store.get("mv_onboarded", false) || store.get(RESUME_KEY, null) ? "boot" : "welcome",
  // ONBOARDING_WITH_SIGN_IN on a signed-out computer.
  onboardSteps: store.get(RESUME_KEY, null) === "with_sign_in" ? ONBOARDING_WITH_SIGN_IN : ONBOARDING,
  checksDone: 0,
  agentPicked: null,
  // What the copy-paste steps run (ADR-111): the short name until
  // `server_command` answers with the full path.
  serverCommand: SHORT_NAME,
  serverCommandAsked: false,
  memType: "semantic",
  addType: "semantic",
  tab: "memories",
  query: "",
  // `agents` is the local record of which app the user picked during
  // onboarding — it drives the connect UI only. What the tab and the footer
  // show is `connectedApps`: the keeper's live list (session 59).
  // `grantedAgents` is the HTTP daemon's access keys (slice 2).
  agents: store.get("mv_agents", []),
  connectedApps: [],                         // from list_connected_apps
  grantedAgents: [],                         // from list_agents (access keys)
  boundaries: [],                            // from list_boundaries
  showInlineAdd: false,
  showBoundaryAdd: false,
  toastTimer: null,
  searchSeq: 0,
  recentSeq: 0,
  keptFirstMemory: false,                    // did they save a first memory? (for the finish toast)
  maintRunning: false,                       // a manual "Run now" is in flight
};

// ---------------------------------------------------------------- screens

function showScreen(name) {
  state.screen = name;
  if (name === "connect") placeConnectPicker("setup-connect-host");
  for (const s of ["welcome", "signin", "location", "connect", "memory", "maintenance", "home", "lock", "moving"]) {
    $(`screen-${s}`).classList.toggle("hidden", s !== name);
  }
  // The dots and the "STEP n OF m" kickers follow this setup's own steps:
  // six with the sign-in step, five without.
  const steps = state.onboardSteps;
  const idx = steps.indexOf(name);
  const dots = $("progress-dots");
  dots.classList.toggle("hidden", idx === -1);
  if (idx !== -1) {
    if (dots.children.length !== steps.length) {
      dots.replaceChildren(...steps.map(() => document.createElement("span")));
    }
    [...dots.children].forEach((el, i) => el.classList.toggle("on", i <= idx));
    const kicker = $(`screen-${name}`).querySelector(".kicker[data-step]");
    if (kicker) kicker.textContent = `STEP ${idx + 1} OF ${steps.length}`;
  }
  if (name === "welcome") runChecks();
  if (name === "location") renderLocationStep();
  if (name === "maintenance") renderMaintEngineStatus();
  if (name === "home") renderHome();
  if (name === "lock") renderLockExport();
}

// -- welcome boot checks ----------------------------------------------------

// One row at a time (founder feedback 2026-07-11): a line appears, spins
// through its phases, resolves to [✓] — only then does the next line
// appear. Rows are created once and updated in place so their entrance
// fade never restarts.
let checkTimers = [];
function runChecks() {
  checkTimers.forEach(clearTimeout);
  checkTimers = [];
  state.checksDone = 0;
  const box = $("checks");
  box.innerHTML = "";
  renderBeginState();

  const rows = [];
  let frame = 0;
  let rowStartFrame = 0;

  function addRow() {
    const row = document.createElement("div");
    row.className = "check-item active";
    row.innerHTML = '<span class="ico"></span><span class="lbl"></span>';
    box.appendChild(row);
    rows.push(row);
    rowStartFrame = frame;
    updateActiveRow();
  }

  function updateActiveRow() {
    const i = state.checksDone;
    if (i >= CHECK_DEFS.length) return;
    const def = CHECK_DEFS[i];
    const row = rows[i];
    if (!row) return; // spinner tick before this row's entrance delay
    const local = frame - rowStartFrame;
    const framesPerPhase = Math.max(2, Math.floor(CHECK_MS / 90 / def.phases.length));
    const phase = def.phases[Math.min(def.phases.length - 1, Math.floor(local / framesPerPhase))];
    const dots = ".".repeat(1 + (Math.floor(local / 4) % 3));
    row.querySelector(".ico").textContent = ` ${SPIN[local % SPIN.length]} `;
    row.querySelector(".lbl").textContent = phase + dots;
  }

  function markDone(i) {
    const row = rows[i];
    row.classList.remove("active");
    row.classList.add("done");
    row.querySelector(".ico").textContent = "[✓]";
    row.querySelector(".lbl").textContent = CHECK_DEFS[i].done;
  }

  const spinner = setInterval(() => {
    if (state.screen !== "welcome" || state.checksDone >= CHECK_DEFS.length) {
      clearInterval(spinner);
      return;
    }
    frame += 1;
    updateActiveRow();
  }, 90);
  checkTimers.push(spinner);

  checkTimers.push(setTimeout(addRow, 400));
  for (let i = 1; i <= CHECK_DEFS.length; i++) {
    checkTimers.push(setTimeout(() => {
      markDone(i - 1);
      state.checksDone = i;
      if (i < CHECK_DEFS.length) {
        addRow();
      } else {
        renderBeginState();
      }
    }, 400 + CHECK_MS * i));
  }
}

// A fourth boot-check row, rendered ONLY while a transfer is genuinely in
// flight. Honest UI (ADR-086): when the files are already present nothing
// appears at all — the app never claims work it did not do. That is also why
// this row is created on the first progress event rather than up front.
function renderEngineRow() {
  const host = $("checks");
  if (!host) return;
  let row = $("check-engine");

  if (engineFetch.done || (!engineFetch.active && !engineFetch.failed)) {
    if (row) row.remove();
    return;
  }

  if (!row) {
    row = document.createElement("div");
    row.id = "check-engine";
    row.className = "check-item active";
    const ico = document.createElement("span");
    ico.className = "ico";
    const lbl = document.createElement("span");
    lbl.className = "lbl";
    row.append(ico, lbl);
    host.appendChild(row);
  }

  const ico = row.querySelector(".ico");
  const lbl = row.querySelector(".lbl");
  if (engineFetch.failed) {
    row.className = "check-item done";
    ico.textContent = "[!]";
    lbl.textContent = "couldn't finish preparing the recall engine, you can retry in a moment";
    return;
  }
  row.className = "check-item active";
  ico.textContent = SPIN[engineFetch.tick % SPIN.length];
  lbl.textContent = `preparing recall engine, ${engineFetch.percent}%`;
}

// The honest "still getting ready" line on the home screen (ADR-090).
//
// Shown ONLY while `preparing` — a finite wait that resolves on its own. Never
// shown for `unavailable` or `not_configured`, because there is no wait to
// report there and claiming one would be a lie the UI never resolves.
//
// White-label (ADR-086): "recall engine", no model or runtime named.
function renderEngineReady() {
  const note = $("engine-note");
  if (!note) return;
  const preparing = engineIsPreparing();
  note.classList.toggle("hidden", !preparing);
  if (preparing) {
    note.textContent =
      "Getting your recall engine ready. You can search now, and results sharpen up in a moment.";
  }
}

function renderBeginState() {
  const ready = state.checksDone >= CHECK_DEFS.length;
  // The Begin button stays invisible (space reserved) until every check
  // has finished, then fades in (founder feedback 2026-07-11).
  $("begin-btn").classList.toggle("reveal", ready);
  // No step count here (founder, session 55): the count depends on whether
  // this computer needs the sign-in step, known only after Begin.
  const signingIn = account.locked === "locked_signed_out";
  $("begin-hint").textContent = ready
    ? (signingIn ? "A few short steps, about three minutes." : "A few short steps, about two minutes.")
    : "Establishing your vault on this device…";
}

// "Begin set up". A computer nobody has signed in on goes to the sign-in
// step first; one whose lock is shut for another reason (a trial that ended
// on an earlier install, no internet) goes to the lock screen, and on into
// setup once it opens.
async function beginSetup() {
  if (state.checksDone < CHECK_DEFS.length) return;
  await account.ready;
  state.onboardSteps = account.locked === "locked_signed_out" ? ONBOARDING_WITH_SIGN_IN : ONBOARDING;
  if (account.locked === null) {
    showScreen("location");
    return;
  }
  if (account.locked === "locked_signed_out") {
    showScreen("signin");
    return;
  }
  showLock(account.locked);
}

// -- connect agent ----------------------------------------------------------

// The picker is one element shared by the setup step and the Agents tab
// (session 59: "Connect it for me" existed only in setup, so anybody who
// skipped connecting there, or came back later, had only raw settings text).
// It lives wherever it was last put; each screen puts it back before showing.
function placeConnectPicker(hostId) {
  const host = $(hostId);
  const picker = $("connect-picker");
  if (host && picker && picker.parentElement !== host) {
    host.appendChild(picker);
    // Its list depends on where it is ("Connected apps" on the Agents tab
    // only, session 64), so it is drawn again for its new place.
    renderAgentCards();
  }
}

function renderAgentCards() {
  // On the Agents tab the list opens with "Connected apps" (session 64),
  // shown while no app is picked; setup has no such item.
  const onAgentsTab = $("connect-picker").parentElement === $("agents-connect-host");
  const showConnected = onAgentsTab && state.agentPicked === null;
  const apps = AGENTS.map((a, i) => `
    <button class="agent-card${state.agentPicked === i ? " picked" : ""}" data-i="${i}">${esc(a.name)}</button>`).join("");
  $("agent-grid").innerHTML = onAgentsTab
    ? `<button class="agent-card${showConnected ? " picked" : ""}" data-i="connected">Connected apps</button>
       <div class="agent-nav-label">Connect an app</div>${apps}`
    : apps;
  $("connected-view").classList.toggle("hidden", !showConnected);
  const picked = state.agentPicked;
  const agent = picked === null ? null : AGENTS[picked];
  const auto = agent ? agent.connect || null : null;
  $("connect-empty").classList.toggle("hidden", agent !== null || showConnected);
  $("connect-head").classList.toggle("hidden", agent === null);
  $("snippet-wrap").classList.toggle("hidden", agent === null);
  $("connect-auto").classList.toggle("hidden", auto === null);
  $("connect-auto-status").textContent = "";
  $("connect-auto-show").classList.add("hidden");
  if (auto !== null) $("connect-auto-note").textContent = CONNECT_WORDS[auto].note;
  const check = agent ? AGENT_CHECKS[agent.name] || null : null;
  $("connect-check").classList.toggle("hidden", check === null);
  if (check !== null) $("connect-check-text").textContent = check;
  const tip = agent ? AGENT_TIPS[agent.name] || null : null;
  $("connect-tip").classList.toggle("hidden", tip === null);
  if (tip !== null) {
    $("connect-tip-where").textContent = tip;
    $("connect-tip-text").textContent = TIP_LINE;
  }
  if (agent !== null) {
    $("connect-app-name").textContent = agent.name;
    $("connect-app-desc").textContent = agent.desc;
    $("snippet-title").textContent = auto === null ? "Add it yourself" : "Or add it yourself";
    $("snippet-hint").textContent = agent.hint;
    const openers = agent.openers || [];
    $("snippet-openers").innerHTML = openers.map((o, i) => `
      <div class="opener">
        <div class="opener-label">${esc(o.label)}</div>
        <div class="code-box">
          <div class="snippet-code">${esc(o.command)}</div>
          <button class="copy-chip" data-opener="${i}">copy</button>
        </div>
      </div>`).join("");
    $("snippet-paste-hint").classList.toggle("hidden", !agent.pasteHint);
    $("snippet-paste-hint").textContent = agent.pasteHint || "";
    $("snippet-code").textContent = agent.snippet(state.serverCommand);
    $("connect-cta").textContent = "I've added it, continue";
    askServerCommand();
  } else {
    $("connect-cta").textContent = "Continue";
  }
}

// Once per session, when an app is first picked (after sign-in: the command
// is gated). A refusal leaves the short name and asks again next time.
function askServerCommand() {
  if (state.serverCommandAsked) return;
  state.serverCommandAsked = true;
  invoke("server_command").then((answer) => {
    if (!answer || typeof answer.command !== "string" || answer.command === "") return;
    state.serverCommand = answer.command;
    if (state.agentPicked !== null) {
      $("snippet-code").textContent = AGENTS[state.agentPicked].snippet(state.serverCommand);
    }
  }).catch(() => { state.serverCommandAsked = false; });
}

// "Connect it for me" (ADR-106): the page names only the app; the link and
// everything else are fixed on the Rust side.
async function onConnectAuto() {
  const agent = state.agentPicked === null ? null : AGENTS[state.agentPicked];
  if (!agent || !agent.connect) return;
  const words = CONNECT_WORDS[agent.connect];
  const button = $("connect-auto-btn");
  button.disabled = true;
  $("connect-auto-status").textContent = "";
  $("connect-auto-show").classList.add("hidden");
  try {
    const answer = await invoke("connect_app", { app: agent.connect });
    const outcome = answer && answer.outcome;
    $("connect-auto-status").textContent = words[outcome] || words.could_not_open;
    // Saved, not opened: the person installs it from Claude's settings, and
    // may want to see where it is.
    $("connect-auto-show").classList.toggle("hidden", outcome !== "saved");
  } catch (err) {
    $("connect-auto-status").textContent = String(err);
  } finally {
    button.disabled = false;
  }
}

function connectContinue() {
  if (state.agentPicked !== null) {
    const name = AGENTS[state.agentPicked].name;
    if (!state.agents.some((a) => a.name === name)) {
      state.agents.push({ name, when: Date.now() });
      store.set("mv_agents", state.agents);
    }
  }
  showScreen("memory");
}

// -- first memory -----------------------------------------------------------

// The engine's memory_type taxonomy (semantic / episodic / procedural) is
// write-side metadata — it never gates recall. The UI speaks plain English
// and maps to the backend values; onboarding doesn't ask at all (founder
// decision 2026-07-11).
const TYPE_OPTIONS = [
  { value: "semantic", label: "a fact about me" },
  { value: "episodic", label: "something that happened" },
  { value: "procedural", label: "how I do things" },
];
const TYPE_ROW_LABEL = { semantic: "fact", episodic: "event", procedural: "how-to" };

function renderTypeChips(containerId, current, onPick) {
  $(containerId).innerHTML = TYPE_OPTIONS.map((t) =>
    `<button class="type-chip${current === t.value ? " on" : ""}" data-t="${t.value}">${t.label}</button>`).join("");
  [...$(containerId).querySelectorAll(".type-chip")].forEach((el) => {
    el.addEventListener("click", () => onPick(el.dataset.t));
  });
}

function renderMemoryScreen() {
  const has = $("mem-text").value.trim().length > 0;
  $("mem-save").classList.toggle("disabled", !has);
}

async function saveFirstMemory() {
  const text = $("mem-text").value.trim();
  if (!text) return;
  $("mem-save").classList.add("disabled");
  $("mem-err").textContent = "";
  try {
    await invoke("add_memory", {
      content: text,
      memoryType: state.memType,
      boundary: "default",
    });
    // Onboarding now has a 4th step (automatic maintenance) before it finishes.
    state.keptFirstMemory = true;
    showScreen("maintenance");
  } catch (err) {
    $("mem-err").textContent = `Couldn't keep that memory: ${err}`;
    $("mem-save").classList.remove("disabled");
  }
}

// Reflect "still preparing" on the last onboarding screen, the nightly
// tidy-up (page 6). "Finish setup" stays disabled while we wait, so the gate
// cannot be clicked past, and the line carries the live percentage so the
// wait never looks like a hang. Session 58: this used to write to the first
// memory's screen, which page 6 had replaced as the last step, so the wait
// showed no progress, and every progress event re-enabled "Keep this memory"
// over an empty box.
function renderFinishWait() {
  if (!engineFetch.waiting) return;
  const cta = $("maint-onboard-cta");
  if (cta) cta.classList.add("disabled");
  const line = $("maint-onboard-wait");
  if (line) {
    line.textContent = engineFetch.failed
      ? "Couldn't finish preparing the recall engine. Check your connection. Retrying…"
      : `Preparing your recall engine (${engineFetch.percent}%)…`;
  }
}

// Onboarding completion GATES on acquisition (founder decision 2026-07-20).
// The download has been running underneath the previous screens; by the time
// most people reach here it is already done and this returns immediately.
// When it is not, we wait rather than dropping the user into a vault whose
// engine is still half-prepared.
//
// Note this is the ONLY gate: a returning user (`mv_onboarded` already true)
// goes straight to the home screen without passing through here. That is
// exactly why an unprepared engine has to DEGRADE rather than error
// (ADR-089) — the gate cannot cover them, so the engine must cope without it.
async function finishOnboarding(withToast) {
  // Re-entrancy guard: `.disabled` is a visual class, so a second click can
  // still reach here while the first is awaiting.
  if (engineFetch.waiting) return;
  if (!engineFetch.done) {
    engineFetch.waiting = true;
    renderFinishWait();
    try {
      await startEngineFetch();
    } catch {
      // Do not strand the user mid-onboarding. Surface the failure, let them
      // through, and leave search on the degraded path until a later attempt
      // succeeds — a vault they can use beats a modal they cannot dismiss.
      engineFetch.waiting = false;
      renderFinishWait();
      const err = $("maint-onboard-wait");
      if (err) {
        err.textContent = "Couldn't finish preparing the recall engine, so setup carries on anyway. Recall will sharpen once it completes.";
      }
    }
    engineFetch.waiting = false;
    renderFinishWait();
  }
  store.set("mv_onboarded", true);
  showScreen("home");
  if (withToast) {
    $("toast-wrap").classList.remove("hidden");
    clearTimeout(state.toastTimer);
    state.toastTimer = setTimeout(() => $("toast-wrap").classList.add("hidden"), 3200);
  }
}

// Slice 2 removed the browser-local `mv_recent` cache: the home list now
// reads `list_recent_memories` from the vault itself, so memories an AGENT
// wrote show up alongside the ones added here. Clear the stale cache once so
// upgrading installs don't leave dead data in localStorage.
try { localStorage.removeItem("mv_recent"); } catch { /* non-fatal */ }

// -- home -------------------------------------------------------------------

function renderHome() {
  renderNav();
  renderTab();
  renderFooter();
  // The banners: read from disk, no network, and no use recorded.
  refreshAccountView();
  // What this start did about a move, said once (ADR-105 L-e).
  refreshLocationNotice();
  // Whether the background part is still opening the memories (ADR-108).
  watchLink();
  // The footer reports how many AI apps are connected, so it needs the list
  // even when the user never opens the Agents tab. Fire-and-forget: a failed
  // read shows none rather than blocking home.
  refreshConnectedApps();
}

function renderNav() {
  const items = [
    { label: "Memories", key: "memories" },
    { label: "Boundaries", key: "boundaries" },
    { label: "Agents", key: "agents" },
    { label: "Consolidation", key: "maintenance" },
    { label: "Settings", key: "settings" },
  ];
  $("nav").innerHTML = items.map((n) =>
    `<span class="${state.tab === n.key ? "on" : ""}" data-k="${n.key}">${n.label}</span>`).join("");
  [...$("nav").children].forEach((el) => {
    el.addEventListener("click", () => {
      // The Agents tab opens on "Connected apps" (session 64).
      if (el.dataset.k === "agents") state.agentPicked = null;
      state.tab = el.dataset.k;
      renderTab();
    });
  });
}

function renderTab() {
  renderNav2();
  for (const t of ["memories", "boundaries", "agents", "maintenance", "settings"]) {
    $(`tab-${t}`).classList.toggle("hidden", t !== state.tab);
  }
  if (state.tab === "memories") renderMemList();
  if (state.tab === "boundaries") renderBoundaries();
  if (state.tab === "agents") renderAgents();
  if (state.tab === "maintenance") renderMaintenance();
  if (state.tab === "settings") renderSettings();
}

// re-style nav highlights without rebuilding listeners
function renderNav2() {
  [...$("nav").children].forEach((el) => el.classList.toggle("on", el.dataset.k === state.tab));
}

// -- memories tab --

async function renderMemList() {
  const q = state.query.trim();
  const seq = ++state.searchSeq;
  if (!q) {
    $("mem-list-title").textContent = "Recently remembered";
    const rseq = ++state.recentSeq;
    try {
      const recent = await invoke("list_recent_memories", { limit: 20 });
      if (rseq !== state.recentSeq || state.query.trim()) return; // superseded
      renderMemRows(recent.map((m) => ({
        id: m.id, memory_type: m.memory_type, content: m.content, when: relTime(m.created_at),
      })), "Nothing here yet. Keep a memory below, or connect an agent and let it remember for you.");
    } catch (err) {
      if (rseq !== state.recentSeq) return;
      $("mem-rows").innerHTML = `<div class="empty-note">Couldn't load your memories: ${esc(String(err))}</div>`;
    }
    return;
  }
  $("mem-list-title").textContent = "Recalling…";
  try {
    const results = await invoke("search_memories", { query: q, limit: 8 });
    if (seq !== state.searchSeq) return; // stale response — a newer query is in flight
    $("mem-list-title").textContent = "Recalled";
    renderMemRows(results.map((r) => ({
      id: r.id, memory_type: r.memory_type, content: r.content, when: relTime(r.created_at),
    })), "Nothing recalled for that yet.");
  } catch (err) {
    if (seq !== state.searchSeq) return;
    $("mem-list-title").textContent = "Recall failed";
    $("mem-rows").innerHTML = `<div class="empty-note">${esc(String(err))}</div>`;
  }
}

function renderMemRows(rows, emptyText) {
  if (!rows.length) {
    $("mem-rows").innerHTML = `<div class="empty-note">${esc(emptyText)}</div>`;
    return;
  }
  $("mem-rows").innerHTML = rows.map((m) => `
    <div class="mem-row" data-id="${esc(m.id)}">
      <span class="ty">${esc(TYPE_ROW_LABEL[m.memory_type] || m.memory_type)}</span>
      <span class="tx">${esc(m.content)}</span>
      <span class="wh">${esc(m.when)}</span>
      <button class="forget" title="Delete this memory">forget</button>
    </div>`).join("");
  [...$("mem-rows").querySelectorAll(".forget")].forEach((btn) => {
    btn.addEventListener("click", async () => {
      const row = btn.closest(".mem-row");
      const id = row.dataset.id;
      const ok = await confirmAction({
        title: "Forget this memory?",
        body: "It will be permanently deleted from your vault straight away. "
            + "There is no undo and no recovery period.",
        confirmLabel: "Forget it",
      });
      if (!ok) return;
      try {
        await invoke("delete_memory", { id });
        row.remove();
      } catch (err) {
        alert(`Couldn't forget it: ${err}`);
      }
    });
  });
}

function toggleInlineAdd(show) {
  state.showInlineAdd = show ?? !state.showInlineAdd;
  $("inline-add").classList.toggle("hidden", !state.showInlineAdd);
  if (state.showInlineAdd) {
    renderTypeChips("add-chips", state.addType, (t) => { state.addType = t; toggleInlineAdd(true); });
    $("add-text").focus();
  }
}

async function saveInlineMemory() {
  const text = $("add-text").value.trim();
  if (!text) return;
  $("add-err").textContent = "";
  try {
    await invoke("add_memory", {
      content: text,
      memoryType: state.addType,
      boundary: "default",
    });
    $("add-text").value = "";
    toggleInlineAdd(false);
    state.query = "";
    $("search-input").value = "";
    renderMemList();
  } catch (err) {
    $("add-err").textContent = `Couldn't keep that memory: ${err}`;
  }
}

// -- boundaries tab --

async function renderBoundaries() {
  $("boundary-rows").innerHTML = `<div class="empty-note">Loading…</div>`;
  try {
    const rows = await invoke("list_boundaries");
    state.boundaries = rows;
    $("boundary-rows").innerHTML = rows.map((b) => {
      const n = Number(b.memory_count) || 0;
      const count = n === 1 ? "1 memory" : `${n} memories`;
      const desc = b.description
        || (b.name === "default" ? "Everything this app remembers by default" : "");
      return `
      <div class="b-row">
        <span class="nm">${esc(b.name)}</span>
        <span class="ds">${esc(desc)}</span>
        <span class="mt">${esc(count)}</span>
      </div>`;
    }).join("");
  } catch (err) {
    $("boundary-rows").innerHTML = `<div class="empty-note">Couldn't load boundaries: ${esc(String(err))}</div>`;
  }
  $("boundary-add-label").textContent = state.showBoundaryAdd ? "cancel" : "+ New boundary";
  $("boundary-add").classList.toggle("hidden", !state.showBoundaryAdd);
}

function toggleBoundaryAdd(show) {
  state.showBoundaryAdd = show ?? !state.showBoundaryAdd;
  $("boundary-err").textContent = "";
  $("boundary-add").classList.toggle("hidden", !state.showBoundaryAdd);
  $("boundary-add-label").textContent = state.showBoundaryAdd ? "cancel" : "+ New boundary";
  if (state.showBoundaryAdd) $("boundary-name").focus();
}

async function saveBoundary() {
  const name = $("boundary-name").value.trim();
  const description = $("boundary-desc").value.trim();
  if (!name) return;
  $("boundary-err").textContent = "";
  try {
    const created = await invoke("create_boundary", {
      name,
      description: description || null,
    });
    if (!created) {
      $("boundary-err").textContent = "You already have a boundary with that name.";
      return;
    }
    $("boundary-name").value = "";
    $("boundary-desc").value = "";
    toggleBoundaryAdd(false);
    renderBoundaries();
  } catch (err) {
    // The backend rejects anything outside letters, digits, - and _ ; say so
    // in plain language rather than surfacing the validation error verbatim.
    $("boundary-err").textContent =
      `Couldn't create that boundary: ${String(err).replace(/^invalid boundary name: /, "")}`;
  }
}

// -- agents tab --

// An app's own MCP name, as people know it. Naming the person's apps is fine;
// naming our stack is not (ADR-086). Anything unknown shows as it came.
function friendlyAppName(raw) {
  const n = String(raw || "").toLowerCase();
  if (n === "an ai app" || n === "zaaheen-relay") return "An AI app";
  if (n.includes("claude-code") || n.includes("claude code")) return "Claude Code";
  if (n.startsWith("claude")) return "Claude";
  // Claude Desktop's Cowork mode introduces itself this way (session 64).
  if (n.startsWith("local-agent-mode")) return "Claude";
  if (n.includes("cursor")) return "Cursor";
  if (n.includes("chatgpt") || n.includes("codex") || n.includes("openai")) return "ChatGPT";
  if (n.includes("antigravity")) return "Antigravity";
  return String(raw);
}

// The connected apps (session 59): the keeper's live list, so the tab and the
// footer tell the truth while Claude, Cursor or ChatGPT are using the vault.
async function refreshConnectedApps() {
  try {
    state.connectedApps = await invoke("list_connected_apps");
  } catch {
    state.connectedApps = [];
  }
  rememberApps(state.connectedApps);
  renderFooter();
}

// Every AI app ever seen connected, by the name people know it by, with when
// it was last seen. Session 64: the keeper's live list starts empty after
// every tidy-up (a new keeper), so the tab and the footer said "no AI app
// connected" while three were set up and working (founder: "in agents tab it
// shows no agent connected? but we have already 3 connected"). Names and
// times only, on this page; "Delete everything" clears it.
const KNOWN_APPS_KEY = "mv_known_apps";

function knownApps() {
  const known = store.get(KNOWN_APPS_KEY, {});
  return known && typeof known === "object" && !Array.isArray(known) ? known : {};
}

function rememberApps(apps) {
  const known = knownApps();
  let changed = false;
  for (const a of apps) {
    const name = friendlyAppName(a.name);
    const seen = a.last_used || a.since || new Date().toISOString();
    if (!known[name] || Date.parse(seen) > Date.parse(known[name])) {
      known[name] = seen;
      changed = true;
    }
  }
  if (changed) store.set(KNOWN_APPS_KEY, known);
}

function renderConnectedApps() {
  const active = new Set(state.connectedApps.map((a) => friendlyAppName(a.name)));
  const known = Object.entries(knownApps())
    .sort((x, y) => Date.parse(y[1]) - Date.parse(x[1]));
  $("no-apps").classList.toggle("hidden", known.length > 0);
  $("app-table").classList.toggle("hidden", known.length === 0);
  $("app-rows").innerHTML = known.map(([name, seen]) => {
    const on = active.has(name);
    return `
      <div class="a-row${on ? "" : " idle"}">
        <span class="st"></span>
        <span class="nm">${esc(name)}</span>
        <span class="tr"></span>
        <span class="ac">${esc(on ? "active now" : `last used ${relTime(seen)}`)}</span>
      </div>`;
  }).join("");
}

async function renderAgents() {
  await refreshConnectedApps();
  renderConnectedApps();

  // The HTTP daemon's access keys (ADR-SEC-001), under their own heading and
  // only when one exists: nobody on the desktop path has one.
  let agents = [];
  try {
    agents = await invoke("list_agents");
    state.grantedAgents = agents;
  } catch (err) {
    $("agent-keys").classList.remove("hidden");
    $("agent-rows").innerHTML = `<div class="empty-note">Couldn't load access keys: ${esc(String(err))}</div>`;
    renderAgentsConnect();
    return;
  }

  $("agent-keys").classList.toggle("hidden", agents.length === 0);
  if (!agents.length) {
    $("agent-rows").innerHTML = "";
  } else {
    $("agent-rows").innerHTML = agents.map((a) => {
      const scope = a.boundaries.length
        ? `can read: ${a.boundaries.join(", ")}`
        : "no boundaries granted";
      const when = a.active
        ? `connected ${relTime(a.created_at)}`
        : `revoked ${relTime(a.revoked_at)}`;
      return `
      <div class="a-row${a.active ? "" : " revoked"}" data-name="${esc(a.name)}">
        <span class="st"></span>
        <span class="nm">${esc(a.name)}</span>
        <span class="tr">${esc(scope)}</span>
        <span class="ac">${esc(when)}</span>
        ${a.active ? `<button class="revoke">revoke</button>` : `<span class="ac"></span>`}
      </div>`;
    }).join("");
    [...$("agent-rows").querySelectorAll(".revoke")].forEach((btn) => {
      btn.addEventListener("click", async () => {
        const row = btn.closest(".a-row");
        const name = row.dataset.name;
        const ok = await confirmAction({
          title: `Revoke ${name}'s access?`,
          body: `${name} won't be able to read or write memories until you `
              + `connect it again. It stays listed here, and everything it `
              + `did before stays in the audit log.`,
          confirmLabel: "Revoke access",
        });
        if (!ok) return;
        try {
          await invoke("revoke_agent", { agentName: name });
          renderAgents();
        } catch (err) {
          alert(`Couldn't revoke access: ${err}`);
        }
      });
    });
  }
  renderAgentsConnect();
}

// "Connect another app" shows the same picker the setup uses, "Connect it
// for me" included, under its own heading (session 64: always shown, no
// toggle).
function renderAgentsConnect() {
  placeConnectPicker("agents-connect-host");
  renderAgentCards();
}

// -- settings tab --

// Settings' sections down the left, the chosen one on the right (founder,
// session 55). Account shows only in a build with sign-in, and is where
// Settings opens there; elsewhere it opens on "Your memories".
function renderSettingsNav() {
  const accountShown = !$("account-section").classList.contains("hidden");
  const nav = $("settings-nav");
  nav.querySelector("[data-section=\"account\"]").classList.toggle("hidden", !accountShown);
  if (!state.settingsSection) state.settingsSection = account.signIn ? "account" : "memories";
  // Account asked for before its panel has drawn: it shows once it has.
  const shown = state.settingsSection === "account" && !accountShown ? "memories" : state.settingsSection;
  for (const button of nav.querySelectorAll("button[data-section]")) {
    button.classList.toggle("on", button.dataset.section === shown);
  }
  for (const section of document.querySelectorAll("#tab-settings .settings-section")) {
    section.classList.toggle("on", section.dataset.section === shown);
  }
}

function onSettingsNav(e) {
  const button = e.target.closest("button[data-section]");
  if (!button) return;
  state.settingsSection = button.dataset.section;
  renderSettingsNav();
}

async function renderSettings() {
  renderSettingsNav();
  refreshAccountView();
  $("settings-rows").innerHTML = `<div class="empty-note">Loading…</div>`;
  let info = null;
  try {
    info = await invoke("get_settings_info");
  } catch (err) {
    $("settings-rows").innerHTML = `<div class="empty-note">Couldn't read vault details: ${esc(String(err))}</div>`;
    return;
  }

  const n = Number(info.memory_count) || 0;
  const b = Number(info.boundary_count) || 0;
  const rows = [
    { label: "Encryption", value: "AES-256 · key in Windows Credential Manager", good: true },
    { label: "Vault location", value: info.data_dir },
    {
      label: "Stored",
      value: `${n === 1 ? "1 memory" : `${n} memories`} across ${b === 1 ? "1 boundary" : `${b} boundaries`}`,
    },
    { label: "Recall engine", value: "on-device · works offline", good: true },
    // Honest UI: report what the chain check actually returned. A broken
    // chain means something modified the history outside the app, and the
    // user needs to know rather than see a reassuring constant.
    info.audit_chain_verified
      ? { label: "Audit log", value: "recorded locally · history verified", good: true }
      : { label: "Audit log", value: "history could not be verified, so the record may have been altered", good: false },
    { label: "Version", value: `zaaheen ${info.version} · V0.2 beta` },
  ];
  $("settings-rows").innerHTML = rows.map((r) => `
    <div class="s-row">
      <span class="lbl">${esc(r.label)}</span>
      <span class="val${r.good ? " good" : ""}">${esc(r.value)}</span>
    </div>`).join("");

  // ADR-SEC-008: show the user where their memories actually live, so
  // "uninstalling does not remove them" is a checkable statement.
  // textContent, not markup — this is a filesystem path from the backend.
  $("data-location").textContent = info.data_dir || "";
  renderMoveSection();
}

// -- delete everything (ADR-SEC-008) --

const ERASE_PHRASE = "DELETE";

function resetEraseConfirm() {
  $("erase-confirm").classList.add("hidden");
  $("erase-reveal").classList.remove("hidden");
  $("erase-phrase").value = "";
  $("erase-confirm-btn").disabled = true;
  $("erase-status").textContent = "";
}

function revealEraseConfirm() {
  $("erase-reveal").classList.add("hidden");
  $("erase-confirm").classList.remove("hidden");
  // §8.26 §7: a subscription outlives the memories, so somebody paying is
  // told so, with the way to cancel, before they type DELETE.
  const view = account.view;
  const subscriber = !!(account.signIn && view
    && (view.state === "active" || view.state === "payment_failed"));
  $("erase-subscription").classList.toggle("hidden", !subscriber);
  $("erase-phrase").value = "";
  $("erase-confirm-btn").disabled = true;
  $("erase-phrase").focus();
}

// The button unlocks only on an exact match. This is the one action in the
// app that cannot be undone — not even from a copy of the data folder —
// so passing the gate should require intent, not a reflex click. A yes/no
// dialog is too easy to click through for a consequence this size.
function onErasePhraseInput() {
  $("erase-confirm-btn").disabled = $("erase-phrase").value !== ERASE_PHRASE;
}

async function eraseEverything() {
  if ($("erase-phrase").value !== ERASE_PHRASE) return;

  $("erase-confirm-btn").disabled = true;
  $("erase-cancel").disabled = true;
  $("erase-status").textContent = "Deleting your memories…";

  let result;
  try {
    result = await invoke("erase_everything");
  } catch (err) {
    // Honest failure. The vault is STILL READABLE when this path runs, and
    // saying anything softer than that would be a lie about the one
    // property the user was trying to obtain. "erasure_busy" means a
    // background tidy-up is using the vault and was not interrupted.
    $("erase-status").textContent = String(err).includes("erasure_busy")
      ? "Zaaheen is tidying up your memories right now, so nothing was deleted and your memories are still readable. Please try again in a few minutes."
      : "Your memories were NOT deleted, and they are still readable. Nothing was changed. Please try again, or restart Zaaheen and retry.";
    $("erase-cancel").disabled = false;
    $("erase-confirm-btn").disabled = false;
    return;
  }

  // Report what actually happened. Leftover files are a disk-space fact,
  // not a confidentiality one — once the key is destroyed the remaining
  // bytes cannot be read by anyone.
  const leftover = Number(result && result.undeletable_count) || 0;
  $("erase-status").textContent = leftover > 0
    ? "Your memories are permanently deleted and can no longer be read. A few files could not be removed from disk, but they are now unreadable. Zaaheen will close."
    : "Your memories are permanently deleted. Zaaheen will close.";
  // The apps it had seen connect go too: names and times, but the person
  // asked for everything.
  store.set(KNOWN_APPS_KEY, {});

  // The app is now running against a vault that no longer exists; staying
  // open would show stale, already-unreadable state. Close rather than
  // pretend.
  setTimeout(() => {
    try {
      if (window.__TAURI__ && window.__TAURI__.window) {
        window.__TAURI__.window.getCurrentWindow().close();
      }
    } catch (_) { /* best-effort; the message above already stands */ }
  }, 2500);
}

// -- diagnostic log export (ADR-SEC-017) --

// The save dialog. Guarded like the other bridges so the UI still renders in a
// plain browser for design review.
const saveDialog = window.__TAURI__ && window.__TAURI__.dialog
  ? window.__TAURI__.dialog.save
  : async () => null;

// A default filename the user can recognise in their Downloads folder a week
// later. Dated, because the second thing we ask is always "when was this?".
function logExportFilename() {
  const d = new Date();
  const pad = (n) => String(n).padStart(2, "0");
  return `zaaheen-log-${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}.txt`;
}

async function exportLogs() {
  const status = $("export-logs-status");
  status.textContent = "";

  let destination;
  try {
    destination = await saveDialog({
      defaultPath: logExportFilename(),
      filters: [{ name: "Text file", extensions: ["txt"] }],
    });
  } catch (_) {
    status.textContent = "Could not open the save window. Please try again.";
    return;
  }
  // Cancelled. Not an error, and saying nothing is the right response.
  if (!destination) return;

  $("export-logs").disabled = true;
  status.textContent = "Saving…";
  try {
    const bytes = await invoke("export_logs", { destination });
    const kb = Math.max(1, Math.round(Number(bytes) / 1024));
    status.textContent = `Saved (${kb} KB). You can attach this file to an email.`;
  } catch (err) {
    // Distinguish "nothing to send" from "we could not write it" — they lead
    // the user to completely different next steps.
    status.textContent = String(err) === "log_export_empty"
      ? "There is nothing recorded yet, so there is nothing to save."
      : "Could not save the file. Please pick a different folder and try again.";
  } finally {
    $("export-logs").disabled = false;
  }
}

// -- the lock, sign-in and the subscription (S3 step 4d-2) --

// The ONLY place this app asks the lock whether it is open (§8.38). A served
// ask records a use (§4), so it is asked when the app opens and after an
// account action, and never on a timer: a window that polled it would keep
// somebody "active" for as long as it stayed open, and the 30-days-unused
// sign-out would never fire. `account_access_is_never_asked_on_a_timer` holds
// this, and lists every caller with its reason.
async function askAccess() {
  let answer;
  try {
    answer = await invoke("account_access");
  } catch {
    // It does not fail inside the app. Outside it (a design review in a
    // plain browser) there is no lock to ask, and the try-again screen is the
    // honest thing to show.
    answer = { sign_in: true, locked: "locked_cannot_confirm" };
  }
  account.signIn = !!answer.sign_in;
  account.locked = answer.locked || null;
  return account.locked;
}

// When the app opens: ask the lock once, then go where it says.
async function enterApp() {
  account.ready = askAccess();
  routeAfterAccess(await account.ready);
}

// -- moving your memories (ADR-105 L-f) --------------------------------------

// A start that moves the memories serves this page before it opens them: until
// `startup_state` says "ready", nothing else is asked (the vault, the key and
// the lock open after the move). A normal start answers "ready" at once and
// nothing is shown. The question cannot fail in the app (its state is in
// place before the page loads); if it does anyway, it is asked again for a
// few seconds, and only then does the page carry on: every gated command
// still asks the lock. Outside the app (a plain browser) there is no start.
const MOVING_POLL_MS = 250;
const STARTUP_TRIES = 20;

async function untilStarted() {
  if (!(window.__TAURI__ && window.__TAURI__.core)) return;
  const before = state.screen;
  let shown = false;
  let failed = 0;
  for (;;) {
    let answer;
    try {
      answer = await invoke("startup_state");
      failed = 0;
    } catch {
      failed += 1;
      if (failed >= STARTUP_TRIES) break;
      await sleep(MOVING_POLL_MS);
      continue;
    }
    if (!answer || typeof answer !== "object" || answer.stage === "ready") break;
    renderMoving(answer);
    if (!shown) {
      shown = true;
      showScreen("moving");
    }
    await sleep(MOVING_POLL_MS);
  }
  // The screen stays up until the lock answers (routeAfterAccess moves on
  // from "boot"), so the person never sees an empty window in between.
  if (shown) {
    if (before === "welcome") showScreen("welcome");
    else state.screen = "boot";
  }
}

// The move's own words, founder-approved (session 55). Only what the move
// reports is shown: the phase, bytes done of the total, the folder.
function renderMoving(answer) {
  let words = "Getting ready to move them.";
  let percent = 0;
  let amount = "";
  const total = Number(answer.total) || 0;
  const done = Math.min(Number(answer.done) || 0, total);
  const part = total > 0 ? done / total : 1;
  if (answer.stage === "opening") {
    words = "Opening your memories.";
    percent = 100;
  } else if (answer.phase === "copying") {
    words = `Copying them to ${answer.to || "the new folder"}.`;
    percent = part * 50;
    amount = `${formatBytes(done)} of ${formatBytes(total)} copied`;
  } else if (answer.phase === "checking") {
    words = "Checking that every memory arrived safely.";
    percent = 50 + part * 50;
    amount = `${formatBytes(done)} of ${formatBytes(total)} checked`;
  } else if (answer.phase === "finishing") {
    words = "Almost done.";
    percent = 100;
  }
  $("moving-phase").textContent = words;
  $("moving-amount").textContent = amount;
  // "Your memories stay safe where they were" holds until the switch; from
  // "Almost done." on they are already in the new folder.
  const beforeTheSwitch = answer.stage === "moving"
    && ["waiting", "copying", "checking"].includes(answer.phase);
  $("moving-note").classList.toggle("hidden", !beforeTheSwitch);
  const rounded = Math.round(percent);
  $("moving-fill").style.width = `${rounded}%`;
  document.querySelector("#screen-moving .moving-bar").setAttribute("aria-valuenow", String(rounded));
}

// Sizes as Windows shows them (1 MB = 1,048,576 bytes).
function formatBytes(bytes) {
  const mb = bytes / 1048576;
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  if (mb >= 10) return `${Math.round(mb)} MB`;
  return `${mb.toFixed(1)} MB`;
}

// Where the app goes once the lock has answered — at open, from the lock
// screen, after an account action. The gated background work starts here, on
// the branch where the lock said yes, and nowhere else.
function routeAfterAccess(locked) {
  if (locked === null) {
    startEntitledWork();
    refreshAccountView();
    if (state.screen === "boot" || state.screen === "lock" || state.screen === "signin") {
      // The setup's first gated step is where the memories live.
      showScreen(store.get("mv_onboarded", false) ? "home" : "location");
    }
    return;
  }
  // A new install still watching its welcome: "Begin set up" routes it.
  if (state.screen === "welcome") return;
  // Already on the setup's sign-in step, which is where this belongs.
  if (state.screen === "signin" && locked === "locked_signed_out") return;
  showLock(locked);
}

// Everything that calls a gated command in the background. Before the lock
// says yes each of these is refused, which is how a fresh install's first
// download was being turned away before anybody could sign in. Started once.
let entitledWorkStarted = false;
function startEntitledWork() {
  if (entitledWorkStarted) return;
  entitledWorkStarted = true;

  // First-run acquisition. Started for EVERY entitled launch, not just
  // onboarding ones: a returning user whose files were never fetched (or
  // were removed) has no onboarding gate to pass through, so this is their
  // only route to a fully prepared engine. It short-circuits once the files
  // are present and verified, and runs entirely in the background.
  startEngineFetch()
    .then(() => {
      // ADR-090: files on disk is not the same as engine ready. Ask for the
      // load explicitly here — on a genuine first run the files were still
      // downloading when `main.rs` made its startup attempt, so without this
      // the model stays cold until the user's first search.
      warmEngine();
    })
    .catch(() => {
      // Swallowed here on purpose: a background failure must not throw. It
      // is surfaced where it matters — the onboarding gate retries and
      // reports, and search degrades gracefully meanwhile (ADR-089).
    });

  // Warm-launch case: the files were already present, so `main.rs` started
  // the load before the window existed. Nothing to request — just watch for
  // it to finish so the "getting ready" line clears itself (ADR-090).
  pollEngineReady();

  // Maintenance engine acquisition — downloads DURING ONBOARDING for new
  // users (founder 2026-07-24), never gating it. A returning user does NOT
  // auto-download 2.5 GB on every launch; they get the engine when they turn
  // maintenance on (see saveMaintenance), and a scheduled or catch-up run
  // self-heals by fetching it if absent.
  if (!store.get("mv_onboarded", false)) startMaintenanceFetch();

  // Missed-run catch-up (task 10): if maintenance is on and overdue, run a
  // background pass. Also re-checked when the engine download completes.
  catchUpMaintenanceIfDue();
}

// Show the lock screen for a code from the lock. For "could not confirm"
// the account view picks the remedy: a computer whose account could not be
// read at all needs the app reopened; one that is signed in but offline
// needs the internet. The view also carries "Signed in as …".
async function showLock(code) {
  account.lockCode = code;
  const view = await readAccountView();
  if (account.lockCode !== code) return; // a newer answer arrived meanwhile
  renderLock(lockVariant(code, view));
  if (state.screen !== "lock") showScreen("lock");
}

function lockVariant(code, view) {
  const variant = LOCK_VARIANT[code] || "cannot_confirm";
  if (variant === "cannot_confirm" && view && !view.signed_in && view.state === "cannot_confirm") {
    return "reopen";
  }
  return variant;
}

// Draw one lock screen. Only the reason's own part changes: the foot, with
// the export and the support address, is the same whatever the reason.
function renderLock(variant) {
  const copy = LOCK_COPY[variant] || LOCK_COPY.cannot_confirm;
  const changed = account.lockVariant !== variant;
  account.lockVariant = variant;
  $("lock-heading").textContent = copy.heading;
  $("lock-heading").classList.toggle("hidden", !copy.heading);
  $("lock-text").textContent = copy.text;
  const paying = copy.action === "subscribe" && account.checkout !== null;
  $("lock-signin").classList.toggle("hidden", copy.action !== "sign_in");
  $("lock-subscribe").classList.toggle("hidden", copy.action !== "subscribe" || paying);
  $("lock-paying").classList.toggle("hidden", !paying);
  $("lock-retry").classList.toggle("hidden", copy.action !== "retry");
  $("lock-close").classList.toggle("hidden", copy.action !== "close");
  if (changed) $("lock-status").textContent = "";
  const view = copy.who ? account.view : null;
  const who = !!(view && view.signed_in);
  $("lock-who").classList.toggle("hidden", !who);
  if (who) showWho($("lock-email"), view);
}

// A returning computer whose lock has not answered yet.
function showChecking() {
  if (state.screen !== "boot") return;
  renderLock("checking");
  showScreen("lock");
}

// "Signed in as … Not you?" The address came from the account service, so it
// is shown as text and never as markup.
function showWho(el, view) {
  el.textContent = view && view.email ? `Signed in as ${view.email}. Not you?` : "Signed in.";
}

// The account as it stands on disk: no network, no lock asked, no use
// recorded, so it is safe to read whenever a screen is drawn.
async function readAccountView() {
  if (!account.signIn) return null;
  try {
    account.view = await invoke("account_status");
  } catch {
    // Keep the last view rather than invent one.
  }
  return account.view;
}

async function refreshAccountView() {
  await readAccountView();
  renderAccountSurfaces();
}

function renderAccountSurfaces() {
  renderBanners();
  renderAccountPanel();
}

// Where an account action's message goes: the lock screen when it is up,
// Settings' account panel otherwise.
function accountStatusEl() {
  return state.screen === "lock" ? $("lock-status") : $("account-note");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function setSignInBusy(busy) {
  for (const id of ["lock-signin-btn", "lock-signup-btn", "signin-btn", "signup-btn"]) {
    $(id).classList.toggle("disabled", busy);
  }
}

// "Sign in" and "Create an account", on the lock screen and the setup step
// alike (§8.41). Both are the one sign-in: the browser opens on the page the
// person chose, and comes back here either way. This waits for it (up to the
// ten minutes of §8.26 §3), then asks the lock where to go.
async function beginAccount(entry) {
  if (account.signingIn) return;
  account.signingIn = true;
  const status = state.screen === "signin" ? $("signin-status") : $("lock-status");
  setSignInBusy(true);
  status.textContent = entry === "sign_up" ? LOCK_LINES.signingUp : LOCK_LINES.signingIn;
  let signedIn = false;
  try {
    await invoke("account_sign_in", { entry });
    signedIn = true;
    status.textContent = "";
  } catch (err) {
    status.textContent = String(err);
  } finally {
    account.signingIn = false;
    setSignInBusy(false);
  }
  if (signedIn) routeAfterAccess(await askAccess());
}

function onSignIn() {
  return beginAccount("sign_in");
}

function onSignUp() {
  return beginAccount("sign_up");
}

async function onSignOut() {
  const ok = await confirmAction({
    title: "Sign out of Zaaheen?",
    body: "Your memories stay on this computer, encrypted. You'll need to sign in again to use them.",
    confirmLabel: "Sign out",
  });
  if (!ok) return;
  try {
    await invoke("account_sign_out");
  } catch (err) {
    accountStatusEl().textContent = String(err);
    return;
  }
  account.checkout = null;
  routeAfterAccess(await askAccess());
}

// Subscribe, or manage an existing subscription. The app asks for a plan and
// the browser opens where the account service says: a checkout for somebody
// without a subscription, the subscription's own page for somebody with one
// (§8.26 §5). No address crosses into this page (ADR-SEC-026).
async function onSubscribe(plan) {
  if (account.checkout || account.subscribing) return;
  account.subscribing = true;
  const status = accountStatusEl();
  status.textContent = "";
  let view = null;
  try {
    view = await invoke("account_subscribe", { plan });
  } catch (err) {
    status.textContent = String(err);
  } finally {
    account.subscribing = false;
  }
  if (view === null) {
    // A failed checkout can end with this computer signed out (§8.37).
    refreshAccountView();
    return;
  }
  account.view = view;
  await afterCheckout(view);
}

// After the browser opened: wait for the payment (refreshing only), then ask
// the lock once. A visit to an existing subscription's own page has nothing
// to wait for.
async function afterCheckout(before) {
  if (before.state !== "active") {
    const wait = { startState: before.state };
    account.checkout = wait;
    renderPaying();
    const after = await waitForPayment(wait);
    if (account.checkout === wait) account.checkout = null;
    if (after) account.view = after;
  }
  renderPaying();
  routeAfterAccess(await askAccess());
}

// §8.26 §4: after a checkout, refresh every 10 s for 10 min, and stop as soon
// as the account changes (paid, or signed out) or "I've paid" settles it.
// It only refreshes: asking the lock is afterCheckout's, once, afterwards.
async function waitForPayment(wait) {
  const until = Date.now() + CHECKOUT_POLL_FOR_MS;
  let view = null;
  while (Date.now() < until && account.checkout === wait) {
    await sleep(CHECKOUT_POLL_MS);
    if (account.checkout !== wait) break;
    try {
      view = await invoke("account_refresh_now");
    } catch {
      continue;
    }
    if (view.state !== wait.startState) break;
  }
  return view;
}

// Redraw whichever surfaces show a checkout in progress.
function renderPaying() {
  if (state.screen === "lock" && account.lockVariant) renderLock(account.lockVariant);
  renderAccountPanel();
}

// "I've paid": refresh now, then ask the lock. If the payment has not
// arrived yet, say so and keep waiting.
async function onPaid() {
  // One check at a time: a double click must not ask the lock twice.
  if (account.checkingPaid) return;
  account.checkingPaid = true;
  const status = accountStatusEl();
  status.textContent = "";
  let view = null;
  try {
    view = await invoke("account_refresh_now");
  } catch {
    // The lock below decides either way.
  }
  const locked = await askAccess();
  account.checkingPaid = false;
  const waiting = account.checkout;
  if (waiting && view && view.state === waiting.startState) {
    status.textContent = LOCK_LINES.notPaidYet;
  } else {
    account.checkout = null;
  }
  if (view) account.view = view;
  renderPaying();
  routeAfterAccess(locked);
}

// "Try again", when the subscription could not be confirmed.
async function onRetry() {
  const button = $("lock-retry-btn");
  if (button.classList.contains("disabled")) return;
  button.classList.add("disabled");
  $("lock-status").textContent = "";
  try {
    await invoke("account_refresh_now");
  } catch {
    // The lock below decides either way.
  }
  const locked = await askAccess();
  button.classList.remove("disabled");
  if (locked === "locked_cannot_confirm") $("lock-status").textContent = LOCK_LINES.stillOffline;
  routeAfterAccess(locked);
}

// The plan buttons, on the lock screen and in Settings.
function onPlanClick(e) {
  const button = e.target.closest("button[data-plan]");
  if (button) onSubscribe(button.dataset.plan);
}

// "Manage subscription". Somebody already subscribed is sent to their
// subscription's own page whatever plan is named (§8.26 §5: a live check
// decides "already subscribed"), so the plan here is only the command's
// required argument.
function onManage() {
  onSubscribe("monthly");
}

function onCloseApp() {
  try {
    if (window.__TAURI__ && window.__TAURI__.window) {
      window.__TAURI__.window.getCurrentWindow().close();
    }
  } catch (_) { /* nothing else to do; the words on screen still stand */ }
}

// -- the account panel and the banners --

function accountStateLine(view) {
  const base = friendlyAccountState(view.state) || "";
  if (view.state !== "trial" || view.days_left === null) return base;
  if (view.days_left <= 0) return `${base}, less than a day left`;
  return `${base}, ${view.days_left === 1 ? "1 day" : `${view.days_left} days`} left`;
}

function renderAccountPanel() {
  const section = $("account-section");
  if (!account.signIn || !account.view) {
    section.classList.add("hidden");
    renderSettingsNav();
    return;
  }
  const view = account.view;
  section.classList.remove("hidden");
  renderSettingsNav();
  // The row is named "Signed in as": just the address under it (session 55).
  // Text, never markup: the address came from the account service.
  $("account-who").textContent = view.signed_in
    ? view.email || "your Zaaheen account"
    : "Nobody is signed in on this computer.";
  $("account-signout").classList.toggle("hidden", !view.signed_in);
  $("account-state").textContent = accountStateLine(view);
  const subscriber = view.state === "active" || view.state === "payment_failed";
  const paying = account.checkout !== null;
  $("account-plans").classList.toggle("hidden", subscriber || paying);
  $("account-manage").classList.toggle("hidden", !subscriber || paying);
  $("account-paying").classList.toggle("hidden", !paying);
}

function trialEndsLine(days) {
  if (days <= 0) return "Your free trial ends in less than a day.";
  if (days === 1) return "Your free trial ends in 1 day.";
  return `Your free trial ends in ${days} days.`;
}

// The home screen's account notices (§8.26 §4 and §6). Information, never a
// lock: while any of these shows, everything still works.
function bannerLines(view) {
  const lines = [];
  if (!view) return lines;
  if (view.state === "trial" && view.days_left !== null && view.days_left <= TRIAL_BANNER_DAYS) {
    lines.push({ text: trialEndsLine(view.days_left), action: "subscribe", label: "Subscribe" });
  }
  if (view.state === "payment_failed") {
    lines.push({ text: "Your last payment didn't go through.", action: "update_card", label: "Update your card", warn: true });
  }
  if (view.clock_wrong) {
    lines.push({ text: "Your computer's clock is wrong. Set it to update automatically." });
  }
  return lines;
}

function bannerElement(line) {
  const row = document.createElement("div");
  row.className = line.warn ? "banner-line warn" : "banner-line";
  const text = document.createElement("span");
  text.textContent = line.text;
  row.append(text);
  if (line.action) {
    const button = document.createElement("button");
    button.className = "link-underline";
    button.dataset.action = line.action;
    button.textContent = line.label;
    row.append(button);
  }
  return row;
}

function renderBanners() {
  const host = $("account-banner");
  if (!account.signIn) {
    host.classList.add("hidden");
    return;
  }
  const lines = bannerLines(account.view);
  host.replaceChildren(...lines.map(bannerElement));
  host.classList.toggle("hidden", lines.length === 0);
}

function onBannerClick(e) {
  const button = e.target.closest("button[data-action]");
  if (!button) return;
  if (button.dataset.action === "update_card") {
    onManage();
    return;
  }
  // Subscribing from the trial banner happens in Settings, beside the plans.
  state.tab = "settings";
  state.settingsSection = "account";
  renderTab();
}

// -- download my memories (S4) --

function memoryExportFilename() {
  const d = new Date();
  const pad = (n) => String(n).padStart(2, "0");
  return `zaaheen-memories-${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}.md`;
}

// One readable file, wherever the person chooses. It works locked or not,
// which is its whole point (BRD §1.6 amendment 1).
async function exportMemories(status, button) {
  status.textContent = "";
  let destination;
  try {
    destination = await saveDialog({
      defaultPath: memoryExportFilename(),
      filters: [{ name: "Markdown", extensions: ["md"] }],
    });
  } catch (_) {
    status.textContent = "Could not open the save window. Please try again.";
    return;
  }
  // Cancelled. Not an error, and saying nothing is the right response.
  if (!destination) return;

  button.disabled = true;
  status.textContent = "Saving…";
  try {
    const n = Number(await invoke("export_memories", { destination })) || 0;
    status.textContent = n === 0
      ? "Saved. There were no memories to put in it yet."
      : `Saved ${n === 1 ? "1 memory" : `${n} memories`}.`;
  } catch (err) {
    status.textContent = String(err);
  } finally {
    button.disabled = false;
  }
}

function onLockExport() {
  exportMemories($("lock-export-status"), $("lock-export"));
}

// The lock screen offers the download whenever there is something to take
// (founder, session 55; SIGNIN-DESIGN.md §8.42). A computer with no memories
// yet, such as a new install still in its setup, has nothing to download, so
// the button is hidden there, and only on a count that reads exactly zero.
// A count that cannot be read shows it: hiding it from somebody who has
// memories is the failure that matters (BRD §1.6 amendment 1). The count
// comes from `get_settings_info`, already on the locked allowlist, and is
// asked each time the lock screen is entered, never per lock answer.
async function renderLockExport() {
  let none = false;
  try {
    const info = await invoke("get_settings_info");
    none = info !== null && typeof info === "object" && info.memory_count === 0;
  } catch {
    // Unsure: keep it.
  }
  $("lock-export-box").classList.toggle("hidden", none);
}

function onSettingsExport() {
  exportMemories($("export-memories-status"), $("export-memories"));
}

// -- where the memories live (ADR-105 L-e) --
//
// The setup's location step, Settings' "Move my memories", and the notice
// after a start that moved them. The Rust side decides everything — which
// folders are refused, when a move happens, what is removed — and answers
// with codes and with folders as the person reads them. This side shows
// them, always as text. All four commands are gated, so none of this runs
// before the lock has said yes.

// The folder picker. Guarded like the save dialog, so the page still renders
// in a plain browser for design review.
const openDialog = window.__TAURI__ && window.__TAURI__.dialog
  ? window.__TAURI__.dialog.open
  : async () => null;

const place = {
  status: null,       // the last location_status answer
  pending: null,      // { folder, target, notes }: checked, awaiting "Move them here"
  busy: false,        // a check or a move in flight
  noticeShown: false, // this start's outcome is said once
};

const MOVE_CLOSING = "Zaaheen is closing to move your memories. It opens again by itself once they're moved.";

// Why a folder or a move was refused. Same rule as the lock codes: every
// code ships with its words (`every_location_code_has_words_in_the_app`).
function friendlyLocationError(code) {
  switch (code) {
    case "location_not_local":
      return "That isn't a folder on this computer. Choose a folder on this computer, or on a drive plugged into it.";
    case "location_network":
      return "That folder is on a network drive. Choose a folder on this computer, or on a drive plugged into it.";
    case "location_unreachable":
      return "Zaaheen can't open that folder. Check it's still there, then try again.";
    case "location_drive_root":
      return "Choose a folder on that drive, not the drive itself.";
    case "location_system_folder":
      return "That folder belongs to Windows or to a program. Choose one of your own folders.";
    case "location_app_folder":
      return "That folder is one of Zaaheen's own. Choose one of your own folders.";
    case "location_cloud_synced":
      return "A cloud storage app syncs that folder, so your memories would leave this computer, and syncing can damage them. Choose a folder that isn't synced.";
    case "location_already_exists":
      return "There's already a folder called \"Zaaheen Memories\" there. Choose another folder, or move that one somewhere else first.";
    case "location_not_writable":
      return "Zaaheen can't save files in that folder. Choose another one.";
    case "location_not_enough_space":
      return "There isn't enough free space there for your memories. Free up some space, or choose another drive.";
    case "location_old_copy_waiting":
      return "Zaaheen is still removing the old copy from your last move. You can move your memories again once it's gone.";
    case "location_move_waiting":
      return "A move is already waiting. Close Zaaheen and open it again to finish it.";
    case "location_erasure_unfinished":
      return "Zaaheen is still finishing deleting your memories. Close Zaaheen and open it again first.";
    case "location_unavailable":
      return "Zaaheen can't reach your memories right now. Close Zaaheen, open it again, and try once more.";
    case "location_record_failed":
      return "Zaaheen couldn't save that. Try again in a moment.";
    case "location_nothing_to_forget":
      return "There's no old copy waiting any more.";
    case "location_old_copy_still_there":
      return "That drive is connected now, so Zaaheen will remove the old copy the next time it opens.";
    case "location_failed":
      return "That didn't work. Try again in a moment.";
    default:
      return null;
  }
}

function locationErrorLine(err) {
  const raw = String(err);
  return friendlyLocationError(raw) || raw;
}

// What to tell the person about a folder that was accepted.
function locationNoteLine(code) {
  switch (code) {
    case "other_drive":
      return "Memories on another drive only open on this computer, and only while that drive is connected. AI apps can't reach them while it's out.";
    case "cloud_unchecked":
      return "Zaaheen couldn't check whether a cloud app syncs this folder. Make sure none does.";
    default:
      return null;
  }
}

// Why a start's move did not happen.
function moveFailureLine(code) {
  switch (code) {
    case "folder_not_usable":
      return "The folder you chose can't be used any more.";
    case "not_enough_space":
      return "The drive ran out of space.";
    case "copy_failed":
      return "Copying them didn't work.";
    case "copy_did_not_match":
      return "The copy didn't match your memories, so Zaaheen removed it.";
    case "interrupted":
      return "Zaaheen was closed, or the computer went off, while they were moving.";
    case "memories_erased":
      return "Your memories are being deleted, so there was nothing to move.";
    case "record_failed":
      return "Zaaheen couldn't save where they're kept.";
    default:
      return null;
  }
}

// What this start did about a move, as sentences; none when there is
// nothing to say.
function outcomeLines(atStart) {
  if (!atStart) return [];
  switch (atStart.kind) {
    case "nothing":
      return [];
    case "moved": {
      const lines = [`Your memories are now kept in ${atStart.to}.`];
      if (atStart.not_restricted) {
        lines.push("This drive can't lock the folder to your Windows account. Your memories stay encrypted.");
      }
      if (atStart.old_copy_waiting) {
        lines.push("Zaaheen couldn't remove the old copy yet. It will try again the next time it opens.");
      }
      return lines;
    }
    case "old_copy_removed":
      return ["The old copy of your memories has now been removed."];
    case "old_copy_waiting":
      return ["The old copy of your memories hasn't been removed yet. Zaaheen will try again the next time it opens."];
    case "failed": {
      const why = moveFailureLine(atStart.reason);
      return [
        "Zaaheen couldn't move your memories.",
        ...(why ? [why] : []),
        "They're still where they were, safe and unchanged.",
        atStart.retrying ? "Zaaheen will try again the next time it opens." : "You can try again.",
      ];
    }
    case "deferred":
      return ["Zaaheen couldn't move your memories just now, because something else was using them. They're still where they were, and Zaaheen will try again the next time it opens."];
    default:
      return [];
  }
}

async function refreshLocation() {
  try {
    place.status = await invoke("location_status");
  } catch {
    // Keep the last answer rather than invent one.
  }
  return place.status;
}

// The setup's step: the folder, and — after the restart for a move asked
// for here — what happened.
async function renderLocationStep() {
  store.set(RESUME_KEY, null);
  $("location-status").textContent = "";
  hideMovePanel();
  const s = await refreshLocation();
  $("location-folder").textContent = s ? s.folder : "";
  const lines = s ? outcomeLines(s.at_start) : [];
  if (lines.length) place.noticeShown = true;
  $("location-outcome").textContent = lines.join(" ");
  $("location-outcome").classList.toggle("hidden", lines.length === 0);
}

// "Choose another folder…" and "Move my memories…": pick, check, then the
// confirmation. Never a move straight from the picker.
async function chooseFolder(host, statusEl) {
  if (place.busy) return;
  statusEl.textContent = "";
  hideMovePanel();
  let folder;
  try {
    folder = await openDialog({ directory: true, multiple: false, title: "Choose where to keep your memories" });
  } catch (_) {
    statusEl.textContent = "Could not open the folder window. Please try again.";
    return;
  }
  // Cancelled. Not an error, and saying nothing is the right response.
  if (!folder || typeof folder !== "string") return;
  place.busy = true;
  statusEl.textContent = "Checking that folder…";
  try {
    const checked = await invoke("location_check", { folder });
    statusEl.textContent = "";
    place.pending = { folder, target: checked.target, notes: checked.notes || [] };
    showMovePanel(host);
  } catch (err) {
    statusEl.textContent = locationErrorLine(err);
  } finally {
    place.busy = false;
  }
}

function showMovePanel(host) {
  const pending = place.pending;
  const panel = $("move-panel");
  host.appendChild(panel);
  $("move-target").textContent = pending.target;
  $("move-notes").replaceChildren(...pending.notes.map((code) => {
    const line = document.createElement("p");
    line.className = "move-note";
    line.textContent = locationNoteLine(code) || code;
    return line;
  }));
  // ADR-105 L4.3: the cloud check could not run, so the person confirms.
  const unchecked = pending.notes.includes("cloud_unchecked");
  $("move-cloud").classList.toggle("hidden", !unchecked);
  $("move-cloud-ok").checked = false;
  $("move-go").disabled = unchecked;
  $("move-cancel").disabled = false;
  $("move-status").textContent = "";
  panel.classList.remove("hidden");
  // In the setup, the step's own buttons step aside: one decision at a time.
  $("screen-location").classList.toggle("choosing", host === $("location-panel-host"));
}

function hideMovePanel() {
  if (place.busy) return;
  $("move-panel").classList.add("hidden");
  $("screen-location").classList.remove("choosing");
  place.pending = null;
}

function onCloudConfirm() {
  $("move-go").disabled = !$("move-cloud-ok").checked;
}

// "Move them here". The move is recorded, then Zaaheen restarts to make it
// before anything opens the memories; nothing moves if it is refused.
async function onMoveGo() {
  const pending = place.pending;
  if (!pending || place.busy || $("move-go").disabled) return;
  if (pending.notes.includes("cloud_unchecked") && !$("move-cloud-ok").checked) return;
  place.busy = true;
  $("move-go").disabled = true;
  $("move-cancel").disabled = true;
  $("move-status").textContent = MOVE_CLOSING;
  const duringSetup = state.screen === "location" && !store.get("mv_onboarded", false);
  if (duringSetup) {
    store.set(RESUME_KEY, state.onboardSteps === ONBOARDING_WITH_SIGN_IN ? "with_sign_in" : "plain");
  }
  try {
    await invoke("location_move", { folder: pending.folder });
  } catch (err) {
    if (duringSetup) store.set(RESUME_KEY, null);
    place.busy = false;
    $("move-status").textContent = locationErrorLine(err);
    $("move-go").disabled = false;
    $("move-cancel").disabled = false;
  }
}

function onMoveCancel() {
  hideMovePanel();
}

function onLocationContinue() {
  if (place.busy) return;
  showScreen("connect");
}

// Settings: a move waiting for the next start, and an old copy still to go.
async function renderMoveSection() {
  hideMovePanel();
  $("settings-move-status").textContent = "";
  const s = await refreshLocation();
  const waiting = s && s.move_waiting;
  $("move-waiting").classList.toggle("hidden", !waiting);
  if (waiting) {
    $("move-waiting").textContent = `A move to ${waiting} is waiting. It happens the next time Zaaheen opens.`;
  }
  const old = s && s.old_copy;
  $("old-copy").classList.toggle("hidden", !old);
  if (old) {
    $("old-copy-text").textContent = old.connected
      ? `An old copy of your memories is still in ${old.from}. Zaaheen removes it the next time it opens.`
      : `An old copy of your memories is still in ${old.from}, which isn't connected. Zaaheen removes it once that drive is back, and until then your memories can't be moved again.`;
    $("old-copy-forget").classList.toggle("hidden", old.connected);
  }
}

// A drive that never comes back would block every later move (L-d
// decision 9). Stopping the wait removes nothing.
async function onForgetOldCopy() {
  const old = place.status && place.status.old_copy;
  if (!old || old.connected) return;
  const ok = await confirmAction({
    title: "Stop waiting for the old copy?",
    body: `Zaaheen is waiting for ${old.from} to come back, so it can remove the old copy of your memories there. If that drive is gone for good, you can stop waiting. If it turns up later, you can delete that folder yourself. It's encrypted, and it only opens on this computer.`,
    confirmLabel: "Stop waiting",
  });
  if (!ok) return;
  let line;
  try {
    await invoke("location_forget_old_copy");
    line = "Done. You can move your memories again whenever you like.";
  } catch (err) {
    line = locationErrorLine(err);
  }
  await renderMoveSection();
  $("settings-move-status").textContent = line;
}

// The home screen's notice: what this start did about a move, once.
async function refreshLocationNotice() {
  if (place.noticeShown) return;
  place.noticeShown = true;
  const s = await refreshLocation();
  const lines = s ? outcomeLines(s.at_start) : [];
  if (!lines.length) return;
  $("location-notice-text").textContent = lines.join(" ");
  $("location-notice").classList.remove("hidden");
}

function onLocationNoticeOk() {
  $("location-notice").classList.add("hidden");
}

// -- the link to the background part (ADR-108 D6) --
//
// The window opens without waiting for the part of Zaaheen that holds the
// memories. While it starts, the home screen says so; `link_state` only reads
// (it never starts anything), and the watch stops once the link is serving or
// has said why it could not start.
const LINK_POLL_MS = 1000;
let linkWatching = false;

function linkLine(s) {
  switch (s.state) {
    case "connecting":
      return "Opening your memories…";
    case "tidying":
      return friendlyAccountError("vault_maintenance_in_progress");
    case "failed":
      return s.message || friendlyAccountError("keeper_unreachable");
    default:
      return null;
  }
}

async function watchLink() {
  if (linkWatching) return;
  linkWatching = true;
  try {
    for (;;) {
      let s;
      try {
        s = await invoke("link_state");
      } catch {
        break;
      }
      const line = linkLine(s);
      $("link-notice-text").textContent = line || "";
      $("link-notice").classList.toggle("hidden", !line);
      if (s.state === "serving" || s.state === "failed") break;
      await new Promise((r) => setTimeout(r, LINK_POLL_MS));
    }
  } finally {
    linkWatching = false;
  }
}

// -- maintenance tab --

const WEEKDAY_NAMES = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

function pad2(n) { return String(n).padStart(2, "0"); }

// 24h hour+minute → "3:00 AM"
function friendlyTime(hour, minute) {
  const h12 = ((hour + 11) % 12) + 1;
  return `${h12}:${pad2(minute)} ${hour < 12 ? "AM" : "PM"}`;
}

// Map an opaque backend code (§11.7.2) to plain English.
function friendlyMaintError(code) {
  switch (code) {
    case "maintenance_vault_busy":
      return "an agent was connected, so it will run at the next opportunity";
    case "maintenance_paused_not_subscribed":
      return "paused until you subscribe";
    case "maintenance_engine_unavailable":
      return "the consolidation engine is still downloading";
    case "maintenance_run_failed":
      return "it didn't finish, and it will try again on schedule";
    case "maintenance_schedule_failed":
      return "the schedule couldn't be updated";
    default:
      return code;
  }
}

// Plain-English line for a locked vault (SIGNIN-DESIGN.md §8.26 §6.4).
//
// A code with no arm here falls through to showing the user the raw code, so
// every code ships with its line — pinned by the Rust-side test
// `every_lock_code_has_a_plain_english_line_in_the_app`.
//
// Deliberately NOT the wording the MCP gate sends an agent: that text says
// "Open the Zaaheen app on this computer", which is nonsense shown inside the
// app itself. These are the lock screen's headings (founder-approved, §8.39),
// for the call site that shows a refusal inline while the lock screen takes
// over.
function friendlyLockError(code) {
  switch (code) {
    case "locked_signed_out":
      return "Sign in to open your memories.";
    case "locked_cannot_confirm":
      return "We couldn't confirm your subscription.";
    case "locked_trial_ended":
      return "Your free trial has ended.";
    case "locked_subscription_ended":
      return "Your subscription has ended.";
    case "locked_unlocking":
      return "Zaaheen is unlocking. Try again in a moment.";
    default:
      // null, NOT the code: the `invoke` wrapper uses null to mean "not a
      // lock error, re-throw it untouched". Returning the code here would
      // swallow every other error into a fake lock message.
      return null;
  }
}

// Plain-English line for an account or export failure (S3 steps 4b, 4c).
//
// Same rule as the lock codes: a code with no arm here falls through to
// showing the user the raw code, so every arm ships with its code. Pinned by
// `every_account_code_has_a_plain_english_line_in_the_app`. Whole sentences
// since 4d-2, because they now stand on their own under a button.
function friendlyAccountError(code) {
  switch (code) {
    case "account_sign_in_did_not_finish":
      return "Signing in didn't finish. Try again.";
    case "account_busy":
      return "Something else on this computer is using your account. Try again in a moment.";
    case "account_unreachable":
      return "Zaaheen couldn't reach the internet. Check your connection and try again.";
    case "account_refused":
      return "That didn't go through. Try again in a moment.";
    case "account_bad_plan":
      return "That plan isn't one we offer.";
    case "account_unavailable":
      return "Zaaheen couldn't reach your account on this computer. Close Zaaheen and open it again.";
    case "export_bad_destination":
      return "That isn't somewhere Zaaheen can save the file. Try a different folder.";
    case "export_read_failed":
      return "Your memories couldn't be read just now. Try again in a moment.";
    case "export_write_failed":
      return "The file couldn't be saved. Check there's room on the disk and try again.";
    // ADR-108: the desktop is a client of the background part that holds the
    // memories. Founder-approved lines (session 60).
    case "vault_maintenance_in_progress":
      return "Zaaheen is tidying your memories. This can take a few minutes. Try again soon.";
    case "update_needed":
      return "Zaaheen was updated. Close it and open it again to carry on.";
    case "outcome_unknown":
      return "We couldn't confirm that was saved. Check your memories before adding it again.";
    case "keeper_unreachable":
      return "Zaaheen couldn't start its background part. Close it and open it again; if this keeps happening, contact customerservice@zaaheen.com.";
    case "admin_busy":
      return "Zaaheen is answering another app right now. Try again in a moment.";
    default:
      return null;
  }
}

// What each account state says on screen: the account panel's state line
// (S3 step 4b, shown since 4d-2). No raw state string can reach a person.
// Pinned by `every_account_state_has_a_plain_english_line_in_the_app`.
function friendlyAccountState(state) {
  switch (state) {
    case "signed_out":
      return "Not signed in";
    case "no_lease":
      return "Checking your subscription…";
    case "trial":
      return "Free trial";
    case "active":
      return "Subscribed";
    case "payment_failed":
      return "Your last payment didn't go through";
    case "ended":
      return "Subscription ended";
    case "cannot_confirm":
      return "We couldn't confirm your subscription";
    default:
      return null;
  }
}

// Turn a run's raw stdout into one plain-English clause.
//
// The backend stores `zaaheen`'s output verbatim (truncated to 500 chars), so
// naively showing its first line reads "starting consolidation run..." — the
// least informative line in the report. Pull the counts out instead, and say
// plainly when there was nothing to do.
function summariseRun(summary) {
  const text = String(summary || "");
  const count = (label) => {
    const m = text.match(new RegExp(label + "\\s*:\\s*(\\d+)"));
    return m ? Number(m[1]) : null;
  };
  const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

  const merged = count("merges applied");
  const archived = count("memories archived");
  const contradictions = count("contradictions queued");
  if (merged === null && archived === null && contradictions === null) {
    return ""; // unrecognised format — say nothing rather than something wrong
  }

  const parts = [];
  if (merged) parts.push(`merged ${plural(merged, "duplicate", "duplicates")}`);
  if (contradictions) parts.push(`flagged ${plural(contradictions, "contradiction", "contradictions")}`);
  if (archived) parts.push(`archived ${plural(archived, "old memory", "old memories")}`);
  return parts.length ? parts.join(", ") : "nothing needed tidying";
}

async function renderMaintenance() {
  const status = $("maint-status");
  let view;
  try {
    view = await invoke("get_maintenance_schedule");
    maintState.view = view;
  } catch (err) {
    if (status) status.innerHTML = `<div class="empty-note">Couldn't load consolidation settings: ${esc(String(err))}</div>`;
    return;
  }

  // Reflect the saved schedule into the controls.
  $("maint-enabled").checked = !!view.enabled;
  $("maint-frequency").value = view.frequency === "weekly" ? "weekly" : "daily";
  $("maint-weekday").value = String(view.weekday || 0);
  $("maint-time").value = `${pad2(view.hour)}:${pad2(view.minute)}`;
  $("maint-weekday-wrap").classList.toggle("hidden", $("maint-frequency").value !== "weekly");

  let html = "";
  if (view.enabled && view.registered) {
    const when = view.frequency === "weekly"
      ? `every ${WEEKDAY_NAMES[view.weekday] || "week"} at ${friendlyTime(view.hour, view.minute)}`
      : `every day at ${friendlyTime(view.hour, view.minute)}`;
    html += `<div class="maint-on">On, runs ${esc(when)}.</div>`;
  } else if (view.enabled && !view.registered) {
    html += `<div class="maint-warn">Turned on, but the scheduled run isn't registered. Try saving again.</div>`;
  } else {
    html += `<div class="maint-off">Off. Your vault won't be consolidated automatically.</div>`;
  }
  if (view.last_run) {
    const lr = view.last_run;
    if (lr.ok) {
      const what = summariseRun(lr.summary);
      html += `<div class="maint-last">Last run ${esc(relTime(lr.finished_at))}${what ? ": " + esc(what) : ""}.</div>`;
    } else {
      html += `<div class="maint-last">Last attempt ${esc(relTime(lr.finished_at))}: ${esc(friendlyMaintError(lr.summary))}.</div>`;
    }
  }
  if (status) status.innerHTML = html;

  renderMaintEngineStatus();
}

async function saveMaintenance() {
  const note = $("maint-save-note");
  const enabled = $("maint-enabled").checked;
  const frequency = $("maint-frequency").value === "weekly" ? "weekly" : "daily";
  const weekday = Number($("maint-weekday").value) || 0;
  const [hour, minute] = ($("maint-time").value || "03:00").split(":").map(Number);
  if (note) note.textContent = "Saving…";
  try {
    await invoke("set_maintenance_schedule", { enabled, frequency, weekday, hour, minute });
    if (note) note.textContent = enabled ? "Saved ✓" : "Turned off ✓";
    setTimeout(() => { if (note && (note.textContent === "Saved ✓" || note.textContent === "Turned off ✓")) note.textContent = ""; }, 2400);
    // A returning user turning maintenance ON is when we fetch the engine (it
    // short-circuits if already present) — not on every launch.
    if (enabled) startMaintenanceFetch();
    renderMaintenance();
  } catch {
    if (note) note.textContent = "Couldn't save. Please try again.";
  }
}

async function runMaintenanceNow() {
  if (state.maintRunning || !maintEngineReady()) return;
  state.maintRunning = true;
  // Card 3 has its own note — sharing card 2's would report a run's outcome
  // under "Save schedule".
  const note = $("maint-run-note");
  renderMaintEngineStatus(); // disables the run button while it runs
  if (note) note.textContent = "Consolidating now. This can take a few minutes…";
  // The outcome stays until the next run (session 64: "Done ✓" was cleared
  // after 3 s, so after a few minutes' wait the founder saw "no message
  // just finished").
  try {
    await invoke("run_maintenance_now");
    if (note) note.textContent = "Done. Your memories are tidied up.";
  } catch (err) {
    if (note) note.textContent = friendlyMaintError(String(err)) + ".";
  } finally {
    state.maintRunning = false;
    renderMaintenance();
  }
}

// Missed-run catch-up (task 10). The OS scheduler already catches up a run
// missed while the machine slept (Task Scheduler StartWhenAvailable / launchd
// wake / systemd Persistent). This is the app-side safety net for the harder
// case — the machine off for longer, or the OS task never firing: on launch,
// if maintenance is on and overdue, run one pass in the background.
//
// Safe to fire even if a scheduled run already happened: consolidation is
// idempotent (BRD §5.6), and `run_maintenance_now` skips cleanly if the vault
// is busy. Runs at most once per session and only when the engine is ready.
let catchUpDone = false;
async function catchUpMaintenanceIfDue() {
  if (catchUpDone) return;
  let view;
  try {
    view = await invoke("get_maintenance_schedule");
    maintState.view = view;
  } catch { return; }
  if (!view.enabled) { catchUpDone = true; return; }
  // Engine not ready yet — leave the flag unset so a later call (once the
  // download finishes) can still catch up this session.
  if (!view.engine_ready && !maintFetch.done) return;

  const intervalMs = view.frequency === "weekly" ? 7 * 864e5 : 864e5;
  const last = view.last_run && view.last_run.ok ? Date.parse(view.last_run.finished_at) : 0;
  const overdue = !last || (Date.now() - last) > intervalMs;
  catchUpDone = true;
  if (!overdue) return;
  tracingHint("maintenance overdue on launch, running a catch-up pass");
  // Fire-and-forget: never block the window, and let the tab reflect the
  // result on its next render. A catch-up never interrupts anyone (ADR-108
  // D8): it waits for the vault like the nightly run, and once it has it,
  // runs only if a run is still due.
  invoke("run_maintenance_now", { catchUp: true }).catch(() => {});
}

// A no-op console breadcrumb (kept quiet — the webview console is dev-only).
function tracingHint(msg) { try { console.debug(`[maintenance] ${msg}`); } catch { /* ignore */ } }

// Onboarding step 4 → finish. Optionally register the schedule the user chose,
// then hand off to finishOnboarding (which still gates on the recall engine).
async function finishFromMaintenance() {
  const cta = $("maint-onboard-cta");
  if (cta.classList.contains("disabled")) return;
  cta.classList.add("disabled");
  const on = $("maint-onboard-on").checked;
  let ok = true;
  if (on) {
    const [hour, minute] = ($("maint-onboard-time").value || "03:00").split(":").map(Number);
    try {
      await invoke("set_maintenance_schedule", { enabled: true, frequency: "daily", weekday: 0, hour, minute });
    } catch { ok = false; }
  }
  $("maint-onboard-note").textContent = ok
    ? "Finishing setup…"
    : "Couldn't schedule it now. You can turn it on later in Consolidation. Finishing…";
  await finishOnboarding(state.keptFirstMemory);
}

// -- footer --

function renderFooter() {
  // Counts the AI apps that have connected (session 64: the keeper's live
  // list alone read "none" after every tidy-up), not apps the person merely
  // copied a setting for.
  const n = Object.keys(knownApps()).length;
  $("footer-status").textContent = "Encrypted on this device · " +
    (n > 0 ? `${n} AI app${n > 1 ? "s" : ""} connected` : "no AI app connected yet");
}

// ---------------------------------------------------------------- wiring

function init() {
  // welcome
  $("begin-btn").addEventListener("click", beginSetup);

  // sign in (the setup step) and the lock screen
  $("signin-btn").addEventListener("click", onSignIn);
  $("signup-btn").addEventListener("click", onSignUp);
  $("lock-signin-btn").addEventListener("click", onSignIn);
  $("lock-signup-btn").addEventListener("click", onSignUp);
  $("lock-plans").addEventListener("click", onPlanClick);
  $("lock-paid").addEventListener("click", onPaid);
  $("lock-retry-btn").addEventListener("click", onRetry);
  $("lock-close-btn").addEventListener("click", onCloseApp);
  $("lock-signout").addEventListener("click", onSignOut);
  $("lock-export").addEventListener("click", onLockExport);

  // home — the account notices, and Settings' account panel
  $("account-banner").addEventListener("click", onBannerClick);
  $("account-signout").addEventListener("click", onSignOut);
  $("account-plans").addEventListener("click", onPlanClick);
  $("account-manage-btn").addEventListener("click", onManage);
  $("account-paid").addEventListener("click", onPaid);
  $("export-memories").addEventListener("click", onSettingsExport);
  $("erase-manage").addEventListener("click", onManage);

  // where the memories live (ADR-105 L-e): the setup's step, Settings, the
  // move's confirmation and the home notice
  $("location-cta").addEventListener("click", onLocationContinue);
  $("location-change").addEventListener("click", () =>
    chooseFolder($("location-panel-host"), $("location-status")));
  $("move-reveal").addEventListener("click", () =>
    chooseFolder($("settings-move-host"), $("settings-move-status")));
  $("move-cloud-ok").addEventListener("change", onCloudConfirm);
  $("move-go").addEventListener("click", onMoveGo);
  $("move-cancel").addEventListener("click", onMoveCancel);
  $("old-copy-forget").addEventListener("click", onForgetOldCopy);
  $("location-notice-ok").addEventListener("click", onLocationNoticeOk);

  // connect
  renderAgentCards();
  $("agent-grid").addEventListener("click", (e) => {
    const card = e.target.closest(".agent-card");
    if (!card) return;
    state.agentPicked = card.dataset.i === "connected" ? null : Number(card.dataset.i);
    renderAgentCards();
    $("connect-head").scrollIntoView({ behavior: "smooth", block: "nearest" });
  });
  $("copy-snippet").addEventListener("click", () => {
    if (state.agentPicked !== null) copyText(AGENTS[state.agentPicked].snippet(state.serverCommand), $("copy-snippet"));
  });
  $("copy-tip").addEventListener("click", () => copyText(TIP_LINE, $("copy-tip")));
  $("snippet-openers").addEventListener("click", (e) => {
    const button = e.target.closest("[data-opener]");
    const agent = state.agentPicked === null ? null : AGENTS[state.agentPicked];
    const opener = button && agent && agent.openers ? agent.openers[Number(button.dataset.opener)] : null;
    if (opener) copyText(opener.command, button);
  });
  $("connect-auto-btn").addEventListener("click", onConnectAuto);
  $("connect-auto-show").addEventListener("click", () => {
    invoke("show_claude_extension").catch(() => { /* nothing to add: the steps are on screen */ });
  });
  $("connect-cta").addEventListener("click", connectContinue);
  $("connect-skip").addEventListener("click", () => showScreen("memory"));

  // first memory
  renderMemoryScreen();
  $("mem-text").addEventListener("input", renderMemoryScreen);
  $("mem-save").addEventListener("click", saveFirstMemory);
  $("mem-skip").addEventListener("click", () => {
    state.keptFirstMemory = false;
    showScreen("maintenance");
  });

  // onboarding step 4 — automatic maintenance
  $("maint-onboard-on").addEventListener("change", () =>
    $("maint-onboard-time-wrap").classList.toggle("hidden", !$("maint-onboard-on").checked));
  $("maint-onboard-off").addEventListener("change", () =>
    $("maint-onboard-time-wrap").classList.add("hidden"));
  $("maint-onboard-cta").addEventListener("click", finishFromMaintenance);

  // home — search
  //
  // ADR-090: a vault search runs on ENTER, not per keystroke.
  //
  // The old 350 ms debounce stopped keystroke *spam* but still fired a real
  // query on every natural typing pause, and each one became a full ranking
  // pass. Measured live 2026-07-22: typing "work" ran four passes, "keyboard"
  // three, all queued behind each other — which is why a single search felt
  // like it took 26 s when one pass is ~5 s. `renderMemList`'s sequence guard
  // discarded the stale RESPONSES but the work was already paid for.
  //
  // That waste is invisible on a local vault (the user's own CPU) and real on
  // the hosted tier, where every pass is compute we pay for. Agents over MCP
  // were never affected — they send one query per question rather than typing.
  //
  // Clearing the box still updates instantly: returning to the recent list is
  // a cheap unfiltered read, not a search, so there is nothing to defer.
  let clearDebounce = null;
  $("search-input").addEventListener("input", () => {
    const next = $("search-input").value;
    const wasEmpty = state.query.trim() === "";
    state.query = next;
    if (next.trim() === "") {
      // Fall back to the recent list as soon as the box empties.
      clearTimeout(clearDebounce);
      clearDebounce = setTimeout(renderMemList, 120);
    } else if (wasEmpty) {
      // Typing has started: cancel a pending "show recent" so it cannot land
      // after the user has begun a query and blank their in-progress view.
      clearTimeout(clearDebounce);
    }
  });
  $("search-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      clearTimeout(clearDebounce);
      state.query = $("search-input").value;
      renderMemList();
    }
  });
  document.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      if (state.screen === "home") {
        state.tab = "memories";
        renderTab();
        $("search-input").focus();
      }
    }
  });

  // home — inline add
  $("add-toggle").addEventListener("click", () => toggleInlineAdd());
  $("add-save").addEventListener("click", saveInlineMemory);
  $("add-cancel").addEventListener("click", () => toggleInlineAdd(false));

  // home — boundaries
  $("boundary-add-label").addEventListener("click", () => toggleBoundaryAdd());
  $("boundary-save").addEventListener("click", saveBoundary);
  $("boundary-cancel").addEventListener("click", () => toggleBoundaryAdd(false));
  $("boundary-name").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); saveBoundary(); }
  });
  $("boundary-desc").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); saveBoundary(); }
  });

  // home — maintenance
  $("maint-frequency").addEventListener("change", () =>
    $("maint-weekday-wrap").classList.toggle("hidden", $("maint-frequency").value !== "weekly"));
  $("maint-save").addEventListener("click", saveMaintenance);
  $("maint-run").addEventListener("click", runMaintenanceNow);

  // home — settings
  $("settings-nav").addEventListener("click", onSettingsNav);
  $("export-logs").addEventListener("click", exportLogs);

  // home — settings — delete everything (ADR-SEC-008)
  $("erase-reveal").addEventListener("click", revealEraseConfirm);
  $("erase-cancel").addEventListener("click", resetEraseConfirm);
  $("erase-phrase").addEventListener("input", onErasePhraseInput);
  $("erase-confirm-btn").addEventListener("click", eraseEverything);

  // Progress events for the two engine downloads. Only the listeners are
  // attached here: the downloads themselves are gated commands, started by
  // startEntitledWork once the lock has said yes.
  listenEvent("recall-engine://progress", (event) => {
    const p = event && event.payload;
    if (!p) return;
    engineFetch.percent = typeof p.percent === "number" ? p.percent : 0;
    engineFetch.active = true;
    engineFetch.tick += 1;
    renderEngineRow();
    renderFinishWait();
  }).catch(() => { /* no event bridge outside Tauri, so progress just stays hidden */ });
  listenEvent("maintenance-engine://progress", (event) => {
    const p = event && event.payload;
    if (!p) return;
    maintFetch.percent = typeof p.percent === "number" ? p.percent : 0;
    maintFetch.active = true;
    renderMaintEngineStatus();
  }).catch(() => { /* no event bridge outside Tauri */ });

  // A new install starts its welcome at once: the checks it replays really
  // happened, and none of them needs the lock. A returning computer waits
  // for the lock's first answer (enterApp) with nothing on screen, and says
  // what it is doing if that takes more than a moment.
  if (state.screen === "welcome") {
    showScreen("welcome");
  } else {
    setTimeout(showChecking, CHECKING_AFTER_MS);
  }
}

// Wire the page, wait for a start that is moving the memories (ADR-105 L-f),
// then ask the lock where to go (SIGNIN-DESIGN.md §8.38: asked when the app
// opens, and after an account action).
document.addEventListener("DOMContentLoaded", async () => {
  init();
  await untilStarted();
  enterApp();
});
