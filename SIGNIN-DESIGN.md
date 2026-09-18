# Sign-in, trial and subscription — the locked design (ADR-104 + ADR-SEC-022)

> **Live text.** Moved out of `HANDOFF.md` on 2026-09-18 (session 44) so the handoff stays short; the words below are unchanged from HANDOFF §8.26 (locked 2026-09-17, session 43) and §8.27 (amendment 1, session 44). Build S2–S6 against this file, quote it, don't paraphrase it, and record any change here as a numbered amendment. Account identifiers never go here (this repo is public): they live in the local `OPS-HANDOFF.md`.

**Build order:** S0 PKCE spike ✅ · S1 `crates/vault-account` ✅ built (session 44) · S2 Worker + `/pay` page (step 1, the offline core: built session 45, §8.29) · S3 gate + keeper lock mode + desktop UI · S4 export (ships with S3) · S5 coaching · S6 production + live test.

---

## 8.26 · 🆕 ADR-104 + ADR-SEC-022 (LOCKED 2026-09-17) — sign-in, trial and subscription, for the desktop app and coaching

> Locked verbatim from the session-43 working draft (`signin-design-v3.md`), after three adversarial review rounds by two independent reviewers and the S0 spike. v1/v2 and the six review reports stay in the session-43 scratchpad. Account identifiers live in local `OPS-HANDOFF.md`, never here.

Session 43, 2026-09-17. **ADR-104** (architecture) + **ADR-SEC-022** (security). History: v1 → round 1 (reviewers A and B, independent) → v2 → round 2 (same reviewers; both verdicts: "ready to lock once these spec edits are made") → v3. v1/v2 are in the same folder; §12–§14 map each review finding to its resolution.

**STATUS: LOCKED 2026-09-17 (session 43).** Both reviewers signed off on round 3 once its edits were applied. Founder decisions made this session: offline 30 days · `comp_until` for beta testers · "Open the Zaaheen app and choose Subscribe" (subscribe from the app only) · no reminder emails for now. `[FOUNDER, later]` marks spend decisions deferred to launch.

---

## 0. Locked inputs
BRD §1.6 amendment 1 (30-day no-card trial → $5/mo or $48/yr; Paddle; Clerk; PKCE loopback; lock screen with Subscribe + Download my memories; export always available) · one Zaaheen account for app + coaching · coaching sign-in before booking · desktop signed in until 30 days unused or sign-out · free plans until traction · sign-in never touches the vault key · ADR-102/103 keeper/relay (D4 not built).

## 1. Verified facts
All of v2 §1, plus:
- `ClientRequest` in rmcp 2.2.0 is an intentionally exhaustive enum (`model.rs:3573-3592`, `#[expect(clippy::exhaustive_enums)]`): Ping, Initialize, Complete, SetLevel, GetPrompt, ListPrompts, ListResources, ListResourceTemplates, ReadResource, Subscribe, Unsubscribe, CallTool, ListTools, GetTask, ListTasks, GetTaskPayload, CancelTask, Custom. `impl<S: Service<RoleServer>> ServiceExt<RoleServer> for S` (`service/server.rs:104`).
- `KeeperExit` = `Idle | Yielded | HandedOver | Shutdown` (`keeper/runtime.rs:79`); the serve loop has an `idle_tick` interval (`:122`, `:133`).
- Reviewer-verified: Clerk `user.deleted` payload is only `{id, object, deleted}` (BAPI `DeletedObject`); Workers free plan: 50 subrequests and 10 ms CPU per invocation, cron included; Paddle list-subscriptions filters by `customer_id`, `status`, `price_id` (not `custom_data`); Paddle rate limit 240 requests/min per IP; Bot Fight Mode is zone-wide and cannot be bypassed; SChannel on Windows 10 has no TLS 1.3; Google OAuth apps in "Testing" only admit listed test users.

**S0 spike must answer:** OAuth app creatable on free · any loopback port matches · refresh token returned with `offline_access` · refresh-token rotation · refresh token survives browser sign-out / 7-day session end · public client can call `…/oauth/token/revoke` · consent-screen frequency · `iss` in the callback · verify-endpoint claims · whether the Account Portal lists OAuth grants. **S2 must answer:** Workers rate-limit binding on free · Paddle domain approval for `zaaheen.com`.

## 2. Parts
| Part | Owns |
|---|---|
| Clerk production (`zaaheen.com`) | identity, `accounts.zaaheen.com`, `private_metadata` |
| OAuth app "Zaaheen for Windows" | public, PKCE, scopes `openid profile email offline_access`, consent screen on, opaque access tokens |
| Account Worker `api.zaaheen.com` | `/v1/lease`, `/v1/checkout`, `/paddle/webhook`, `/clerk/webhook`, paged daily cron; kill switch; own Clerk secret key, Paddle keys, Svix secret, lease signing key |
| Paddle | checkout on `zaaheen.com/pay` (Paddle.js, domain approval), portal, dunning, receipts |
| `vault-account` (new crate) | PKCE client + loopback listener, token store (own `CredentialStore`, Local persistence), lease verify, entitlement state machine, activity timer, refresh lock, revoke |
| `vault-mcp::EntitledService` + `EntitlementCheck` trait | the gate, as a `Service<RoleServer>` wrapper |
| vault-app | wires the check; keeper lock mode; mode-change exit; maintenance-runner check; export |
| vault-tauri | onboarding sign-in step, account panel, lock screen, banners, typed per-command guard |
| Coaching | `@clerk/nextjs`, own secret key, `authorizedParties`, bookings keyed by user ID, own `user.deleted` handler |
| `site/` | links, pricing, **terms / privacy / refund pages** (Paddle review), `/pay` page |

