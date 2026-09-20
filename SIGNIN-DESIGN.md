# Sign-in, trial and subscription — the locked design (ADR-104 + ADR-SEC-022)

> **Live text.** Moved out of `HANDOFF.md` on 2026-09-18 (session 44) so the handoff stays short; the words below are unchanged from HANDOFF §8.26 (locked 2026-09-17, session 43) and §8.27 (amendment 1, session 44). Build S2–S6 against this file, quote it, don't paraphrase it, and record any change here as a numbered amendment. Account identifiers never go here (this repo is public): they live in the local `OPS-HANDOFF.md`.

**Build order:** S0 PKCE spike ✅ · S1 `crates/vault-account` ✅ built (session 44) · S2 Worker + `/pay` page ✅ (step 1, the offline core, and step 2, the endpoints: built session 45, §8.29–§8.30; step 3, `/pay` and the sandbox deploy, live-tested session 46, §8.31) · S3 gate + keeper lock mode + desktop UI (step 1, the gate in `vault-mcp`: built session 47, §8.32) · S4 export (ships with S3) · S5 coaching · S6 production + live test.

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

**Left for step 2 (the endpoints):** validating the `/v1/lease` request body, the Clerk and Paddle clients (paging and error mapping), and the Clerk metadata-merge check above. *(All done in step 2, §8.30.)*

**Evidence:** 103 Worker tests and 3 new `vault-account` tests. 17 planted bugs each turned a test red: 4 in lease signing, 12 in the rules, 1 in the Rust vector check. Each was run once and then restored byte for byte.

## 8.30 · ADR-104 / ADR-SEC-022 amendment 4 (2026-09-18, session 45) — decisions made building S2 step 2 (the Worker's endpoints)

Implementation choices §5 left open, each pinned by a test in `workers/account/test/`. None changes a locked rule. Still nothing is deployed and no account is touched: Clerk and Paddle are fakes in every test.

**Configuration:**
- Every account-specific value is a Cloudflare secret, or `.dev.vars` locally (gitignored). None is in `wrangler.jsonc`.
- The names: `CLERK_SECRET_KEY`, `CLERK_OAUTH_CLIENT_ID`, `CLERK_WEBHOOK_SECRET`, `PADDLE_API_KEY`, `PADDLE_ENVIRONMENT`, `PADDLE_PRODUCT_ID`, `PADDLE_PRICE_MONTHLY`, `PADDLE_PRICE_ANNUAL`, `PADDLE_WEBHOOK_SECRET`, `LEASE_PRIMARY_KID`, `LEASE_PRIMARY_KEY`, and `NEVER_END_PAYERS` (on only for exactly `1`).
- Anything missing or inconsistent makes every route answer 503. That includes a Paddle key whose prefix does not match `PADDLE_ENVIRONMENT`: a live key against the sandbox, or the reverse, is a deploy mistake.

**Clerk (BAPI spec 2026-05-12):**
- A token is refused (401) only on a definitive answer: another app's `client_id`, `revoked`, `expired`, an inactive JWT, 400 or 404.
- Our own key refused (401/403), 429, 5xx, no network, or a 200 we cannot read is an upstream error (503), **never a 401**, so a Worker-side fault can never look like the user's token being bad.
- A user Clerk no longer has is 401.
- **§8.29's open point is closed:** `PATCH /v1/users/{id}/metadata` deep-merges, and "You can remove metadata keys at any level by setting their value to `null`" (spec, quoted).

**Paddle:**
- A `next` page link is followed only on Paddle's own API host, so the key is never sent elsewhere.
- One customer's list reads at most 5 pages. Running out is an upstream error, never a derivation from a partial list.
- The all-customers list (for `/clerk/webhook`) reads at most 20 pages of 200. The sweep reads exactly one.

**`/v1/lease`:**
- A bad request is refused before any upstream call: 400 for the body (at most 1 KB; unknown fields ignored), 401 for a missing or malformed `Authorization`.
- The signing key is imported before any upstream call, so a broken deploy fails before it writes anything.
- `Cache-Control: no-store` on every answer. Error bodies are fixed codes that never echo a token, a key or an upstream body.

**`/v1/checkout`:**
- **The record (`paddle_customer_id`, `checkout_at`) is written before the transaction is created.** A checkout that could not be recorded is never started (503).
- A customer already in the record is used as it is. Otherwise the user's primary email finds one by exact match (Paddle's `email` filter), or creates one. A 409 creation race re-reads once. An email containing a comma is refused, because the filter is a comma-separated list.
- A user with no email gets 422 `no_email`, and nothing is created.
- "Already subscribed" means one of our product's subscriptions is `active` or `past_due` (a live fetch). `paused` or `canceled` goes to a new checkout.
- The portal link must be `https` on `paddle.com` or one of its subdomains.

**`/paddle/webhook`:**
- **Binding rule.** A subscription's `custom_data.clerk_user_id` can be set by a buyer's own browser (Paddle.js `customData`, "stored against the transaction and subscription"). So a record is updated only when it **already names that Paddle customer**, and that link is made only by our authenticated `/v1/checkout`. A forged tag cannot point anyone's payment at another account.
- The derivation re-fetches the customer's current list. The event's own copy is never used.
- A genuine event that changes nothing answers 200, so Paddle stops retrying: another event type, no user tag, a user gone, or an unbound record.
- Body at most 256 KB (413 otherwise). The `ts` tolerance is ±5 s, both directions, and any `h1` may match.

**`/clerk/webhook` (Svix):**
- Signature: HMAC-SHA256 over `id.timestamp.body`, keyed with the base64 part of the `whsec_` secret. Any `v1` entry in the list may match; other versions are ignored.
- The timestamp tolerance is ±5 minutes, the default of Svix's own libraries. Body at most 64 KB.
- **Cancelling:** it lists our two prices with status `active,past_due,paused` and cancels with `effective_from: immediately` every subscription tagged with the deleted user's id. A forged tag only ever cancels the forger's own subscription.

**The sweep (the daily cron, 03:17 UTC):**
- It reads one page of our `active,past_due` subscriptions and takes those whose period ends within ±48 h inclusive, one per user.
- It applies the webhook's binding rule.
- **It counts its own outbound calls** (budget 45 < 50, 3 per user) and reports how many it left for the next run. One user's failure is logged and skipped.

**Accepted residuals:**
- A signed-in user can create many unpaid checkout transactions. They are only drafts, and each costs us nothing.
- Flooding `/v1/lease` with bogus tokens costs Clerk verify calls. Flood protection (the rate-limit binding, or a Cloudflare rule) comes with the deploy, step 3.
- Record writes are blind merges (Clerk has no version check). Two invocations for the same user at the same moment (say a lease refresh and a webhook) can each patch from the same snapshot, and the later write wins for the fields both changed. Both derive from Paddle's live list, and the next lease call, webhook or sweep re-derives, so it corrects itself. It never clears `trial_started_at` or touches `comp_until`, because patches only carry changed fields. Raised as informational by the step-2 review; accepted.

**Not yet (step 3):**
- The sandbox's **default payment link** must be set before Paddle will create any transaction.
- The `/pay` page.
- The first deploy.
- A live test against the Zaaheen sandbox.

**Evidence:** 261 Worker tests inside workerd. 32 new planted bugs, each caught, then restored byte for byte, and all 17 of step 1's still caught against the full suite: 8 for the clients, config and `/v1/lease`; 6 for checkout; 6 for the Paddle webhook; 7 for the Clerk webhook; 5 for the sweep.

