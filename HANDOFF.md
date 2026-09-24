# Zaaheen (Memory Vault) — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2). **Last updated:** 2026-09-23, session 57.

> **How to read this file.** §1 is what to do next; act on it. §2 is where things stand. §3–§7 are the working rules and reference. Everything older, including every full ADR, lives in the archives (§8): cross-link to them and quote them, never paraphrase. This file was reset to this short form in session 44 (founder request); the previous 2,632-line version is `HANDOFF_V0.2_PART4_ARCHIVE.md`, word for word.
>
> **Keep it short.** Each session replaces §1 and updates §2, rather than appending a log. A decision goes to the design file it belongs to (e.g. `SIGNIN-DESIGN.md`) or to an ADR; a finished session gets two or three lines in §2's "Recent sessions".

---

## 1 · 🟢 Next — confirm CI on the pushed batch, then the pull request (ask), then the live test

**Session 57 (2026-09-22/23): the fresh gate run is DONE and the batch is committed and pushed** on `feat/s3-step4d` (founder: *"go ahead with the fresh build ... run it in steps"*, then *"when all steps finish and build is complete push and commit"*). One combined commit, because the steps share their wiring files; the relaxation (one gate run for the whole batch) is said in its body.

**The gate run, from a wiped `target\`, `-j 2`, line-tables-only, one cargo at a time:** launched detached through a Task Scheduler task (`gates-s57.ps1`, each step in a fresh `powershell -File gate-step.ps1`, stop below 15 GB), watched by a status-file monitor. fmt ✓ · build 53.6 min, zero warnings ✓ · clippy `--workspace --all-targets` ✓ (after three fixes, below) · tests, 0 failed: vault-core 62 · vault-account 244 · vault-retrieval 168 (1 ignored) · vault-mcp 91 · vault-app 493 (26 ignored) · vault-cli 52 · vault-tauri 186 (8 ignored). About 4.5 h; disk 170.7 → 84.2 GB free; links took free RAM to ~0.2 GB with no failure.

**Clippy found three things the earlier test-only builds never ran** (all in the location code, no behaviour change): `location/pointer.rs`'s `is_none_or_else` helper trait (`wrong_self_convention`) replaced by `map_or(true, …)` as the rest of the codebase does at MSRV 1.81; `moving/crash_tests.rs` a `CrashCase` type alias (`type_complexity`); `moving/model.rs` `.contains(&first)` (`manual_contains`). A `--keep-going` clippy confirmed nothing else in any crate. All 66 plants still match.

**Do, in order:**
0. At open: `gh run list` for the branch push and every workflow on `main` (§6); disk.
1. **If the branch's CI is red, fix it that session** (memory `feedback_broken_ci_is_regression_not_techdebt`).
2. **Pull request to `main` — ask the founder first** (auto-merge ruleset: pin the full head sha, memory `reference_github_auto_merge_ruleset`).
3. **Live test after an MSI build:** a move with the progress screen; "Connect it for me" for Claude (Store and direct install if possible) and Cursor end to end; then §3 item 1.
4. **Continue the walk-through** at page 5 (first memory), then the home tabs. Known already: the Agents tab shows "mcp · stdio" and "No agents connected yet" (§3 item 2, now also what would confirm "Connect it for me").

**Lessons from session 57:**
- **Clippy `--all-targets` is the only step that lints test code and the whole workspace;** the crate-scoped test builds of sessions 52–56 never ran it. Before a batch commit, one `cargo clippy --workspace --all-targets --keep-going` is cheap (seconds when warm) and shows every finding at once instead of one crate per run.
- **A detached Task Scheduler driver survives Claude Code's shell reaping,** and a status file plus a 30-min monitor that also checks the task is still "Running" is a reliable watcher.
- **Slip, caught:** a `python -` heredoc again (the memory rule); stopped within 2 min, no disk harm, the edit redone with the Edit tool.

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
- **S0 spike: done** (session 43). **S1: merged** (session 44, `main` `99ff08e`). **S2: done** (session 45: the Worker's offline core and endpoints, `8b99454` and `5a62e11`, `SIGNIN-DESIGN.md` §8.29–§8.30; session 46: `/pay/`, flood protection and the sandbox deployment at `api-sandbox.zaaheen.com`, live-tested end to end, §8.31). **S3:** steps 1–4c merged (sessions 47–49, `main` `e3c0b23`); **4d-1, 4d-2 and 4d-3 built** (sessions 50–51, §8.38–§8.40), **gated and committed on `feat/s3-step4d`** (session 57) — S3 is complete in code; the PR to `main` is next. **S4** (the export) is built, and its button shipped with 4d-2.
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
- **Website:** the Astro site, with `/pay/`, is on `main` under `site/` (merged in session 59 from `site/production-ready`, founder: *"lets merge it"*). **Not deployed:** publishing is switched off by the repository variable `SITE_PUBLISH` (unset). Until it is `true`, `main` builds and audits the site and `site.yml` neither runs the release audit (placeholders, live payments) nor publishes. At launch: final copy, live Paddle, Hostinger `site-deploy` set up (`SEO-HANDOFF.md` §8), then set `SITE_PUBLISH=true`. zaaheen.com still shows Hostinger's parked page. The old worktree `C:\Projects\GitHub\Memory Vault site` and its stash `session-41 docs …` are superseded.
- **Coaching booking app:** separate repo `C:\Projects\GitHub\Training Page` (its own CLAUDE.md, HANDOFF and rules). Parked.
- **Accounts:** Clerk (dev instance set up), Microsoft 365, Paddle, Cloudflare and the bank. All details live **only** in the local `OPS-HANDOFF.md`; this repo is public.

### Where the vault and its key live (ADR-SEC-029 + ADR-105, `VAULT-KEY-AND-LOCATION.md`)
- **The key (ADR-SEC-029): built** (session 52), **gated and committed on `feat/s3-step4d`** (session 57). It stays on this computer; one key lock per Windows user; never a new key over existing data; a spare-copy move to Local; erasure that can never leave the key behind or bring it back; five founder-approved startup messages.
- **The location (ADR-105 + ADR-SEC-030): designed and locked; L-a–L-f built (sessions 52–55), reviewed and plant-proved (session 56), gated and committed on `feat/s3-step4d` (session 57).** One record and one resolver for every process; the vault's own ID file; the folder checks; the move and its recovery; the screens (a setup step after sign-in, Settings "Move my memories…", the notice after a move) and "Start again in the default place". Session 55 added **amendment 1, L-f: the "Moving your memories" screen**; session 56 compiled, reviewed and plant-proved all of it. **Gated and committed in session 57; the PR to `main` is next.**

### Recent sessions
- **57 (2026-09-22/23):** Founder's go for the fresh gate run, then *"when all steps finish ... push and commit"*. **The cold gate run ran detached through Task Scheduler in ten steps, all green**: build 53.6 min with zero warnings; clippy `--all-targets` after three fixes to the location code it alone lints (no behaviour change); tests vault-core 62, vault-account 244, vault-retrieval 168, vault-mcp 91, vault-app 493, vault-cli 52, vault-tauri 186, none failed. **The batch (4d-1/2/3, ADR-SEC-029, ADR-105 L-a–L-f, ADR-106, session 56's fixes) committed as one and pushed to `feat/s3-step4d`.** One slip: a `python -` heredoc, stopped at once, no harm.
- **56 (2026-09-22):** `main` checked green at open. Reordered at the founder's question so the review and plant planning came before the first build. **Three independent reviewers of session 55's code: nothing at BLOCKER, MAJOR or MINOR.** Plant planning found two tests that could not fail (a capped count hiding a double count; the lock's download pin), both tightened. **First build of sessions 54–55 clean under `-D warnings`**; three test defects fixed, no code defect, one of them a wiring-guard blind spot now guarded. **28/28 planted bugs caught** (ADR-105 47/47 in all); clean runs: vault-app 454, vault-tauri 113 + 59 + bins. Stopped before the fresh build, as the founder asked. Settings row names approved. Then, at his request, a read-only disk scan and an approved cleanup (`target\`, Docker, download caches, emulator data, VS Code caches, codeguard logs, the old dev-vault backup): 17.5 → 190.4 GB free, no build run. **Next session: the fresh gate run on his "go", then the commits.**
- **55 (2026-09-22):** `main` checked green at open (every workflow). **The founder's walk-through, page by page, in the browser preview** (builds deferred: the laptop hangs). Pages 1–4 and Settings changed, each approved: no step count on the welcome; matching sign-in buttons; the lock's download only where there are memories (§8.42); "Choose another folder…" under the path; **the "Moving your memories" screen with real progress** (ADR-105 amendment 1, L-f, and `startup_state` open before the lock, §8.43); plain words on the connect step and **"Connect it for me"** for Claude Desktop and Cursor through each app's own install route (ADR-106, `CONNECT-APPS-DESIGN.md`; a file-writing design withdrawn after review; the Claude extension route live-tested on the founder's Claude); Settings laid out like Claude's. Two adversarial design reviews (L-f: no BLOCKER/MAJOR; connect round 1: 1 BLOCKER + 6 MAJOR, hence the redesign). **Nothing compiled yet**; the build, plants, a code review and the gates are §1's next steps.
- **54 (2026-09-22):** `main` checked green at open. **L-e built** (founder: *"yes partner lets go"*): the setup's location step (after sign-in: the location commands are gated), the move's confirmation and restart, Settings "Move my memories…" and "Stop waiting for the old copy", the notice after a move, and L6 "Start again" on the startup dialog; words shown as built and approved (*"yes all good"*); "move back to the usual place" left for later (*"leave it for now and go ahead"*). 28 tests red first; driven in the browser, then a full walk-through from a fresh install at the founder's request. Two independent reviewers: nothing at BLOCKER/MAJOR/MINOR. A test the ADR listed had never been written (the relay's missing-location answer) — added. Planted bugs: 38/38 caught, paused at T06 (the builds froze the founder's laptop). Then the founder's screen changes, one at a time with screenshots: the icon on every screen, the new welcome headline (animated), no long dashes anywhere, and "Create an account" beside "Sign in" (`SIGNIN-DESIGN.md` §8.41). Built in the source; the vault-tauri/vault-app builds that prove them are next.
- **53 (2026-09-21):** `main` checked green at open (every workflow). **L-d built** (founder: *"yes partner go ahead with the move"*): the move copies, checks every file, switches the record in one atomic write, then removes the old copy; a start that finds a move half done removes only a folder carrying that move's ID and tries again, twice at most. Every crash point and every pair of crash points checked on an in-memory disk; 39 tests, the desktop wired to finish a move before it opens anything. One design addition recorded as an L-d decision: M1 also waits until no window has the memories open (the desktop holds no vault lock). One crash-window bug found and fixed before any commit (a half-written `.move-id`).
- **52 (2026-09-21):** `main` checked green at open. **Founder: no full build until the batch (vault key + location) is done; the wipe takes all of `target\`.** **ADR-SEC-029, the vault key**, designed through four adversarial rounds, founder-locked ("yes plz"), built tests-first and finished: 81 tests including live Credential Manager, 29/29 plants, independent review "committable". It found and fixed two real bugs besides the roaming key — the startup dialog told people to delete their key, and "Delete everything" left the database behind so the app could not start again (confirmed live). The five startup messages founder-approved ("partner wording is good .."). **ADR-105 + ADR-SEC-030, the location**, designed through three rounds, founder decisions (new installs local; exFAT allowed with a note) and founder-locked; L-a, L-b and L-c built and green. Found on the way: 4d-3's tool-contract change needed `WIRE` 2 → 3. Slips, caught and recorded: two more `python -` heredocs (memory updated), and a planted-bug harness that ran cargo from Git Bash (a ~30-min rebuild) and later overflowed PATH (plants re-run in fresh shells).
- **51 (2026-09-21):** `main` checked green at open. **Two founder decisions:** the support address `customerservice@zaaheen.com` (already live in Outlook) and the **lock screen's wording** ("yes partner go with that wording"). **4d-2 built on `feat/s3-step4d`, not committed**: the lock screen, the setup's sign-in step, the account panel, the banners, the checkout wait, Download my memories and the Delete dialog's subscription note, all in `dist/` (§8.39). 34 frontend guards (14 new, 13 run failing first), 19/19 plants, 13 flows driven in a browser against a stand-in backend, and an independent review whose two findings were fixed. Recorded, not decided: BRD §11.4.1's multi-factor before a full export (§3). One planted run was stopped for low memory in the background and re-run, with the founder's yes, in the foreground. **Then 4d-3, also not committed** (§8.40): the founder approved the trial-reminder wording and once-per-conversation ("yes all good"); the AI apps now get `SUBSCRIPTION_TRIAL_ENDING` / `SUBSCRIPTION_PAYMENT_FAILED` in `memory_read`'s warnings (ADR-054 Contract 2 amendment 3), and the keeper and the desktop share one routine refresh. 12 run failing first, 9/9 plants, and a review with nothing at its threshold.
- **50 (2026-09-21):** PR #75 confirmed merged (`e3c0b23`), every workflow green. **ADR-SEC-027 founder-locked** (the signed-in email kept in Credential Manager, "yes A"). **4d-1 built on `feat/s3-step4d`, not committed**: the remembered email, one `Account` per desktop process (ADR-SEC-028; it had been built twice), `account_access`, `AccountSlot`, erasure signs out, the clock notice, and the routine refresh that §8.37 said ran but had no caller. 240 + 258 + 87 + 20 tests green; 15 planted bugs caught after one test was replaced (d1-08); the review found one doc overclaim. **Founder: build 4d-2, 4d-3, the key fix and the vault location first, then wipe `target\debug` and run the gates once, fresh.** Two process slips, both fixed and recorded: a python heredoc probe (59 MB, stopped) and a `Start-Process` launch that ran the whole workspace's tests for 45 min.
- **49 (2026-09-20):** PR #73 merged (`84b5d43`); **step 4 split into 4a–4d**; **S4's shape founder-locked: one readable `.md` file**. **4a merged** (PR #74 → `4a792af`): the guard, 15 gated commands, ADR-SEC-023, 17 tests, 13/13 plants. **4b + 4c built and pushed as one commit** (`4bc0543`, PR #75): the checkout client, `ExternalLink`, the token-rotation extraction, `AccountOps` (holds no vault), the export, six new commands, ADR-SEC-024/025/026 in §8.37. **Independent reviews found seven defects that tests and planted bugs both missed**, including an unmatched code fence that would have corrupted the whole export and a UNC path that would have written every memory over SMB. The gates then caught six commands with no Tauri permission — they would have shipped as buttons that do nothing. Only **4d** (the screens) remains.
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

1. **Before the next installer: live-test Claude Desktop and Cursor.** ADR-SEC-029's operating rule applies on the founder's machine: **close every other Zaaheen build (the installed app, its keeper, any dev build) before the first open of the new one** — its first open moves the key to Local persistence. rmcp 2.x answers any known protocol version a client asks for, where 1.5.0 capped it. Nobody has seen what either app now negotiates. Read the `tauri` advisory first (BRD §11.12, vault-tauri).
2. **The Agents tab says "No agents connected yet" while Claude and Cursor are connected** (founder-found, 2026-09-11). It breaks the honest-UI rule. A design exists (session-37 opener, item 1): the relay passes the app's name upstream, the keeper keeps a live list, and the tab reads it. **Now also what would confirm "Connect it for me" worked** (ADR-106 decision 6): next after the session-55 batch. The tab also shows "mcp · stdio" (jargon; the walk-through will reach it).
3. **Walk through the app with the founder** and record every change he raises, in his words, before building more.
4. **D4: the desktop app becomes a client of the keeper.** Decided, not built (archive §8.23). It fixes "Delete everything" while an AI app is connected, among other things.
5. **The second machine.** The app has never run on a computer other than the founder's; this is the highest-information test left.
6. ~~The vault key is stored with roaming ("Enterprise") persistence.~~ **Built in session 52 (ADR-SEC-029), not yet committed** — see §1.
7. **A founder decision before launch: a second check before "Download my memories"?** BRD §11.4.1 lists *"Exporting full vault"* among the operations that get multi-factor by default; the export has none, and 4d-2 put a button on it (on the lock screen too). My recommendation is to **keep it as it is and record the divergence in the BRD**: whoever can press the button is at the same unlocked Windows session that can already read every memory in the app, and a second factor would need either the account — which a locked person may not be able to use, the very case the export exists for — or a vault passphrase V0.2 does not have. Reasoning in `SIGNIN-DESIGN.md` §8.39. Raised in session 51; not yet decided.
8. **The agent-facing recovery hints name `vault-cli consolidate run`**, a binary we don't ship (`vault-retrieval/src/structured_read_pipeline.rs` ~857–894). The user's path is the app's Consolidation → Run now.
9. **Dependabot:** PRs #45–#49 (quinn-proto HIGH, cmov, serde_with, openssl, tar) and 9 open alerts. Triage one at a time; each needs its own CI run.
10. **Rebuild the `.mcpb` bundle**; write a real README; list the server in the MCP registry.
11. **"Move back to the usual place" — only if beta testers ask** (founder, session 54: *"leave it for now and go ahead"*). ADR-105 L4.2 refuses every folder inside `%LOCALAPPDATA%\com.zaaheen.app`, so a person who moved to a USB stick can bring the memories back to a folder of their own, but not into the hidden default one. Needs an L4 exception for exactly the new-install folder (it may still exist with empty lockfiles) and its own tests. "Start again" already covers a lost drive.
12. **Ideas, not scheduled:** an email-handling agent (design before go-live, draft-for-approval first); rmcp 3.x (the 2026-07-28 spec; a planned upgrade with a live test).

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
- **`export_logs` accepts a UNC destination** (`\\host\share\...`), so a log export can be written to another machine. `export_memories` refuses it (session 49); the shared validator in `commands/logs.rs` was left alone because a log file reaching a share is a much smaller fact than a vault doing so. Fix both in one place when either is touched next.
- **The three "no vault" source tests substring-match after stripping comments**, so a deliberate type alias (`type VaultHandle = Application;`) would pass them while holding the real vault. Real but low-likelihood — it needs an intentional rename, not an ordinary edit. Noted by session 49's review.
- **`set_maintenance_schedule` carries `#[allow(clippy::too_many_arguments)]`** (8 args, one past the limit, because the guard added `State<'_, Entitlement>`). The structural fix — collapsing the five schedule fields into one `#[derive(Deserialize)]` struct — changes the command's wire contract and needs a matching change in `dist/app.js`, so it belongs with frontend work, not with a security gate. **Still open after 4d-2**, which kept to the screens.
- **A sign-in in progress cannot be cancelled from the app.** Closing the browser tab leaves "Sign in" waiting until the listener's ten-minute window ends (§8.26 §3); there is no cancel command, and `PendingSignIn`'s wait has no abort handle the desktop can reach. A deny on the consent screen ends it at once. Found building 4d-2; a small UX item for the founder walk-through (§3 item 3).
- **`zaaheen mcp --direct` has no routine refresh** (§8.40). Direct mode is a developer opt-out that bypasses the keeper; it carries the account notices through the same gate, but its lease is refreshed only on a denial. Add `run_routine_refresh` there if direct mode ever ships to people.
- **The desktop UI's behaviour harness lives outside the repo** (`C:\Projects\MemoryVault-artifacts\ui-harness\`: a stand-in Tauri bridge driven by Playwright). `frontend_contract.rs` reads the source, so in CI the screens' behaviour is pinned by structure only. Bringing the harness into CI means a JS runner in a crate that deliberately has none; decide if a UI regression ever slips through.
- **`every_gated_command_asks_before_it_serves` splits raw source on `"\n}\n"`** (`vault-tauri/src/guard.rs`), with an `unwrap_or(len)` fallback. On a CRLF checkout it does not fail, it silently scans to the end of the file, which can find a `.require().await` belonging to the next function. Normalise line endings first, as §8.38's tests do. Found in session 50.
- **The "one account" source test forbids named builders only** (`the_desktop_builds_its_account_once`): an inline second `Account` assembled from `AccountDir`, `TokenStore` and friends in `main.rs` would pass it. Needs a deliberate rewrite, not an ordinary edit; noted by session 50's review.
- **Key-step errors outside ADR-SEC-029's five messages still show details** (`vault-tauri/src/lib.rs` `format_keychain_error_dialog` falls through to `format_startup_failure_dialog`): a V0.1 bridge `Storage` error or a `getrandom` failure shows "Details: …". Reachable only with `VAULT_KEY` set or a failing OS random source. Found by session 52's code review (finding 9).
- **The V0.1 bridge's snapshot `vault.db.pre_v0_2_bridge` is not in `erasure::VAULT_ENTRIES`**, so it survives "Delete everything" (keyed with the V0.1 passphrase, not the master key). `VAULT-KEY-AND-LOCATION.md` ADR-SEC-029 Consequences.
- ~~**`KeyLocation::production()`'s erasure folder is `install_paths::data_dir()`.**~~ Resolved in session 52 (ADR-105 L-a, ADR-SEC-029 amendment 5): it is the recorded location, resolved under K1, and none when it cannot be resolved.
- **The V0.1 bridge's snapshot and a move:** `vault.db.pre_v0_2_bridge` is not in `VAULT_ENTRIES` (above), so a move neither copies nor removes it; it stays in the old folder.
- ~~**No "moving your memories" screen while a move runs**~~ **Built in session 55 (ADR-105 amendment 1, L-f); compiled, reviewed and plant-proved in session 56; not yet committed.** The original entry, for the record: (ADR-105 L-e decision 2). The move runs in the desktop's `setup()` before the vault opens, under a window Tauri has already created but that cannot draw. Today: honest words before the restart. A real screen needs the start restructured — the move after the window is up, the rest of `setup()` (key, `Application::new`, managed state) after the move, and the frontend waiting for it. **Founder-agreed (session 55 walk-through), before the public launch:** *"I agree with you to have the screen or lets say an animation which shows memories moving ... or a popup which shows progress of memories moving"*: a "Moving your memories" screen with real progress (bytes copied of the total, from the copy loop), not a fake animation.
- **"Delete everything" is not refused while a move is waiting** (session 54 review, below its bar). The next start then moves an already-emptied vault, or gives the move up while the marker remains; nothing is lost. Refusing erasure while `pending_move` is set, or clearing `pending_move` in the erasure, would make it tidy.
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
- **Keychain entries:** vault key `default.com.zaaheen.v0.2` (Enterprise on the installed 0.2.2; the next build moves it to Local on first open, through a spare `default.com.zaaheen.v0.2.migrating` that exists only during the move); account refresh token `refresh-token.com.zaaheen.account` (Local).
- **From the next build (ADR-105):** the location record `%LOCALAPPDATA%\com.zaaheen.app\vault-location.json`, the key lock and erasure marker in `%LOCALAPPDATA%\com.zaaheen.app\keys\`, and `.vault-id` inside the vault folder. The founder's existing vault stays in `%APPDATA%\com.zaaheen.app` (adopted on first run); new installs go to `%LOCALAPPDATA%\com.zaaheen.app\vault`; models stay in `%APPDATA%\com.zaaheen.app\models`.
- **Artefacts outside the repo:** `C:\Projects\MemoryVault-artifacts\`: the MSIs, `verify-msi-0.2.2.ps1`, `live-test-0.2.2.ps1`, the release-build scripts, design sources.
- **Release build:** cold ~6¾ h at `-j 1` (the desktop feature set recompiles the heavy dependencies a second time); incremental ~70 min. **Keep `target/release`.**
- **Env (fresh PowerShell):** `$env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"; $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"`.
- **Local-only docs (gitignored):** `CLAUDE.md`, `OPS-HANDOFF.md` (accounts), `SEO-HANDOFF.md` + `SEO-RESEARCH-A/B.md` (website).

---

## 8 · 🗂️ Archives and where each ADR lives

| File | Covers | ADRs (full text) |
|---|---|---|
| `SIGNIN-DESIGN.md` | **Live:** the sign-in, trial and subscription design | ADR-104, ADR-SEC-022 + amendments 1–17 (§8.41 Create an account, §8.42 the walk-through's screens, §8.43 `startup_state` on the open list); ADR-SEC-023–028; ADR-054 Contract 2 amendment 3 (§8.40) |
| `VAULT-KEY-AND-LOCATION.md` | **Live:** where the vault key and the vault live | ADR-SEC-029 (session 52); ADR-105 + ADR-SEC-030 (sessions 52–54) and amendment 1, L-f, the moving screen (session 55) |
| `CONNECT-APPS-DESIGN.md` | **Live:** "Connect it for me" | ADR-106 + ADR-SEC-031 (session 55) |
| `HANDOFF_V0.2_PART4_ARCHIVE.md` | Sessions 19–44 (frozen 2026-09-18) | 073–084 bodies, 086–103, ADR-SEC-003–021 (the keeper arc is §8.19–§8.25) |
| `HANDOFF_V0.2_PART3_ARCHIVE.md` | Sessions 2–18 | 080–085, ADR-SEC-001, ADR-SEC-002 (+ Part 2, Amendment 1) |
| `HANDOFF_V0.2_PART2_ARCHIVE.md` | T0.2.3c3 → T0.3.x | 047–072, tech-debt narratives, technique map, consolidator inventory |
| `HANDOFF_V0.2_PART1_ARCHIVE.md` | T0.2.0 → T0.2.3c2 | 037–046 |
| `HANDOFF_V0.1_ARCHIVE.md` | V0.1 | 001–036 |

**Next free numbers:** ADR-107 and ADR-SEC-032.