Dependencies: `vault-tauri → vault-app → {vault-account, vault-mcp} → vault-core`. vault-mcp does not depend on vault-account.

## 3. Desktop sign-in
Unchanged from v2 §3: Rust opens the browser; verifier/challenge/state; listener on `127.0.0.1:0`, concurrent connections, ≤ 8 KB and ≤ 5 s each, 10-minute window, only `GET /callback` considered, `state` (constant time) **and** `iss` must match, `error=` ends the flow with a static page, no kill-after-N; code exchange; access token memory-only; refresh token in Credential Manager via vault-account's own store with Local persistence; keychain errors are transient; "Signed in as <email> — not you? Sign out" shown at once. On storing the token, the keeper-independent **sign-in marker** file is written (§4). First `/v1/lease` starts the trial.

## 4. Lease and local state
- **Wire:** `b64url(payload) "." b64url(sig)`, `sig = Ed25519(sk, "zaaheen-lease-v1\0" || payload_bytes)`, verified on the exact bytes before parsing (dryoc).
- **Keys:** the app ships **two** public keys from day one: `primary` (in Cloudflare Secrets) and `backup` (generated offline, private half kept offline by the founder). Losing or leaking the primary means rotating to the backup without stranding anyone.
- **Payload:** `{ v, kid, sub, state: "trial"|"active"|"payment_failed"|"ended", trial_ends_at, active_until, issued_at, client_time, offline_days }`.
  - `client_time`: the client sends its clock reading `client_now` in the request, and the server signs it **as received, with no clamp**. It is the anchor for all local elapsed-time maths. (Round 3: a ±24 h clamp would permanently lock a user whose clock runs more than a day slow. The backwards-clock rule already defeats the freeze trick.)
- **Entitlement check (all times UTC epoch seconds):**
  - `floor = max(stored_floor, system_now)`, where `stored_floor` = `state.floor` **only if `state.lease_issued_at == lease.issued_at`**, else `client_time` (a reader that sees a new lease before `state.json` is rewritten must not use the old floor). `elapsed = max(0, floor − client_time)`.
  - Entitled iff `elapsed < offline_days` and:
    - `trial`: `elapsed < trial_ends_at − issued_at`;
    - `active` / `payment_failed`: `elapsed < active_until − issued_at`.
  - `system_now < client_time − 10 min` (clock moved backwards past the anchor) ⇒ treated as "needs refresh"; offline ⇒ not entitled, with "Zaaheen couldn't confirm your subscription. Check your internet connection."
  - At receipt, if `|client_now − server_now| > 24 h`, the desktop shows **"Your computer's clock is wrong. Set it to update automatically."** as information only; entitlement is unaffected, because all maths is relative to `client_time`.
  - **Accepted residual:** a user who blocks `api.zaaheen.com` and runs a script pinning the clock just behind the floor keeps a lease that never ages. This is a business control, not a security boundary (§6.7).
