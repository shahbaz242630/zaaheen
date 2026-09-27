# Zaaheen (Memory Vault) — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2). **Last updated:** 2026-09-27, session 69 (legal documents and Pricing page done; review, commit and push this session; next: Knowledge Centre documents, SEO and GEO, then the build; see §1).

> **How to read this file.** §1 is what to do next; act on it. §2 is where things stand. §3–§7 are the working rules and reference. Everything older lives in the archives (§8): quote them, never paraphrase. Reset to this short form in session 60 (founder: *"lets archive it and start fresh clean handoff"*); trimmed again in session 69 (sessions 58 to 68 in `HANDOFF_V0.2_PART6_ARCHIVE.md`, word for word).
>
> **Keep it short.** Each session replaces §1 and updates §2 rather than appending a log. Decisions go to their design file or an ADR; a finished session gets two or three lines in §2's "Recent sessions".

---

## 1 · 🟢 Next — the Knowledge Centre's documents, then SEO and GEO, then the big build (founder, session 69: *"after 1 and 2 ... we go for the build partner"*)

**Session 68 (closed):** every pre-build item committed and pushed as `7969e58` on PR #84, CI all green; `target` and Docker deleted (176 GB free). Its full notes, and sessions 64 to 67's, are in `HANDOFF_V0.2_PART6_ARCHIVE.md` §1, word for word. **Do not merge #84** until the full gate run and a new installer pass. **No local cargo builds until the founder says.**

