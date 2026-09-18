# Zaaheen (Memory Vault) — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2). **Last updated:** 2026-09-18, session 45.

> **How to read this file.** §1 is what to do next; act on it. §2 is where things stand. §3–§7 are the working rules and reference. Everything older, including every full ADR, lives in the archives (§8): cross-link to them and quote them, never paraphrase. This file was reset to this short form in session 44 (founder request); the previous 2,632-line version is `HANDOFF_V0.2_PART4_ARCHIVE.md`, word for word.
>
> **Keep it short.** Each session replaces §1 and updates §2, rather than appending a log. A decision goes to the design file it belongs to (e.g. `SIGNIN-DESIGN.md`) or to an ADR; a finished session gets two or three lines in §2's "Recent sessions".

---

## 1 · 🟢 Next — S2 step 3: the `/pay` page and the first deploy

**State (2026-09-18, session 45, mid-session):**
- **S2 step 1 is merged:** PR #68 → `main` `8b99454` (rebase; its tree is identical to the tested `468bee1`). PR #68's checks were all green on 3 OSes; `vault-account` ran 208/208 on Windows CI.
  - `main`'s own push run was still going at the last look: everything had passed except Windows build + test, which was 35 min in. **Check it.**
- **S2 step 2, the endpoints, is built and uncommitted** (`SIGNIN-DESIGN.md` §8.30): `/v1/lease`, `/v1/checkout`, `/paddle/webhook`, `/clerk/webhook` and the daily sweep (cron 03:17 UTC), with the Clerk and Paddle clients.
  - 261 Worker tests inside workerd; `tsc` clean.
  - 32 new planted bugs, each caught, and step 1's 17 still caught against the full suite.
  - Contracts were read from Clerk's BAPI spec, Paddle's API reference and docs, and Svix's docs. Nothing was guessed. Clerk's metadata deep-merge (null deletes a key) is confirmed.
  - **Security rule found while reading Paddle's docs:** a buyer's browser can set `custom_data`, so the Paddle webhook and the sweep only update a record that already names that Paddle customer. Only our own authenticated `/v1/checkout` makes that link.
  - **Independent review: no defects.** Its one informational point (blind-merge races, self-correcting) is recorded as an accepted residual in §8.30.
  - **Next:** commit + PR (founder yes), then CI.
- **Product decisions (founder, session 45):**
  - buyers see the product name **"Zaaheen"**, with prices "Monthly" ($5) and "Yearly" ($48);
  - **tax is added on top, everywhere** (Paddle `tax_mode: external`), because Paddle's own fee already takes 5%. The live prices must match, and the pricing page should say "plus applicable tax" (wording to confirm with the adviser).
- **Account-side status** (the bank, Stripe, the Paddle and Cloudflare accounts, keys and ids) lives only in the local `OPS-HANDOFF.md`, section F.
- **Tooling:** Node 24.19.0 via fnm (`fnm exec --using=24 -- npm.cmd ...` from PowerShell). Node 22's npm 10.9.8 crashes on this tree (`edgesOut`). **Edit files with Write/Edit or a `.mjs` file, never `node -e` inside double-quoted bash:** backticks got executed twice this session (memory `feedback_no_backticks_in_double_quoted_bash`).
- **Standing approval (founder, session 44):** small `cargo test -p <crate in hand>` runs need no ask. Whole-app builds, every commit and every merge still do.

**Do, in order:**
1. **Finish step 2:** act on the review's findings (tests-first), then commit + PR (founder yes), then CI on every workflow. Step 2 touched no Rust, so the cargo gates only need `fmt --check`.
2. **S2 step 3: the `/pay` page, then the first deploy.**
   - Set the sandbox's **default payment link** (Paddle refuses to create any transaction without one).
   - Build `/pay` (Paddle.js v2 overlay, `_ptxn` only), reusing the founder's other live, Paddle-approved site for the pattern and the CSP entries (memory `reference_affiliate_site_paddle_reuse`). Its code, account and keys are not reused.
   - The lease signing keys: the primary is generated for Cloudflare Secrets; **the backup is generated offline by the founder and its private half never touches a server.**
   - The first deploy: flood protection (the rate-limit binding, or a Cloudflare rule; confirm the binding on the free plan), secrets set by clipboard, then a live test against the Zaaheen sandbox.
   - **The Cloudflare login stays as it is** (founder). The recommended "add the company login as a second admin" is noted in the local `OPS-HANDOFF.md` for when he wants it; don't raise it before the deploy.
   - Work step by step with the founder: one item, show it, ask only that item's decision.
4. Then **S3** (the gate in vault-mcp, keeper lock mode, the desktop sign-in step, account panel and lock screen), with **S4** (export) in the same release. The build order and every rule is in `SIGNIN-DESIGN.md`. S3's wiring rules are in its §8.27.
5. **Then: the user chooses where their memories live** (founder, 2026-09-18: "add it to the plan after S3 + S4"). Write its design (ADR-105 + an ADR-SEC, BRD §11 re-read) once S2 is finished; build it right after S3 + S4, whose onboarding it adds a step to.
   - **Onboarding:** "Your memories will be saved here: [default] — Change…". The native folder picker (drives, "New folder"); the default is preselected. Settings gets "Move my memories".
   - **One location for every process:** the desktop app, the keeper, every relay and the nightly maintenance runner (so consolidation follows the vault). Today they all resolve `%APPDATA%\com.zaaheen.app` (`vault-app/src/install_paths.rs::data_dir`, Tauri's `app_data_dir`), so a pointer at the default location is the likely shape. Models stay where they are.
   - **Must-haves:**
     - refuse cloud-synced folders (OneDrive, often behind "Documents"; Dropbox; Google Drive; iCloud) and network drives, because sync clients corrupt a database mid-write;
     - an external drive that is missing means "reconnect your drive", never a new empty vault (a drive letter can change);
     - a folder on exFAT/FAT32 has no Windows permissions, so the ADR-SEC-019 ACL cannot apply there (the files stay encrypted).
   - **Say it plainly in the UI:** a USB drive does not carry memories to another computer (the key stays in this computer's Credential Manager). That needs the BRD §11.5.4 backup and restore.
6. Then **S5** (coaching) and **S6** (production and the live test).

**Lessons from session 44, already saved in memory:**
- Gate every item used only by `#[cfg(windows)]` code, constants included (`feedback_cfg_gate_transitively_platform_only_items`).
- Test failure messages name the kind of a result and never print account values, because CodeQL's cleartext-logging rule flags them (`feedback_tests_never_print_account_values`).

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
- **S0 spike: done** (session 43). **S1: merged** (session 44, `main` `99ff08e`). **S2: steps 1 and 2 built** (session 45: the Worker's offline core, merged as `8b99454`, and its endpoints; `SIGNIN-DESIGN.md` §8.29–§8.30). Step 3 (`/pay` and the first deploy) is next.
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
- **Website:** branch `site/production-ready` (`2ccf3f1`) holds the Astro site. Not merged, not deployed; zaaheen.com still shows Hostinger's parked page. Rebasing that branch will conflict on `HANDOFF.md`: take `main`'s version. Its local stash `session-41 docs …` is superseded; drop it.
- **Coaching booking app:** separate repo `C:\Projects\GitHub\Training Page` (its own CLAUDE.md, HANDOFF and rules). Parked.
- **Accounts:** Clerk (dev instance set up), Microsoft 365, Paddle, Cloudflare and the bank. All details live **only** in the local `OPS-HANDOFF.md`; this repo is public.

### Recent sessions
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
