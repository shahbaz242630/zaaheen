# Zaaheen (Memory Vault) — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2). **Last updated:** 2026-09-25, session 62.

> **How to read this file.** §1 is what to do next; act on it. §2 is where things stand. §3–§7 are the working rules and reference. Everything older lives in the archives (§8): quote them, never paraphrase. Reset to this short form in session 60 (founder: *"lets archive it and start fresh clean handoff"*); the previous file is `HANDOFF_V0.2_PART5_ARCHIVE.md`, word for word.
>
> **Keep it short.** Each session replaces §1 and updates §2 rather than appending a log. Decisions go to their design file or an ADR; a finished session gets two or three lines in §2's "Recent sessions".

---

## 1 · 🟢 Next — the live test on the new installer, then the account pages

**The batch is gated, committed and merged** (session 62): sessions 58–61 in one commit — the walk-through screens, the connection fixes (ADR-SEC-032, ADR-107), **D4** (ADR-108, ADR-SEC-033) and the website redesign. Fresh cold gate run all green (numbers in §2 "Recent sessions"). **The installer** was built from that commit by `C:\Projects\MemoryVault-artifacts\release-build6.ps1` (sandbox account, `-j 2` with a `-j 1` retry) → `Zaaheen_0.2.2_s62-d4-sandbox.msi`; its result, SHA256 and commit are in `release-build-logs\release-build6-summary.txt`. Keep `target\release`: the next installer build is then incremental.

**Do, in order:**
0. **At open:** `gh run list` for every workflow on `main`; read `release-build6-summary.txt` (SUCCESS?).
1. **The live test**, every app from a clean slate: Claude (chat and Cowork), Cursor, ChatGPT (Work and Codex), Antigravity, **and Hermes and OpenClaw** (founder: *"we will test them shortly"*; on a pass, move them from `DEMO_APPS` to `VERIFIED_APPS` in `site/src/data/site.ts`). Plus D4’s checks: "Opening your memories…" then works; a fresh install reaches sign-in with no keeper; "Run now" pauses the AI apps and they recover; "Delete everything" with an app connected removes everything; closing the app leaves the AI apps working. **Before it, confirm the real sandbox’s `paths` and `development_origin` are empty** (the spike reverted them; they must stay so).
2. **Then the account pages, build step 1 (measure first)** per `AUTH-PAGES-DESIGN.md` "Build order", on the second Clerk app "Zaaheen pages (dev)" (OPS-HANDOFF §H), never the real sandbox.

**Lessons from session 62:**
- **Retry-once in the gate driver paid off twice over:** the first clippy failure was a real lint (`sort_by_key`, one vault-storage test, the only finding across the workspace with `--keep-going`); fixed while the automatic retry ran, which then passed in 1.7 min. Links took free RAM to ~0.2 GB in five steps without a single failure (detached Task Scheduler task, BelowNormal, `-j 2`).
- **Cold timings this run:** build 67 min · clippy 40 · storage 38 · retrieval 48 · mcp 29 · app 16 · cli 17 · tauri 19; ~4 h 40 min in all; disk 182 → 88 GB free.

---

## 2 · 🧭 Where things stand

### The product
- **Works end to end on Windows.** Local-first and encrypted at rest (SQLCipher, sealed Lance, sealed graph and REPORTs); MCP over stdio. Claude Desktop, Cursor, ChatGPT desktop and (after this batch) Antigravity connect.
- **Read path:** structured facts, no LLM at read time. `memory_read` is primary and never false-empties; `memory_search` is recall-safe; the Qwen3-Reranker-0.6B cross-encoder is the relevance authority; one question at a time at the keeper's read desk (ADR-107).
- **Nightly consolidation** (Phi-4-mini) through Windows Task Scheduler and the windowless `zaaheen-maintenance.exe`.
- **One keeper owns the vault** (ADR-102/103, ADR-SEC-019/020): every AI app's `zaaheen mcp serve` is a relay to it. **After this batch the desktop app is a client of it too** (ADR-108): nothing but the keeper and the maintenance runner opens the vault or creates the key.
- **Installed on the founder's machine:** the sandbox installer `Zaaheen_0.2.2_s3-4d-sandbox.msi` (session 58).

