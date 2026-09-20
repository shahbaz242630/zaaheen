# Zaaheen (Memory Vault) — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2). **Last updated:** 2026-09-20, session 49.

> **How to read this file.** §1 is what to do next; act on it. §2 is where things stand. §3–§7 are the working rules and reference. Everything older, including every full ADR, lives in the archives (§8): cross-link to them and quote them, never paraphrase. This file was reset to this short form in session 44 (founder request); the previous 2,632-line version is `HANDOFF_V0.2_PART4_ARCHIVE.md`, word for word.
>
> **Keep it short.** Each session replaces §1 and updates §2, rather than appending a log. A decision goes to the design file it belongs to (e.g. `SIGNIN-DESIGN.md`) or to an ADR; a finished session gets two or three lines in §2's "Recent sessions".

---

## 1 · 🟢 Next — S3 (the gate, lock mode, desktop sign-in) with S4 (export)

**State (2026-09-19, session 46): S2 is finished and live-tested against the sandbox.** Every step-3 decision and the live test are in `SIGNIN-DESIGN.md` §8.31; account identifiers only in the local `OPS-HANDOFF.md` §G.
- **The sandbox account service is live at `api-sandbox.zaaheen.com`** (`wrangler deploy --env sandbox` from `workers/account`), with all 11 secrets, the daily sweep and its own lease keys. Paddle sandbox and Clerk development only; nothing is live or bought.
- **Live test, every step passed:** PKCE sign-in → a signed 30-day `trial` (the signature verified with the sandbox public key) → checkout → paid on `/pay/` with Paddle's test card → both webhooks delivered first time → the lease turned `active` about 2 s later → a second checkout gave the portal → deleting the Clerk user canceled the subscription within a second.
- **`/pay/` is built** in the Astro site on `site/production-ready` (worktree `C:\Projects\GitHub\Memory Vault site`; its gitignored `site/.env` holds the sandbox client-side token). `allowLogout: false` locks the buyer's email (the binding rule, §8.30). Its CSP was measured live and is pinned by the audit. The founder signed off the site's one third-party exception (the local site rules 2 and 8).
- **Flood protection:** the Worker's rate-limit binding is in (`src/flood.ts`), but it was **measured not refusing** on the free plan (300 requests in 38 s). The enforced protection is a Cloudflare WAF rate-limiting rule at S6 (§8.31).
- **The founder's offline backup key moved to S6:** the sandbox has its own throwaway keys, so a test-card lease can never unlock a real app.
- **Machine:** Node 24 via fnm; wrangler 4.132.0 is signed in to the founder's Cloudflare account. The Claude Code auto-mode classifier refuses secret-store writes from me: the founder runs those scripts with `!` (or a clipboard script is allowed).