## 8.31 · ADR-104 / ADR-SEC-022 amendment 5 (2026-09-19, session 46) — decisions made in S2 step 3 (the `/pay` page and the sandbox deploy)

Implementation choices §5 and §8.30 left to step 3, and what the first live run found. Account identifiers are in the local `OPS-HANDOFF.md` §G only.

**`/pay`, Paddle's default payment link (`site/`, §2):**
- Served at `https://zaaheen.com/pay/`. `/pay?_ptxn=…` is redirected there with the query kept, but S3's app should build `https://zaaheen.com/pay/?_ptxn=…` directly.
- It opens exactly one well-formed `_ptxn` (`^txn_[a-z0-9]{26}$`). Anything else, including a second `_ptxn`, never initialises Paddle.js, so it cannot open whatever else the URL carries. No price IDs or items are taken from the URL.
- **`allowLogout: false`** in `Paddle.Initialize({ checkout: { settings } })`, which the checkout Paddle.js opens from `_ptxn` inherits (Paddle: "The opened checkout inherits settings from `checkout.settings`"). Found while reading Paddle's docs: by default the buyer can change their email in the checkout. That would make them pay as a different Paddle customer, and §8.30's binding rule would never credit the payment: a payer locked out. Verified live: the email shows as fixed text.
- **Its CSP is the site's one third-party exception** (founder sign-off 2026-09-19; the site's local rules 2 and 8): `script-src 'self' https://cdn.paddle.com`, `style-src 'self' 'unsafe-inline' https://*.paddle.com`, `connect-src` and `frame-src https://*.paddle.com`, everything else as strict as the rest of the site.
  - Measured against the sandbox: with `style-src 'self'`, Paddle.js's stylesheet (from `sandbox-cdn.paddle.com`) and its inline overlay styles were blocked, and the checkout rendered as an unstyled box. Hashes would break at Paddle's next release, hence `'unsafe-inline'` for styles only.
  - The card fields, fonts and fraud checks load inside Paddle's own frame, under Paddle's policy.
  - No SRI: Paddle updates v2 in place ("Always load Paddle.js directly from https://cdn.paddle.com/").
- The site audit pins that CSP (a widening is a reviewed code change), allows Paddle.js on `/pay/` only and as a `<script>` only, requires `noindex` there, and refuses a release build unless the page was built for `production` with a `live_` client-side token.
- The environment and client-side token come from the build environment (`PUBLIC_PADDLE_ENVIRONMENT`, `PUBLIC_PADDLE_CLIENT_TOKEN`; repository variables in CI), never the repo.
- Observation, no action: Paddle's checkout frame sends a report-only `frame-ancestors` naming the default payment link's origin (on localhost, without the port). It is report-only today, and `zaaheen.com` matches it in production.

