# "Delete my account" — design (ADR-112, ADR-SEC-035)

**Status:** DESIGN v2, session 67 (2026-09-26). v1 approved in outline by the founder, then two
independent reviews (both GO-WITH-FIXES, no NO-GO); every finding is folded in below and listed in
"Disposition of the v1 review". D8 decided by the founder: (b). **Built:** the web page and the
Worker (session 67), the review fixes and the app's side (session 68, see "Build notes, session
68"); the Rust is compiled and tested by the next full gate run.

## Context

Founder, session 67: *"we need to have delete option which wipes out the account and everything ...
add it if its a smaller piece .. before we do the big run"*. Then, choosing the page route over the
app deleting the account itself: *"ok partner we shuold also give delete my acccount button which
links t o delete request in ap"* (read as: the app's button wipes the memories and opens the delete
page; the page is also reachable from the website). Approved v1: *"yes"*.

Today (verified in code, session 67, both reviewers):
- **Delete everything** (Settings › Your memories) erases the vault, key first (ADR-SEC-008), then
  signs out locally and revokes the refresh token (`vault-tauri/src/commands/erasure.rs:156-178`).
  After a successful erasure the window closes itself 2.5 s later (`dist/app.js` ~1527-1543). With
  no vault folder it returns `erasure_failed`; a busy vault `erasure_busy`.
- **`/clerk/webhook`** handles `user.deleted` (payload only `{id}`): cancels the person's active,
  past-due and paused Paddle subscriptions immediately, 5xx on failure so Svix retries
  (`workers/account/src/routes/clerk-webhook.ts`).
- **The daily sweep** (cron 03:17 UTC, SIGNIN-DESIGN "The sweep") reads subscriptions whose period
  ends within ±48 h; it re-derives records but does not cancel for a missing Clerk user.
- **One Zaaheen account for the app and coaching.** Coaching has its own `user.deleted` handler.
- The trial start lives only in the Clerk user's `private_metadata.zaaheen_memory.trial_started_at`
  (`workers/account/src/record.ts`).
- **Clerk reverification** (docs.clerk.com/guides/secure/reverification): "Delete account" is a
  sensitive action Clerk's server refuses unless the session's first factor was verified within the
  reverification window (default 10 minutes, settable 1-10 in Dashboard › Sessions, changelog
  2026-08-28). Custom flows re-verify with `session.startVerification`. The exact error code is
  measured in the spike.
- BRD §11.4.1 lists "Permanently deleting account" among multi-factor operations (MFA off: free
  plan, AUTH-PAGES-DESIGN D2/D9). BRD §11.5.4: "Account deletion: master key is destroyed".

## ADR-112 — "Delete my account"

### D1 Where it happens, and the ways in
The Clerk user is deleted by Clerk's self-deletion (`user.delete()`, Frontend API). **We add no
server route that deletes accounts.** Our own place for it is a new page,
**`https://account.zaaheen.com/delete-account/`**. Two ways in:
- **The app:** a **Delete my account** button in Settings › Account (below *Manage subscription*)
  **and on every lock screen** (trial ended, subscription ended, signed out), because people whose
  trial ended are the likeliest to leave and can't reach Settings.
- **The website:** the Privacy Policy, Terms and Documents link to the page, for anyone without the
  app.

Once the instance setting is on (D4), Clerk also offers deletion on its own hosted account page
(`accounts.zaaheen.com`, live for OAuth consent) and in any Clerk profile component the coaching
app renders. These are further ways in, all ending at the same webhook; recorded in ADR-SEC-035.

### D2 The app's side: one command, erase then open
A new command **`delete_account_start`**, one Rust function so the ordering is testable:
1. If a vault exists, run the existing erasure unchanged. If it fails (including busy), stop and
   show the existing failure; nothing opens. If there is **no vault** (never set up, already
   erased), skip straight to 2.
2. Sign out locally and revoke, as Delete everything already does.
3. Open the fixed link (D5). The command returns the link's address whether or not the browser
   opened.

