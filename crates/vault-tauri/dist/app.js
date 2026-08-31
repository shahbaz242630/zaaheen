// Zaaheen — V0.2 beta UI logic ("Quiet" direction).
// Vanilla JS, no framework, no eval, no remote resources (CSP: 'self').
// All user/vault content rendered through esc() — BRD §11.12 vault-tauri
// checklist (XSS prevention in webview).

// Guard the bridge so the UI still renders (with failing commands) when
// opened outside Tauri — e.g. design review in a plain browser.
const invoke = window.__TAURI__ && window.__TAURI__.core
  ? window.__TAURI__.core.invoke
  : async () => { throw new Error("vault engine not connected (running outside Tauri)"); };

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
    phases: ["locating vault store", "deriving key — Credential Manager", "unsealing store — AES-256"],
    done: "vault unsealed — AES-256, key in Windows Credential Manager",
  },
  // White-label rule (founder, 2026-07-11): never name the underlying
  // models or stack in the UI — the user-facing promise is "on-device".
  {
    phases: ["waking the recall engine", "loading on-device intelligence", "indexing your memory space"],
    done: "recall engine ready — runs entirely on this device",
  },
  {
    phases: ["attaching audit log", "opening default boundary"],
    done: "audit log active — every read & write recorded",
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
        ? "The consolidation engine will finish downloading later — you can still turn this on."
        : (maintFetch.active
            ? `Preparing the consolidation engine in the background — ${maintFetch.percent}%. You can finish now either way.`
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
          note.textContent = "The consolidation engine didn't finish downloading — it will retry when you turn consolidation on, or on the next scheduled run.";
        } else if (maintFetch.active) {
          note.textContent = `Preparing the consolidation engine — ${maintFetch.percent}%. Step 3 becomes available once it is ready.`;
        } else {
          note.textContent = "Consolidation uses a one-time on-device engine (~2.5 GB). It downloads when you turn consolidation on.";
        }
      }
    }
    const runBtn = $("maint-run");
    if (runBtn) runBtn.classList.toggle("disabled", !maintEngineReady() || state.maintRunning);
  }
}

// MCP connection snippets name `zaaheen`, the binary the installer actually
// lays down and puts on PATH (ADR-SEC-018), invoked with no paths because
// ADR-101 taught it to find its own vault and models. This pairing is pinned by
// installer_contract.rs: a comment claiming the snippet works is worth nothing,
// which is exactly how `vault-cli` survived the rename here and would have sent
// every tester a config for a program that does not exist.
const SNIPPET_JSON = `{
  "mcpServers": {
    "zaaheen": {
      "command": "zaaheen",
      "args": ["mcp", "serve"]
    }
  }
}`;
const SNIPPET_TOML = `[mcp_servers.zaaheen]
command = "zaaheen"
args = ["mcp", "serve"]`;

const AGENTS = [
  { name: "Claude Code", desc: "CLI agent, connects over stdio", hint: "Add this to your MCP settings, then restart Claude Code:", snippet: SNIPPET_JSON },
  { name: "Claude Desktop", desc: "Edit its MCP config file", hint: "Add this to claude_desktop_config.json:", snippet: SNIPPET_JSON },
  { name: "Codex", desc: "OpenAI's coding agent", hint: "Add this to ~/.codex/config.toml as an MCP server:", snippet: SNIPPET_TOML },
  { name: "Cursor", desc: "AI code editor with MCP support", hint: "Add this to Cursor's MCP settings (mcp.json):", snippet: SNIPPET_JSON },
  { name: "Antigravity", desc: "Google's agentic IDE", hint: "Add this to Antigravity's MCP config:", snippet: SNIPPET_JSON },
  { name: "Custom client", desc: "Any MCP-compatible agent", hint: "Point your client at the vault's stdio server:", snippet: SNIPPET_JSON },
];

