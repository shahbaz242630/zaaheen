# Zaaheen (Memory Vault) — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2). **Last updated:** 2026-09-19, session 47 (in progress).

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

**Session 47 (2026-09-19/20), in progress:**
- **S3 step 1 is merged** (PR #71 → `main` `4bf1c1a`, every check green on 3 OSes; `main`'s own push runs were started at 02:58Z — check them).
- **S3 step 2 is built on `feat/s3-step2` (from `4bf1c1a`), not committed yet.** 2a `vault-app/src/entitlement/` — `AccountCheck` over a new `AccountAccess` trait, refresh-then-decide, the §8.27 denial mapping. 2b `vault-app/src/account.rs` — the build-time account settings (`option_env!`, seven names, all-absent means no sign-in), the account folder with its ACL, and `build_account`. `vault-app` now depends on `vault-account` and `thiserror`. 28 tests, 25 planted bugs each caught, `SIGNIN-DESIGN.md` §8.33 (amendment 7) and §8.34 (amendment 8, the founder's app-vs-coaching rule).
- **The wiring (2c) is done:** the keeper (`keeper/runtime.rs::serve` takes `Option<Gate>`), direct mode (`application.rs::start_with_mcp`) and the daemon (`daemon.rs::call_tool`, after `authorize`) all take the gate; `vault_app::account::build_gate` builds the one per process and `vault-cli` calls it in all three commands. A build with no account settings serves as before; a build whose settings are broken refuses to start. 5 wiring tests over the real transports, `SIGNIN-DESIGN.md` §8.33.
- **All six DoD gates passed** from a wiped `target\debug` (`gates27.ps1`): build 40.7 min, clippy 30.2, `test -p vault-mcp` 45.8, `-p vault-app` 8.8, `-p vault-cli` 15.4, fmt. **344 tests over 28 binaries, zero warnings.**
- **Disk lesson (cost ~1 h):** `cargo build --workspace --all-targets` on top of an existing `target\debug` drove free space from 33 GB to 7.4 GB and the build died. Wiping `target\debug` returned 61 GB. Build the workspace, then let each `test -p` build its own targets; never `--all-targets` on a tight disk.

**Earlier in session 47:**
- **PR #70 merged** (rebase, auto-merge pinned to the head sha, founder yes) → `main` `53a22a1`; every PR check green on 3 OSes. `main`'s own push runs are green too (CI 35431951486, CodeQL, secret scan; checked session 47).
- **S3 step 1, the gate, is built on `feat/s3-gate`, not committed yet** (the branch was cut from `add49b6`; `git rebase origin/main` drops that commit as already applied). New: `crates/vault-mcp/src/gate.rs` (`EntitlementCheck`, `Verdict`, `LockReason`, `InFlight`, `EntitledService`), its re-export in `lib.rs`, `crates/vault-mcp/tests/entitlement_gate.rs` (19 tests), `SIGNIN-DESIGN.md` §8.32 (amendment 6, with one §6.1 wording change: the three empty lists do not ask). Proof: 12 red on an empty gate, 19 green, 16 planted bugs each caught, an independent review found no defects.
- **DoD: all four gates passed** (`gates26.ps1`, logs in `C:\Projects\MemoryVault-artifacts\gate-logs\`): `build --workspace` 61.1 min, `clippy --workspace --all-targets -D warnings` 55.7 min, `test -p vault-mcp` 75.4 min (78 tests over 13 binaries), `fmt --check`. Zero warnings anywhere. `RUSTFLAGS='-D warnings'`, line-tables-only for dev **and** test, `-j 2` for build and clippy, `-j 1` for the test step.
- **Disk and memory on this machine:** the wipe of `target\debug` freed 33 GB (20.8 → 53.6); the run ended at 17.8 GB free, and `target` is back to ~36 GB. The RAM ran critically low several times (background watchers were reaped three times; the detached gate run was never affected). The pagefile is 16 GB and the machine was not restarted.
- **Docker: do not delete `docker_data.vhdx` (58 GB).** The founder uses Docker for another project. Compacting it (Docker closed, admin `diskpart`) keeps the data; only on his say-so.
- The founder's standing yes covers small `cargo test -p` runs of the crate in hand once its dependencies are warm. A `-p` run can still compile cold (different feature unification): the first vault-mcp run rebuilt the stack in ~35 min.

**Do, in order:**
0. **S3 step 1 close-out:** the DoD gates are done; what is left is the PR (CI on 3 OSes, CodeQL, the cron workflows) and, once green, **ask the founder before merging**.
1. **S3 + S4**, one release. Step 2 next: the real check in `vault-app` over `vault-account`, and the gate wired into the keeper (`keeper/runtime.rs` `handle_connection`), direct mode (`application.rs` `start_with_mcp`) and the daemon (`daemon.rs` `call_tool`, after `authorize`). The step plan is in `SIGNIN-DESIGN.md` §8.32. The build order and every rule is in `SIGNIN-DESIGN.md` (S3's wiring rules §8.27; the app builds `https://zaaheen.com/pay/?_ptxn=…` with the trailing slash, §8.31; development builds carry the sandbox lease public keys from `OPS-HANDOFF.md` §G). Re-read BRD §11 first: S3 touches vault-mcp and the Tauri IPC layer.
2. **The user chooses where their memories live** (founder, 2026-09-18: "add it to the plan after S3 + S4"). S2 is finished, so its design (ADR-105 + an ADR-SEC, BRD §11 re-read) can be written now; it is built right after S3 + S4, whose onboarding it adds a step to.
   - **Onboarding:** "Your memories will be saved here: [default] — Change…". The native folder picker (drives, "New folder"); the default is preselected. Settings gets "Move my memories".
   - **One location for every process:** the desktop app, the keeper, every relay and the nightly maintenance runner (so consolidation follows the vault). Today they all resolve `%APPDATA%\com.zaaheen.app` (`vault-app/src/install_paths.rs::data_dir`, Tauri's `app_data_dir`), so a pointer at the default location is the likely shape. Models stay where they are.
   - **Must-haves:**
     - refuse cloud-synced folders (OneDrive, often behind "Documents"; Dropbox; Google Drive; iCloud) and network drives, because sync clients corrupt a database mid-write;
     - an external drive that is missing means "reconnect your drive", never a new empty vault (a drive letter can change);
     - a folder on exFAT/FAT32 has no Windows permissions, so the ADR-SEC-019 ACL cannot apply there (the files stay encrypted).
   - **Say it plainly in the UI:** a USB drive does not carry memories to another computer (the key stays in this computer's Credential Manager). That needs the BRD §11.5.4 backup and restore.
3. Then **S5** (coaching) and **S6** (production and the live test). **S6 now also carries:** the WAF rate-limiting rule for the API hosts' two `/v1` paths; production lease keys (the primary for Cloudflare, the founder's offline backup); a per-consumer Clerk secret key; the live default payment link `https://zaaheen.com/pay/` and the repository variables `PADDLE_ENVIRONMENT=production` + a `live_` `PADDLE_CLIENT_TOKEN` for the site build.
4. **In parallel, the founder's own account-side items** are in the local `OPS-HANDOFF.md` §F–§G. The live Paddle company account is already approved; only its domain approval waits for zaaheen.com to go live (S6).
5. **Optional, the founder's call:** add "account Worker (types + tests)" and "Analyse (TypeScript)" to the "main protection" ruleset's required checks. Open Dependabot PRs #45–#49 are still untouched.

**Lessons from session 46:**
- A design's "confirm it live" step earns its keep: the rate-limit binding passed every test in workerd and did nothing in production, and the strict CSP passed the audit and broke the real checkout.
- Read the provider's settings, not just its API: Paddle's default checkout lets the buyer change email, which would have silently broken the binding rule.
- Secrets never enter the conversation: clipboard → script → `wrangler secret put`; a browser-revealed secret goes to a file through `browser_evaluate`'s `filename`, and the script deletes it.

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
- **47 (2026-09-19/20):** PR #70 merged (`main` `53a22a1`, all green). S3 step 1, the gate (`vault_mcp::gate`), built tests first: 19 tests, 12 red on an empty gate, 16 planted bugs each caught, an independent review found no defects, all four DoD gates green. `SIGNIN-DESIGN.md` §8.32 records the decisions, including the one §6.1 wording change (the three empty lists do not ask the check). Learned: a file restored from a copy keeps its old timestamp, so cargo reuses the previous binary — touch it and check each log says "Compiling".
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