The dialog, before anything happens: *"Delete my account deletes the memories on this computer,
then opens a page where you confirm deleting your Zaaheen account. Deleting the account also
removes it from coaching bookings and cancels any subscription straight away, with no refund of the
time left. Want a copy first? Download my memories."* Then DELETE typed, then the button.

After it runs, **this path never auto-closes**: the window shows "Your memories are deleted. Finish
on the page that just opened." with the address as plain text and **Open the page again**. If the
browser could not open, the same screen says so. (Delete everything keeps its own close.)

The command is **open** (callable while locked or signed out): it joins `OPEN_COMMANDS` in the
**account slot** (`vault-tauri/src/guard.rs`), receives no vault handle, and its erasure step goes
through the same keeper path as Delete everything. Widening the locked open list is argued here as
ADR-SEC-030 amendment 1 argued its own: the account slot holds nothing from the vault, and the
command's only vault effect is the erasure the person already may run.

### D3 The page: sign in fresh, confirm, delete
- **No session:** the page shows sign-in (email code, or Continue with Google), the heading "Sign in
  to delete your account".
- **A session (fresh or not):** straight to confirm. **The page never calls `signOut()`** (spike:
  not needed, and it would touch coaching's session too). If Clerk answers the delete with
  `session_reverification_required`, the page re-verifies **in place**: "For your security, enter
  the code we just sent to <email>", `session.startVerification({ level: 'first_factor' })` →
  `prepareFirstFactorVerification({ strategy: 'email_code' })` → `attemptFirstFactorVerification`
  → the delete again, once. Measured in the spike, end to end. A Google-only account, whose only
  first factor is Google, has no email-code factor to use: the page shows its own fixed line
  ("We can't confirm it's you on this page, so we can't delete this account here. Please email
  customerservice@zaaheen.com and we'll delete it for you."), never "try again" (session 68,
  review m2). Google re-verification is still unmeasured.
- This is convenience only: **the security control is Clerk's server-side reverification**, which
  refuses a stale session whatever the page does (measured: 403 at 11 minutes).
- **Confirm:** the account's email, what goes (D2's words), DELETE typed (spaces around it ignored;
  a guard against slips, not a security control), a red **Delete my account** button.
- **Reverification demanded again after the in-place code:** the fixed failure line below; one
  re-verify per delete, never a loop, and never `signOut()` (session 68, review m5: this now matches
  the code and its tests).
- **Success:** "Your Zaaheen account has been deleted. Any subscription is being cancelled, so you
  won't be charged again. The memories on your computers are not affected by this page; to remove
  them, use Delete everything in the app." Clerk ends the session with the user (on every Zaaheen
  subdomain, coaching included).
- **Failure:** a fixed line by Clerk's error code ("We couldn't delete your account. Please try
  again, or email customerservice@zaaheen.com."); Clerk's message never shown.
- The page carries the `clerk-captcha` element, as the audit requires for pages that sign in.

### D4 Google from the delete page never creates an account
Google returns through `/sso-callback/`. The delete page passes **a fixed marker, `?after=delete`,
not a URL**; `/sso-callback/` turns it into the one constant
`new URL('/delete-account/', location.origin).href` and calls `handleRedirectCallback` with
`transferable: false`, so a Google address with no Zaaheen account can never become a sign-up. That
case shows the fixed line "There is no Zaaheen account for this Google address." The shared
`redirect_url` validator is **unchanged**: `/sign-in/?redirect_url=/delete-account/` stays refused.

### D5 The app's link
`ExternalLink::delete_account()`: a **fixed** URL, no parameters (production
`https://account.zaaheen.com/delete-account/`; a sandbox build the dev pages' origin, chosen as D7
of AUTH-PAGES-DESIGN). Nothing from the app travels in it: no email, no user ID, no token.

### D6 Billing: the webhook, and a backstop
The webhook cancels billing, as today. **Backstop (new):** the daily sweep, for each subscription it
reads (those renewing within ±48 h) whose `custom_data.clerk_user_id` Clerk answers with 404,
cancels it immediately, so a webhook that failed past Svix's retries is caught before any charge.
Logged with the existing wording pattern (a count, never an ID or email). The production webhook's
subscription to `user.deleted` is part of the post-deploy check.