const state = {
  screen: store.get("mv_onboarded", false) ? "home" : "welcome",
  checksDone: 0,
  agentPicked: null,
  memType: "semantic",
  addType: "semantic",
  tab: "memories",
  query: "",
  // `agents` is the local record of which agent the user set up a config
  // snippet for during onboarding — it drives the connect UI only. The
  // authoritative list of agents holding vault access is `grantedAgents`,
  // read from the backend registry (slice 2).
  agents: store.get("mv_agents", []),
  grantedAgents: [],                         // from list_agents (authoritative)
  boundaries: [],                            // from list_boundaries
  showConnectPanel: false,
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
  for (const s of ["welcome", "connect", "memory", "maintenance", "home"]) {
    $(`screen-${s}`).classList.toggle("hidden", s !== name);
  }
  const steps = ["welcome", "connect", "memory", "maintenance"];
  const idx = steps.indexOf(name);
  $("progress-dots").classList.toggle("hidden", idx === -1);
  if (idx !== -1) {
    [...$("progress-dots").children].forEach((el, i) => el.classList.toggle("on", i <= idx));
  }
  if (name === "welcome") runChecks();
  if (name === "maintenance") renderMaintEngineStatus();
  if (name === "home") renderHome();
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
    lbl.textContent = "couldn't finish preparing the recall engine — you can retry in a moment";
    return;
  }
  row.className = "check-item active";
  ico.textContent = SPIN[engineFetch.tick % SPIN.length];
  lbl.textContent = `preparing recall engine — ${engineFetch.percent}%`;
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
      "Getting your recall engine ready — you can search now, results sharpen up in a moment.";
  }
}

function renderBeginState() {
  const ready = state.checksDone >= CHECK_DEFS.length;
  // The Begin button stays invisible (space reserved) until every check
  // has finished, then fades in (founder feedback 2026-07-11).
  $("begin-btn").classList.toggle("reveal", ready);
  $("begin-hint").textContent = ready
    ? "Three gentle steps — about two minutes."
    : "Establishing your vault — nothing leaves this device…";
}

// -- connect agent ----------------------------------------------------------

function renderAgentCards() {
  $("agent-grid").innerHTML = AGENTS.map((a, i) => `
    <div class="agent-card${state.agentPicked === i ? " picked" : ""}" data-i="${i}">
      <div class="nm">${esc(a.name)}</div>
      <div class="ds">${esc(a.desc)}</div>
    </div>`).join("");
  const picked = state.agentPicked;
  $("snippet-wrap").classList.toggle("hidden", picked === null);
  if (picked !== null) {
    $("snippet-hint").textContent = AGENTS[picked].hint;
    $("snippet-code").textContent = AGENTS[picked].snippet;
    $("connect-cta").textContent = "I've added it — continue";
  } else {
    $("connect-cta").textContent = "Continue";
  }
}

function connectContinue() {
  if (state.agentPicked !== null) {
    const name = AGENTS[state.agentPicked].name;
    if (!state.agents.some((a) => a.name === name)) {
      state.agents.push({ name, transport: "mcp · stdio", when: Date.now() });
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

// Reflect "still preparing" on the last onboarding screen. The two buttons
// that leave onboarding are disabled while we wait, so the gate cannot be
// clicked past, and the message carries the live percentage so the wait never
// looks like a hang.
function renderFinishWait() {
  const waiting = engineFetch.waiting;
  const save = $("mem-save");
  const skip = $("mem-skip");
  if (save) save.classList.toggle("disabled", waiting);
  if (skip) skip.classList.toggle("disabled", waiting);
  if (!waiting) return;
  const err = $("mem-err");
  if (err) {
    err.textContent = engineFetch.failed
      ? "Couldn't finish preparing the recall engine. Check your connection — retrying."
      : `Finishing setup — preparing your recall engine (${engineFetch.percent}%)…`;
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
      const err = $("mem-err");
      if (err) {
        err.textContent = "Couldn't finish preparing the recall engine — continuing anyway. Recall will sharpen once it completes.";
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
  // The footer reports how many agents hold access, so it needs the registry
  // even when the user never opens the Agents tab. Fire-and-forget: a failed
  // read leaves the footer at its last known value rather than blocking home.
  refreshGrantedAgents();
}

async function refreshGrantedAgents() {
  try {
    state.grantedAgents = await invoke("list_agents");
    renderFooter();
  } catch { /* footer keeps its last known value */ }
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
    el.addEventListener("click", () => { state.tab = el.dataset.k; renderTab(); });
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
      })), "Nothing here yet — keep a memory below, or connect an agent and let it remember for you.");
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
    })), "Nothing recalled for that — yet.");
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