**Session 69 (2026-09-27, in progress): the website's legal documents and Pricing page, before the build** (founder: *"before the build we need to work on our documents for website"*). All decisions and rules in `LEGAL-DOCUMENTS-DESIGN.md` (ADR-SEC-036, L1 to L7, the comparison rules). **Nothing committed yet.**
- **Research:** two independent legal reports (`MemoryVault-artifacts\legal-research-s69\`), reconciled; competitor research + dated evidence screenshots (`MemoryVault-artifacts\pricing-research-s69\`).
- **Founder decisions:** L1 liability cap = greater of 12 months paid or USD 50, stated as not a refund; L2 refund anti-abuse (one per person, no part refunds, abuse, bank reversals, app-only); L3 no address/phone yet (no US marketing email until a postal address); L4 no EU/UK representatives yet, contact `customerservice@`; L5 60 days' notice + unused yearly refund if we stop; L6 tax line on Pricing; L7 licence notices ship with the app, `/licences/` names no component. Paid compliance waits for traction (founder).
- **Website (approved page by page):** `/terms/` rewritten; `/refunds/`, `/privacy/` (trial fingerprint disclosed, deletion routes, transfers, complaints), `/security/`, `/company/`, `/docs/` (Delete my account row, download-copy wording), new `/licences/`, `/.well-known/security.txt` (+ audit check, 3 negative controls); `/pricing/` tax line, 7-row comparison (where kept / encrypted / used to train AI) and a 16-question FAQ (`src/data/pricing.ts`). Sign-up tickbox: 18+, Terms and Refund Policy, "I have read" Privacy. Coaching-account claim corrected in the delete page and the app's dialog (+ its contract test). Site audit OK, 46/46 audit tests, account build + 39/39.
- **Settings › Documents (Rust written, not compiled; ADR-SEC-036):** two dropdowns approved (founder: *"its perfect keep it"*), 12 documents incl. Open-source licences; `Document` closed list + `ExternalLink::document`; gated `open_document`; permission, capability, `GATED_COMMANDS`; tests in vault-app, `frontend_contract.rs`, guard scans. `cargo fmt` done.
- **Independent review: GO WITH FIXES, no blocker** (the command's gate and closed list verified; no likely compile, clippy or test failure found). All 17 findings fixed: the Terms' licence and after-subscription wording (what works while locked: download and delete everything), refund wording that named only the Refund Policy, the outage refund limited to our service, "only"/"nothing else" overclaims, privacy complaints answered in 5 working days, the ChatGPT free-plan cell, missing comparison sources, and the site.ts contract test now reads inside `POLICIES` and `DOCS_SECTIONS` only. The licence-notices file is still owed in the big build (L7): `/licences/` says the notices ship with the app.
- **Later, not blocking the app launch:** coaching's own Terms, cancellation policy, privacy notice, checkout tickbox, EU withdraw page and cookie (coaching repo; founder decisions on cut-offs, recordings, cookie); AI-written text labels for EU AI Act Art 50(2); re-check the comparison every 3 months; renew security.txt yearly (the audit fails 30 days before).

**Session 69 closed:** committed and pushed as `46c7b38` (founder's go). At close: Site and Secret scan green; **CI and CodeQL still running** (the first compile of sessions 68 and 69's Rust on CI; runs `36312952781`, `36312952788`). Founder: *"ci will take time.. lets close session we can check next session"*.

**Session 70 (2026-09-27, in progress, nothing committed):** CodeQL on `46c7b38` green; CI still running at 30 min. Coaching policy locked by the founder (no refunds, reschedule only 24 h+ before, no-show forfeits, only the coach cancels, bold terms + consent tickbox before payment; memory `project_coaching_no_refund_policy`). Two independent legal research agents running (Stripe's requirements, UK/EU/US/UAE consumer and data-protection law) → `MemoryVault-artifacts\legal-research-s70\coaching-legal-{A,B}.md`; founder decides after reading them (he questioned whether UK/EU rules bind a UAE company). **Knowledge Centre footer built:** `Footer.astro` variant `coaching` (Base `footer` prop), coaching documents driven by `COACHING_POLICIES` in `site.ts` (all `live: false` until written); audit: the Knowledge Centre must use it, it links Company Information and no app policy (+4 negative controls). Audit OK, site tests 15/15, audit tests 50/50; desktop and 390 px checked.
- **Both reports in (A and B agree):** the policy stands for change of mind, late changes and no-shows; the law forces: we cancel/can't deliver → customer chooses refund or new time (UAE Res 66/2023 art 13, CRA Sch 2 para 4); poor session → repeat then refund (UAE art 29, CRA ss 49-57); UK/EU 14-day cancel until the session is delivered, with a second tickbox when the session is within 14 days (CCR reg 36, CRD 16(a)); never a bare "no refunds" (UAE art 34(11)). New: UAE requires Arabic contract/ads/invoice (Law 15/2020 arts 8(4), 25, 26; AED 100k fines); the booking app's `ats` cookie (gclid/fbclid, 90 days) needs consent or removal; Stripe Checkout has no `consent_collection`; keep records 7 years (CT Law art 56).
- **Pages written and approved by the founder (*"all look good partner"*):** `/knowledge-centre/terms/`, `/knowledge-centre/booking-and-refunds/`, `/knowledge-centre/privacy/` (Policy layout `coaching` prop: coaching nav + footer); `knowledge-centre.astro` moved to `knowledge-centre/index.astro`. Defaults I chose, for the founder: move any number of times with 24 h notice within 90 days (refund only if we can offer no time); send someone else with 24 h notice; 15-minute no-show; poor session reported within 14 days; liability = session price; UAE law, Dubai courts, consumer home rights kept, no arbitration; privacy notice assumes `ats` removed. Audit requires each coaching page (+2 controls): audit OK, 15/15, 52/52.
- **Founder:** no published address, email only (*"we are small for now.. only email can work"*); Stripe receipts will still ask for a support address at live activation. Arabic: Stripe cannot do it (its invoice languages exclude Arabic, checked on docs.stripe.com); recommended own Arabic drafts + a native read, not yet decided.
- **Coaching app merged to `main` (founder: *"merge for now"*):** #60 policy pages → zaaheen.com redirects, `BOOKING_POLICY`, FAQ fixed (`1052560`); #61 `ats` cookie removed + cookie guard test (`b30f4ce`); #63 (replaces #62, auto-closed with #60's branch) bold key terms + tickboxes + `booking_consents` table + Stripe `consent_collection` (`8519a8e`). All 10 checks green each. **Blocking staging checkout:** (1) the Stripe sandbox needs its Terms of service URL set (Settings → Business → Public details → `https://zaaheen.com/knowledge-centre/terms/`; founder doing it; the Chrome extension was not connected); (2) the staging Supabase project is paused (`db:check`: ENOTFOUND), restore it, then `pnpm db:migrate` for `20260927120000_booking_consents`. Not yet seen rendered: check the booking panel on staging.
- **Still owed before coaching takes a booking (booking app, `Training Page` repo):** confirmation email contents (terms PDF, express-request line, withdrawal link, Arabic invoice), the "Withdraw from contract here" page, retention purge jobs (the notice promises 30 days / 12 months). **Founder decision left:** Arabic (see above).

**Next session, in order:**
0. **At open:** check every workflow on `46c7b38` (`gh run view 36312952781`, `36312952788`); a red CI is fixed first (broken CI is a regression).
1. **The Knowledge Centre's own documents** (founder: *"I noticed on knowledge-centre it shows our documents for zaaheen .. pricing .. t&c etc.. privacy ..refund. which it shouldnt .. that is a completely seperate thing... here we should have everything related to coaching and our sessions and stripe"*): coaching Terms, cancellation and refund policy, privacy notice (drafts: legal reports A and B §5.1 to §5.3 in `MemoryVault-artifacts\legal-research-s69\`), a checkout terms tickbox and an EU "withdraw from contract" page on the coaching site, and the Knowledge Centre pages and footer showing coaching's documents, not the app's. Founder decisions then: cancellation cut-offs (both reports: free move at 24 h, full refund at 48 h, 15-minute no-show), recordings (both: none), the 90-day attribution cookie (A: consent banner; B: drop it). Separate repo for the booking app: `C:\Projects\GitHub\Training Page`.
2. **SEO and GEO** (founder: *"next session we work on our SEO and GEO"*). `SEO-HANDOFF.md` (local) is the plan; the home page `FAQ` in `site.ts` is still empty.
3. **The big build (founder's go; heavy, freezes the laptop):** wipe `target/`, build, test, clippy, fmt; fix whatever the first compile of the new Rust finds (sessions 68 and 69: vault-account, vault-app, vault-tauri). **Add the open-source licence notices file to the installer (L7): the public installer must not ship without it.**
4. **New installer, sandbox first.** Before the live test the founder runs, from `workers/account`: `npx wrangler kv namespace create TRIALS --env sandbox` (id → `env.sandbox` in `wrangler.jsonc`), `openssl rand -hex 32 | npx wrangler secret put TRIAL_KEY --env sandbox`, then deploy the sandbox Worker (**without `TRIAL_KEY` every route answers 503**). Post-deploy: `/clerk/webhook` answers 2xx (m4); the secrets pair (Paddle `live` with Clerk `sk_live_`, `sandbox` with `sk_test_`, or every route is 503); one real sweep instance check (watch the log for `gone_guard`: the `oauth_applications` shape is only unit-tested); the sandbox Clerk webhook subscribes to `user.created` and `user.deleted`, self-delete on.
5. **Live test on the sandbox installer:** sign in, Create an account, Delete my account from Settings and from a lock screen (the hosted `/user` page on the sandbox), the lease for a deleted user answers signed out; the delete page itself on the dev instance again; Settings › Documents opens each page (once zaaheen.com is live).
6. **Order locked by the founder (session 68, *"lets follow your order"*):** then **zaaheen.com live** (`SEO-HANDOFF.md` §8: Hostinger, Cloudflare, GitHub switches) **together with the account pages switched on**; then Paddle domain review + Google publish. The memory app uses Paddle only; Stripe is only the coaching site. The download stays off until the installer live test passes.
7. **The switch-on list (founder's go, at launch):** Hostinger site for `account.zaaheen.com` deploying `account-deploy`; its DNS record in Cloudflare; branch protection on `account-deploy`; Cloudflare script-injecting features off for it; repo variables `CLERK_PUBLISHABLE_KEY`, `ZAAHEEN_CLIENT_ID`, then `ACCOUNT_PUBLISH=true`; a post-deploy check (AUTH-PAGES D9 invariants incl. MFA/CIMD/DCR off, headers) to build before that. Production Clerk: self-delete on, reverification 5 min, webhook on `user.created` + `user.deleted`, production webhook signing secret, allowed subdomains. **Google:** once `/privacy/` and `/terms/` are live, set them in Google Branding → **Publish app**; logo + brand verification; Clerk legal consent (Terms URL). Details: `OPS-HANDOFF.md` §I, `AUTH-PAGES-DESIGN.md` "Prerequisites and open items".
8. **Paddle checks, through the Paddle CLI or MCP once the live account is open (founder):** yearly renewal reminder emails on? Does Paddle provide the EU "withdraw from contract here" function? (The Terms rely on Paddle for both.)
9. **Still to confirm on the founder's Claude Desktop:** where the personal-preferences box is, and that Settings → Extensions shows Zaaheen with a switch.
10. **Later go-live steps (session 63 order):** **coaching takes no booking until its own Terms, cancellation policy and privacy notice are published on coaching.zaaheen.com** (the app's pages already say coaching has them, "which you see before you book"; review, session 69); coaching.zaaheen.com → the booking app, then a test booking from zaaheen.com; booking fully working (Microsoft Graph secret + a sender mailbox, real availability hours, the scheduled jobs, live Stripe); the public download last.

---

## 2 · 🧭 Where things stand

### The product
- **Works end to end on Windows.** Local-first and encrypted at rest (SQLCipher, sealed Lance, sealed graph and REPORTs); MCP over stdio. Claude Desktop, Cursor, ChatGPT desktop and (after this batch) Antigravity connect.
- **Read path:** structured facts, no LLM at read time. `memory_read` is primary and never false-empties; `memory_search` is recall-safe; the Qwen3-Reranker-0.6B cross-encoder is the relevance authority; one question at a time at the keeper's read desk (ADR-107).
- **Nightly consolidation** (Phi-4-mini) through Windows Task Scheduler and the windowless `zaaheen-maintenance.exe`.
- **One keeper owns the vault** (ADR-102/103, ADR-SEC-019/020): every AI app's `zaaheen mcp serve` is a relay to it. **After this batch the desktop app is a client of it too** (ADR-108): nothing but the keeper and the maintenance runner opens the vault or creates the key.
- **Installed on the founder's machine:** the sandbox installer `Zaaheen_0.2.2_s62-d4-sandbox.msi` (session 64), after "Delete everything": signed out, no memories.

### Arcs
- **Sign-in and subscription (ADR-104 + ADR-SEC-022, `SIGNIN-DESIGN.md`):** S0–S3 done and merged (S3 → PR #77, `main` `21d5002`); S4 (the export) built. Business model: 30-day no-card trial, then $5/month or $48/year; Clerk + Paddle; the keeper is the one gate; export always available.
- **The vault key and its location (ADR-SEC-029, ADR-105 + ADR-SEC-030, `VAULT-KEY-AND-LOCATION.md`):** built, gated, merged (PR #77).
- **Connecting apps (ADR-106, ADR-SEC-031, ADR-SEC-032, ADR-107, `CONNECT-APPS-DESIGN.md`):** "Connect it for me", the connected-apps list, rmcp 3.4.1 — in this batch.
- **D4 (ADR-108, ADR-SEC-033, `DESKTOP-CLIENT-DESIGN.md`):** in this batch.
- **Account pages (ADR-109, ADR-SEC-034 + amendments S1-1…S2-1, `AUTH-PAGES-DESIGN.md`):** steps 1-2 done (session 65), on PR #84; deploy, header wiring, Worker consent copy, Rust S1-3/D8 and the installer test to go (§1).

### Company, website, accounts (not code in this repo)
- **Zaaheen is a licensed parent company.** Launch waits on the licence; the aim is readiness. Checklist: `SEO-HANDOFF.md` §0a (local, gitignored).
- **Website:** merged into `main` with publishing switched off (PR #78, `SITE_PUBLISH` unset); zaaheen.com still shows Hostinger's parked page.
- **Coaching booking app:** separate repo `C:\Projects\GitHub\Training Page`. Parked.
- **Accounts:** Clerk, Microsoft 365, Paddle, Cloudflare, the bank. Details only in the local `OPS-HANDOFF.md`; this repo is public.

### Recent sessions
- **69 (2026-09-27):** the app's legal pages hardened from two independent legal reports and the founder's decisions L1 to L7 (`LEGAL-DOCUMENTS-DESIGN.md`): Terms rewritten, Refunds (anti-abuse), Privacy (trial fingerprint disclosed), Security, Company, Documents, new `/licences/` and `security.txt`; Pricing got the tax line, a sourced 7-row competitor comparison (where kept, encrypted, used to train AI) and a 16-question FAQ; Settings › Documents in the app (ADR-SEC-036, Rust uncompiled). HANDOFF trimmed (Part 6 archived).
- **68 (2026-09-27):** every pre-build item finished and pushed (`7969e58`, CI green): the delete batch's review fixes; Delete my account in the app; S1-3; D8 loopback CSP; D6 consent copy. Target and Docker deleted (176 GB free).
- **67, second half:** "Delete my account" (ADR-112, ADR-SEC-035): designed, two reviews, spiked on the dev Clerk app, built tests-first (the delete page; the Worker's deleted-account backstop with a mass-cancel guard; one free trial per email via an HMAC fingerprint in KV). Browser-tested end to end on the dev instance. Site account tests 58 + audit 39, Worker 321.
- **67 (2026-09-26):** PR #84's CodeQL alerts fixed (two rounds: substring, then an unanchored regex; now an exact host comparison). The policy pages built and approved: Terms, Privacy, Refunds, AI and Your Data, Security, Company Information, each on its own address, in a Documents "Policies" dropdown and every footer; a Pricing page (in the top menu) and a new professional footer; researched by two independent agents, no lawyer (founder). Audit 43/43, site tests 15/15.
- **66 (2026-09-26):** Clerk production on the free plan (passkeys/MFA off), Cloudflare DNS verified, prod OAuth app `sKtut4UNaAyJtImJ`, Zaaheen's own Google client (Testing until a privacy URL); zaaheen.com header + `/sign-in`, `/sign-up` forward to `account.zaaheen.com`, `site.yml` account deploy, all switched off (`ACCOUNT_PUBLISH`). Tests first, all site suites green.
- **65 (2026-09-26):** Account pages steps 1-2 (ADR-109, ADR-SEC-034 amendments S1-1…S2-1): measured on a practice Clerk app, built tests-first (48 unit tests, 35 audit controls), browser-tested incl. the founder's email and Google runs, independent review SAFE WITH FIXES (all fixed), pushed on PR #84 (`53cfa93`, `1a1edc5`, `ca3b814`). Founder kept `account.zaaheen.com`; production needs Terms/Privacy pages and Zaaheen's own Google OAuth client. No local builds until the founder says.
- **64 (2026-09-26):** PR #82 merged by itself in 90 s: ADR-110 proven (a website-only PR needs the branch up to date with `main`, the ruleset's rule). **Live test on the s62 installer from a clean slate:** Claude, Cursor, ChatGPT (Work/Codex), Antigravity connect and share memories both ways; Run now pauses and the apps recover; works with the window closed; Delete everything leaves nothing personal. Found and fixed (PR #84, ADR-111): apps open before the install can't find `zaaheen` (full path now); the Agents tab forgot apps after a tidy-up; the connect page and Agents tab redesigned like Settings (founder-approved in the browser); Run now's message stays. Not our bugs: Google's telemetry plugin broke Antigravity's tools (switched off with the founder's yes); Hermes' installer builds from source (cancelled, removed). Checks: vault-app 546 / vault-tauri 203 passed, clippy clean, review safe. Lessons: the apps don't reliably think of Zaaheen (a one-line tip per app, founder: keep it simple); the tests caught a missing Tauri permission for the new command; `python3 -` typed a tenth time.
- **Before that:** `HANDOFF_V0.2_PART6_ARCHIVE.md` §2 (sessions 58–63), then `HANDOFF_V0.2_PART5_ARCHIVE.md` §2 (sessions 42–57), then Part 4.

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
   - **Session 63:** the Products intro ("Zaaheen is a Dubai company that builds software for people who use AI every day…") and its coaching card; the whole Documents page; the Knowledge Centre lede "Private 1-to-1 coaching built around a real task you bring, from Zaaheen." and its bookings contact line; the footer's legal line.
8. **Monitoring dashboard: "signed up, never opened the app" (founder, session 61).** Everyone who signs up is a Clerk user; the app's first `/v1/lease` is what starts the trial (§8.26 §3). So a Clerk user with no trial started signed up on the website but never ran the app. The dashboard lists them so we can send a "welcome aboard, you haven't tried Zaaheen yet, here's the download" email. Before sending: session 43 locked "no reminder emails for now", and under UK PECR / GDPR this is marketing, so it needs consent captured at sign-up (a "send me tips and updates" box on the new Create-an-account page) and an unsubscribe.

---

## 4 · 🐛 Tech debt (live, with anchors)

- **Delete my account (session 68, from the reviews):**
  - the web delete page's re-verify view has no "send a new code" (a reload works);
  - the desktop dialog does not trap focus after it has run (Tab reaches controls that talk to a poisoned link);
  - `copyConsent` (`workers/account/src/routes/clerk-webhook.ts`) rewrites `marketing.at` on each Svix retry, so the stored time can move later;
  - `crates/vault-account/src/signin/http.rs` is 591 lines (soft cap 500; mostly tests);
  - no Rust test pins `delete_page_wire`'s exact fields (vault-tauri cannot name `AccountConfig` to build a link in a test).

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
- **A live-move test leaks Credential Manager entries:** session 63 found ~45 `default.com.zaaheen.test.v0.2.live_move.<hex>` entries on the founder's machine (one per run). The live-only keychain/move test should delete its test key on the way out, including on failure.
- **The site's installer facts are stale** (`site/src/data/site.ts` `RELEASE.windows`): "206 MB" is the old `adr103` MSI; the s62 build is 155,373,568 bytes. Set file, size and URL when the public installer is built (go-live step 6). At the same time fix the home page JSON-LD `offers` (`site/src/lib/seo.ts`): it says price 0 in GBP, which is neither the $5/$48 plan nor its currency.

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
- **ADR-110 (session 63): website-only pull requests skip the app's long checks.** Founder: *"is there a way ... website changes only triggers website related checks and deploys"*. `.github/scripts/app-changed.sh` decides (pull requests only; every changed file under `site/` or a root `*.md`, else the full run); `app-changed.test.sh` proves it (11 cases, shown to fail on a loosened rule) and runs first in each workflow. `ci.yml`: stand-in jobs report the required "cargo clippy (<os>)" / "cargo build + test (<os>)" names as passed; `codeql.yml`: "Analyse" (not a matrix) skips. If the decision job fails, the full checks run. No ruleset change. fmt, secrets, the Worker and TypeScript CodeQL always run. **Unproven until the first website-only PR after it merges:** that the stand-ins satisfy the ruleset (watch that PR merge by itself; if it sits "expected", the skipped-matrix naming is different from assumed).
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
| `LEGAL-DOCUMENTS-DESIGN.md` | **Live (session 69):** the legal document set (research A/B, decisions); the app's Settings › Documents link | ADR-SEC-036 |
| `ACCOUNT-DELETION-DESIGN.md` | **Live, web page + Worker built (session 67):** "Delete my account"; the app's side rides with the big run | ADR-112, ADR-SEC-035 (amends ADR-SEC-034) |
| `AUTH-PAGES-DESIGN.md` | **Live, steps 1-2 built (session 65):** our own account pages on `account.zaaheen.com` | ADR-109, ADR-SEC-034 (session 61), amendments S1-1…S1-4, S2-1 (session 65) |
| `DESKTOP-CLIENT-DESIGN.md` | **Live:** the desktop as a client of the keeper (D4) | ADR-108, ADR-SEC-033, the ADR-104 amendment (session 60) |
| `CONNECT-APPS-DESIGN.md` | **Live:** connecting AI apps | ADR-106 + ADR-SEC-031 (s55); ADR-SEC-032, ADR-107 (s59) |
| `SIGNIN-DESIGN.md` | **Live:** sign-in, trial and subscription | ADR-104, ADR-SEC-022 + amendments 1–17; ADR-SEC-023–028; ADR-054 Contract 2 amendment 3 |
| `VAULT-KEY-AND-LOCATION.md` | **Live:** where the vault key and the vault live | ADR-SEC-029; ADR-105 + ADR-SEC-030 and amendment 1 (L-f) |
| `HANDOFF_V0.2_PART6_ARCHIVE.md` | Sessions 58–68: the old §1 (session 68's state, sessions 64–67 notes) and recent-session lines 58–63 (frozen 2026-09-27) | none new |
| `HANDOFF_V0.2_PART5_ARCHIVE.md` | Sessions 44–60 (frozen 2026-09-24) | none new: pointers to the design files above |
| `HANDOFF_V0.2_PART4_ARCHIVE.md` | Sessions 19–44 | 073–084 bodies, 086–103, ADR-SEC-003–021 (the keeper arc is §8.19–§8.25; D4's origin §8.23) |
| `HANDOFF_V0.2_PART3_ARCHIVE.md` | Sessions 2–18 | 080–085, ADR-SEC-001, ADR-SEC-002 |
| `HANDOFF_V0.2_PART2_ARCHIVE.md` | T0.2.3c3 → T0.3.x | 047–072 |
| `HANDOFF_V0.2_PART1_ARCHIVE.md` | T0.2.0 → T0.2.3c2 | 037–046 |
| `HANDOFF_V0.1_ARCHIVE.md` | V0.1 | 001–036 |

**Next free numbers:** ADR-113 and ADR-SEC-037.