**Guard (session 67, found while building):** a Clerk key from the wrong instance makes every user
answer 404, which would cancel every subscription renewing that day. So gone accounts are collected
first and cancelled after the loop, and a run that finds **more than `MAX_GONE_PER_RUN` (3)**
cancels none of them and logs `gone_guard` with the count; live users are still re-derived. Their
pending cancels stay inside the subrequest budget. Only a Clerk 404 counts; any other Clerk error
is a failure and cancels nothing.

**The guard at launch volume (session 68, review M1).** With three or fewer renewals a day the
count never trips, so two more checks come first:
- `readConfig` refuses Paddle `live` with a Clerk `sk_test_` key, and Paddle `sandbox` with
  `sk_live_` (the Worker answers 503, as for any bad config).
- Before the first cancel, one positive instance check: `GET /v1/oauth_applications?limit=100`
  must answer 200 and list our `CLERK_OAUTH_CLIENT_ID` (a wrong-instance key lists that instance's
  applications, not ours). Anything else cancels none of the gone accounts and logs `gone_guard`.
  The call is only made when someone is gone, and one call is always reserved in the budget. The
  response shape was read from the dev instance before the code was written.
- A gone account whose subscription ids all fail the pattern counts as `failed`.

### D7 Other computers, the lease, and what the website says
- Another computer signed in to the deleted account: its refresh fails with an invalid grant. It
  must show **signed out**, never "trial ended" (to verify in the spike and pin in a test).
- `/v1/lease` with a still-valid access token for a deleted user: Clerk's user lookup 404s; it must
  answer as signed out (to verify; pin in a Worker test).
- Privacy Policy "How long we keep it" and "Your rights", Terms "Closing an account", Documents:
  "Use Delete my account in the app, or go to account.zaaheen.com/delete-account/". The emailed
  route stays for anyone who can't sign in. The Privacy Policy also says Paddle keeps its own
  records of your payments as the law requires. Shipped with the switch-on of `account.zaaheen.com`,
  and only once coaching's handler is confirmed (open items), so the wording is true when it goes
  live.

### D8 One free trial per email, even after deleting (founder: (b))
Deleting the Clerk user deletes the trial start, so the same email could sign up again for a fresh
30-day trial. I recommended accepting that for launch; the founder chose to close it: *"b partner
we do it properly from day 1 .."*.

- **Where the trial starts today:** the first `/v1/lease` for a user with no `trial_started_at`
  writes it (SIGNIN-DESIGN §8.26 §3). That is the one place the check goes; no new webhook event.
- **The fingerprint:** `HMAC-SHA256(TRIAL_KEY, normalise(email))`, hex. `TRIAL_KEY` is a new
  Worker secret (32 random bytes, generated offline, never in the repo), so a leaked store cannot
  be reversed by guessing emails. `normalise`: trim, lower-case; drop a `+tag` from the local part
  for every domain; for `gmail.com` and `googlemail.com` also drop dots and map to `gmail.com`.
- **The store:** a new Workers KV namespace `TRIALS` (free plan: ample for our volume). Key = the
  fingerprint; value = the original trial start (ISO time); **expires two years after that start**
  (`expirationTtl`), after which a returning person gets a new trial.
- **At the first lease** (no `trial_started_at` on the Clerk record): read the email from the user
  the Worker already fetches; look up the fingerprint. Found → write that earlier start into the
  record (the trial continues from its first day, usually already over). Not found → start now and
  put the fingerprint. A KV failure fails closed for the lease call (retryable 503), never granting a
  fresh trial on an error, and never logging the email or fingerprint.
- **Records and logs:** the Worker never stores or logs the plain email; the fingerprint appears in
  no log line.