### Arcs
- **Sign-in and subscription (ADR-104 + ADR-SEC-022, `SIGNIN-DESIGN.md`):** S0–S3 done and merged (S3 → PR #77, `main` `21d5002`); S4 (the export) built. Business model: 30-day no-card trial, then $5/month or $48/year; Clerk + Paddle; the keeper is the one gate; export always available.
- **The vault key and its location (ADR-SEC-029, ADR-105 + ADR-SEC-030, `VAULT-KEY-AND-LOCATION.md`):** built, gated, merged (PR #77).
- **Connecting apps (ADR-106, ADR-SEC-031, ADR-SEC-032, ADR-107, `CONNECT-APPS-DESIGN.md`):** "Connect it for me", the connected-apps list, rmcp 3.4.1 — in this batch.
- **D4 (ADR-108, ADR-SEC-033, `DESKTOP-CLIENT-DESIGN.md`):** in this batch.

### Company, website, accounts (not code in this repo)
- **Zaaheen is a licensed parent company.** Launch waits on the licence; the aim is readiness. Checklist: `SEO-HANDOFF.md` §0a (local, gitignored).
- **Website:** merged into `main` with publishing switched off (PR #78, `SITE_PUBLISH` unset); zaaheen.com still shows Hostinger's parked page.
- **Coaching booking app:** separate repo `C:\Projects\GitHub\Training Page`. Parked.
- **Accounts:** Clerk, Microsoft 365, Paddle, Cloudflare, the bank. Details only in the local `OPS-HANDOFF.md`; this repo is public.

### Recent sessions
- **62 (2026-09-24/25):** **Full fresh gates green** (`gates-s62.ps1`, detached, `-j 2`): fmt · build · clippy `--all-targets -D warnings` (one lint fixed mid-run) · tests vault-core 62 · vault-account 244 · vault-storage 329 · vault-retrieval 170 (1 ignored) · vault-mcp 109 · vault-app 535 (26 ignored, live-only) · vault-cli 54 · vault-tauri 202 (2 ignored), 0 failed; site build, audit, tests green. Batch committed, pushed, merged; installer built from the commit.
- **61 (2026-09-24):** Branch fast-forwarded to `main` (website). **Website redesign built, uncommitted** (`site/`): new home page and header from the Claude Design handoff (old Download section removed; buttons download the MSI; ChatGPT added to `VERIFIED_APPS`; Hermes and OpenClaw only in the demo window, `DEMO_APPS`; "no account" claims corrected, `llms.txt` too), and the Knowledge Centre hub (coaching data mirrored in `site/src/data/coaching.ts`, guarded by `scripts/coaching-sync.test.mjs`, which was shown to fail on a changed price). Site build, audit and 47 tests green. **Account pages designed, not built:** `AUTH-PAGES-DESIGN.md` (ADR-109, ADR-SEC-034) after a runtime spike on the sandbox (fully reverted) and two independent reviews, both GO-WITH-FIXES, all findings folded into v2; build step 1 is the measurements. A second free Clerk app "Zaaheen pages (dev)" created for it (OPS-HANDOFF §H). **Coaching app restyled** in its own repo on `feat/zaaheen-restyle`, `pnpm verify` green, uncommitted (its HANDOFF). Lesson: a `python3 -` heredoc was typed again and stopped at once; disk unchanged.
- **60 (2026-09-24):** **D4 designed, reviewed, built and tested.** Plan to two independent reviewers twice (round 1 NO-GO on a signed-out fresh install; round 2 GO-WITH-FIXES, all applied); founder approved every new line, deleting the V0.1 bridge, and "Run now" pausing the AI apps. Built in eight phases tests-first: the `ADMIN` handshake purpose, the drain on hand-over, the admin server and the locked host, the keeper's start-failure note and lock-mode fallback, the pool's admin purpose, `--take-over` / `--only-if-due`, the desktop as a client, the bridge deleted. Security review: safe to commit, six points fixed. HANDOFF reset (Part 5 archived).
- **59 (2026-09-24):** Website merged (PR #78). Live connection test on the sandbox installer found four problems — the new MCP spec (Antigravity), the queue failure (Cursor), developer-speak hints (Claude), the Agents-tab gaps — all fixed in code (ADR-SEC-032, ADR-107).
- **58 (2026-09-23):** S3 merged (PR #77 → `main` `21d5002`); sandbox installer built; the setup pages' walk-through.
- **Before that:** `HANDOFF_V0.2_PART5_ARCHIVE.md` §2 "Recent sessions" (sessions 42–57), then Part 4.

---

## 3 · 🟠 Open items, after this batch

Roughly in priority order; the founder picks. Context for each is in `HANDOFF_V0.2_PART5_ARCHIVE.md` §3 (search the item's name).

1. **The second machine.** The app has never run on a computer other than the founder's; the highest-information test left.
2. **Walk through the app with the founder** and record every change he raises, in his words.
3. **A founder decision before launch: a second check before "Download my memories"?** BRD §11.4.1 lists full-vault export among multi-factor operations. Recommendation: keep it as it is and record the divergence (reasoning in `SIGNIN-DESIGN.md` §8.39).
4. **Dependabot:** PRs #45–#49 and the open alerts, one at a time, each with its own CI run.
5. **Rebuild the `.mcpb` bundle**; a real README; list the server in the MCP registry.
6. **"Move back to the usual place" — only if beta testers ask** (ADR-105 L4.2 exception needed).
7. **Ideas, not scheduled:** an email-handling agent (design before go-live, draft-for-approval first).
9. **Wording to review at the pre-launch check (founder, session 61: *"choose the wording for now .. no need for my approval.. once we eyeball final check before going live we can review then"*).** Chosen by me, not yet seen by the founder:
   - Home: H1 "One memory for all your AI apps."; the lede; example memories ("Prefers short answers in plain English", "Book meetings after 10am, never on Fridays", "Launching the new website in March", "Runs a small coaching business"); the demo window's Consolidation and Settings panes; "Your account never holds your memories."; the dots "Stays on your computer · 30-day free trial, no card · Delete it all in one step".
   - The home page lost the old Download section (founder: remove it); its "publisher is unknown / More info, Run anyway" and "administrator permission" notes and "macOS/Linux not available yet" now appear nowhere on the site but `llms.txt`. Recommended home: a Documents install guide.
   - Knowledge Centre: the lede "Private 1-to-1 coaching built around a real task you bring, plus free guides from Zaaheen."; "From AED 1,299 · no package"; the dots "90 minutes, 1-to-1 · Microsoft Teams · Evenings, Mon–Thu"; the booking card's "Securing payment…", "Your session is booked", "Confirmed once payment is verified." (not the design's "Joining details sent to your email", which the coaching app does not promise). Everything else on the page is the coaching app's own wording, word for word (`site/src/data/coaching.ts`).
   - Sign-up consent box: approved by the founder (AUTH-PAGES-DESIGN D6).
8. **Monitoring dashboard: "signed up, never opened the app" (founder, session 61).** Everyone who signs up is a Clerk user; the app's first `/v1/lease` is what starts the trial (§8.26 §3). So a Clerk user with no trial started signed up on the website but never ran the app. The dashboard lists them so we can send a "welcome aboard, you haven't tried Zaaheen yet, here's the download" email. Before sending: session 43 locked "no reminder emails for now", and under UK PECR / GDPR this is marketing, so it needs consent captured at sign-up (a "send me tips and updates" box on the new Create-an-account page) and an unsubscribe.

---

## 4 · 🐛 Tech debt (live, with anchors)

- **D4 (session 60):**
  - the full-host admin ops (`vault_app::admin::ops` over a real `Application`) have no local test; the keeper-side surface is pinned over a locked host (`tests/keeper_admin.rs`);
  - the catch-up run writes no audit row (only "Run now" does, from the runner);
  - vault-app's `rusqlite` dev-dependency is kept (still used by `keychain/tests/live.rs`; dropping dev-deps changes feature resolution);
  - the connected-apps file's write retry sleeps on a runtime thread (`vault-app/src/keeper/clients.rs` ~300-308, ~80 ms under the `Clients` mutex) — move to `spawn_blocking` or a tokio sleep (security review N4).
- **The declared MSRV is stale:** `rust-version = "1.81"` against rmcp 3.4.1's 1.88 and the pinned 1.92; raising it turns on held-back clippy lints. Its own small change. `File::try_lock` sites carry `#[allow(clippy::incompatible_msrv)]`.
- **Read relevance:** carry the raw cosine through `HybridRetriever` fusion and filter per candidate; retire the vestigial BM25 gate (`vault-retrieval/src/strategies/hybrid.rs`).
- **The graph:** the merge path leaves stale relationships (`phases/merge.rs::apply_merge`); `graph.duckdb` is not encrypted by DuckDB itself; the read channel is parked OFF (`VAULT_ENABLE_GRAPH_CHANNEL=1`).
- **`VaultError::Storage(String)`** is a grab-bag; `retry_queue.rs::is_permanent` matches error wording.
- **The real-model smoke job** shares one `.partial` download path (`model_loader.rs`); only `--test-threads=1` protects it.
- **CI's MSVC toolset pin (14.44)** on the Windows jobs: drop it once DuckDB and llama.cpp support VS2026.
- **`scripts/run-desktop-dev.ps1 -NoReranker`** docs are stale since ADR-089.
- **Unpinned rmcp behaviours:** a wire test for `isError` on unparsable arguments, and the last line without a newline.
- **Session-36 leftovers:** the keeper's reranker uses 2.86 GB (latency waits for the founder); a relay closed before `initialize` logs a normal event as an error; `get_info` doesn't name the authorized boundaries; audit which direct-mode failures should be `isError`.
- **`export_logs` accepts a UNC destination** (`commands/logs.rs`); `export_memories` refuses it. Fix both in one place when either is touched next.
- **The "no vault" source tests substring-match after stripping comments**, so a deliberate type alias would pass them. Low likelihood.
- **`set_maintenance_schedule` carries `#[allow(clippy::too_many_arguments)]`**; the fix (one `Deserialize` struct) changes the wire contract and `dist/app.js` together.
- **A sign-in in progress cannot be cancelled from the app** (closing the browser tab leaves it waiting up to ten minutes).
- **`zaaheen mcp --direct` has no routine refresh** (§8.40); add it if direct mode ever ships.
- **The desktop UI's behaviour harness lives outside the repo** (`C:\Projects\MemoryVault-artifacts\ui-harness\`); CI pins the screens by structure only.
- **`every_gated_command_asks_before_it_serves` splits raw source on `"\n}\n"`** (`vault-tauri/src/guard.rs`) without normalising CRLF first.
- **The "one account" source test forbids named builders only** (`the_desktop_builds_its_account_once`).
- **"Delete everything" is not refused while a move is waiting** (nothing lost; untidy).
- **A V0.1 bridge snapshot `vault.db.pre_v0_2_bridge`**, on a machine where the (now deleted) bridge once ran, is not in `erasure::VAULT_ENTRIES` and survives "Delete everything". Only the founder's own machine could have one; check and remove by hand.
- **The lance NaN-distance upstream issue:** file a minimal repro.

---

## 5 · 🔒 Architectural locks (don't relitigate without the founder)

- **The LLM is out of the read path.** The vault returns facts; the agent composes. Phi-4-mini runs only at nightly consolidation.
- **Recall is sacrosanct.** A false "I don't know" is worse than a wrong answer; read changes are reorder-only and never false-empty.
- **Correctness of output is the product; correctness before latency.**
- **Zero-knowledge:** the server can never read vault contents. Sign-in never touches the vault key. Re-read BRD §11 and add an ADR-SEC before any crypto, auth or IPC change.
- **One keeper owns the vault;** relays and the desktop are its clients (ADR-102, ADR-108).
- **One codebase, three modes** (Local / BYOK / Managed). Sync is deferred until there are paying users.
- **The app subscription and a coaching session are separate purchases** (`SIGNIN-DESIGN.md` §8.34).
- **White label:** no model, vendor or stack names in anything a user sees; no long dashes on any screen (founder, session 54).
- **Free plans and provider defaults until traction.**

---

## 6 · 📐 How we work (standing rules)

Full rules live in memory (`~/.claude/projects/C--Projects-GitHub-Memory-Vault/memory/`) and the project CLAUDE.md.

- **Definition of Done (BRD §0.1):** `cargo build --workspace` with zero warnings; the affected crates' tests pass; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`; this file updated. **Local gates first, then CI**; only the founder can relax this, per batch.
- **Ask before any compiling cargo run, and report disk first.** Small `cargo test -p <crate>` runs are pre-approved, but **only in command shapes already built on this machine**; watch for heavy dependencies compiling and stop at once (session 60). Only `cargo fmt` is always free.
- **Cargo on Windows:** every cargo call through `C:\Projects\MemoryVault-artifacts\gate-step.ps1 -Name .. -Cmd ..` (its environment IS the cache fingerprint); from PowerShell; one cargo at a time; long runs detached through a Task Scheduler task plus a status Monitor; PowerShell scripts pure ASCII; never `python -` heredocs.
- **Commits:** one founder yes covers the commit and its push; message via `git commit -F`; `Co-Authored-By: Claude <noreply@anthropic.com>`; never commit `CLAUDE.md` or account details; admin and doc edits ride with code; `fmt --check` last, then `git status --short` before `git add`.
- **CI:** after every push and at session open, check **every workflow**: `for w in ci.yml codeql.yml secret-scan-history.yml monthly-tech-debt.yml; do gh run list --workflow=$w -L 2; done`. Broken CI is a regression, fixed the same session. `gh run watch`'s exit code is not the run's result.
- **Talking to the founder:** plain English; one step at a time; state a recommendation; don't escalate purely technical choices.
- **Engineering:** tests first, and prove they can fail; an independent review before any security-relevant commit; contract-establishing designs go to two independent reviewers first; no drive-by refactors (log them in §4); an ADR in the same commit as any architectural decision; quote locked text.

---

## 7 · 🔧 Key reference

- **Repo:** `https://github.com/shahbaz242630/zaaheen` (public). **Local:** `C:\Projects\GitHub\Memory Vault`. **Spec:** `Agent Build Specification.txt` (the BRD, canonical).
- **Crates:** vault-core, -storage, -embedding, -llm, -retrieval, -consolidator, -scheduler, -account, -mcp, -sync (stub; deferred), -connectors, -app, -cli (builds `zaaheen.exe` and `zaaheen-maintenance.exe`), -tauri (the desktop app).
- **Workers:** `workers/account` (TypeScript, Cloudflare; sandbox `api-sandbox.zaaheen.com`). From PowerShell: `fnm exec --using=24 -- npm.cmd test`.
- **The founder's machine:** install `C:\Program Files\Zaaheen\`; vault `%APPDATA%\com.zaaheen.app` (adopted in place by ADR-105); new installs `%LOCALAPPDATA%\com.zaaheen.app\vault`; models `%APPDATA%\com.zaaheen.app\models`; logs `%LOCALAPPDATA%\com.zaaheen.app\logs`; account `%LOCALAPPDATA%\com.zaaheen.app\account`; location record `%LOCALAPPDATA%\com.zaaheen.app\vault-location.json`; key lock and erasure marker `%LOCALAPPDATA%\com.zaaheen.app\keys\`; Claude's MCP log `%LOCALAPPDATA%\Claude\Logs\mcp-server-zaaheen.log`.
- **Files the keeper keeps in the vault folder:** `.vault-host.json` (discovery), `.vault-clients.json` (connected apps), `.vault-start-failure.json` (why a start failed, ADR-108 D7), `.vault.lock` / `.vault.intent` (never deleted).
- **Keychain entries:** vault key `default.com.zaaheen.v0.2` (Local); account refresh token `refresh-token.com.zaaheen.account` (Local).
- **Artefacts outside the repo:** `C:\Projects\MemoryVault-artifacts\`: MSIs, release-build and gate scripts (`gates-s57.ps1`, `gate-step.ps1`, `tests-s60*.ps1`), gate logs, design sources (`d4-design\`, `session35-design\`).
- **Release build:** cold ~6¾ h at `-j 1`; incremental ~70 min. Keep `target/release` except when the gates call for a full wipe.
- **Local-only docs (gitignored):** `CLAUDE.md`, `OPS-HANDOFF.md` (accounts), `SEO-HANDOFF.md` + `SEO-RESEARCH-A/B.md` (website).

---

## 8 · 🗂️ Archives and where each ADR lives

| File | Covers | ADRs (full text) |
|---|---|---|
| `AUTH-PAGES-DESIGN.md` | **Draft v2 (reviewed):** our own account pages on `account.zaaheen.com` | ADR-109, ADR-SEC-034 (session 61) |
| `DESKTOP-CLIENT-DESIGN.md` | **Live:** the desktop as a client of the keeper (D4) | ADR-108, ADR-SEC-033, the ADR-104 amendment (session 60) |
| `CONNECT-APPS-DESIGN.md` | **Live:** connecting AI apps | ADR-106 + ADR-SEC-031 (s55); ADR-SEC-032, ADR-107 (s59) |
| `SIGNIN-DESIGN.md` | **Live:** sign-in, trial and subscription | ADR-104, ADR-SEC-022 + amendments 1–17; ADR-SEC-023–028; ADR-054 Contract 2 amendment 3 |
| `VAULT-KEY-AND-LOCATION.md` | **Live:** where the vault key and the vault live | ADR-SEC-029; ADR-105 + ADR-SEC-030 and amendment 1 (L-f) |
| `HANDOFF_V0.2_PART5_ARCHIVE.md` | Sessions 44–60 (frozen 2026-09-24) | none new: pointers to the design files above |
| `HANDOFF_V0.2_PART4_ARCHIVE.md` | Sessions 19–44 | 073–084 bodies, 086–103, ADR-SEC-003–021 (the keeper arc is §8.19–§8.25; D4's origin §8.23) |
| `HANDOFF_V0.2_PART3_ARCHIVE.md` | Sessions 2–18 | 080–085, ADR-SEC-001, ADR-SEC-002 |
| `HANDOFF_V0.2_PART2_ARCHIVE.md` | T0.2.3c3 → T0.3.x | 047–072 |
| `HANDOFF_V0.2_PART1_ARCHIVE.md` | T0.2.0 → T0.2.3c2 | 037–046 |
| `HANDOFF_V0.1_ARCHIVE.md` | V0.1 | 001–036 |

**Next free numbers:** ADR-110 and ADR-SEC-035.