**At close (2026-09-20, session 49):**
- **S3 step 3 is merged**: PR #73 → `main` `84b5d43` (rebase, auto-merge), 14 checks green on 3 OSes. Confirmed this session, which was session 48's stated first job.
- **S3 step 4 is split into four commits** (founder-approved this session): **4a** the guard · **4b** the account commands (sign in / out / status / Subscribe / "I've paid", plus §4's missing refresh triggers) · **4c** the export (S4) · **4d** the screens (lock screen, onboarding sign-in step, account panel, trial and payment-failed banners). Rationale: 4a is where a mistake is expensive, so it lands alone and gets its own review; 4d goes last because building a lock screen before the lock works means testing against pretend data.
- **4a is built and green, NOT yet committed.** `vault_tauri::guard` + all 15 gated commands + `guard::build()` wiring in `main.rs` + `friendlyLockError` in `dist/app.js`. Decisions and ADR-SEC-023 in `SIGNIN-DESIGN.md` §8.36 (amendment 10).
- **S4's shape is founder-locked: one readable `.md` file** ("Download my memories"). Recorded in §8.36; build it at 4c.
- **Next session, before staging anything:** confirm `main`'s own runs on `84b5d43` are green — `gh run list --commit 84b5d43...` for **every** workflow, not just ci.yml (§6). They were still in progress at close.

**What session 49 built (S3 step 4a — the desktop entitlement guard):**
- **`vault_tauri::guard`** — `Entitled` (private field, so a gated function that never asked cannot compile), `Entitlement` (the one door), five stable `ERR_LOCKED_*` codes mapped from `LockReason`, and the `GATED_COMMANDS` / `OPEN_COMMANDS` classification with a source test over `main.rs`.
- **All 15 gated commands ask before doing anything.** Nine (`memory`, `boundary`, `agent`) pass an `&Entitled` the compiler demands; six (`engine`, `maintenance`) have no inner to hand one to, so their gate is the `require()` call, held by a source test. **That asymmetry is deliberate and documented** — do not read "the compiler enforces the guard" as covering all fifteen.
- **§8.35's obligation discharged:** `run_maintenance_now` is gated, so it can no longer report a run as done on a locked computer.
- **ADR-SEC-023:** a desktop whose account check cannot be built serves **locked**, not refused and not open. Refusing would take away the export, which is the one thing that must survive a lock.
- **Evidence:** 17 guard tests (run failing first: 11 passed / 3 failed, each for the reason it names), 64 passed / 0 failed for the whole `vault-tauri` lib suite, **13 planted bugs all 13 caught**.
- **The independent review found two defects that 17 tests and 13 plants all missed**, both written up in §8.36: (1) the constructors were `pub`, so a future command could have minted an always-open guard and still passed the source test — fixed by making them private behind `guard::build()`; (2) `friendlyLockError` was never called, so the test pinning it gave false comfort while a locked user would still see a raw code — fixed by wrapping `app.js`'s central `invoke`. **Steps 4b–4d get the same review; §8.36 records it as not optional.**
- **Founder, this session:** "take the lead partner whatever needs to be done in order", and he chose the close-out order planted bugs → review → gates. §6's rule stands for the commit and the push.
- **Disk: watch it, and do not diagnose it.** The founder runs Docker for another project, so free space moves for reasons unrelated to cargo. Reclaim our own instead: `cargo clean -p <crate>` returns tens of GB in seconds. Docker was wiped entirely in session 48 (78.4 GB back), so the old "never delete `docker_data.vhdx`" rule is retired.

**What session 48 built (S3 step 3):**
- **`vault_app::entitlement::ModeCheck`** — a second contract beside `EntitlementCheck`: `peek` (what the files say, no network, no writes, **no use recorded**) and `refresh_and_peek` (one refresh, then answer from disk). The keeper's tick asks this, never the serving check. §8.35 opens with why that distinction is a correctness matter, not a tidiness one.
- **`LockModeCheck` + `Flip`** (`entitlement/lock_mode.rs`) — asks the real check and never serves a call; answers `Unlocking` and raises the flip when entitlement returns.
- **`Subscription` / `KeeperMode` / `KeeperExit::ModeChanged`** (`keeper/runtime.rs`) — one parameter replacing `Option<Gate>`, carrying the gate, the mode, the tick's check and the flip. The evaluation is spawned off the serve loop, latched to one at a time, waits for calls in flight, and exits through the existing `stop_tx`.
- **Lock mode is the same `runtime::serve` loop** over `NoVaultAdapter`, so the tool list, `WIRE`, discovery roles, erasure intent and handover path are identical **by construction**. `vault-cli` decides the mode from `check.check()` **before** `build_application`, so no models load for a locked computer (the 2.86 GB §6.2 is about).
- **The relay short-circuit** — `RelaySettings::account_dir` + `marker_recheck`; `vault_app::account::sign_in_marker` answers `Option<bool>` (three states, not two) and never creates the folder.
- **The maintenance pause** — `RunOutcome::Paused` with its own code, `pause_for` in `vault-cli`, and the plain-English line in `dist/app.js`.

**Do, in order:**
0. **Confirm `main`'s own runs on `84b5d43` are green** before staging anything else — every workflow, not just ci.yml (§6). A red check is a regression: fix it that session.
1. **Commit 4a** (built and green at close; see "What session 49 built"). Then **4b → 4c → 4d**, each with its own failing-tests-first cycle, planted-bug campaign, **independent review (not optional, §8.36)** and DoD gates.
   - **4b — the account commands:** sign in (PKCE via `vault-account`), sign out, account status, Subscribe (`/v1/checkout` → validate `txn` against `^txn_[a-z0-9]{26}$` → open `https://zaaheen.com/pay/?_ptxn=…` with the trailing slash, §8.31), "I've paid". **Ships §4's missing refresh triggers**, inherited from step 3 (§8.35): `Trigger::Routine` (keeper start, desktop open, the daily jittered timer) and `Trigger::UserAction` (checkout polling every 10 s for 10 min, "I have paid") still have **no production caller**.
   - **4c — S4, the export.** **Founder-locked shape (§8.36): one readable `.md` file**, grouped by topic, each memory with its date and topic, and a header saying it came from the person's own computer and nothing was sent anywhere. It must work on a *locked* computer, where a lock-mode keeper holds `.vault.lock` — the handover path erasure already uses (`erasure_takes_the_vault_from_a_lock_mode_keeper`, §8.35).
   - **4d — the screens:** lock screen with **Subscribe** and **Download my memories** plus the support email, the onboarding sign-in step, the account panel, the trial banners (day 23; `health.warnings` in the last 5 days), the payment-failed banner. **The lock screen's real copy is a founder decision** — the five lines in `friendlyLockError` are provisional placeholders (§8.36).
   - **Before 4b, re-read BRD §11** — it touches the Tauri IPC layer — and the vault key's roaming-persistence fix (§3 item 6) rides with this work.
   - The app builds `https://zaaheen.com/pay/?_ptxn=…` with the trailing slash (§8.31), and validates `txn` against `^txn_[a-z0-9]{26}$`.
   - **Before any of it, re-read BRD §11** — this touches the Tauri IPC layer — and the vault key's roaming-persistence fix (§3 item 6) rides with this work.
2. **A live run needs the sandbox values** (`OPS-HANDOFF.md` §G) in the seven `ZAAHEEN_ACCOUNT_*` / `ZAAHEEN_LEASE_*` build variables (§8.33). Without them a build simply has no sign-in, which is why every test above passes with the gate off.
3. **The user chooses where their memories live** (founder, 2026-09-18: "add it to the plan after S3 + S4"). S2 is finished, so its design (ADR-105 + an ADR-SEC, BRD §11 re-read) can be written now; it is built right after S3 + S4, whose onboarding it adds a step to.
   - **Onboarding:** "Your memories will be saved here: [default] — Change…". The native folder picker (drives, "New folder"); the default is preselected. Settings gets "Move my memories".
   - **One location for every process:** the desktop app, the keeper, every relay and the nightly maintenance runner (so consolidation follows the vault). Today they all resolve `%APPDATA%\com.zaaheen.app` (`vault-app/src/install_paths.rs::data_dir`, Tauri's `app_data_dir`), so a pointer at the default location is the likely shape. Models stay where they are.
   - **Must-haves:**
     - refuse cloud-synced folders (OneDrive, often behind "Documents"; Dropbox; Google Drive; iCloud) and network drives, because sync clients corrupt a database mid-write;
     - an external drive that is missing means "reconnect your drive", never a new empty vault (a drive letter can change);
     - a folder on exFAT/FAT32 has no Windows permissions, so the ADR-SEC-019 ACL cannot apply there (the files stay encrypted).
   - **Say it plainly in the UI:** a USB drive does not carry memories to another computer (the key stays in this computer's Credential Manager). That needs the BRD §11.5.4 backup and restore.
4. Then **S5** (coaching) and **S6** (production and the live test). **S6 now also carries:** the WAF rate-limiting rule for the API hosts' two `/v1` paths; production lease keys (the primary for Cloudflare, the founder's offline backup); a per-consumer Clerk secret key; the live default payment link `https://zaaheen.com/pay/` and the repository variables `PADDLE_ENVIRONMENT=production` + a `live_` `PADDLE_CLIENT_TOKEN` for the site build.
5. **In parallel, the founder's own account-side items** are in the local `OPS-HANDOFF.md` §F–§G. The live Paddle company account is already approved; only its domain approval waits for zaaheen.com to go live (S6).
6. **Optional, the founder's call:** add "account Worker (types + tests)" and "Analyse (TypeScript)" to the "main protection" ruleset's required checks. Open Dependabot PRs #45–#49 are still untouched.

**Lessons from session 48:**
- **`cargo test -p <crate>` is `--all-targets` for that crate.** `vault-app` has eleven integration test files, so one `test -p vault-app` builds twelve binaries: 43 min from a cleaned crate, and it stops at the first failing binary, so the rest never run. `cargo test -p vault-app --lib` or `--test <name>` builds one: **0.4–2.3 min**. Use the narrow form for every red/green cycle and keep the full `test -p` for the gate run, where being exhaustive is the point.
- **Copy the gate script's environment exactly, or nothing is reusable.** A focused run started rebuilding the whole dependency tree because it set the two profile variables but not `RUSTFLAGS='-D warnings'` — a different fingerprint. `gates28-step.ps1` now holds the full gates27 environment (including `CARGO_PROFILE_TEST_DEBUG`, which `gate-step.ps1` omits, so that helper silently rebuilds every test binary).
- **Do not edit ANY file in a crate while that crate is compiling** — not just the file being built. Cargo had already read `account.rs` when it was edited mid-run, so the binary would have mixed old and new source while the fingerprint recorded the new timestamps. The run was killed and restarted rather than trusting the result. This is the same family as session 47's restored-mtime lesson.
- **`cargo clean -p <crate>` is the cheap disk lever:** 14.5 GB and then 14.1 GB back, twice, in seconds, because cargo keeps every previous build of a test binary. It costs only that crate's rebuild. Reach for it long before a full wipe.
- **A test with production-shaped timing finds production-shaped bugs.** `ticks_never_pile_up_behind_a_slow_evaluation` used a refresh six times slower than the tick — which a 5 s network refresh against a 5 s tick reproduces routinely — and caught a latch released one statement too early. A fast test would never have hit it.

**Lessons from session 46:**
- A design's "confirm it live" step earns its keep: the rate-limit binding passed every test in workerd and did nothing in production, and the strict CSP passed the audit and broke the real checkout.
- Read the provider's settings, not just its API: Paddle's default checkout lets the buyer change email, which would have silently broken the binding rule.

---

## 2 · 🧭 Where things stand

### The product today
- **The vault works end to end on Windows.** Local-first and encrypted at rest (SQLCipher, sealed Lance, sealed graph and REPORTs), with an MCP server over stdio. Claude Desktop, Cursor and Antigravity all read and write it correctly.
- **Read path:** structured facts, no LLM at read time. `memory_read` is the primary answer path and never false-empties; `memory_search` is recall-safe. The Qwen3-Reranker-0.6B cross-encoder is the relevance authority.
- **Nightly consolidation** (Phi-4-mini): incremental; merges duplicates, resolves contradictions, decays, archives cold facts, enriches aliases and the graph, writes checkpoints you can roll back. Runs from Windows Task Scheduler through the windowless `zaaheen-maintenance.exe`.
- **One keeper owns the vault** (ADR-102/103, ADR-SEC-019/020): every `zaaheen mcp serve` an AI app starts is a relay to one keeper process, so several apps share the vault at once. Live-verified with Claude Desktop and Cursor together (2026-09-11).
- **Installed on the founder's machine:** `Zaaheen_0.2.2_adr103.msi` (in `C:\Projects\MemoryVault-artifacts\`). Main has moved on since (rmcp 2.2.0, wire 2), so the next installer needs the live test in §3.
- **Every ADR through ADR-103 and ADR-SEC-021 is shipped.** Old "IN FLIGHT" labels in the archives are stale.

### The sign-in and subscription arc (ADR-104 + ADR-SEC-022)
- **Locked design: `SIGNIN-DESIGN.md`.** Clerk OAuth public client with PKCE on a loopback port; the refresh token in Windows Credential Manager; an Ed25519 lease from a Cloudflare Worker, verified offline for up to 30 days; Paddle as merchant of record; the keeper as the one gate; export always available.
- **Business model:** a 30-day trial with no card, then $5/month or $48/year (BRD §1.6 amendment 1). Beta testers get a per-person `comp_until` date.
- **S0 spike: done** (session 43). **S1: merged** (session 44, `main` `99ff08e`). **S2: done** (session 45: the Worker's offline core and endpoints, `8b99454` and `5a62e11`, `SIGNIN-DESIGN.md` §8.29–§8.30; session 46: `/pay/`, flood protection and the sandbox deployment at `api-sandbox.zaaheen.com`, live-tested end to end, §8.31). S3 is next.
- **What S1 is.** `crates/vault-account` (no other crate depends on it yet):
  - `PendingSignIn`: PKCE and the loopback listener, with its adversarial suite.
  - `OAuthClient`: exchange, refresh, revoke, userinfo.
  - `TokenStore`: the crate's own credential store, `persistence=Local`, checked live against Credential Manager.
  - `LeaseVerifier`: exact bytes checked before any parsing, primary and backup keys.
  - The time model (`assess`, `LocalState`).
  - `Account` over `AccountDir` and `LeaseClient`: one refresher under `refresh.lock`, the rotated token stored first, `invalid_grant` looked at twice before it can mean "signed out".
  - **Proof:** 205 tests, each step run failing first. Planted-bug runs proved the key tests can fail. An independent review found one defect (a refused token save), fixed tests-first.
- **Contracts S2 and S3 must honour:** `SIGNIN-DESIGN.md` §8.27.

### Company, website, accounts (not code in this repo)
- **Zaaheen is a licensed parent company.** Launch waits on details from the licence, so the aim is readiness, not going live. The readiness checklist is `SEO-HANDOFF.md` §0a (local, gitignored).
- **Website:** branch `site/production-ready` holds the Astro site, now with `/pay/` (session 46; checked out as the worktree `C:\Projects\GitHub\Memory Vault site`). Not merged, not deployed; zaaheen.com still shows Hostinger's parked page. Rebasing that branch will conflict on `HANDOFF.md`: take `main`'s version. Its local stash `session-41 docs …` is superseded; drop it.
- **Coaching booking app:** separate repo `C:\Projects\GitHub\Training Page` (its own CLAUDE.md, HANDOFF and rules). Parked.
- **Accounts:** Clerk (dev instance set up), Microsoft 365, Paddle, Cloudflare and the bank. All details live **only** in the local `OPS-HANDOFF.md`; this repo is public.

### Recent sessions
- **49 (2026-09-20):** PR #73 merged (`main` `84b5d43`). **S3 step 4 split into 4a–4d** (founder-approved). **S4's shape founder-locked: one readable `.md` file.** **4a, the desktop entitlement guard**, built tests-first: 17 tests (11/3 red against an inert guard), 64 passed / 0 failed for the `vault-tauri` lib suite, 13 planted bugs all caught, ADR-SEC-023. An independent review found **two defects the tests and plants both missed** — public constructors that let a command forge an always-open guard, and a plain-English mapping that was never called — both fixed, both written up in `SIGNIN-DESIGN.md` §8.36, which also makes the review mandatory for 4b–4d.
- **47 (2026-09-19/20):** PR #70 merged (`main` `53a22a1`). **S3 step 1**, the gate (`vault_mcp::gate`): 19 tests, 12 red on an empty gate, 16 planted bugs each caught, an independent review, four DoD gates green → PR #71 merged (`main` `4bf1c1a`). **S3 step 2**, the check (`vault_app::entitlement`), the build-time account settings and the account folder (`vault_app::account`), and the wiring into the keeper, direct mode and the daemon: 28 unit tests + 5 wiring tests over the real transports, 25 planted bugs each caught, an independent review found no defects, all six DoD gates green (344 tests over 28 binaries) → PR #72, auto-merge queued at close. Decisions in `SIGNIN-DESIGN.md` §8.32 (the gate), §8.33 (the check and where a build's account settings come from) and §8.34 (**founder-locked:** the app subscription and a coaching session are separate purchases).
- **46 (2026-09-19):**
  - S2 step 3: `/pay/` built tests first on the site branch (`allowLogout: false`, a CSP measured live and pinned by the audit), flood protection in the Worker, and the sandbox deployment `api-sandbox.zaaheen.com`.
  - Live test against the Paddle sandbox and Clerk development: trial → paid → active in about 2 s → deletion canceled the subscription. 19 planted bugs, each caught.
  - Found live: the rate-limit binding does not refuse on the free plan (WAF rule at S6); the strict CSP broke the checkout until Paddle's styles were allowed.
- **45 (2026-09-18):**
  - S2 steps 1 and 2 built tests first and merged (PR #68 → `8b99454`, PR #69 → `5a62e11`; 261 Worker tests in workerd, lease vectors checked by Rust).
  - 49 planted bugs, each caught. Two independent reviews: step 1 caught a real case-(b) deviation, fixed; step 2 found nothing.
  - The Zaaheen Paddle sandbox, product and prices set up; tax on top.
  - The user-chosen vault location added to the plan after S3 + S4.
  - A fake test value tripped the secret scans; the branch was amended.
- **44 (2026-09-18):** S1 built tests first and merged (PR #67 → `99ff08e`, 205 tests, all CI green on 3 OSes). An independent review caught one real defect (a refused token save), fixed tests-first. CI caught a Windows-only dead constant and five test messages CodeQL flagged; both fixed. The handoff was reset to this file (old one archived word for word; the design moved to `SIGNIN-DESIGN.md`).
- **43 (2026-09-17):** security PR #64 merged; the silently red secret scan fixed at both root causes; Clerk dev set up; ADR-104 + ADR-SEC-022 locked after three review rounds and a live spike.
- **42 (2026-09-17):** rmcp 1.5.0 → 2.2.0 and rustls 0.23.45 for advisories (ADR-SEC-021), verified by CI only (founder: no local build).
- **Before that:** see the archives, newest first in `HANDOFF_V0.2_PART4_ARCHIVE.md`.

---

## 3 · 🟠 Open items, after the sign-in arc

Roughly in priority order; the founder picks. Full context for each is in `HANDOFF_V0.2_PART4_ARCHIVE.md` (search the item's name).

1. **Before the next installer: live-test Claude Desktop and Cursor.** rmcp 2.x answers any known protocol version a client asks for, where 1.5.0 capped it. Nobody has seen what either app now negotiates. Read the `tauri` advisory first (BRD §11.12, vault-tauri).
2. **The Agents tab says "No agents connected yet" while Claude and Cursor are connected** (founder-found, 2026-09-11). It breaks the honest-UI rule. A design exists (session-37 opener, item 1): the relay passes the app's name upstream, the keeper keeps a live list, and the tab reads it.
3. **Walk through the app with the founder** and record every change he raises, in his words, before building more.
4. **D4: the desktop app becomes a client of the keeper.** Decided, not built (archive §8.23). It fixes "Delete everything" while an AI app is connected, among other things.
5. **The second machine.** The app has never run on a computer other than the founder's; this is the highest-information test left.
6. **The vault key is stored with roaming ("Enterprise") persistence** (`vault-app/src/keychain.rs`). Set `Local`, as S1 did for the account token, and migrate the existing entry. Do it with S3's vault-app work.
7. **The agent-facing recovery hints name `vault-cli consolidate run`**, a binary we don't ship (`vault-retrieval/src/structured_read_pipeline.rs` ~857–894). The user's path is the app's Consolidation → Run now.
8. **Dependabot:** PRs #45–#49 (quinn-proto HIGH, cmov, serde_with, openssl, tar) and 9 open alerts. Triage one at a time; each needs its own CI run.
9. **Rebuild the `.mcpb` bundle**; write a real README; list the server in the MCP registry.
10. **Ideas, not scheduled:** an email-handling agent (design before go-live, draft-for-approval first); rmcp 3.x (the 2026-07-28 spec; a planned upgrade with a live test).

---

## 4 · 🐛 Tech debt (live, with anchors)

- **Read relevance:** carry the raw cosine through `HybridRetriever` fusion and filter per candidate; retire the vestigial BM25 gate (`vault-retrieval/src/strategies/hybrid.rs`).
- **The graph:**
  - the merge path leaves stale relationships (`phases/merge.rs::apply_merge`);
  - `graph.duckdb` is not encrypted by DuckDB itself (no offline-writable option);
  - the read channel is parked, OFF by default (`VAULT_ENABLE_GRAPH_CHANNEL=1`). Revisit only on real demand.
- **`VaultError::Storage(String)`** is a grab-bag; `retry_queue.rs::is_permanent` matches error wording.
- **The real-model smoke job** shares one `.partial` download path (`model_loader.rs`). Only `--test-threads=1` protects it.
- **CI's MSVC toolset pin (14.44)** on the Windows jobs: drop it once DuckDB and llama.cpp support VS2026.
- **`scripts/run-desktop-dev.ps1 -NoReranker`** docs are stale since ADR-089.
- **Unpinned rmcp 2.2.0 behaviours:** a wire test for `isError` on unparsable arguments, and the last line without a newline.
- **Session-36 leftovers:**
  - the keeper's reranker uses 2.86 GB (latency work waits for the founder's call);
  - a relay closed before `initialize` logs a normal event as an error;
  - `get_info` doesn't name the authorized boundaries;
  - audit which direct-mode failures should be `isError`.
- **`set_maintenance_schedule` carries `#[allow(clippy::too_many_arguments)]`** (8 args, one past the limit, because the guard added `State<'_, Entitlement>`). The structural fix — collapsing the five schedule fields into one `#[derive(Deserialize)]` struct — changes the command's wire contract and needs a matching change in `dist/app.js`, so it belongs with step 4d's frontend work, not with a security gate.
- **The workspace `rust-version` (1.81) is stale** against the pinned 1.92 toolchain. `File::try_lock` sites carry `#[allow(clippy::incompatible_msrv)]`.
- **The lance NaN-distance upstream issue:** file a minimal repro.

---

## 5 · 🔒 Architectural locks (don't relitigate without the founder)

- **The LLM is out of the read path.** The vault returns facts; the agent composes. Phi-4-mini runs only at nightly consolidation.
- **Recall is sacrosanct.** A false "I don't know" is worse than a wrong answer; read changes are reorder-only and never false-empty.
- **Correctness of output is the product; correctness before latency.**
- **Zero-knowledge:** the server can never read vault contents. Sign-in never touches the vault key. Re-read BRD §11 and add an ADR-SEC before any crypto, auth or IPC change.
- **One keeper owns the vault;** relays forward.
- **One codebase, three modes** (Local / BYOK / Managed). Sync is deferred until there are paying users.
- **The app subscription and a coaching session are separate purchases** (founder, 2026-09-20; `SIGNIN-DESIGN.md` §8.34). One account identifies the person in both and grants nothing across them: an app subscriber pays for a session like anyone else, and a coaching client gets no app entitlement. The app side already enforces it (only our app product's Paddle subscriptions count); S5 must build the coaching side the same way.
- **White label:** no model, vendor or stack names in anything a user sees.
- **Free plans and provider defaults until traction.**

---

## 6 · 📐 How we work (standing rules)

Full rules live in memory (`~/.claude/projects/C--Projects-GitHub-Memory-Vault/memory/`) and in the project CLAUDE.md.

- **Definition of Done (BRD §0.1):**
  - `cargo build --workspace` with zero warnings;
  - the affected crates' tests pass;
  - `cargo clippy --workspace --all-targets -- -D warnings`;
  - `cargo fmt --all --check`;
  - this file updated.
- **Local gates first, then CI.** Both are required. Only the founder can relax this, per batch.
- **Ask before any compiling cargo run, and report disk first.**
  - Exception (founder, 2026-09-18): small `cargo test -p <crate in hand>` runs are pre-approved.
  - Only `cargo fmt` is always free.
- **Cargo on Windows:**
  - run from PowerShell (the sqlcipher/openssl build needs Strawberry Perl);
  - one cargo at a time, never in parallel;
  - line-tables-only profile throughout (`$env:CARGO_PROFILE_DEV_DEBUG`/`CARGO_PROFILE_TEST_DEBUG='line-tables-only'`) with `-j 2`, and never mix profiles;
  - long runs go detached (`Start-Process`) with a log, not the tool's 10-minute cap;
  - a cold build needs `vcvars64` (VS 2019 BuildTools);
  - PowerShell scripts must be pure ASCII.
- **Commits:**
  - one founder yes covers the commit and its push;
  - write the message to a file and use `git commit -F`;
  - `Co-Authored-By: Claude <noreply@anthropic.com>`;
  - never commit `CLAUDE.md`, and never commit account details;
  - admin and doc edits ride with the next code commit;
  - run `fmt --check` last, then `git status --short` before `git add`.
- **CI:** after every push, check **every workflow**, not just `ci.yml`, and do the same at session open (a cron failure belongs to no commit): `for w in ci.yml codeql.yml secret-scan-history.yml monthly-tech-debt.yml; do gh run list --workflow=$w -L 2; done`. Broken CI is a regression: fix it in the same session. `gh run watch`'s exit code is not the run's result.
- **Talking to the founder:**
  - plain English;
  - one step at a time, show it, and ask only that step's decision;
  - state a recommendation, not "your call";
  - don't escalate purely technical choices.
- **Engineering:**
  - tests first, and prove they can fail (run them failing; plant the bug for the key ones);
  - execute shipped artefacts rather than reading them;
  - no drive-by refactors (log them in §4);
  - an ADR in the same commit as any inline architectural decision;
  - source-read the call graph before an empirical plan;
  - quote locked text, don't paraphrase it.

---

## 7 · 🔧 Key reference

- **Repo:** `https://github.com/shahbaz242630/zaaheen` (public). **Local:** `C:\Projects\GitHub\Memory Vault`. **Spec:** `Agent_Build_Specification.txt` (the BRD, canonical).
- **Crates:** vault-core, -storage, -embedding, -llm, -retrieval, -consolidator, -scheduler, **-account** (new), -mcp, -sync (stub; deferred), -connectors, -app, -cli (builds `zaaheen.exe`), -tauri (the desktop app).
- **Workers:** `workers/account` (TypeScript, Cloudflare; `api.zaaheen.com`, not deployed yet). From PowerShell: `fnm exec --using=24 -- npm.cmd test` (tests run in workerd), `npm.cmd run types` then `npm.cmd run typecheck`.
- **The founder's installed app:**
  - `C:\Program Files\Zaaheen\` (`zaaheen.exe`, `zaaheen-maintenance.exe`, the desktop exe);
  - vault: `%APPDATA%\com.zaaheen.app`;
  - logs: `%LOCALAPPDATA%\com.zaaheen.app\logs`;
  - the account folder (S3 will create it): `%LOCALAPPDATA%\com.zaaheen.app\account`;
  - Claude's MCP log: `%LOCALAPPDATA%\Claude\Logs\mcp-server-zaaheen.log`;
  - Cursor: `~/.cursor/mcp.json` → `zaaheen mcp serve`.
- **Keychain entries:** vault key `default.com.zaaheen.v0.2`; account refresh token `refresh-token.com.zaaheen.account` (Local).
- **Artefacts outside the repo:** `C:\Projects\MemoryVault-artifacts\`: the MSIs, `verify-msi-0.2.2.ps1`, `live-test-0.2.2.ps1`, the release-build scripts, design sources.
- **Release build:** cold ~6¾ h at `-j 1` (the desktop feature set recompiles the heavy dependencies a second time); incremental ~70 min. **Keep `target/release`.**
- **Env (fresh PowerShell):** `$env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"; $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"`.
- **Local-only docs (gitignored):** `CLAUDE.md`, `OPS-HANDOFF.md` (accounts), `SEO-HANDOFF.md` + `SEO-RESEARCH-A/B.md` (website).

---

## 8 · 🗂️ Archives and where each ADR lives

| File | Covers | ADRs (full text) |
|---|---|---|
| `SIGNIN-DESIGN.md` | **Live:** the sign-in, trial and subscription design | ADR-104, ADR-SEC-022 + amendment 1 |
| `HANDOFF_V0.2_PART4_ARCHIVE.md` | Sessions 19–44 (frozen 2026-09-18) | 073–084 bodies, 086–103, ADR-SEC-003–021 (the keeper arc is §8.19–§8.25) |
| `HANDOFF_V0.2_PART3_ARCHIVE.md` | Sessions 2–18 | 080–085, ADR-SEC-001, ADR-SEC-002 (+ Part 2, Amendment 1) |
| `HANDOFF_V0.2_PART2_ARCHIVE.md` | T0.2.3c3 → T0.3.x | 047–072, tech-debt narratives, technique map, consolidator inventory |
| `HANDOFF_V0.2_PART1_ARCHIVE.md` | T0.2.0 → T0.2.3c2 | 037–046 |
| `HANDOFF_V0.1_ARCHIVE.md` | V0.1 | 001–036 |

**Next free numbers:** ADR-105 and ADR-SEC-023.