- **Files** (`%LOCALAPPDATA%\com.zaaheen.app\account\`, ADR-SEC-019 ACL before first write; only the keeper and the desktop write here, **never a relay**):
  - `lease`: the signed lease bytes, written atomically (temp + rename) under the lock. Self-contained: no separate anchor file to fall out of step.
  - `state.json`: `{ lease_issued_at, floor, last_active_anchor, last_refresh_attempt }`, written under the lock, throttled to at most one write per minute.
    - `last_active_anchor` and `last_refresh_attempt` always merge with `max()`, across leases too. At sign-in, `last_active_anchor = issued_at` of the first lease.
    - **Only `floor` is lease-scoped.** A new lease resets `floor` to its `client_time`. For the same `lease_issued_at`, `floor` merges with `max()`. A writer holding an older `lease_issued_at` never changes `floor`. So a clock that once jumped far ahead stops counting once it is fixed and a fresh lease arrives.
    - Deleting the file only lowers the floor to "now".
  - `signed-in`: marker written when a refresh token is stored, removed on sign-out.
  - `refresh.lock`: never deleted.
- **Offline allowance: 30 days — LOCKED (founder, session 43: "yes 30 days is fine partner").** Always capped by trial end / `active_until`.
- **Refresh rules:**
  - Refresh-then-decide on a denial: refresh first (≤ 5 s), then answer. The lock wait is ≤ 2 s; if the lock is still held after that, skip this refresh and decide from whatever is on disk. At most one attempt per minute, tracked in `state.json` so a new process honours it. A `last_refresh_attempt` later than `system_now` counts as no attempt (a clock that jumped back must not block refreshes).
  - At keeper start and desktop open: refresh if the lease is older than 24 h or any deadline is within 3 days.
  - Otherwise a jittered daily timer while running (never triggered by tool activity).
  - After checkout: every 10 s for 10 min, plus an "I've paid" button. (The Worker allows a live Paddle fetch every 30 s per user while `checkout_at` is within 1 h.)
  - **After a signed `ended`:** refresh-on-denial backs off to at most hourly. It refreshes at once after checkout, "I've paid", or opening the desktop app. This keeps churned trials from flooding the Worker's free daily quota.
  - Every refresh takes `refresh.lock`, re-reads token and lease after acquiring it, writes atomically.
  - A network error, 5xx, or keychain error never discards a lease or signs anyone out. Only a bad signature or a valid signed state changes the decision.
- **30 days unused:** activity is recorded in **server time** (`last_active_anchor = issued_at + elapsed`, at most hourly), so the client's clock error never enters it. The rule is evaluated **only after a successful refresh**, as `server issued_at − last_active_anchor > 30 days` → sign out (delete token, lease, marker; revoke). A local clock jump cannot trigger it.

## 5. Account Worker
- **Auth:** BAPI `POST /oauth_applications/access_tokens/verify`; require our `client_id`, not revoked, not expired → `subject`. Opaque access tokens.
- **Derived billing record** (written only by the Worker): `private_metadata.zaaheen_memory = { trial_started_at, paddle_customer_id, active_until, payment_failed, comp_until, checkout_at, synced_at }`.
  - Only subscriptions whose items belong to **our allowlisted Paddle product ID** count (checked on the fetched items, so a future price change cannot lock new subscribers out). `/v1/checkout` still maps `plan` to allowlisted price IDs.
  - `status=active`, no scheduled cancel → `active_until = current_billing_period.ends_at + 3 days` (renewal gap).
  - `status=active` with a scheduled cancel → `active_until = scheduled change effective_at`.
  - `status=past_due` → `active_until = current_billing_period.starts_at + 7 days`, `payment_failed = true`.
  - `paused`/`canceled` → nothing. The latest `active_until` over all counting subscriptions wins.
  - Writes never clear `trial_started_at` (tested); a PATCH is skipped when nothing changed.
- **`/paddle/webhook`:** subscribed to `subscription.*` only. Verify `Paddle-Signature` (any `h1`, 5 s tolerance), **derive synchronously** (re-fetch that customer's subscriptions), write, then 200. Any failure → 5xx so Paddle retries.
- **`/v1/lease`:**
  - body `{ client_now, app_version }`; first desktop call sets `trial_started_at`.
  - `state`:
    - `active` if `now < max(active_until, comp_until)` (`payment_failed` flag → state `payment_failed`);
    - **live Paddle re-derive first** when (a) the record is active-flavoured but `now ≥ active_until`, or (b) `paddle_customer_id` exists and the result would be `ended`, or (c) `checkout_at` is within 24 h and the result is not active. Rate-limited to one live fetch per user per 10 min. Cases (a) and (b) are skipped when `synced_at > active_until`; case (c), the just-paid check, never is;
    - else `trial` if `now < trial_started_at + 30 d`; else `ended`.
  - **Paddle or Clerk error for a record that was active-flavoured → sign `active` until the stored `active_until + 3 days`.** Any other upstream error → 503 (never a signed `ended`).
  - **Kill switch:** env flag `NEVER_END_PAYERS=1` → anyone with a `paddle_customer_id` gets `active` with **`active_until = now + 3 days`** (a bad deploy must not lock everyone, and leases granted during an incident, including to abandoned checkouts, lapse soon after it).
- **`/v1/checkout`:** body `{ plan: "monthly"|"annual" }` mapped to allowlisted price IDs. A **live** fetch decides "already subscribed" (active or past_due) → `{ kind:"portal", url }` (portal session). Otherwise find-or-create the customer, store `paddle_customer_id` and `checkout_at`, create a transaction with `custom_data.clerk_user_id` server-side → `{ kind:"checkout", txn }`. The app validates `txn` (`^txn_[a-z0-9]{26}$`) and builds `https://zaaheen.com/pay?_ptxn=…`; portal URLs must be `https` on the allowlisted Paddle host. Nothing from the server reaches the OS unchecked.
- **`/clerk/webhook`** (Svix-verified): `user.deleted` → page through subscriptions with status active/past_due/paused and our price IDs, match `custom_data.clerk_user_id`, cancel immediately. Failure → 5xx (Svix retries). Runbook backup.
- **Cron (daily, paged):** each run handles one page of subscriptions whose billing period ends within ±48 h and stays under 50 subrequests, PATCHing only changes. **At ~30 paying customers move to Workers Paid ($5/month)** [FOUNDER, later].
- **Bot Fight Mode is zone-wide:** keep it off for `zaaheen.com` (the static site and coaching don't need it; Clerk has its own bot protection). Recorded.
- **Secrets / residuals:** per-consumer Clerk secret keys; `zaaheen_memory` Worker-only; `is_admin` founder-only; `unsafe_metadata` never trusted; a coaching compromise can write metadata / impersonate (instance-wide keys) — accepted and recorded.
- **Beta comp — LOCKED (founder, session 43: "yes go with the free until date"):** `comp_until`, set per person by the founder in the Clerk dashboard (the founder's own account included). No other grace code.

## 6. Enforcement
1. **`vault_mcp::EntitledService<S: Service<RoleServer>>`** implements `Service<RoleServer>` (`handle_request`, `handle_notification`, `get_info`). An **exhaustive `match` on `ClientRequest`** (a future rmcp variant fails to compile) sorts each request:
   - `Ping`, `Initialize`, `ListTools` always delegate **without consulting the check**, so `initialize` stays fast (ADR-102 K8).
   - `CallTool` (plain or task-flavoured) and every other request ask `Arc<dyn EntitlementCheck>` (refresh-then-decide per §4). When entitled, they delegate. When not:
     - a plain `CallTool` → `CallToolResult::error([message])`;
     - a task-flavoured `CallTool` → the inner server's own invalid-params error (a task caller expects `CreateTaskResult`; our tools forbid tasks anyway);
     - `ListPrompts`, `ListResources`, `ListResourceTemplates` delegate (empty lists);
     - every other request → an error.
   `EntitledService` counts calls in flight (used by §6.2). Notifications pass through. The adapter is never touched while locked.
   Built at `application.rs:959` and `keeper/runtime.rs:282` around `StdioServer`, and in the daemon after `authorize` (`daemon.rs:63/98`).
2. **Keeper modes:**
   - At start the keeper reads the master key (the handshake needs it; none ⇒ publish `failed`), takes `.vault.lock`, then evaluates entitlement.
   - Locked ⇒ **lock mode** = the same `runtime::serve` loop with `EntitledService<StdioServer<NoVaultAdapter>>` and a **`LockModeCheck`**: the real refresh-then-decide check, except that a "yes" is never served. It answers "unlocking" and triggers `ModeChanged`. Identical tool list, `WIRE`, discovery roles, and erasure intent/handover path; no `Application`, no models.
   - On each `idle_tick`, both modes re-evaluate (file read, no network) and exit with a new **`KeeperExit::ModeChanged`** only when entitlement flips. Lock mode's re-evaluation uses the real check; the always-deny gate only serves calls.
     - A full keeper that finds the user locked **refreshes first** (a merely stale lease must not unload and reload 2.86 GB).
     - It then **waits for calls in flight to finish** (the `EntitledService` counter; new calls are already refused) before exiting through the existing `stop_tx` path. The idle exit requires zero connections; `ModeChanged` does not, so it must not cut a write off mid-way. Lock → full lets the next relay start a full keeper; full → lock frees the 2.86 GB instead of holding it while relays stay connected. A lock-mode call that finds the user now entitled answers "Zaaheen is unlocking. Try again in a moment." and triggers the exit.
3. **Relay short-circuit:** no `signed-in` marker (checked twice, 200 ms apart) ⇒ the relay answers the sign-in message itself and starts no keeper. A marker but no lease ⇒ normal keeper start (it retries the fetch).
4. **Desktop (pre-D4):** gated `*_inner` functions take an `Entitled` token that only the check can mint (the compiler enforces the guard). A source test in the existing `include_str!` pattern lists all registered commands and the ungated allowlist (account, export, erasure, logs, settings, maintenance status).
5. **Maintenance runner:** skips while not entitled ("paused until you subscribe").
6. **Audit:** gated calls are logged to the application log only (no vault write). Divergence recorded.
7. Business control, not a security boundary. No DRM.

**Messages** (fixed text, no links, no user data):
- signed out: "Zaaheen needs you to sign in. Open the Zaaheen app on this computer to sign in."
- can't confirm: "Zaaheen couldn't confirm your subscription. Check your internet connection and try again."
- payment failed (still entitled; `health.warnings` + desktop banner): "Your last Zaaheen payment didn't go through. Open the Zaaheen app to update your card."
- trial ended: "Your Zaaheen free trial has ended. Open the Zaaheen app and choose Subscribe to keep using your memories. Nothing has been deleted."
- subscription ended: "Your Zaaheen subscription has ended. Open the Zaaheen app and choose Subscribe to keep using your memories. Nothing has been deleted."
- unlocking: "Zaaheen is unlocking. Try again in a moment."
- **LOCKED (founder, session 43: "yes use that wording, subscribe from the app"):** "Open the Zaaheen app and choose Subscribe" replaces BRD §1.6's "subscribe at zaaheen.com". Subscribing happens only from the app; the website shows prices and explains how. (BRD divergence, recorded in ADR-SEC-022.)
- The lock screen shows a support email address.
- ADR-SEC-009's flagged patterns gain look-alike sign-in / subscribe instructions inside memory content.

**Trial reminders:** desktop banner from day 23; `health.warnings` in the last 5 days. **Email reminders: NOT NOW — LOCKED (founder, session 43: "yes skip emails for now")**; revisit at launch if trial-to-paid conversion is weak.

## 7. Other flows
Sign-out (client revokes its own refresh token at the public endpoint; fallback Worker endpoint with the access token → BAPI `revoke_token`) · Delete everything (erasure + local sign-out + revoke; dialog says the subscription continues until cancelled, with the portal link) · Clerk account deletion cancels billing via `/clerk/webhook` · export always available (S3 and S4 ship together) · "sign out all computers" deferred · upgrade from 0.2.x: sign-in step on first run, no grace code, beta testers via `comp_until`.

## 8. TLS, proxies, privacy
- Account calls use reqwest's default native-tls backend (SChannel, OS roots, corporate inspection works) and the system proxy (PAC unsupported, documented). **TLS floor 1.2**, recorded as a divergence (Windows 10's SChannel has no 1.3; a 1.3-only floor would break inspecting proxies; the lease signature carries integrity). No `Cargo.toml` feature change.
- No certificate pinning (CDN). Worker learns: user ID, subscription state, trial start, app version, clamped client clock, request IP, about one request a day per active install. Never vault content or anything key-derived.

## 9. Coaching (S5, off the launch critical path)
As v2 §9.

## 10. Launch checklist (S6)
Clerk production + DNS-only records · subdomain allowlist · per-consumer secret keys · opaque tokens · **Google OAuth app published "In production"** · Paddle live, **website review (visible pricing, terms, privacy naming Clerk/Paddle/Cloudflare and the daily lease call, refund policy)**, domain approval, dunning window, notification emails for failed webhooks · Cloudflare failure notifications · Bot Fight Mode off for the zone · Worker deploy with the kill switch documented · backup lease key generated offline · support email on the lock screen · daily manual check of Paddle failed deliveries and Worker errors during launch · live test with Claude Desktop (Store) and Cursor, including relays reading `%LOCALAPPDATA%\…\account` from inside the Store package · **signed Windows installer** (SmartScreen) — spend decision [FOUNDER, later].

## 11. BRD divergences (ADR-SEC-022)
As v2 §11, with item 10 now: TLS floor 1.2 (not 1.3).

## 12. Round-2 findings → resolution
| Finding | Resolution |
|---|---|
| Renewal gap still signs `ended`; Paddle error keeps an expired lease (B-X1) | §5 `+3 days`, active-flavoured error → signed active |
| Short-lived keeper never runs the daily timer; denial decided before refresh (B-M2) | §4 refresh-then-decide; refresh at start |
| Lock-mode restart loop (A4, B-M3) | §6.2 exit only on flip at `idle_tick`, `ModeChanged`, persisted attempt time |
| Lease/anchor desync; unsigned anchor (A3, B-M4, B-N9) | §4 signed `client_time`, self-contained lease, `max()` merge |
| Single signing key (B-M5) | §4 primary + offline backup |
| `user.deleted` can't find the customer (A2, B-M6) | §5 page through + match `custom_data` |
| 200-then-derive loses retries; just-paid lockout (A1, B-M7) | §5 synchronous derive, 5xx, live checks (b)(c), `subscription.*` only |
| Cron exceeds free-plan subrequests (A5, B-M8) | §5 paged ±48 h, PATCH on change, Workers Paid at ~30 payers |
| 30-day timer on wall clock (A6) | §4 anchored activity, evaluated after refresh |
| past_due UX and fetch storms (A7) | §5 `payment_failed` state, message; fetch limits |
| Other Paddle products (A8) | §5 price-ID allowlist |
| 30 s lock wait inside a call (A9) | §4 ≤ 2 s |
| EntitledServer fragility / task path (A10, B-N11) | §6.1 `Service<RoleServer>`, exhaustive match, deny by default |
| Short-circuit on "no lease file" (A11, B-N10) | §6.3 marker, double check |
| Bot Fight Mode zone-wide (A12) | §5 off for the zone |
| TLS floor (A13) | §8 native-tls, 1.2, divergence |
| Allowlist proves names only (B-N12) | §6.4 typed token |
| Store-packaged relay reading `%LOCALAPPDATA%` (B-N13) | §4 relays never write; §10 live test |
| Launch gaps (A, B) | §10 |

## 13. Round-3 edits (reviewer B: "DO NOT LOCK as written; make two one-line edits, then lock")
| Finding | Edit |
|---|---|
| T1 MAJOR: a +/-24 h clamp on `client_time` permanently locks a user whose clock is more than a day slow | §4: `client_now` signed unclamped; clock message is informational only; residual recorded |
| T2 MAJOR: "newer lease replaces the record outright" could wipe `last_active_anchor` and sign everyone out | §4: only `floor` is lease-scoped; activity and attempt times always `max()`; anchor seeded at sign-in |
| Activity anchor should be server time | §4: `last_active_anchor = issued_at + elapsed` |
| Price-ID allowlist can lock new subscribers after a price change | §5: product-ID allowlist on fetched items |
| `synced_at` skip must not apply to the just-paid check | §5 |
| Gate must not delay `initialize` | §6.1: Ping/Initialize/ListTools never consult the check |
| Lock-wait wording contradictory | §4: wait <= 2 s, then decide from disk |
| Lock mode cannot be `NeverEntitled` and still say "unlocking" | §6.2: `LockModeCheck` |
| "A clock that far off breaks TLS" was inaccurate | removed |

## 14. Round-3 edits (reviewer A: "LOCK, with wording-level edits")
| Finding | Edit |
|---|---|
| Use the stored floor only for the same lease | §4 keyed floor |
| 10-minute live-fetch limit fights the 10 s post-checkout poll | §4 / §5: every 30 s while `checkout_at` is within 1 h |
| Locked task-style `CallTool` | §6.1: inner invalid-params error; list requests delegate |
| Lock mode needs the real check to detect a flip | §6.2 |
| `ModeChanged` can cut off a call in flight | §6.2: refresh first, wait for in-flight calls |
| Refresh traffic from churned trials | §4: hourly back-off after a signed `ended` |
| Kill switch grants abandoned checkouts open-ended access | §5: kill-switch leases last 3 days |
| (A's clamped-clock lockout note) | already resolved by reviewer B's T1 edit (no clamp) |

**Status: both reviewers' round-3 edits applied. Reviewer A: LOCK. Reviewer B: LOCK once T1/T2 are in (they are). Ready for founder approval.**

## 15. S0 spike results (run live against the dev instance, 2026-09-17)
Script `pkce-spike.mjs`, log `spike-log.jsonl` (no token value was ever printed or stored). OAuth app `Zaaheen for Windows`, public + PKCE, scopes `openid profile email offline_access`.

| Question | Answer |
|---|---|
| (a) OAuth application on the free plan | **Yes** — created on the dev (Hobby) instance |
| (b) Random loopback port | **Yes** — `http://127.0.0.1/callback` registered once; ports 52686 and 50072 both accepted (authorize pre-check 302 → `/sign-in`, then a real callback) |
| (c) Refresh token with `offline_access` | **Yes** — 48-char opaque refresh token; access token 36-char **opaque** (not a JWT), `expires_in` 86400 |
| (d) Refresh-token rotation | **Yes, with reuse detection.** A refresh returns a NEW refresh token; replaying the old one fails `invalid_grant` ("The refresh token was already used") **and kills the successor too** — the whole grant dies |
| (e) Survives a browser sign-out | **Yes** — after signing out in the browser, the refresh still worked (200) |
| (f) Public revocation | **Yes** — `POST /oauth/token/revoke` with `token` + `client_id`, no secret, returned **200**; the refresh then failed and the access token was gone ("OAuth Access Token not found"). `/v1/signout` stays dropped |
| (g) Consent frequency | Second authorize completed in 36 s with interaction; founder observation pending. Treat as "may appear each time" |
| (h) `iss` in the callback | **Yes** — matches the issuer exactly (RFC 9207); the client checks it |
| (i) Claims | userinfo: `sub`, `user_id`, `email`, `email_verified`, `given_name`, `family_name`, `name`, `instance_id`, `object`. `id_token`: adds `sid`, `auth_time`, `at_hash`, `jti`, `rat`; `aud` = our client id; lifetime 86400 |
| (j) Account Portal lists grants | **No** — its Security section lists browser devices only ("Windows / Edge"), not the desktop app. The app's own "Signed in on this computer" stands (D-BRD-3) |
| extra | Authorization-code replay is rejected (`invalid_grant`) |
| extra | BAPI `access_tokens/verify` returns `client_id`, `subject`, `scopes`, `revoked`, `expired` — exactly what the Worker needs |

**Design change forced by (d) — refresh-token rotation with reuse detection (amends §4):**
- Every refresh MUST hold `refresh.lock`. A process that cannot take the lock **never refreshes**; it waits (≤ 2 s) and re-reads.
- After a refresh returns, the new refresh token is written to the credential store **before** the new lease is written, and the write is confirmed before the old token is considered spent.
- On `invalid_grant`: **re-read the store once** (another process may have rotated the token a moment earlier) and retry with what is there. Only if that also fails is the state `signed_out`. A rotation race must never look like a sign-out.
- Residual: a crash between "Clerk issued a new token" and "the store wrote it" spends the old token and ends the grant; the user signs in again. Window is milliseconds; accepted and recorded.

## 8.27 · 🆕 ADR-104 / ADR-SEC-022 amendment 1 (2026-09-18, session 44) — decisions made building S1 (`crates/vault-account`)

Implementation choices §8.26 left open, each pinned by a test. None changes a locked rule.

**Contracts S2 (the Worker) must follow:**
- `/v1/lease`: `POST`, `Authorization: Bearer <access token>`, JSON body `{"client_now": <i64>, "app_version": "<1-32 of 0-9A-Za-z.+->"}`; `200 {"lease": "<wire>"}`; `401` = token refused; `429`/`5xx` = try later. Pinned in `lease_client.rs`.
- Lease payload wire names: `v` (must be 1), `kid`, `sub`, `state` ∈ `trial|active|payment_failed|ended`, `trial_ends_at`, `active_until` (integer or `null`/absent), `issued_at` > 0, `client_time` > 0, `offline_days` 1–366. `trial` needs `trial_ends_at`; `active`/`payment_failed` need `active_until`. Unknown fields ignored, duplicates refused.
- **For a signed `ended`, include `active_until` if the user ever paid**: the app picks "subscription ended" over "trial ended" from it.

**Contracts S3 (gate, keeper, desktop) must follow:**
- `AccountDir::open` refuses a folder that does not exist: vault-app creates `%LOCALAPPDATA%\com.zaaheen.app\account\` and applies the ADR-SEC-019 ACL first.
- The `signed-in` marker holds the user's `sub` (§8.26 left its content open); every lease is verified against it.
- `Account::refresh` is not cancel-safe between the server rotating the token and the store writing it: run it in its own task and stop waiting, never drop it.
- Triggers: `Denial` (1/min, hourly after a signed `ended`), `Routine` (1/min: keeper start, desktop open, daily timer), `UserAction` (no local limit: checkout polling, "I've paid").
- After a failed refresh, only `Denial::Ended` shows the ended messages; every other denial shows "Zaaheen couldn't confirm your subscription…".

**Security decisions (ADR-SEC-022 amendment 1):**
- Refresh tokens are capped at **1024** characters (access tokens 4096): Windows Credential Manager holds at most 2,560 bytes of UTF-16 (1,280 chars), and a token accepted but unstorable would sign the user in once, then fail "transiently" forever.
- **dryoc 0.7.2's Ed25519 verify reduces a non-canonical `S`** rather than rejecting it (libsodium rejects). A genuine signature therefore has a second encoding. No forgery or payload change follows, and nothing identifies a lease by its signature bytes: recorded, not patched (patching means a new crypto dependency, SP-5).
- A stored value that is not a valid token reads as "no token" (the store worked; signing in again overwrites it). A store *failure* stays transient.
- A missing or damaged `state.json` reads as absent and never signs anyone out; an activity anchor of 0 or less is absent.
- Sign-in: token stored, then marker, then any previous lease and record cleared, then the first lease. Sign-out: marker first (relays stop at once), then lease, record and token, then revoke (best effort, so it works offline).
- Loopback listener: one request per connection; an `error=` callback cancels only with the right `state` and `iss`; the first valid callback claims the flow atomically.
- **Residual:** the listener does not set `SO_EXCLUSIVEADDRUSE`, so a process running as the same Windows user could bind the same loopback port. Such a process can already read the user's Credential Manager; accepted.
- The email for "Signed in as" comes from userinfo (server-verified), not by parsing the `id_token`.
- **A rotated refresh token that the store refuses is kept in memory** (found by an independent review of S1, session 44). After Clerk rotates, the stored token is spent, so one refused save would otherwise sign the user out on the *next* refresh (`invalid_grant` → re-read finds the same spent token), breaking "a keychain error never signs anyone out". Now the save is tried 5 times (~0.4 s); if the store still refuses, the process keeps the token, the next refresh stores it and uses it first (never the spent one), and sign-out revokes it. **Residual:** if that process exits, or another Zaaheen process refreshes, while the store is still refusing, the newer token is lost or bypassed and that refresh signs the user out. Same class as §15's crash residual; accepted.

**Evidence:** 205 tests; planted-bug runs proved the §15 tests (re-read, one refresher, token-first) and the floor-keying and server-time-activity tests each fail when their rule is broken; the four refused-save tests failed on the code before the fix.

## 8.28 · ADR-104 / ADR-SEC-022 amendment 2 (2026-09-18, session 45) — the two questions S2 owns (§1)

Web research, session 45. Runtime confirmation is due at the Worker's first deploy (memory `feedback_runtime_confirmation_after_web_spike`).

**1. Workers rate-limit binding on the free plan: yes, as far as the docs go, not yet seen live.** The `ratelimit` binding went GA on 2025-09-19 ("stable and recommended for all production workloads"). Neither the binding page nor Workers pricing names a plan restriction, and secondary sources report it on Free. Wrangler ≥ 4.36.0.
- **It cannot carry §5's per-user live-fetch limits.** Its `period` "must be either 10 or 60" seconds, each key counts separately "per Cloudflare location", and it is "permissive, eventually consistent, and intentionally designed to not be used as an accurate accounting system".
- **So §5's "one live fetch per user per 10 min" and §4's "every 30 s while `checkout_at` is within 1 h" are enforced by a timestamp in the per-user record:** `private_metadata.zaaheen_memory` gains `live_fetch_at` (written only by the Worker, like the rest). Exact, global, and no extra subrequest (the Worker reads the record on every lease call anyway).
- The binding stays available for coarse flood protection only.

**2. Paddle domain approval for `zaaheen.com`: needed for live, not for sandbox.**
- Paddle: "you will only be allowed to sell through the domain(s) that have been approved"; the default payment link "should be a page for an approved website" that includes Paddle.js, and "You can't create transactions without it". Sandbox: "You can use `localhost` or a test domain".
- Review needs the site "live and secured with an SSL certificate (HTTPS)", a product description, pricing, key features, and Terms (with the company's legal name), Refund and Privacy pages. zaaheen.com today is a parked page and would fail.
- Most submissions are approved automatically; a manual review takes "5-7 business days". Plan for that lead time in S6.
- **Consequence:** S2 is built and tested entirely against the Paddle sandbox. Live approval joins the S6 checklist items that already wait on the licence details (§10).

## 8.29 · ADR-104 / ADR-SEC-022 amendment 3 (2026-09-18, session 45) — decisions made building S2 step 1 (the Worker's offline core)

Implementation choices §5 left open, each pinned by a test in `workers/account/test/`. None changes a locked rule.

**Where and how it is built:**
- `workers/account/`: TypeScript on Cloudflare Workers. Tests run inside workerd (the production runtime) through `@cloudflare/vitest-plugin`. CI job "account Worker (types + tests)"; CodeQL job "Analyse (TypeScript)".
- Dev tools are pinned exactly and were at least 3 days old when pinned. **The Worker has no runtime dependencies:** everything deployed is our own code.

**The lease (with §4 and §8.27):**
- The signing secret is PKCS#8 DER in standard base64, imported non-extractable.
- Payload fields are written in a fixed order (`v, kid, sub, state, trial_ends_at, active_until, issued_at, client_time, offline_days`). A deadline that is absent is left out, never written as `null`.
- The Worker refuses to sign anything `LeaseVerifier` would refuse (same limits), because a lease the app rejects locks its user out.
- **Shared vectors:** `workers/account/test/vectors/lease-v1.json` holds six leases under the RFC 8032 §7.1 test keys, made by Node's OpenSSL. workerd must sign the same bytes (Worker tests), and dryoc must verify them (`vault-account` `lease::worker_vectors`). Neither side can drift on the wire, the domain prefix, the field names or the key encoding without a test failing.

**Billing derivation (§5 "Derived billing record"):**
- A subscription counts if **any** item's `price.product_id` is ours.
- A scheduled **pause** ends access at `effective_at`, like a cancel. A scheduled resume ends nothing.
- `trialing` and unknown statuses count for nothing.
- On an equal `active_until`, the subscription that is paying wins over the one whose payment failed.
- Unreadable data on one of **our** subscriptions is an upstream error (the §5 error rules apply), never a silent "nothing". Dropping a payer's subscription would sign them `ended`.

**The record (`private_metadata.zaaheen_memory`):**
- A malformed field is dropped, with a log warning that names the field, never its value.
- `comp_until` may be epoch seconds or `YYYY-MM-DD`, meaning free through the end of that day (UTC), because the founder types it by hand.
- When a derivation finds nothing counting:
  - an `active_until` already past is kept, so the app says "subscription ended" (§8.27);
  - one still in the future is cut to `now` (a refund or an immediate cancel).
- The write back carries only the Worker's fields that changed; a field removed is sent as `null`. It never writes `comp_until` and never changes or clears an existing `trial_started_at`. (Clerk's merge semantics for this are confirmed in step 2, with the Clerk client.)

**The `/v1/lease` decision (§5):**
- "Active-flavoured" means the record has an `active_until`.
- "Skipped when `synced_at > active_until`" is implemented as `synced_at ≥ active_until`, because a derivation that finds nothing sets both to the same second.
- **With no `active_until`, cases (a) and (b) are never skipped.** There is no paid period to have synced since, so a Paddle customer who is `ended` keeps being re-checked, at most once per 10 minutes (the app backs off to hourly after a signed `ended`). A payment that lands after case (c)'s 24 hours, with its webhook lost, is still found. The cost is extra Paddle calls for abandoned checkouts; the other way, a payer stays locked out. (The first draft skipped them after any sync; the independent review of step 1 caught it.)
- A failed live fetch still stamps `live_fetch_at` (so Paddle is not hammered during an outage). A `live_fetch_at` later than now counts as no fetch.
- **A trial whose start could not be saved is never signed** (503). Otherwise every failed write would restart the trial.
- On any Clerk write failure, the terms come from the *stored* record's upstream-error rule, even if the Paddle fetch succeeded.
- Upstream-error terms are `active` until `max(active_until + 3 days, comp_until)`, so a later comp date is kept.
- The kill switch never calls Paddle.
- The `payment_failed` flag gives state `payment_failed` whenever `now < max(active_until, comp_until)` (literal §5).

**Left for step 2 (the endpoints):** validating the `/v1/lease` request body, the Clerk and Paddle clients (paging and error mapping), and the Clerk metadata-merge check above.

**Evidence:** 103 Worker tests and 3 new `vault-account` tests. 17 planted bugs each turned a test red: 4 in lease signing, 12 in the rules, 1 in the Rust vector check. Each was run once and then restored byte for byte.