**Two deployments, never sharing a key:**
- `wrangler.jsonc`: `--env sandbox` → `api-sandbox.zaaheen.com` (Paddle sandbox, Clerk development); the top level is production (`api.zaaheen.com`, route added at S6). `workers_dev` is off everywhere, so a deploy without `--env` publishes nothing reachable.
- **Each deployment has its own lease keys.** A sandbox lease, paid with a test card, must never verify in a production app. The sandbox has its own primary and backup public keys (kids `sandbox-p1`, `sandbox-b1`; S3's development builds carry them, release builds the production pair). **So the founder's offline backup key is a production item and moves to S6.**
- A Cloudflare account must have a workers.dev subdomain before a cron trigger deploys, even with `workers_dev` off (error 10063).
- The sandbox Worker uses the Clerk development instance's own secret key. Per-consumer secret keys (§5) stay a production (S6) item.

**Flood protection (§8.30's residual): the binding is in, but measured not refusing.**
- `src/flood.ts`: `/v1/lease` and `/v1/checkout` only, keyed by `CF-Connecting-IP`, 60 per 60 s, answering 429 with `Retry-After: 60`. The webhooks are never limited. A limiter error lets the request through (flood protection, not a gate); a missing binding is a 503, like any missing configuration. The address is never logged.
- **§8.28's runtime confirmation failed:** deployed on the free plan, 300 requests in 38 s from one address all passed a 60-per-minute limit. A Cloudflare community report of the same behaviour (2026-08-28) went unanswered and closed.
- So the binding stays as a harmless second layer, and **the enforced flood protection is a Cloudflare WAF rate-limiting rule, added at S6** (the free plan has one): the API hosts' two `/v1` paths, per IP. It acts before the Worker runs, so refused requests cost no Worker quota either. The sandbox gets no rule: a flood there costs nothing that matters.

**Live test against the sandbox (2026-09-19): every step passed.**
1. PKCE sign-in with the development OAuth app, as a Clerk test user (`state` and `iss` checked).
2. `/v1/lease` → a signed `trial` of 30 days; `client_time` echoed exactly; `offline_days` 30. The signature verified against the sandbox public key with the `zaaheen-lease-v1\0` prefix.
3. `/v1/checkout` monthly → a transaction. `/pay/` opened it with the email locked, and Paddle's test card paid it: $5.00, with VAT added on top.
4. Paddle delivered `subscription.created` and `subscription.activated` to `/paddle/webhook` on the first attempt (200). `/v1/lease` turned `active` about 2 s after the payment, with `active_until` = the period's end + 3 days (§5).
5. `/v1/checkout` again → the customer portal (already subscribed).
6. Deleting the Clerk user → `/clerk/webhook` → the subscription canceled at Paddle within a second.
- Also: an unknown path gives 404. No token, or a bogus one, gives 401; a bogus token reaching Clerk proves the Worker's Clerk key works, because a refused key would be 503. Webhooks without a signature give 401. Every answer is `Cache-Control: no-store`.
- Clerk's bot protection (Turnstile) stops automated **sign-up**, and it was not bypassed: the test user was created through BAPI, and sign-in has no challenge.

**Evidence:** Worker: 273 tests inside workerd (12 new). 7 planted bugs in the flood code, each caught. Site: 12 page-logic tests and 31 audit tests (13 new). 12 planted bugs, each caught. Each bug was restored byte for byte.

## 8.32 · ADR-104 / ADR-SEC-022 amendment 6 (2026-09-19, session 47) — decisions made in S3 step 1 (the gate, `vault_mcp::gate`)

Implementation choices §8.26 §6.1 left open, each pinned by a test in `crates/vault-mcp/tests/entitlement_gate.rs`. One wording change to §6.1, marked below. BRD §11 was re-read before the step (§11.15).

**S3's steps:** 1 the gate in `vault-mcp` (this amendment) · 2 the real check in `vault-app` over `vault-account`, and the gate wired into the keeper, direct mode and the daemon · 3 keeper lock mode, `ModeChanged`, the relay short-circuit and the maintenance runner · 4 the desktop (sign-in step, account panel, lock screen, banners, the `Entitled` token) · with S4, "Download my memories".

**The shape:**
- `EntitlementCheck` (async, one method) answers a `Verdict`: `Entitled` or `Locked(LockReason)`. `LockReason` is `SignedOut`, `CannotConfirm`, `TrialEnded`, `SubscriptionEnded` or `Unlocking`, and owns §6's fixed words verbatim (`LockReason::message`). `vault-mcp` still does not depend on `vault-account`; `vault-app` maps account state to a reason (step 2).
- `EntitledService<S: Service<RoleServer>>` sorts each request with one `match` over all 18 `ClientRequest` variants of rmcp 2.2.0, with no catch-all arm. A test fails if a `_ =>` ever appears in `gate.rs`.
- A locked plain tool call answers `CallToolResult::error` with the reason's words. Every other refused request is a JSON-RPC error with the crate's existing access-denied code (-32001) and the same words.

**§6.1 wording change — the three empty lists do not ask.** §6.1 has `ListPrompts`, `ListResources` and `ListResourceTemplates` ask the check and then "delegate (empty lists)" whether entitled or not. Asking cannot change the answer and can only add a refresh (up to 5 s) to a list request, so they pass without asking, like `ping`, `initialize` and `tools/list`. Pinned: the check is asked 0 times.

**A locked task-style tool call** gets rmcp's own answer to a task-style call of a tool that forbids tasks (invalid params, "Tool does not support task-based invocation"), built by the gate rather than by passing the call on, so a locked request never reaches the server. The test compares the gate's answer with the server's real one, so an rmcp change there fails a test instead of drifting.

**Calls in flight (for §6.2):**
- `InFlight` counts a request from its arrival, before the check decides: otherwise the keeper could see zero and change mode between a "yes" and the call starting.
- The count is held by a guard, not decremented after the call, so a request whose future is dropped (a panic unwinding, an aborted task) stops being counted. rmcp 2.2.0 does not drop a cancelled request's future; it only cancels its token, so a cancelled call stays counted until it answers, which is what §6.2 wants.
- One `InFlight` is shared by every connection the keeper serves; `wait_idle()` resolves when it reaches zero.

**Logging (§6.6):** a refusal is one `info` line, target `vault_mcp::gate`, with the request kind (`tools/call` or `other`) and the reason. Nothing the client sent is logged, not even the tool name. No audit row and no adapter call.

## 8.33 · ADR-104 / ADR-SEC-022 amendment 7 (2026-09-20, session 47) — decisions made in S3 step 2 (the check, and where a build's account settings come from)

**The check (`vault_app::entitlement`, step 2a).** `AccountCheck` implements `vault_mcp::EntitlementCheck` over `vault-account`. It asks the account for what is on disk; entitled, it serves the call and records the use (the 30-day unused rule, §8.26 §4). Otherwise it applies refresh-then-decide and answers from disk afterwards, whatever the refresh did.
- The refresh runs in **its own task** and is waited on for at most 5 s; it is never dropped (§8.27: cancelling one between the server rotating the refresh token and the store writing it loses the live token).
- **Signed out short-circuits**: nothing a refresh could change, and §6.3's relay short-circuit counts on it being cheap.
- **Only a signed `ended` says "ended"** (§8.27). A folder that cannot be read, a failed refresh, a lease that has not arrived: all say "could not confirm". A wrong clock or a dead network must never tell a paying user their trial is over.
- The gate asks with `Trigger::Denial`; every rate limit stays inside `vault-account`.
- `AccountAccess` is a trait in `vault-app` (`state`, `refresh`, `record_use`), implemented for `vault_account::Account`. `AccountState` is `Status` without the lease, which only `vault-account` can build — that is what lets the tests run with no credential store and no network.

**Where a build's account settings come from (step 2b).** §8.26 never said, and the values (issuer, OAuth client id, account-service origin) are account identifiers, which this public repo must not carry.
- They are read with `option_env!`, so they are baked in **at build time**, like the site's `PUBLIC_PADDLE_*` (§8.31). The names: `ZAAHEEN_ACCOUNT_ISSUER`, `ZAAHEEN_ACCOUNT_CLIENT_ID`, `ZAAHEEN_ACCOUNT_API`, `ZAAHEEN_LEASE_PRIMARY_KID`, `ZAAHEEN_LEASE_PRIMARY_KEY`, `ZAAHEEN_LEASE_BACKUP_KID`, `ZAAHEEN_LEASE_BACKUP_KEY`. The lease keys are 64 hex characters (32 bytes).
- **All seven absent means this build has no sign-in** — which is what every build before this arc is. **Any one missing or malformed is refused by name**, so no build can sign in against one instance and verify leases with another's key. The error never carries the value.
- Development builds carry the sandbox pair, release builds the production pair (§8.31). **S6 gains:** the production values in the release build's environment, and a release-build guard that fails when they are absent (the site's audit does the same for its live token).
- The account folder `%LOCALAPPDATA%\com.zaaheen.app\account` is created and restricted to its owner (ADR-SEC-019) before `AccountDir::open`, per §8.27. A failed restriction is logged, not fatal, as for the vault folder.

**The wiring (step 2c).** Every server that faces an AI app takes the gate, and a build without account settings passes `None` and serves exactly as before:
- **The keeper** (`keeper/runtime.rs`): `serve` takes `Option<Gate>` and every authenticated session is wrapped, so the one gate — and the one `InFlight` — covers the whole process (§6.2).
- **Direct mode** (`application.rs::start_with_mcp`): the same wrapping around its `StdioServer`.
- **The daemon** (`vault-mcp/src/daemon.rs`): it dispatches per request rather than wrapping a server, so it asks the gate itself, **after `authorize`** (BRD §11.4.4: an unknown token is refused first and learns nothing about the account). Calls in flight are the keeper's concern, so the daemon does not count them.
- `vault_mcp::MaybeGated` / `maybe_gated` give both cases one type at the transport.
- `vault_app::account::build_gate` builds the one gate per process; `vault-cli` calls it for the keeper, direct mode and the daemon. **A build whose settings are broken refuses to start** rather than serving ungated: a gate that silently disappears would turn a broken build into a free one.
- The account folder is found through the new `install_paths::local_data_dir()` (`%LOCALAPPDATA%\com.zaaheen.app`), beside the logs.

**Evidence:** 28 unit tests in `vault-app` (14 for the check, 14 for the settings and the folder), with stand-ins for the account so none touches the network; the Windows-only pair opens Credential Manager and writes nothing. 5 wiring tests over the real transports: a locked and an entitled keeper through a real relay over a real pipe (the locked one proves nothing reaches the vault and the counter returns to zero), and a locked, an entitled, and an unknown-token daemon over real loopback HTTP (the last proves the check is never asked for a request that failed to authenticate). 14 planted bugs on the check and 11 on the settings, each turning the intended test red and restored byte for byte. Honest note: the check was run failing first against a stub (12 of 14 red); the settings and the wiring were written tests-first but run only after the implementation, so their proof is the planted-bug pass and the wiring tests, not a red run.

**DoD:** `build --workspace` (40.7 min), `clippy --workspace --all-targets -D warnings` (30.2 min), `test -p vault-mcp` (45.8 min), `test -p vault-app` (8.8 min), `test -p vault-cli` (15.4 min), `fmt --check` — all green from a wiped `target\debug`: 344 tests over 28 binaries, zero warnings.

## 8.34 · ADR-104 amendment 8 (2026-09-20, session 47) — the app subscription and coaching are separate purchases (FOUNDER-LOCKED)

**Founder, 2026-09-20:** "we need to make sure someone with monthly membership via paddle for zaaheen also does not gets automatically free access to the coaching sessions... they should pay for the session ..and vice verca.. a coaching session payment shouldnt allow them to automaticlaly become member of zaaheen".

One Zaaheen account identifies the person in both places (§0). It grants nothing across them. Two purchases, two entitlements, neither derived from the other.

**Coaching payment → no app access (already enforced, verified 2026-09-20).**
- The Worker counts a Paddle subscription only when one of its items' `price.product_id` is **our app product** (`workers/account/src/billing.ts::isOurs`, §8.29). A coaching purchase is a different product, so it never reaches `active_until`.
- `/v1/checkout` maps `plan` to the app's allowlisted price ids only (§5), so an app checkout can never buy coaching or vice versa.
- `private_metadata.zaaheen_memory` is written by the account Worker alone (§5) and describes the app subscription only.

**App subscription → no free coaching (S5 must build it this way).**
- A booking is entitled by its **own** payment. The coaching app never reads the lease, `zaaheen_memory`, or any app subscription state to decide whether a session is paid.
- Coaching keeps its own record under its own key, and its own Paddle product and prices.
- `comp_until` (§5) is the app's beta comp only. A coached client gets no app entitlement from it, and a comped app user gets no session.
- **Gap to close at S5:** §9 of this document is still only a pointer ("As v2 §9"). S5 writes the real section, and it must state this rule and its enforcement points.

**Observation, no action:** a task-style call naming a tool that does not exist gets, from the server, rmcp's default `enqueue_task` answer (an internal error), and from the gate while locked the invalid-params answer above. Both refuse, nothing reaches the vault, and nothing leaks; only the code differs. Raised by the step's independent review as below its reporting threshold.

**Evidence:** 19 tests in `entitlement_gate.rs`, driven over raw JSON-RPC lines against the real `StdioServer` and the recording `MockAdapter`. Run first against an empty gate that passed everything: 12 failed, each for the reason it names. Then 19 pass. 16 planted bugs, each caught and restored byte for byte (the 7 tests the empty gate already passed are among those caught): the handshake or the lists asking, the in-flight guard dropped at once, not covering unwinding, or counting only after a "yes", a list or `tools/list` gated, notifications swallowed, a catch-all arm, an entitled request not passed on, `wait_idle` ignoring the current value, the refusal log carrying the request, a locked task-style call passed on, one word of the locked text changed, a locked tool call not flagged `isError`, and tool calls or other requests never asking. An independent read-only review against this design and rmcp 2.2.0's source found no defects.

---

## 8.35 · ADR-104 / ADR-SEC-022 amendment 9 (2026-09-20, session 48) — decisions made in S3 step 3 (keeper lock mode, `ModeChanged`, the relay short-circuit, the maintenance runner)

Implementation choices §8.26 §6.2 and §6.3 left open, each pinned by a test. BRD §11 was re-read in full before the step (§11.15); the principles that bear on it are SP-4 (fail securely — a mode change must not drop a write), SP-6 (§6.6's refusal log only, no audit row) and §11.7.2 (the paused maintenance outcome is a stable code, never free text).

**Why the tick may not ask the serving check.** §6.2 says both modes re-evaluate "(file read, no network)" and that "lock mode's re-evaluation uses the real check". Reusing the gate's own check for the tick — the obvious reading — is **wrong**, and silently so: `AccountCheck::check()` calls `record_use` on every entitled answer (§8.33), which is what feeds `last_active_anchor` and therefore the 30-day-unused sign-out (§8.26 §4). A keeper ticking every `idle_check` would record a use of the vault every few seconds whether or not anybody touched it, so an install left running could never go unused, and a rule the design deliberately measures in server time would never fire. The two sentences agree once "re-evaluate" means the real check's **read** path: no network, no writes, no use recorded. The refresh appears exactly once, in the full-to-lock direction, which is what §6.2's own sub-bullet asks for. A locked user's lease still arrives through the call path (below) and through the desktop; the tick never fetches one.

**The second contract: `vault_app::entitlement::ModeCheck`.** `EntitlementCheck` (`vault-mcp`) answers a *request*; `ModeCheck` (`vault-app`) answers *the keeper's question about its own mode*, and they are deliberately not the same trait:
- `peek()` — what the files say now. No network, no writes, **no use recorded**. Same mapping as §8.33: only a signed `ended` says "ended"; a folder that cannot be read says "could not confirm".
- `refresh_and_peek()` — one refresh (≤ 5 s, never dropped, as §8.27 requires), then answer from disk whatever the refresh did. Used only by a full keeper that has just seen a denial, so a merely stale lease does not unload and reload 2.86 GB.

Implemented for `AccountCheck`, so one object serves calls and answers ticks; the tests supply their own, so none touches the network or Credential Manager.

**One parameter, not two.** `runtime::serve` took `Option<Gate>` (§8.33); it now takes `Option<Subscription>`, which carries the gate that serves calls, which mode this keeper is, the `ModeCheck` the tick asks, and lock mode's flip signal. Two coupled `Option`s that must agree is a defect waiting to happen, so there is one. `Subscription::full` and `Subscription::lock` are generic over `C: EntitlementCheck + ModeCheck` and build the right gate themselves — **not** `Arc<dyn ModeCheck>` upcast to `Arc<dyn EntitlementCheck>`: trait upcasting is stable from Rust 1.86, this workspace declares `rust-version = "1.81"`, and `clippy::incompatible_msrv` under `-D warnings` would fail the gate.

**`KeeperExit::ModeChanged`,** a fifth exit beside Idle, Yielded, HandedOver and Shutdown. It takes the **existing** `stop_tx` path, so the one cleanup runs: discovery file removed first, then the listener dropped, then every connection closed. `dispatch_keeper` already logs whatever exit it is given and exits 0, so a mode change and an idle exit look the same to the stores — the case they are already built to survive.

**The evaluation never runs inside the serve loop.** A `refresh_and_peek` can take 5 s, and the loop's `select!` also owns `accept`: evaluating in the loop would stop the keeper answering new connections for as long as a refresh. So a tick (or lock mode's flip notification) spawns **one** evaluation, guarded by an atomic so ticks cannot pile up behind a slow one, and that task — not the loop — waits for calls in flight and then sends `ModeChanged`. §6.2's order is preserved exactly: decide, refresh if the decision was a denial, `InFlight::wait_idle()`, then exit. The idle exit requires zero connections; `ModeChanged` does not, which is precisely why it must wait rather than cut a write off mid-way.

**Lock mode adds almost no runtime code, and that is the point.** `handle_connection` already builds `StdioServer::new(ctx.adapter, boundaries)`, so lock mode is the same `runtime::serve` loop with `Arc::new(NoVaultAdapter)` as its adapter and a `LockModeCheck` in its gate. The identical tool list, `WIRE`, discovery roles, erasure intent and handover path that §6.2 asks for are therefore identical **by construction**, not by a second implementation kept in step. No `Application`, no models, no retry worker, no reranker acquisition: that is the 2.86 GB this frees.

**`LockModeCheck`** asks the real `EntitlementCheck` — refresh-then-decide, because §6.2's "a lock-mode call that finds the user now entitled" is a call, and a user who has just paid must unlock on their next one rather than wait for a tick. When the answer is "entitled" it replies `LockReason::Unlocking` and fires the flip signal; when the answer is a denial it passes that reason through unchanged. It can never answer `Entitled`. (A lock-mode call that finds the user entitled does record a use, through the real check — the user was actively calling a tool, so the anchor is honest.)

**Where the mode is chosen: `vault-cli`, before the models load.** `build_gate` moves ahead of `build_application`, and:
- no account settings → full mode, today's path unchanged, `None` throughout;
- entitled → full mode, with `build_application` still the thing that creates the master key on a fresh install;
- locked → lock mode, reading the master key **read-only**.

**A locked keeper with no master key publishes `failed`,** as §6.2 states ("none ⇒ publish `failed`"). This is not a gap to close: the relay handshake is authenticated with a key derived from the master key, so without one there is no channel on which to deliver the locked message at all. The affected case — somebody whose entitlement has lapsed before a vault was ever created on this computer — reaches the lock screen through the desktop app, which is where subscribing happens (§6 LOCKED wording).

**The relay short-circuit (§6.3).** `RelaySettings` gains `account_dir: Option<PathBuf>` — `Some` only when the build carries account settings, decided in `vault-cli` where `build_gate` is already decided — and `marker_recheck`, so the two checks 200 ms apart are a tunable rather than a sleep in a test. The relay reads the marker through a **read-only** helper and never `open_account_dir`, which creates and hardens: §8.26 §4 is that only the keeper and the desktop write in the account folder, never a relay. A relay must therefore not create the folder either, so a missing folder reads as "no marker" rather than being brought into existence. No marker on both checks ⇒ the relay answers §6's signed-out message itself and requests **no** keeper start; a marker, with or without a lease, ⇒ the normal start path.

**Three answers, not two, when a relay reads the marker.** §6.3 reads as a yes/no ("no `signed-in` marker ⇒ the relay answers the sign-in message itself"), but there are three possible readings of that folder and the third one changes what the user is told:
- a marker ⇒ signed in, so the keeper starts as usual;
- **no folder, or a path that is not a folder** ⇒ nobody has ever signed in on this computer. `AccountDir::open` reports both as `NotFound`, which is the right reading: neither can hold a marker. This is the commonest signed-out case there is (a fresh install) and it is a definite answer, so the relay short-circuits;
- **a folder it could not read** (a permission failure) ⇒ the relay must **not** decide. Saying "open the app and sign in" to somebody whose subscription is live and merely unconfirmable is the wrong message — the same reasoning as §8.33's "only a signed `ended` says ended". It falls through to a normal keeper start, and the keeper's own check answers with the accurate reason.

So `sign_in_marker` answers `Option<bool>`, and the decision is split from the file I/O (`marker_verdict`) so the "could not read it" branch is provable without an unreadable folder, which no portable test can create. Fail-open for the short-circuit, which is only an optimisation; fail-closed for entitlement, which the keeper still gates.

**`build_check` beside `build_gate`.** The keeper now needs the check itself, not a gate: it asks it which mode to start in, and the same object then both serves calls and answers ticks. `vault_app::account::build_check` returns `Option<Arc<AccountCheck>>` and `build_gate` is expressed in terms of it, so direct mode and the daemon (§8.33) keep the signature they already had.

**Known consequence, resolved by step 4 — "Run now" on a locked computer.** A paused run exits 0 (a non-zero exit would make the nightly launcher classify it as a *failure*, which is exactly what `Paused` exists to avoid), so `vault_tauri`'s `run_maintenance_now` treats the child as successful and the button reports "Done ✓" although nothing ran. The Maintenance tab itself is correct either way — `record_run` writes `ok: false`, and the panel renders `friendlyMaintError`, so it reads "Last attempt … — paused until you subscribe."
- This cannot reach a user, because **`run_maintenance_now` is gated in step 4**: §6.4's ungated allowlist is account, export, erasure, logs, settings and maintenance *status* — running one is not on it, so a locked computer shows the lock screen instead of the button.
- **Step 4 must verify that**, and the cleanest fix if it ever needs one is for `run_maintenance_now` to return `Err(code)` when the recorded `last_run.ok` is false, rather than for the child to exit non-zero.

**The maintenance runner (§6.5)** gains `RunOutcome::Paused` with its own stable code, alongside `maintenance_vault_busy` and `maintenance_run_failed`. Not a success (nothing ran) and not a failure (nothing is wrong), which is the distinction `Busy` already draws. The plain-English string lives where the other three live, in `friendlyMaintError` in the desktop bundle — a code whose UI arm is missing falls through to showing the raw code, so the arm ships with the code.

**What rides with step 4 instead.** §4's other refresh triggers still have no production caller: only `Trigger::Denial` has one. `Trigger::Routine` (keeper start, desktop open, the daily jittered timer) and `Trigger::UserAction` (checkout polling, "I have paid") are not in §8.32's step 3, and §4 groups the daily timer with "desktop open", which is step 4's own work on the same plumbing. They ship there, together. Until then a stale lease is refreshed on the first denial — one ≤ 5 s stall on one call — rather than quietly in the background.
**Evidence.** 43 new tests, plus one from step 2 rewritten (below). None touches the network, Credential Manager or a real vault; the keeper tests run the real `runtime::serve` loop on a real named pipe, reached by a real relay pool.
- `vault_app::entitlement` — 7 for `ModeCheck` over `AccountCheck`, 7 for `LockModeCheck` and `Flip`.
- `vault_app::account` — 5 for the marker, including that reading it never creates the folder.
- `vault_app::maintenance_state` — 3 for `Paused`, one of them pinning that every outcome code has a plain-English line in the desktop bundle.
- `tests/keeper_end_to_end.rs` — 12 for the modes and `ModeChanged`, 4 for the relay short-circuit, 1 for erasure from a lock-mode keeper.
- `vault-cli` — 4 for the maintenance pause.

**Run failing first: 24 red**, each for the reason it names, against implementations left deliberately inert (a `LockModeCheck` that delegated verbatim, a `peek` that answered without looking, an evaluation that decided nothing) rather than against a stub that could not compile: 10 of 28 in the entitlement tests, 6 of 216 in the rest of the `vault-app` library, 8 of 27 in the keeper tests. Then green: 216, 32 and the `vault-cli` set. Four of the new tests pass against the inert code too, because they assert absences (no refresh, no use recorded); their proof is the implementation review and the green run, not a red one.

**Three defects the tests found, and one the design review found before any code:**
1. **Before code — the tick must not ask the serving check.** See the opening of this amendment: `record_use` on every entitled answer would have cancelled the 30-day unused rule. Pinned by `peek_never_records_a_use_however_often_it_is_asked`.
2. **`spawn_mode_evaluation` released its latch before the loop read the stop message.** `tokio::time::interval` bursts the ticks it missed during a slow refresh, so a queued tick started a *second* evaluation — and therefore a second refresh — on a keeper that had already decided to leave. On a real machine that is a second network call, and `refresh.lock` contention with the desktop, for a keeper on its way out. Caught by `ticks_never_pile_up_behind_a_slow_evaluation` (a refresh six times slower than the tick, which a 5 s refresh against a 5 s tick reproduces routinely). The latch is now held once a mode change is decided.
3. **Step 2's `a_locked_keeper_refuses_the_call_and_never_touches_the_vault` described a state that no longer exists.** A full keeper never *stays* locked now: it refreshes and leaves. The test failed with "keeper never published" because the keeper had already gone. But the behaviour it was reaching for is still real and was untested — the **transient window** between a subscription lapsing and the keeper exiting, in which a call must still be refused without touching the vault. It is now `a_keeper_whose_trial_ends_refuses_the_call_and_never_touches_the_vault`, made deterministic by a refresh slower than the call rather than by timing luck.
4. **A test of mine asserted at the wrong layer.** `KeeperPool` answers a call that never left the process with `Err(UpstreamError::NotSent)`; it is `RelayServer` that turns that into the `isError` tool result the agent reads (ADR-103 D2), which `vault-mcp` already covers. The short-circuit test now asserts where the behaviour lives.

**Additions beyond the floor, each named.** The plan forecast 34 tests; there are 43. The extras: `peek_never_records_a_use` and `refresh_and_peek_records_no_use` (defect 1); `peek_and_a_served_call_agree_on_every_state` (a tick and a call must never disagree about the same state, or the keeper flaps); `each_call_asks_the_real_check_once` (a locked call must not pay for two refreshes); `a_call_still_answers_when_the_keeper_has_stopped_listening`; `a_path_that_is_not_a_folder_is_a_signed_out_computer` and `a_folder_it_could_not_read_leaves_the_decision_to_the_keeper` (the three-state marker above); `a_lock_mode_keeper_stays_while_the_user_is_still_locked`; and `erasure_takes_the_vault_from_a_lock_mode_keeper` (§6.2 asks for an identical handover path, and somebody whose trial has just ended is the likeliest person to ask for their memories and then delete them).

**Deliberate deviation from §6.2's stated order.** §6.2 reads "reads the master key ..., takes `.vault.lock`, then evaluates entitlement". The keeper does it in the order lock, evaluate, key: the key cannot be read before the lock on a fresh install, because only a keeper holding the lock may create it, and the pre-existing code already read it after `build_application` for that reason. The outcome §6.2 specifies is unchanged — no key in lock mode publishes `failed`.
**Planted bugs: 24 invariants, all 24 caught — two of them only after the test was rewritten.** Each bug was introduced as a real source edit, built, run against the one test that should object, then restored byte for byte with a fresh timestamp (session 47's lesson: a restored file keeping its old mtime makes cargo re-run the *previous* binary). The harness refuses a verdict unless the log says `Compiling <crate>` and a test result line exists, so a plant that did not build is reported as INVALID rather than counted.

**Two tests did not catch their bug, and both were genuinely wrong:**

1. **`a_call_in_flight_finishes_before_the_mode_changes` asserted nothing of the sort.** It checked `searches().len() == 1`, but `RecordingAdapter` records a search **on arrival, before its delay** — so a call the keeper cut off half way through satisfied it exactly like one that finished. The whole in-flight wait was deleted from `spawn_mode_evaluation` and the test stayed green. A completion counter did not fix it either: rmcp runs each request in a **detached task**, so the call answers whether or not the keeper waited. The harm only lands because `dispatch_keeper` calls `std::process::exit` the moment serving ends, killing those detached tasks mid-write — and **no in-process test can survive to observe that**. The invariant that *is* testable is the ordering: `serve` must not RETURN while a call is in flight. The test now sleeps to 450 ms, with a 700 ms search and the mode change decided near 300 ms, and asserts `!keeper.is_finished()`. Re-planted: caught.

2. **`ticks_never_pile_up_behind_a_slow_evaluation` was over-claiming.** It caught the latch defect for real at 12:54 and missed the identical defect at 17:04. The cause is not chance but ordering: `try_send(ModeChanged)` happens **before** the latch is released, so the serve loop nearly always breaks before a burst tick can start a second evaluation. Repeating the scenario eight times changed nothing but the runtime. The test now states exactly what it proves — that a 600 ms refresh against a 100 ms tick still yields **one** evaluation, which a removed latch guard turns into six (planted, caught). **The narrower rule — that the latch stays held once a mode change is decided (the `return` in `spawn_mode_evaluation`) — is defensive and reasoned, NOT test-proven.** It is recorded here as such rather than left implied by a test name. Its cost if wrong is one redundant refresh on a keeper already leaving.

**Six plants were faults in the harness, not gaps in the tests** — each reported INVALID or PLANT FAILED rather than passing silently, then fixed and re-run to a CAUGHT verdict:
- An empty replacement string: PowerShell rejects one for a mandatory `[string]` parameter, so the plant never ran and the summary simply had no line for it. A harness that can silently not-plant is worse than none, because "no line" reads as "nothing happened".
- Two plants orphaned an import or a private field. Under `RUSTFLAGS='-D warnings'` that is a compile **error**, so the bug never built. Worth keeping in mind: **some invariants here are held partly by the compiler and cannot be planted by deletion at all** — which is a stronger guarantee than a test, but not the same thing as one.
- Three literals could not match because **`keeper/relay.rs` is CRLF while `keeper/runtime.rs` and `entitlement/mod.rs` are LF**. The repository has mixed line endings in the working copy; git normalises on commit, so no diff is harmed, but any tool matching source text must not assume one or the other.
- One plant replaced **every** occurrence of its literal, including the expected value inside the test itself, so both sides moved together and the assertion still held. A planted bug that also patches its own test proves the opposite of what it appears to.

**What the campaign covered:** the tick's read-only contract and its refusal to record a use · only a signed `ended` saying "ended" · lock mode never serving a call, raising the flip, and passing a real reason through verbatim · the flip staying raised · the marker's three states, and that reading it creates nothing · the paused outcome's code, its not-a-success status and its plain-English line · refresh-before-deciding · the in-flight wait · one evaluation at a time · lock mode reading rather than refreshing on a tick · a full keeper staying while entitled · the relay short-circuiting only when definitely signed out, checking twice, and never on a build without sign-in · its exact wording · and the maintenance pause in both directions.

---

## 8.36 · ADR-104 / ADR-SEC-022 amendment 10 (2026-09-20, session 49) — decisions made in S3 step 4a (the desktop entitlement guard, `vault_tauri::guard`)

§8.26 §6.4 in full: *"Desktop (pre-D4): gated `*_inner` functions take an `Entitled` token that only the check can mint (the compiler enforces the guard). A source test in the existing `include_str!` pattern lists all registered commands and the ungated allowlist (account, export, erasure, logs, settings, maintenance status)."* BRD §11 was re-read in full before the step (§11.15); the parts that bear on it are §11.7.1 (every Tauri command argument validated), §11.7.2 (stable codes, never free text, to the UI) and §11.12's `vault-tauri` checklist.

**The guard is three legs, and the honest description is that they are not equally strong.**
1. **The compiler** — a gated `*_inner` takes `&Entitled`, whose field is private, so a command that never asked has nothing to pass and does not build.
2. **`guard`'s own tests** — no token is handed out while locked, for any reason.
3. **The source test over `main.rs`** — every registered command is on exactly one of `GATED_COMMANDS` / `OPEN_COMMANDS`. The compiler cannot cover this: a brand-new command with a brand-new inner compiles perfectly well while serving a locked user.

**The residual, recorded because no test closes it:** a command placed on `OPEN_COMMANDS` that should have been gated is a human misjudgement and nothing objects. The source test guarantees the judgement is *made and visible*, not that it is right.

**Nine commands are compiler-enforced; six are not.** `memory`, `boundary` and `agent` have `*_inner` functions, which take the token. The three `engine` and three `maintenance` commands have no inner — the body *is* the command — so for them the gate is the `require()` call itself, held by `every_gated_command_asks_before_it_serves`. Extracting inners purely to gain compiler enforcement was considered and rejected: it would reshape four Tauri-coupled functions for a guarantee the source test already gives, which is refactoring dressed as security. **The asymmetry is deliberate and is stated in the test's own doc comment**, so nobody reads "the compiler enforces the guard" as covering all fifteen.

**The classification.** Open, and nothing else — §6.4's allowlist verbatim: `get_settings_info`, `get_maintenance_schedule`, `erase_everything`, `export_logs`. Gated: the other fifteen. Two readings could have gone the other way and are recorded:
- **`revoke_agent` is gated.** Revoking an agent's access is arguably a safety action, like erasure, that anybody should be able to take. The locked allowlist does not include it, and widening a locked list is not a step's decision to make. Moot in practice: a locked computer shows the lock screen, so the Agents tab is unreachable.
- **`set_maintenance_schedule` is gated while `get_maintenance_schedule` is open.** The allowlist says maintenance *status*, which is the reading half of the pair.

**§8.35's obligation is discharged:** `run_maintenance_now` is gated, pinned by `run_maintenance_now_is_gated`, so the "reports the run as done on a locked computer although nothing ran" case can no longer reach a user.

### ADR-SEC-023 — 2026-09-20 — a desktop whose account check cannot be built serves *locked*, not *refused* and not *open*

- **Context.** `vault_app::account::build_check` fails on build settings that do not parse, an account folder that cannot be prepared, or a credential store that will not open. Its doc comment leaves the choice to the caller: *"The caller decides whether to refuse to start or to serve ungated; the keeper refuses, because a gate that silently disappears is worse than a keeper that says why."* The desktop is the third caller and needs its own answer.
- **Decision.** The desktop serves **locked** with `ERR_LOCKED_CANNOT_CONFIRM`. It does not refuse to start, and it does not fall back to ungated.
- **Reasoning.** Refusing to start would take away the **export**, and "your memories are always yours" is the promise the export exists to keep (BRD §1.6 amendment 1: *"Without the export, a locked app would cut people off from their own memories, and we could not honestly say 'your memories are always yours'"*). Serving ungated would make an unreadable account folder a way to unlock the app. Locked-as-cannot-confirm is SP-4 (fail securely) without being cruel: the person still reaches the lock screen, can still download their memories, and can still sign in and subscribe. It is also the same reading §8.33 applies to the keeper — only a signed `ended` ever says ended, so an unreadable folder must never say it.
- **Alternatives considered.** (a) Refuse to start, as the keeper does — rejected: removes the export, which is the one thing that must survive a lock. (b) Serve ungated — rejected: turns a broken credential store into a bypass.
- **Trade-off.** A paying customer whose credential store hiccups at startup sees the lock screen until the app is reopened. Accepted: the app is long-running and retries on next open, `refresh`/"I've paid" retry on demand, and the alternative failure modes are worse.
- **Where it lives.** `guard::build()` — deliberately *not* in `main.rs`, so `Entitlement`'s constructors can stay private (see the review findings below).

**Evidence.** 17 tests in `vault_tauri::guard`, run failing first: **11 passed / 3 failed** against a deliberately inert `require()`, each failure carrying the reason its test names. Then green at 64 passed / 0 failed for the whole `vault-tauri` lib suite (62 before the review, 64 after).

**Two invariants here are held by the compiler and cannot be planted at all**, which is stronger than a test but is not one, and is recorded rather than left to look like a coverage gap:
- a guard that holds a check and **never asks it** does not compile — `-D warnings` rejects the then-unread `Source::Check` field. (Asking **twice** *is* plantable, and bug 4a09 caught it.)
- a command that calls a gated `*_inner` **without a token** does not compile — the argument is missing.

**Planted bugs: 13 invariants, all 13 caught**, each introduced as a real source edit, built, run against the one test that should object, and restored byte for byte with a fresh timestamp. Covered: a locked person handed a token · a failed setup serving · two reasons collapsing onto one code · a lock code losing its plain-English line · a registered command falling off the lists · `run_maintenance_now` un-gated · a command on both lists · the no-sign-in path starting to refuse · the check asked twice · the source test silently reading a junk list · the engine commands holding the guard but not asking it · a command module naming the guard's constructor · the plain-English mapping no longer being called.

**Two things the campaign could not have found, and an independent review did.** Both are recorded in full because each is a class of mistake, not a one-off:

1. **A documented guarantee the code did not honour.** `Entitlement::open()`, `checked()` and `unavailable()` were `pub`. Any future command could have written `crate::guard::Entitlement::open().require().await?` — which compiles, mints a real `Entitled`, never asks the account check, and **satisfies `every_gated_command_asks_before_it_serves`**, because that test looks for a `.require().await` call and not for which value it is called on. None of the three legs distinguished a genuine guard from a locally forged always-open one. **Fixed at the language level:** the three constructors are now private and `guard::build()` is the only way a binary obtains an `Entitlement`; the startup decision moved out of `main.rs` into `guard.rs` to make that possible. A new test, `no_command_module_builds_a_guard_of_its_own`, states the intent where a future reader will see it.
2. **A test that gave false comfort.** `friendlyLockError` was added to `dist/app.js` with `every_lock_code_has_a_plain_english_line_in_the_app` pinning it — and was **never called**. The test passed while a locked person would still have been shown `locked_trial_ended` raw, because ~15 `catch` sites render the caught error verbatim. **Fixed in one place rather than fifteen:** `app.js`'s central `invoke` is now wrapped, so any `locked_*` rejection becomes its plain-English line before it reaches any call site, and anything else is re-thrown untouched (`friendlyLockError` returns `null`, not the code, for a non-lock error — returning the code would swallow every other error into a fake lock message). `the_plain_english_lines_are_actually_used` now asserts the mapping is *reachable*, not merely present.

**The lesson, stated for the next step.** A planted-bug campaign proves the tests catch breakage in code somebody thought to test. It cannot tell you that a test checks the wrong thing, or that a confident comment describes a guarantee the code does not have. Both defects above were of that kind, and both were found by a reader holding the locked design text. **Steps 4b–4d get the same independent review, and it is not optional.**

**The desktop's lock wording is provisional.** The five lines in `friendlyLockError` are deliberately *not* `LockReason::message()`: that text is written for an AI agent to relay and says "Open the Zaaheen app on this computer", which is nonsense shown inside the app. They exist so no raw code can reach a user. **The lock screen's real copy is a founder decision in step 4d.**

**S4's shape is founder-locked (2026-09-20): a single readable `.md` file.** "Download my memories" writes one Markdown file — openable in Notepad, any editor, or pasted straight into another AI — grouped by topic, each memory carrying its date and topic, with a short header saying the file came from the person's own computer and nothing was sent anywhere. No separate machine-readable JSON ships with it; the founder's call was that the most likely next action for a departing user is handing the file to another agent, which Markdown serves directly. Revisit only if a beta user asks for a structured export. This satisfies BRD §1.6 amendment 1's *"The second writes a readable export file"* and §11.8.2's portability right.

---

## 8.37 · ADR-104 / ADR-SEC-022 amendment 11 (2026-09-20, session 49) — decisions made in S3 step 4b (the account commands)

Five commands — `account_status`, `account_sign_in`, `account_sign_out`, `account_subscribe`, `account_refresh_now` — plus the `/v1/checkout` client, the link opener, and §4's missing refresh triggers.

**The allowlist did not widen.** §6.4 has always read *"account, export, erasure, logs, settings, maintenance status"*. Four slots were filled; the **account** slot was empty only because the commands did not exist. Step 4b fills it, 4c fills **export**, and a test (`the_export_slot_is_still_empty`) fires when it does — a reminder that the export, unlike these five, **does** read the vault and so cannot lean on the argument below.

### ADR-SEC-024 — 2026-09-20 — the account commands are never given the vault (FOUNDER-LOCKED)

- **Context.** These five run for somebody who is *not* entitled — they have to, or a lapsed customer could never sign in or pay. That makes them the only ungated commands that a locked person actually reaches, so "they are on an approved list" is a thin guarantee: a list is a note, not a mechanism.
- **Decision (founder, 2026-09-20: "never give them the vault", chosen over keeping the list plus review).** `AccountOps` holds no `Application`, no adapter and no key. Serving a memory is therefore **not a mistake this code can make**, because it is not given the thing that reads memories.
- **Enforcement, both ends.** `account_ops_never_holds_the_vault` pins the absent field; `no_account_command_receives_the_vault` pins that no command asks Tauri for one. Both are source tests, because a *missing* field and a *missing* parameter cannot be observed at runtime. Planted bug 4b10 added a real `State<Application>` to a command and was caught.
- **Alternative considered.** A third list, splitting "open but vault-touching" from "open and vault-free". Rejected: more machinery, and the middle list would still rest on review.
- **The residual.** `get_settings_info` is open *and* takes `Application`, because it reports memory and boundary counts on the Settings screen. It is the one named exception; the rule is "the account and export commands", not "every open command".

### ADR-SEC-025 — 2026-09-20 — `/v1/checkout` is called from inside `Account`

- **Context.** The endpoint needs a Bearer access token. An access token is produced **only** by rotating the refresh token under `refresh.lock`, in about fifty lines that S1 built carefully: a token the credential store refused earlier is the live one; `invalid_grant` is looked at **twice**, re-reading the store between, because another process may have rotated a moment ago; and the rotated token is **stored and confirmed before anything else**.
- **Decision.** `Account::start_checkout(plan)` makes the call. The access token is used and dropped without leaving the type that owns it. The rotation itself moved into one private helper, `rotate_under_lock`, which `refresh` now also calls — so those rules exist **once**.
- **Reasoning.** The alternative was exposing a scoped access token to `vault-app`. That means a second way to obtain one, which either duplicates the rotation rules or bypasses them. Both fail *quietly*, as a subscription that mysteriously signs somebody out — an intermittent, unreproducible bug that destroys trust in the product rather than merely breaking a feature.
- **`with_checkout` is a builder**, not a seventh parameter to `Account::new`: the crate's ~100 existing call sites do not subscribe, and a signature change would have edited all of them for nothing.
- **Proof the extraction stayed covered.** A green suite after a refactor can mean "behaviour preserved" *or* "nothing reaches it any more". Planted bugs 4b07 (invert the second `invalid_grant` look) and 4b09 (drop a rotated token the store refused) were both **caught**. 4b08 was **unplantable**: replacing `load_token_twice` left that method unused, which `-D warnings` rejects — which also proves `rotate_under_lock` is its only caller.

### ADR-SEC-026 — 2026-09-20 — nothing from the account service reaches the OS unchecked

Both checkout answers end with the browser opening something, and both are attacker-controlled in the sense that matters: a compromised or impersonated account service could answer with any string.

- **Neither value leaves `vault-account` as a `String`.** `TransactionId` is checked against §5's `^txn_[a-z0-9]{26}$`; `PortalUrl` against §8.30's *"`https` on `paddle.com` or one of its subdomains"*, with credentials-in-the-authority refused. Both have private fields, so an unchecked string cannot be put where a checked one belongs.
- **`vault_app::external_link::ExternalLink` is the last gate.** Three constructors (sign-in, pay, portal), all ending at one `https`-without-credentials check, private field, and **no `open(&str)` anywhere**. No Tauri command takes a URL: the frontend asks for "the subscribe link" and never supplies one, which keeps ADR-030's rule true on this path.
- **The launcher: the `open` crate, `=5.4.4`, `default-features = false`, and never its `insecure` feature** — whose own documentation says it *"restores the legacy `cmd /c start` launcher on Windows"* and *"must not be enabled when paths or URLs may be attacker-controlled."* Ours may be. `open::that`, never `open::with`: the crate cannot promise a chosen application treats a dash-leading argument as data. Pure Rust, no system libraries, so it adds nothing to any CI leg (BRD §11.7.5 justification).
- **The pay page is `https://zaaheen.com/pay/` with the trailing slash** (§8.31), built with `append_pair` so the transaction id is percent-encoded and cannot add a second parameter. `ExternalLink`'s `Debug` hides the query, because the transaction id identifies a purchase in progress.
- **Adversarially tested**, each case being a way to look right without being right: `paddle.com.evil.test`, `notpaddle.com`, `https://paddle.com@evil.test/`, `file:///…/calc.exe`, `javascript:`, `data:`, scheme-relative, host-in-the-path/query/fragment, and query/fragment injection inside the transaction id.

**§4's refresh triggers are discharged.** `Trigger::Routine` had **no production caller** until now, so a stale lease was noticed only when a call was refused — one five-second stall on somebody's first blocked action, which reads as the app hanging. `spawn_routine_refresh` refreshes on desktop open and then daily, **jittered across a six-hour window** (§4) so installs started on the same morning do not ask together forever; the Worker is on a free daily quota. `daily_period` uses `unsigned_abs`, because a clock set before 1970 with plain modulo yields a period *shorter* than a day — the one failure that would have a single install hammering the Worker. Pinned across `i64::MIN..=i64::MAX`. `Trigger::UserAction` has its callers in `subscribe` and `refresh_now`.

**Evidence.** 222 tests in `vault-account` (14 new, and the pre-existing 205 are the real verdict on the rotation refactor), 12 in `account_ops`, 8 in `external_link`, 71 in `vault-tauri`'s lib. **12 planted bugs: 9 caught, 3 unplantable** because `-D warnings` rejects the broken build (an unused method, an unreachable match arm, an unread field). Recorded as compiler-held rather than counted as coverage.

**Two defects the plants found, both of the same kind — a test whose name promised more than it delivered:**
1. **`a_transaction_id_cannot_smuggle_multi_byte_characters` proved nothing.** Bug 4b05 removed the character-count half of the length check and the test stayed green, because the charset check catches that input first. Working it through: no input can distinguish them, since a multi-byte character can never pass an ASCII-only charset check. The check is **kept** as defence in depth (SP-3) — it is what would still bound the length if the charset rule were ever loosened — and both the code and the test now **say** it is redundant today rather than implying coverage.
2. **`no_account_command_receives_the_vault` fired on its own documentation.** `account.rs`'s module doc says *"no `Application`, no adapter, no key"*. Fixed by stripping comments before scanning, exactly as `registered_commands` does — and `main.rs` already carried the same note about ADR-030's forbidden-term list. **This is the third time in one session that a source-text test read prose as code.** The rule, stated once here: **a source test must scan code, never comments, and must then be planted to prove the stripping did not leave it checking nothing.**

**Open, and deliberately not improvised (step 4d decides):** the signed-in **email is known only at sign-in**. `SignedIn` carries a `UserInfo`; the account folder does not persist it (§4 lists four files, none an identity). So a later app open knows the subject but not the address, and `AccountView::email` is `None`. §3 asks for *"Signed in as &lt;email&gt; — not you? Sign out"*, which holds immediately after signing in and not afterwards. Closing it needs either a stored address (a new file, and a §4 amendment) or a `userinfo` call when the panel opens (a round trip and a fresh access token).

**Also provisional:** `friendlyAccountError` and `friendlyAccountState` in `dist/app.js` exist so no raw code or state string can reach a person. **Their real copy is step 4d's, with the founder.**