- **Privacy Policy (with D7's wording):** a row "A scrambled fingerprint of your email address and
  the date your free trial started | So each person gets one free trial | Our legitimate interest in
  preventing misuse", and under "How long we keep it": kept for two years after your trial started,
  even if you delete your account, and it can't be turned back into your email address.
- **Tests (Worker, written first):** first lease stores the fingerprint and starts now; a second user
  with the same email (and with `+tag`, dots on Gmail, different case) inherits the first start; a
  different email starts fresh; the KV write carries the 2-year TTL; a KV error → 503, no record
  change; no log line contains the email or fingerprint; `comp_until` still wins as today.
- **Residual:** a person using a genuinely different email address gets another trial. Accepted;
  that costs them a new inbox and starting their memories from nothing.
- **Decided while building (session 67):** a user with no primary email gets 503 on the first lease
  (nothing to check); a stored start later than now is clamped to now, so it can never lengthen a
  trial; an unreadable stored value is 503. `TRIAL_KEY` is read as text (at least 32 characters) and
  **must never be rotated**: a new key forgets every earlier trial. Trials started before D8 deploys
  have no fingerprint (a handful of testers).
- **Deploy order:** a missing `TRIAL_KEY` makes the whole config incomplete, so every route answers
  503. Create the `TRIALS` namespaces and put `TRIAL_KEY` **before** deploying this code. No workflow
  deploys the Worker (manual `wrangler deploy` only). The two namespace ids in `wrangler.jsonc` are
  placeholders marked "NOT CREATED YET"; the commands are in HANDOFF.

### Instance invariants (added to AUTH-PAGES-DESIGN D9 and the post-deploy check)
"Allow users to delete their accounts" **on**; the reverification window **5 minutes**; the
production webhook subscribed to `user.deleted`. Applies to production, "Zaaheen pages (dev)" and
the sandbox instance the sandbox app uses (NIT: name which instance the sandbox link reaches).

## ADR-SEC-035 — account self-deletion on our own origin (amends ADR-SEC-034)

**What an attacker gains:** deleting someone's account needs a session whose first factor was
verified within the last 5 minutes, enforced by Clerk's server. The takeovers ADR-SEC-034 already
names (a relayed email code, a tampered page) reach it; so does script injection on **any** origin
holding a fresh Clerk session within the window, which now includes coaching and Clerk's hosted
account page (D1). Deletion is destructive but bounded: it never reaches a vault (sign-in never
touches the vault key), billing is cancelled not charged, and the person can create a new account.
Malware on the computer can't use the app's OAuth tokens here (they are not a Clerk browser
session); malware that drives the browser is bounded by the same 5-minute window.

**Changes to ADR-SEC-034:** none to the `redirect_url` validator (D4 uses a marker, not a URL); one
new constant completion on `/sso-callback/` only; `transferable: false` on that path; no CSP change
(`user.delete()` goes to `https://clerk.zaaheen.com`, already in `connect-src`); the page is
`noindex` and framing is refused by the origin's headers.

**Recorded divergences from BRD §11:**
- §11.4.1 multi-factor for account deletion: met by a fresh first factor within 5 minutes
  (re-authentication), not a second factor. Same residual as AUTH-PAGES D2 and SIGNIN §8.39.
- §11.5.4 "Account deletion: master key is destroyed": true for the app's route (D2 erases first);
  the website route deletes only the account, and other computers keep their own vaults, by design
  (the account never holds or unlocks a vault).
- §11.9.1 audit logging: the app's erasure writes its local log line; the Worker's
  `clerk_webhook user_deleted cancelled N` line records the billing side.

**Explicitly unchanged:** the app's PKCE sign-in, tokens and lease; Delete everything's order,
wording and close; the Worker's routes (only the sweep gains the backstop).

## Tests, written first (floor)
- **Page state machine** (pure module): no session → sign-in; a session → confirm; reload on
  confirm → confirm; `session_reverification_required` → the in-place code step, then one retry
  (never a loop; a second refusal → the fixed line); no path calls `signOut()`; DELETE (spaces around it ignored) must match before the button enables; each Clerk error code → the fixed line;
  `user_delete_self_not_enabled` → the fixed line; Google with no account → the fixed "no account"
  line and no sign-up.
- **`/sso-callback/`:** `after=delete` → exactly the constant; any other `after` value refused;
  `/sign-in/` and `/sign-up/` still refuse `redirect_url=/delete-account/` (and the absolute form);
  a dev build with `__clerk_db_jwt` still correct.
- **`audit-account`:** the page exists, `noindex`, no inline script, has `clerk-captcha`, button
  disabled until DELETE; planted-bug controls for each.
- **Rust:** `delete_account()` is exactly the fixed URL; `delete_account_start` erases then opens,
  opens nothing on a failed or busy erasure, opens with no vault; it is in `OPEN_COMMANDS`, in the
  account slot, and receives no vault (the `no_account_command_receives_the_vault` style test); its
  Tauri permission exists (`frontend_contract`); the dialog words; the lock screens show the button;
  this path does not auto-close and shows the address when the browser didn't open.
- **Worker:** the sweep cancels a renewing subscription whose Clerk user is 404, and leaves one whose
  user exists; `/v1/lease` for a deleted user answers signed out.
- **Live (dev instance + sandbox Worker):** sign in, delete, the user is gone (Backend API); the
  webhook path tested on the sandbox instance, whose webhook reaches the sandbox Worker.

## Spike results (session 67, runtime, "Zaaheen pages (dev)")

Our real account pages built in development mode (`ACCOUNT_DEV=1`, the dev app's `pk_test_`),
served by `scripts/serve-account.mjs` with the pinned production headers, clerk-js 6.34.1, driven
with Playwright; `+clerk_test` addresses, code 424242. The founder had already switched on "Allow
users to delete their accounts". (Clerk's `config pull` exposes neither that switch nor the
reverification window: they are dashboard-only, so the post-deploy check measures them by
behaviour.)

| # | Measured | Result |
|---|---|---|
| S1 | `user.deleteSelfEnabled` after sign-up | `true` |
| S2 | `session.factorVerificationAge` | `[0, -1]` right after sign-up (minutes since first factor; second factor never) |
| S3 | `user.delete()` 11 minutes after sign-in | **403 `session_reverification_required`** (`meta` empty) |
| S4 | In-place re-verify: `startVerification({level:'first_factor'})` → factors `[email_code]` → prepare → attempt | `complete`; age back to 0 |
| S5 | `user.delete()` after S4 | ✅ deleted; `Clerk.user` and `Clerk.session` null; **no navigation** |
| S6 | Backend API lookup of the address afterwards | 0 users |
| S7 | CSP violations across S1-S5 | none; no source added |
| S8 | `signOut()` navigation | not measured cleanly (the bot check stalled a later sign-up); moot: D3 no longer signs out |

**Browser test of the built page (session 67, same setup):** a test user created by the founder
through the Backend API (the bot check had started stalling automated sign-ups after several quick
ones, so sign-up was not the way in); `/delete-account/` → "Sign in to delete your account." →
email code → confirm showing the right email with the button disabled; lowercase "delete" kept it
disabled; DELETE enabled it; delete → "Your account has been deleted.", `Clerk.user` null, no
navigation; the Backend API then found no user with that address; no CSP violation.

Still to measure at the browser test: Google re-verification, `transferable: false` for an
unknown Google address, another computer's refresh after deletion (sandbox). Test users removed
(none left; the instance's one other user predates the spike).

## Build order
1. ✅ **Spike (runtime, dev instance)** (results above): self-deletion on; `user.delete()` from our page under the
   pinned CSP; the reverification error code after 5 minutes; `factorVerificationAge`'s shape;
   whether `signOut()` navigates by itself; `transferable: false` behaviour for an unknown Google
   address; what another computer's refresh sees after deletion.
2. ✅ (session 67) Tests first, then the page, `/sso-callback/`'s marker, the audit; the Worker's sweep backstop
   and D8 (the KV namespace and `TRIAL_KEY` secret are created by the founder with the dashboard
   or `wrangler`, never by me pasting a secret).
3. The app's command, dialog, lock-screen button and link, with the next full gate run.
4. Production at the switch-on: the Clerk settings, the webhook check, D7's website wording.

## Build notes, session 68 (the app's side, and the review of the web build)

Decisions made while building, recorded here as amendments to ADR-112 / ADR-SEC-035:
- **Two commands, not one.** `delete_account_start` (erase, sign out, open) and
  `delete_account_open` for **Open the page again**: it takes no argument, erases nothing, holds only
  the account. Both join `OPEN_COMMANDS` in the account slot (guard.rs, exact-list test updated).
- **The page is asked for first.** `delete_account_start` builds the link before anything else; a
  build with no account or no page answers `account_unavailable` / `account_refused` and erases
  nothing. Then erase (only when a vault location was ever recorded: `KeeperLink::vault_recorded`;
  a recorded folder that is missing, e.g. an unplugged drive, still runs the erasure, which then
  fails as Delete everything does, so the page never opens over memories that still exist), then
  sign out (best effort), then open. Returns `{page, opened, erased}`.
- **The one address the webview receives** is the fixed page, as display text; no command takes it
  back (commands/account.rs module docs amended).
- **A sandbox build's page:** our pages are not hosted for a development instance, so a development
  issuer `https://<name>.clerk.accounts.dev` gets Clerk's hosted profile page
  `https://<name>.accounts.dev/user` (checked in session 68: it answers 200 where an unknown path
  answers 404); it offers deletion once self-delete is on. Production:
  `https://account.<domain>/delete-account/`. Both from `AccountConfig::delete_account_page`, the
  same issuer naming as `sign_up_page`.
- **The dialog** is its own overlay (Cancel focused, Esc cancels until it has run, DELETE typed
  enables the button, "Download my memories" inside it). After it runs, the finish panel stays
  ("Your memories are deleted. Finish on the page that just opened." / "Your browser did not open,
  so go to the address below to finish."), with **Close Zaaheen** and **Open the page again**.
  Exercised in the UI harness: dialog, finish panel, busy failure, browser not opened, no vault.

Review of the web build (session 67) fixed: M1 (D6 guard, above), m1 (`session_exists` on the delete
page goes to confirm), m2 (no email-code factor: its own line, D3), m3 (`FIXED_LINE` stays the
default; only delete paths say "couldn't delete"), m5 (D3 text), NITs (DELETE spaces, `+x@` emails,
gone-without-ids counted as failed). Left: the re-verify view has no "send a new code" (a reload
works), in Tech debt.

## Disposition of the v1 review
Both reviewers: GO-WITH-FIXES.
- **Google looped (both, BLOCKER)** → D3 freshness check replaces "sign out on load".
- **The validator amendment didn't fit and was too broad (both, MAJOR)** → D4 marker; validator
  unchanged.
- **Google could create an account (B M4, A M1)** → D4 `transferable: false` and a fixed line.
- **Reverification is the real control (A M2, B B1)** → stated in D3 and ADR-SEC-035; window 5
  minutes as an invariant; the stale case goes back to sign-in.
- **"Only on this page" false (both, MAJOR)** → D1 lists the other ways in; ADR-SEC-035 names them.
- **Gating unspecified; locked people can't reach it (both)** → D2 open command, account slot, lock
  screens.
- **The window closes after erasure (both)** → D2 never auto-closes on this path; address shown.
- **"Opens only after erasure" untestable as two commands (A)** → one command.
- **No vault → no page (A)** → D2 opens anyway.
- **Webhook alone (B M6)** → D6 sweep backstop.
- **Trial restarts (A M4)** → D8, founder chose (b): an HMAC fingerprint in Workers KV.
- **No refund of remaining time; Paddle keeps records (A)** → D2 dialog, D7 Privacy wording.
- **Divergences §11.5.4, §11.9.1; other computers; lease (both)** → ADR-SEC-035, D7.
- **Webhook live test impossible on the pages' dev app (A)** → tested on the sandbox instance.
- **Coaching's bookings a prerequisite for the wording (B m6)** → D7 gates the wording on it.

## Open items
- Confirm the coaching app's `user.deleted` handler removes or anonymises bookings, and whether it
  renders a Clerk profile with a delete control (its own repo).
- The Worker's `user.created` consent copy (AUTH-PAGES-DESIGN D6): built in session 68 (unrelated;
  D8 lives in `/v1/lease`, not a webhook).
- The desktop's DELETE check is exact, as Delete everything's is; the web page ignores spaces around
  it. Deliberate: the desktop keeps one rule for both of its destructive buttons.