async function renderAgents() {
  // Slice 2: the registry of agents that actually hold vault access, not the
  // browser-local list of cards the user clicked during onboarding. An agent
  // appears here once it has been granted a connection token.
  let agents = [];
  try {
    agents = await invoke("list_agents");
    state.grantedAgents = agents;
    renderFooter();
  } catch (err) {
    $("agent-rows").innerHTML = `<div class="empty-note">Couldn't load agents: ${esc(String(err))}</div>`;
    $("no-agents").classList.add("hidden");
    $("connect-panel-label").textContent = state.showConnectPanel
      ? "hide connection details" : "+ Connect an agent";
    $("agents-panel").classList.toggle("hidden", !state.showConnectPanel);
    $("agents-snippet").textContent = SNIPPET_JSON;
    return;
  }

  if (!agents.length) {
    $("agent-rows").innerHTML = "";
    $("no-agents").classList.remove("hidden");
  } else {
    $("no-agents").classList.add("hidden");
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
        ${a.active ? `<button class="revoke">revoke</button>` : `<span class="ac">—</span>`}
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
  $("connect-panel-label").textContent = state.showConnectPanel
    ? "hide connection details" : "+ Connect an agent";
  $("agents-panel").classList.toggle("hidden", !state.showConnectPanel);
  $("agents-snippet").textContent = SNIPPET_JSON;
}

// -- settings tab --

async function renderSettings() {
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
      : { label: "Audit log", value: "history could not be verified — the record may have been altered", good: false },
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
    // property the user was trying to obtain.
    $("erase-status").textContent =
      "Your memories were NOT deleted, and they are still readable. Nothing was changed. Please try again, or restart Zaaheen and retry.";
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
    case "maintenance_engine_unavailable":
      return "the consolidation engine is still downloading";
    case "maintenance_run_failed":
      return "it didn't finish — it will try again on schedule";
    case "maintenance_schedule_failed":
      return "the schedule couldn't be updated";
    default:
      return code;
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
    html += `<div class="maint-on">On — runs ${esc(when)}.</div>`;
  } else if (view.enabled && !view.registered) {
    html += `<div class="maint-warn">Turned on, but the scheduled run isn't registered. Try saving again.</div>`;
  } else {
    html += `<div class="maint-off">Off — your vault won't be consolidated automatically.</div>`;
  }
  if (view.last_run) {
    const lr = view.last_run;
    if (lr.ok) {
      const what = summariseRun(lr.summary);
      html += `<div class="maint-last">Last run ${esc(relTime(lr.finished_at))}${what ? " — " + esc(what) : ""}.</div>`;
    } else {
      html += `<div class="maint-last">Last attempt ${esc(relTime(lr.finished_at))} — ${esc(friendlyMaintError(lr.summary))}.</div>`;
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
    if (note) note.textContent = "Couldn't save — please try again.";
  }
}

async function runMaintenanceNow() {
  if (state.maintRunning || !maintEngineReady()) return;
  state.maintRunning = true;
  // Card 3 has its own note — sharing card 2's would report a run's outcome
  // under "Save schedule".
  const note = $("maint-run-note");
  renderMaintEngineStatus(); // disables the run button while it runs
  if (note) note.textContent = "Consolidating now — this can take a few minutes…";
  try {
    await invoke("run_maintenance_now");
    if (note) note.textContent = "Done ✓";
  } catch (err) {
    if (note) note.textContent = friendlyMaintError(String(err)) + ".";
  } finally {
    state.maintRunning = false;
    setTimeout(() => { if (note) note.textContent = ""; }, 3200);
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
  tracingHint("maintenance overdue on launch — running a catch-up pass");
  // Fire-and-forget: never block the window, and let the tab reflect the
  // result on its next render.
  invoke("run_maintenance_now").catch(() => {});
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
    : "Couldn't schedule it now — you can turn it on later in Consolidation. Finishing…";
  await finishOnboarding(state.keptFirstMemory);
}

function replayWelcome() {
  state.tab = "memories";
  showScreen("welcome");
}

// -- footer --

function renderFooter() {
  // Counts agents that actually hold vault access (the backend registry),
  // not agents the user merely copied a config snippet for.
  const n = state.grantedAgents.filter((a) => a.active).length;
  $("footer-status").textContent = "Encrypted on this device · " +
    (n > 0 ? `${n} agent${n > 1 ? "s" : ""} connected` : "no agents connected yet");
}

// ---------------------------------------------------------------- wiring

function init() {
  // welcome
  $("begin-btn").addEventListener("click", () => {
    if (state.checksDone >= CHECK_DEFS.length) showScreen("connect");
  });

  // connect
  renderAgentCards();
  $("agent-grid").addEventListener("click", (e) => {
    const card = e.target.closest(".agent-card");
    if (!card) return;
    state.agentPicked = Number(card.dataset.i);
    renderAgentCards();
    $("snippet-wrap").scrollIntoView({ behavior: "smooth", block: "end" });
  });
  $("copy-snippet").addEventListener("click", () => {
    if (state.agentPicked !== null) copyText(AGENTS[state.agentPicked].snippet, $("copy-snippet"));
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

  // home — agents
  $("connect-panel-label").addEventListener("click", () => {
    state.showConnectPanel = !state.showConnectPanel;
    renderAgents();
  });
  $("copy-agents-snippet").addEventListener("click", () =>
    copyText(SNIPPET_JSON, $("copy-agents-snippet")));

  // home — maintenance
  $("maint-frequency").addEventListener("change", () =>
    $("maint-weekday-wrap").classList.toggle("hidden", $("maint-frequency").value !== "weekly"));
  $("maint-save").addEventListener("click", saveMaintenance);
  $("maint-run").addEventListener("click", runMaintenanceNow);

  // home — settings
  $("replay-welcome").addEventListener("click", replayWelcome);
  $("export-logs").addEventListener("click", exportLogs);

  // home — settings — delete everything (ADR-SEC-008)
  $("erase-reveal").addEventListener("click", revealEraseConfirm);
  $("erase-cancel").addEventListener("click", resetEraseConfirm);
  $("erase-phrase").addEventListener("input", onErasePhraseInput);
  $("erase-confirm-btn").addEventListener("click", eraseEverything);

  // First-run acquisition. Started for EVERY launch, not just onboarding
  // ones: a returning user whose files were never fetched (or were removed)
  // has no onboarding gate to pass through, so this is their only route to a
  // fully prepared engine. It short-circuits once the files are present and
  // verified, and runs entirely in the background — nothing here blocks the
  // window from appearing.
  listenEvent("recall-engine://progress", (event) => {
    const p = event && event.payload;
    if (!p) return;
    engineFetch.percent = typeof p.percent === "number" ? p.percent : 0;
    engineFetch.active = true;
    engineFetch.tick += 1;
    renderEngineRow();
    renderFinishWait();
  }).catch(() => { /* no event bridge outside Tauri — progress just stays hidden */ });

  startEngineFetch()
    .then(() => {
      // ADR-090: files on disk is not the same as engine ready. Ask for the
      // load explicitly here — on a genuine first run the files were still
      // downloading when `main.rs` made its startup attempt, so without this
      // the model stays cold until the user's first search.
      warmEngine();
    })
    .catch(() => {
      // Swallowed here on purpose: a background failure must not throw during
      // init. It is surfaced where it matters — the onboarding gate retries and
      // reports, and search degrades gracefully meanwhile (ADR-089).
    });

  // Warm-launch case: the files were already present, so `main.rs` started the
  // load before the window existed. Nothing to request — just watch for it to
  // finish so the "getting ready" line clears itself (ADR-090).
  pollEngineReady();

  // Maintenance engine (Phi-4) acquisition — downloads for everyone, NEVER
  // gates onboarding (founder 2026-07-24). Fire-and-forget in the background;
  // short-circuits once the file is present and verified.
  listenEvent("maintenance-engine://progress", (event) => {
    const p = event && event.payload;
    if (!p) return;
    maintFetch.percent = typeof p.percent === "number" ? p.percent : 0;
    maintFetch.active = true;
    renderMaintEngineStatus();
  }).catch(() => { /* no event bridge outside Tauri */ });
  // Phi-4 downloads DURING ONBOARDING for new users (founder 2026-07-24 —
  // "always during onboarding"). A returning user (already on the home screen)
  // does NOT auto-download 2.5 GB on every launch; they get the engine when
  // they turn maintenance on (see saveMaintenance), and a scheduled/catch-up
  // run self-heals by fetching it if absent.
  if (state.screen !== "home") startMaintenanceFetch();

  // Missed-run catch-up (task 10): if maintenance is on and overdue, run a
  // background pass. Also re-checked when the engine download completes.
  catchUpMaintenanceIfDue();

  showScreen(state.screen);
}

document.addEventListener("DOMContentLoaded", init);
