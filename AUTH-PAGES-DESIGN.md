# Zaaheen — our own account pages

**Status: DRAFT v2 for the founder (session 61, 2026-09-24).** ADR-109 (architecture) and ADR-SEC-034
(security). Nothing here is built. v1 went to two independent read-only reviewers, one on security
and one on feasibility; both returned **GO-WITH-FIXES**, with three blockers each, and they converged
on the same gaps (a Google return page, Clerk's own navigation bypassing our check, the consent-page
shape being sandbox-only). Every finding is resolved below; the disposition table at the end maps
each one (A = security reviewer, B = feasibility reviewer) to where it is answered. v1 was
revised in place and is not kept separately.

**Founder decisions (session 61):**
- Host the account pages ourselves: *"the auth pages we will have to host on our pages that is the
  correct way of doing it"*.
- No passwords: *"for sign in stay without passwords ... what ever is best practise and secure"*.
- The look comes from the Claude Design handoff (`Auth Pages.dc.html`, pages 1a–1f): the 50/50
  split, form on the left, a small animated app window on the right. Its password fields, the two
  password pages (1c, 1d) and the password copy do not apply (D2).
- The consent box wording (D6): *"yes that wording is fine partner"*.
- Page wording chosen by me, reviewed at the pre-launch check (HANDOFF §3 item 9).
- The subdomain, confirmed after the pages were built (session 65: *"ok lets keep the subdomain
  setup"*). With the header wiring, `zaaheen.com/sign-in` and `/sign-up` forward to the account
  origin (a redirect only; no form on zaaheen.com).

## Context

- **Today** the app's sign-in (SIGNIN-DESIGN §8.26 §3: PKCE, loopback `127.0.0.1:<port>/callback`,
  `state` and `iss` checks; `crates/vault-account/src/signin/mod.rs:106-154`) opens Clerk's
  `/oauth/authorize`. A signed-out browser is sent to Clerk's hosted Account Portal. "Create an
  account" opens the Portal's sign-up page with `redirect_url` = the authorize URL (§8.41). The
  Portal takes a logo and colours only.
- **The Clerk sandbox instance** (read 2026-09-24): passwords off; email sign-in `email_code` only,
  `verify_at_sign_up`; first and last name required; Google on; passkeys on (need Pro at launch);
  bot protection on (`smart`); enumeration protection `bulk`; lockout 10 tries / 60 min;
  `legal_consent` off; DCR and client-ID metadata documents off (Clerk's defaults).
- **The site** is static Astro on Hostinger behind Cloudflare. Site-wide CSP: our origin only
  (`public/.htaccess`). `/pay` has its own CSP with Paddle's CDN and `'unsafe-inline'` styles
  (`public/pay/.htaccess`), pinned by `scripts/audit.mjs`.

## Spike (session 61, runtime, sandbox; founder-approved, fully reverted)

A local page on `http://127.0.0.1:4400`; the sandbox's `paths.sign_in/sign_up` and
`development_origin` pointed at it; an authorize URL built as `vault-account` builds it, opened
signed out. All three changes were reverted within the hour and the whole instance config diffed
equal to a copy taken before (`C:\Projects\MemoryVault-artifacts\auth-pages-spike\`).

1. Paths alone changed nothing: a sandbox instance needs `development_origin` too.
2. With it: `/oauth/authorize` → `<our origin>/sign-in?redirect_url=<FAPI>/oauth/authorize-with-immediate-redirect?<the app's query>`.
3. In **sandbox mode only**, clerk-js followed that URL itself (`instanceType !== production`,
   reviewer B read it in the bundle) and Clerk came back with `redirect_url=<Portal>/oauth-consent?…`.
4. Custom sign-up with an email code completed on our page, no Clerk UI, bot check passed.
5. The hand-back reached `http://127.0.0.1:53999/callback?code=…&iss=<issuer>&state=<the app's state>`.
   **The app's sign-in is unchanged.**

**The spike loaded clerk-js from Clerk's CDN, not bundled** (reviewer B). Build step 1 repeats items
4–5 with the bundled script under the pinned CSP before anything else is built (D3).

## Build step 1 — measured (session 65, 2026-09-26)

On the second Clerk app "Zaaheen pages (dev)" only (D9), configured to match the real sandbox
(passwords off, names required, passkeys, MFA optional, email code, Google, bot check `smart`)
plus `paths.sign_in/sign_up`, `development_origin = http://127.0.0.1:4400`, `legal_consent` on,
and one public, PKCE, consent-screen, loopback-only OAuth app. The real sandbox was read first and
is unchanged (identical to the session-61 `before.json`). Measurement pages and a server sending
the D5 headers, byte for byte, with a `securitypolicyviolation` listener:
`C:\Projects\MemoryVault-artifacts\auth-pages-step1\` (`server.mjs`, `src/flow.js`, `tt.js`).
clerk-js **6.34.1** (npm; 6.33.0 was the writing-time version). Every flow was started as the app
starts it (PKCE, `state`, loopback `127.0.0.1:53999/callback`) and ended at the loopback with a
`code`, the right `iss` and a matching `state`.

| # | Measured | Result |
|---|---|---|
| M1 | Bundled clerk-js under the pinned CSP (spike items 4-5 again, no CDN) | ✅ Both builds. **A** Vite-bundled `clerk.mjs`: 568,502 B gzipped, one file. **B** self-hosted `clerk.browser.js`: 81,973 B gzipped; sign-in and sign-up loaded only that file (its lazy chunks, UI and wallet code, were never requested). Neither contains `eval(` or `new Function`; both carry wallet SDK code (Coinbase, Solana) that never runs here. |
| M2 | What the no-UI flow needs from the CSP | **One thing.** With `require-trusted-types-for 'script'` Clerk cannot insert Turnstile's `api.js`, so every sign-up fails closed with `captcha_invalid` (sign-in, which shows no challenge, works). Fixed without widening any source: see amendment S1-1. Clerk's blob Worker is also refused; Clerk logs "Cannot create worker from blob" and carries on (every flow completed). No `worker-src`, no `img.clerk.com`, no `'unsafe-inline'` needed. |
| M3 | Email-code sign-up and sign-in on our page | ✅ Both builds; the bot check passed invisibly (`smart`); `+clerk_test` addresses, code 424242. |
| M4 | Already signed in on load | ✅ `Clerk.user` set on load; "Continue" + `buildUrlWithAuth` reached the loopback. |
| M5 | Terms and consent recorded | ✅ Server-side `legal_accepted_at` set; `unsafe_metadata.marketing_consent` carried. `legal_consent` **cannot be switched on without `terms_of_service_url`** (Clerk 422): the Terms and Privacy pages are a hard prerequisite, not a nice-to-have. |
| M6 | Google through `/sso-callback/` (the founder's own Google account) | ✅ after one new state. Google from the **sign-in** page for a person with no account becomes a sign-up with `missing_requirements` = `["legal_accepted"]` (Google supplied both names). Our page ticked the Terms box and called `signUp.update({ legalAccepted: true })` → `complete` → loopback ✅. See amendment S1-2. |
| M7 | D7: does the Portal's `/sign-up` forward to our page? | **Partly:** it forwards to our **`/sign-in/`**, not `/sign-up/`. Amendment S1-3. |
| M8 | Dev-only navigation by clerk-js | Reproduced spike item 3: on load, clerk-js on our page followed `authorize-with-immediate-redirect` itself and came back to our page with `redirect_url=<Portal>/oauth-consent?…&__clerk_db_jwt=…` (dev instances only, as D4 says). |
| M9 | A subdomain for production `paths.*` | Clerk's docs state the rule for the production **OAuth consent** URL: `https`, same registrable domain as the instance, subdomains allowed. The sign-in/up rule is not stated separately; confirmed at the production setup (unchanged from D1's fallback). |

**Amendments from step 1 (to be reviewed with the build, before its commit):**
- **S1-1 (D5, ADR-SEC-034): one Trusted Types policy instead of dropping Trusted Types.** The
  origin's CSP adds `trusted-types default`; every page loads `/tt.js` first (a static file, `'self'`)
  which creates the only policy the origin can have. Its `createScriptURL` returns the input only if
  it equals `https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit` exactly, and
  throws for anything else; `createHTML` and `createScript` always throw. Measured: sign-up passes the
  bot check with zero violations; the blob Worker is refused through the policy and Clerk carries on.
  Dropping Trusted Types was the alternative and was rejected: it would re-open every string-to-script
  sink on the page to save one line. Tests: the audit pins `tt.js` word for word and its first place
  on every page; a browser test proves another script URL and `innerHTML` are refused.
- **S1-2 (D1 states):** the state list gains **"Terms not yet accepted"**: after Google, `missing_requirements`
  with `legal_accepted` shows the Terms line and box (and the unticked tips box), then
  `signUp.update({ legalAccepted: true, unsafeMetadata })`. Missing names and missing Terms can come
  together; one screen asks for whatever `missingFields` lists. Any other missing field → the fixed line.
- **S1-3 (D7):** decided: `sign_up_page()` returns our own page (`https://account.zaaheen.com/sign-up/`
  in production, from the site-origin constant; the development pages' origin for the sandbox), with
  the authorize URL as `redirect_url`, as D7 already describes for this outcome.
- **S1-4 (D3):** build **B** (self-hosted `clerk.browser.js`) is chosen: 82 KB instead of 569 KB on a
  page people open just to sign in. Its chunk loader can only load from our origin (`currentScript`
  base) and, under S1-1, cannot insert any script at all, so a flow that ever needs a chunk fails closed
  and the browser test catches it. The pin moves to the version current at the build, exact, with
  its lockfile. D3's "Which build is served is measured at build step 1" is answered.
- **Clean-up done:** the three test users on "Zaaheen pages (dev)" (two `+clerk_test`, one the
  founder's Google address) deleted by the founder's command ("deleted 3 test users; left on the
  practice app: 0"); the app's settings stay as configured for step 2.

## Build step 2 — the pages (session 65, 2026-09-26)

Built tests first: `site/scripts/redirect.test.mjs` (D4, 23 tests), `signin-state.test.mjs` (D1 + S1-2,
16), `account-source.test.mjs` (source rules, 7) and `audit-account.test.mjs` (32 negative controls
on a copy of the real build). Each validator rule that is the only guard against an attack was shown
to fail its tests when removed (13 planted bugs; one test added when a plant was missed; the
backslash check is a second layer, its cases are also refused by the raw-path and `state` rules).
Layout: `site/account/` (pages, components, `scripts/redirect.js`, `account-state.js`, `account.js`,
`csp.js`, `clerk-pin.js`), `site/account-public/` (`tt.js`, `robots.txt`, `favicon.svg`, the Clerk file),
`site/astro.account.config.mjs` (fails the build without valid settings; writes `.htaccess` from
`account/htaccess.template` and `csp.js`), output `site/dist-account/`. `npm run build:account`,
`check:account` (the audit), `build:account:test` + `test:account` (CI, with made-up development
settings; `site.yml`). `scripts/serve-account.mjs` serves a build with its `.htaccess` headers.

**Browser test on "Zaaheen pages (dev)"** (real pages, pinned headers, an app stand-in doing PKCE
with a 43-character `state` and a loopback listener; `C:\Projects\MemoryVault-artifacts\auth-pages-step1\app-standin.mjs`):
sign-up from the app's "Create an account" (our `/sign-up/` directly, S1-3) and from the sign-in
page's link; the Terms box refused unticked; a wrong code gets our line; a reload resumes at the code
step; sign-in of a returning person; a tampered `redirect_url` shows only the bad-link view; signed
in with no `redirect_url` shows "You're signed in" and the zaaheen.com line; phone width (390 px).
The founder, in Edge: email-code sign-up with a real inbox; Google as a new person (the "One more
step" Terms view, S1-2) and Google as a returning person (straight to consent). Every run reached the
loopback with a `code`, the right `iss` and a matching `state`; no CSP or Trusted Types violation in
any run. Test users deleted afterwards.

**Observed, development only:** after a sign-up from the app's `/oauth/authorize` link, Clerk's
development instance sent the browser back to our `/sign-in/` once more, which showed "You're signed
in" + Continue (the D1 already-signed-in state) before consent. Production's session cookie is on our
own domain; the production test confirms the extra step does not appear there.

**Amendment S2-1 (D3): Clerk's file is vendored, not an npm dependency.** `@clerk/clerk-js` pulls
wallet SDKs (Coinbase, Solana) into the lockfile that never run on these pages; `npm audit` counted 13
moderate advisories in them, which would sit as GitHub alerts on the public repo. The pages serve one
file, so `scripts/vendor-clerk.mjs <version>` fetches the tarball, checks the registry's sha512
integrity, and writes `dist/clerk.browser.js` as `account-public/clerk/clerk.browser.<version>.js`
(6.34.1: 313,762 bytes, byte-identical to the file measured in step 1). Its SHA-256 is pinned in
`account/scripts/clerk-pin.js`; the audit refuses any other bytes or any other file in `clerk/`;
`.gitattributes` keeps it byte-exact in git. Upgrading = run the script, paste the new pin, repeat the
browser test. This replaces D3's "committed lockfile" for this one file with a stronger pin.

**Independent review of step 2 (session 65, read-only): SAFE WITH FIXES**, no blocker or major;
about 30 hostile `redirect_url` inputs refused or harmless; `tt.js`, DOM handling, fail-closed settings
and headers found sound. Dispositions:
1. *A finished Google sign-in is navigated by Clerk itself* (to its redirect props, not through our
   `navigate`, and without checking `allowedRedirectOrigins`). Safe because all four props are `done`
   (validator output or our own `/sign-in/`); now pinned by `account-source.test.mjs`, comments
   corrected. **Residual:** a pending session task after Google is handled by Clerk's own navigator,
   not our fixed line; no task is configured on the instance (D9 invariant).
2. *A pk_test_ build could be deployed:* the build now refuses a `pk_test_` key unless `ACCOUNT_DEV=1`;
   the deploy job (to be built) runs the `--release` audit.
3. *The CSP text was not pinned* (only `.htaccess` = `csp.js`): a word-for-word test of `csp.js` added.
4. *The audit read only `Header always set` lines:* the whole server file is now pinned
   (`HTACCESS_SHA256`, the template with `{{CSP}}`); three new negative controls (unset, a renamed
   `<IfModule>`, an added rewrite).
5. *MFA is optional on the instances, but the pages cannot do a second factor* (a person who enrols one
   would be locked out): D9 invariant added, MFA off on every instance these pages serve; turned off on
   the practice app by the founder's command. Revisit with passkeys (D2).
6. *The app still sends "Create an account" to the Portal:* S1-3 (`sign_up_page()`) must ship before or
   with the pages going live, and the Portal is not retired before it (step 3, with the full gates).
7. *The vendored file's pin is only checked against itself:* `vendor-clerk.mjs --check` in `site.yml`
   re-fetches npm's published file and requires it equals the pin and the committed file.
8. Nits fixed: `clerk.load()` times out after 20 s to the fixed line; "We sent a new code." is a status
   note, not an alert; the Terms and Privacy links open in a new tab; `serve-account.mjs`'s path check.

Re-run after the fixes: 48 unit tests, 35 audit negative controls, `--check` against npm, all pass; the
reviewer's scenarios planted one at a time each fail a test.

**New prerequisite for production (founder, session 65: "it says clerk on google shouldnt it say
zaaheen?"):** the development instance uses Clerk's shared Google credentials, so Google's screen
names Clerk. Production needs Zaaheen's own Google OAuth client (Google Cloud console: app name
Zaaheen, logo, zaaheen.com as the authorised domain, links to the Terms and Privacy pages; basic
scopes only, so no sensitive-scope review), its client ID and secret set on the production instance.
White-label rule (HANDOFF §5).

## ADR-109 — the account pages

### D1 Where, and which pages
The pages live on **their own origin, `account.zaaheen.com`**, not under `zaaheen.com/`. Same Astro
repo, a separate build output, its own Hostinger site and `site-deploy`-style branch. Reason
(reviewer A #5): on the same origin, any script on any zaaheen.com page (today Paddle's CDN on
`/pay`, tomorrow anything) could open the sign-in page and read it, and folder-scoped CSPs do not
stop that. A separate origin puts the sign-in pages outside that reach. Build step 1 confirms Clerk
accepts a subdomain of the registrable domain for `paths.*` in production; if it does not, fall
back to `zaaheen.com/sign-in/` with `Cross-Origin-Opener-Policy: same-origin` and a recorded
residual naming `/pay`.

All pages `noindex`, none in a sitemap, `X-Robots-Tag: noindex` on the whole origin.

| Path | Does |
|---|---|
| `/sign-in/` | Email → code step (same page). "Continue with Google". Link to sign-up. |
| `/sign-up/` | Trial badge; first name, last name, email; consent box (D6); Terms/Privacy line → code step. "Continue with Google". Link to sign-in. |
| `/sso-callback/` | Google's return: `Clerk.handleRedirectCallback()`; a `clerk-captcha` element (a Google sign-up is created here); the missing-names state (below). Carries the validated `redirect_url` through Google and back. |

**States each page handles** (a pure state machine module, unit-tested per status, reviewer B #6):
the code step; `missing_requirements` (Google gave no name: ask for first and last name, then
continue); **already signed in on load** (back button, second sign-in: show "Continue" to the
validated URL; `signIn.create` refuses while a session exists); an in-progress attempt found on
reload ("Enter the code we sent to …" or "Use a different email"); resend with a 30-second
cooldown; `needs_protect_check` and any status not listed → a fixed "Something went wrong. Open the
Zaaheen app and press Sign in again."; `setActive` leaving a pending session task → the same line.
`needs_second_factor` and `needs_client_trust` cannot arise (no MFA enrolment; device trust applies
to passwords only) and map to the same fixed line.

With **no `redirect_url`** (someone signing in on the website itself) success shows "You're signed
in" with the email and a link to the download page on zaaheen.com.

**Not built:** 1c, 1d (no passwords); 1e "Signed out" (the app signs out by revoking its own
tokens; no browser page opens); a web account area. 1f is the app's own loopback page (D8).

### D2 No passwords
Sign-in is `email_code`, Google, and passkeys if Pro is bought. This follows BRD §11.4.1's "no
passwords for new accounts" and the session-43 instance. The design's password copy is dropped.

Recorded divergences from BRD §11.4.1 (reviewer A #4):
- **Passkeys are not primary.** §11.4.1 makes passkeys primary because they are phishing-resistant;
  email codes are not. Passkeys need Clerk Pro; the choice (buy Pro at launch, or ship email code +
  Google) is HANDOFF §3's, not this design's. Until then the residual in ADR-SEC-034 applies.
- **The 15-minute IP-bound magic link** is Clerk's 10-minute single-use email code instead.
- **"Multi-factor for sensitive operations"** (subscription changes, deletion) is unmet, as already
  recorded for export in SIGNIN-DESIGN §8.39; this design adds no sensitive operation.

### D3 How the pages talk to Clerk
- `@clerk/clerk-js`, **exact version pinned** (6.33.0 at writing), committed lockfile, no Clerk
  CDN script, no `@clerk/ui`. Which build is served is measured at build step 1: the Vite-bundled
  `dist/clerk.mjs` (~570 KB gzipped, one chunk; no `eval`, no `new Function`, CSSOM-only styling)
  or the self-hosted `dist/clerk.browser.js` (~81 KB gzipped) with its chunks from our own origin.
  `no-rhc` is not usable (it drops the bot check).
- The classic custom-flow API (`client.signIn`, `client.signUp`, `setActive`,
  `authenticateWithRedirect`, `handleRedirectCallback`); Clerk now labels it "legacy (Core 2)". The
  exact pin is what protects us; recorded as a known upgrade risk.
- **One build setting, `PUBLIC_CLERK_PUBLISHABLE_KEY`.** The Frontend API host is decoded from it at
  build time (a `pk_` key is base64 of `<fapi>$`), so the key and the host can never disagree
  (reviewer B #2). Added to `site.yml` as a repository variable. The release audit refuses a key
  that is not `pk_live_` or that does not decode to `clerk.zaaheen.com`. An empty or undecodable key
  fails the build, never matches anything (reviewer A #11).
- `Clerk.load({ telemetry: false, allowedRedirectOrigins: [<FAPI origin>] })` — in development
  builds only, plus the Portal origin. This is the backstop for navigation Clerk does itself, which
  our validator cannot see (both reviewers).
- Fixed error lines keyed by Clerk's error `code`; Clerk's `message` never shown; all text through
  `textContent`. Every form handler calls `preventDefault`.

### D4 Where the page may send the browser
On **load**, before any email is collected, the page validates `redirect_url`. A bad link never
reaches a session: the page shows "This sign-in link isn't valid. Open the Zaaheen app and press
Sign in again." and nothing else (reviewer A #2).

`redirect_url` is accepted only if, parsed with `new URL()`:
- exactly one `redirect_url` parameter on our page; no fragment on it;
- scheme `https:`, no username or password, default port, host **exactly** the FAPI host (from D3);
- path **exactly** `/oauth/authorize-with-immediate-redirect` or `/oauth/authorize`;
- its query holds **exactly one** each of: `client_id` equal to the app's client ID (a second build
  setting, public, from the same account file as the app); `redirect_uri` matching
  `^http://127\.0\.0\.1:[1-9][0-9]{0,4}/callback$`; `response_type=code`;
  `code_challenge_method=S256`; a non-empty `state` and `code_challenge`; `scope` as the app sends
  it. No other parameter except, in development builds, `__clerk_db_jwt` (reviewer A #1: host and
  path alone would let an attacker's own client or redirect ride on our trusted page).
- **Development builds only**, additionally: host exactly the Portal host, path exactly
  `/oauth-consent`, the same query rules (spike item 3 is a sandbox artefact; reviewer B #4, A #9).

The page then calls `buildUrlWithAuth(validated.href)`, **validates the result again**, and
navigates with `location.assign(parsed.href)`, never the raw string (reviewer A #2). A
`redirect_url` is never passed to clerk-js except as validator output (`redirectUrlComplete` for
Google, carried through `/sso-callback/`). The validator is one pure module with its own tests.

Instance invariants (D9): exactly one OAuth application, loopback-only; Dynamic Client Registration
and client-ID metadata documents off; MFA (authenticator app, backup codes) off and no session tasks,
because these pages cannot complete a second factor or a task (step-2 review, finding 5). A config-check script compares the instance against these and
the build step fails the live test if they drift.

### D5 The origin's CSP
One policy for the whole `account.zaaheen.com` origin, in its `.htaccess`, pinned word for word in
the audit. The FAPI host in it is the literal production host `https://clerk.zaaheen.com` (public
DNS, not an identifier; reviewer B #2); a development build writes its own sandbox host and the
audit pins the production form on release only.

`default-src 'self'; script-src 'self' https://challenges.cloudflare.com; connect-src 'self'
https://clerk.zaaheen.com; frame-src https://challenges.cloudflare.com; img-src 'self' data:;
style-src 'self'; font-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'none';
form-action 'self'; require-trusted-types-for 'script'`, plus `Cross-Origin-Opener-Policy:
same-origin`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, HSTS with
`includeSubDomains`.

**Clerk Protect, decided now** (reviewer A #7, B #5): Protect is script chosen by Clerk's server,
run on our origin. It is **not** allowed in the policy. If the instance ever demands it,
`needs_protect_check` shows the fixed line and sign-in fails closed rather than widening the CSP.
Changing this is a reviewed ADR-SEC amendment. `worker-src`, `img.clerk.com` and
`'unsafe-inline'` are added only if build step 1 shows the no-UI flow fails without them, each
with the failure written into the `.htaccess` comment, as `/pay` does. Turnstile's `api.js` runs on
our origin (it is not only a frame); that is named in ADR-SEC-034, not hidden.

`astro preview` does not apply `.htaccess`, so the measurement uses a local server that sends the
pinned headers, and a `securitypolicyviolation` listener fails the browser test on any violation.

### D6 Marketing consent
`/sign-up/` has one unticked box: "Send me occasional tips and product news. You can unsubscribe at
any time." (founder-approved). The choice goes into the sign-up as `unsafeMetadata.marketing_consent`
(also through `authenticateWithRedirect` for Google). `unsafeMetadata` is user-writable, so it is
only the carrier (reviewer A #8, B #10): the account Worker's `user.created` handler copies it into
`private_metadata.marketing` with the server's time and a wording version, and the dashboard reads
only that copy. Withdrawal is the unsubscribe link in each email (clears the private copy). Clerk's
`legal_consent` is turned on so `legalAccepted: true` records the Terms acceptance on Clerk's server.

### D7 "Create an account" from the app
Build step 1 measures whether the Portal's `/sign-up` forwards to ours when `paths.sign_up` is set.
If it does not, `sign_up_page()` (`vault-account/src/config.rs`) returns
`https://account.zaaheen.com/sign-up/` for production, taken from the same site-origin constant the
app already uses for `PAY_PAGE` (`vault-app/src/external_link.rs`), not derived from the issuer; for
the sandbox it returns the development pages' origin, so the sandbox button exercises our page too
(reviewer B #9).

### D8 The loopback "You're signed in" page (1f)
`vault-account/src/signin/http.rs` keeps a static page and `default-src 'none'`, gains one `<style>`
block allowed by its exact hash, system fonts, no script, no images, and
`frame-ancestors 'none'; base-uri 'none'; form-action 'none'` (they do not fall back to
`default-src`). The CSP test becomes an exact match, not a prefix (reviewer A #12). Rust: rides with
the batch's full gates.

### D9 Instances and testing
- **Page development uses a second Clerk application** ("Zaaheen pages (dev)", free plan), with its
  own dev instance, its own OAuth app registered loopback-only, and `development_origin` set to the
  local test server. The real sandbox that the installed app uses is never pointed at a local page
  except for the single end-to-end installer test, and a scripted check fails any installer live test
  while its paths are set (spike lesson; reviewer A #10, B #8). Test users are deleted through the
  Backend API after each run.
- **Production:** `paths.sign_in=https://account.zaaheen.com/sign-in/`, `paths.sign_up=…/sign-up/`
  (Clerk: `https`, same registrable domain; paths do not copy from dev). Consent stays on Clerk's
  hosted `accounts.zaaheen.com`. The production live test includes the step the spike could not
  see: our page navigating after `setActive` with the immediate-redirect shape (reviewer B #4).

## ADR-SEC-034 — accepting sign-in on our own origin

**What an attacker gains if our pages are tampered with:** full takeover of the Zaaheen **account**
(not the vault). A tampered page can relay the email and code to its own sign-in in real time and
keep a 7-day Clerk session, then run the app's OAuth flow with its own PKCE for a refresh token
(reviewer A #4). The account controls the subscription and Paddle portal, coaching bookings,
account deletion and the profile. It never controls a vault: sign-in never touches the vault key
(§8.26), so zero-knowledge holds. Only passkeys resist real-time relay (D2).

**Threats and answers:**
- *Open redirect, code theft:* D4 (client, redirect and flow pinned; validate on load; re-validate;
  `location.assign` of the parsed URL; `allowedRedirectOrigins`); instance invariants (D4, D9). A
  forwarded authorize link lands its code on the victim's own loopback, where the app's `state`
  check rejects it.
- *Other zaaheen.com pages reaching in:* D1's separate origin, COOP.
- *XSS:* no inline script or style (audit), Trusted Types, no HTML from strings, strict CSP.
- *Clickjacking:* `frame-ancestors 'none'`, `X-Frame-Options DENY`.
- *Build supply chain* (reviewer A #6): the build and the audit run in one job, so the audit is not
  a supply-chain control. Mitigations: `npm ci --ignore-scripts` where the build allows it; every
  site dependency, dev or not, falls under BRD §11.7.5 (exact pins, lockfile, audit, a reason in an
  ADR); Dependabot on `site/`. A grep of the built bundle for `eval(`, `new Function`,
  `setAttribute("style"` fails the audit.
- *The deploy branches and hosts:* branch protection on the account pages' deploy branch (only the
  workflow may push); Hostinger and Cloudflare accounts under the team account with 2FA (OPS-HANDOFF);
  Cloudflare features that inject scripts (Rocket Loader, Zaraz, email obfuscation, automatic Web
  Analytics) off on `account.zaaheen.com`, checked by the post-deploy script.
- *Dangling DNS:* `clerk.`, `accounts.` and `account.` records are removed in the same change that
  retires what they point at; the post-deploy script lists them.
- *Cookie tossing from a sibling subdomain* (coaching included) planting an attacker's Clerk
  client: pre-existing, recorded here; coaching's own review owns its cookies (UNVERIFIED whether
  Clerk uses `__Host-` cookies in production; measured at the production test).
- *A swapped download link* on "You're signed in": the answer is installer signing (HANDOFF §3's
  Windows-signing item), not this design.
- *Third-party code on the origin:* Clerk's pinned bundle, and Turnstile's `api.js`, which runs on
  our origin. Protect is refused (D5).
- *Bots, brute force (BRD §11.6.2):* Clerk's bot check, lockout and enumeration protection; §11.6.2's
  own numbers are Clerk-managed (recorded divergence).
- *Logs:* the app's `state` and `code_challenge` appear in Hostinger/Cloudflare access logs via
  `redirect_url`. Accepted: useless without the victim's loopback listener and PKCE verifier
  (reviewer A #13).

**Explicitly unchanged:** the app's PKCE, loopback, `state`, `iss`, token storage, lease and all of
§8.26; the consent screen; Clerk's 7-day browser session.

## Build order
1. **Measure first**, on the second dev instance, before any page: bundled clerk-js under the
   pinned CSP (spike items 4–5 again), both bundle choices; a subdomain accepted for `paths.*` (and
   production's rule, from Clerk's docs or support); Portal `/sign-up` forwarding (D7); Google
   custom flow through `/sso-callback/`; what the no-UI flow needs from the CSP.
2. Tests first (below), then the validator and state-machine modules, then the pages.
3. The Worker's `user.created` consent copy (D6); the Rust changes (D7 if needed, D8) with the
   batch's gates.
4. The end-to-end installer test on the sandbox; then production at launch.

## Tests, written first (floor)
- `redirect.test.mjs` (D4): accepts each allowed shape; refuses `http:`; another host; lookalikes
  (`clerk.zaaheen.com.evil.com`, `evil.com/clerk.zaaheen.com`, `clerk.zaaheen.com@evil.com`, a
  trailing-dot host, a mixed-case host is normalised and then must match exactly); user-info; a
  non-default port; `javascript:`, `data:`, relative, protocol-relative; encoded slashes and
  dot-segments; backslashes, tabs, newlines; a fragment; two `redirect_url`s; a wrong or repeated
  `client_id`; a non-loopback, `localhost`, `https` or `/other` `redirect_uri`; a missing `state`,
  `code_challenge` or S256; an extra parameter; the Portal shape refused in a production build;
  empty and undecodable build settings match nothing; the `buildUrlWithAuth` result re-validated.
- `signin-state.test.mjs`: every status in D1's list, including already-signed-in, resume, resend
  cooldown and the fixed line for anything unknown.
- `audit.test.mjs`: the account origin's CSP and headers pinned; everything unlisted and noindex;
  `clerk-captcha` present on all three pages; no third-party script tag at all; release refuses a
  non-`pk_live_` key or one not decoding to `clerk.zaaheen.com`; the bundle grep.
- A source test: no `innerHTML`, no `location` assignment except `location.assign` of validator
  output.
- A browser test on the local header-sending server: the full sign-up and sign-in with Clerk test
  addresses, zero CSP violations.
- Rust: the 1f page's CSP is an exact match naming its style hash; no script; D7 if it changes.

## Disposition of the v1 review
| Finding | Where resolved |
|---|---|
| A1 query not pinned (blocker) | D4 query rules; instance invariants |
| A2 Clerk navigates itself (blocker) | D3 `allowedRedirectOrigins`; D4 validate on load, re-validate, `location.assign` |
| A3 / B3 no Google return page (blocker) | D1 `/sso-callback/`, missing-names state; D3 |
| A4 residual understated | ADR-SEC-034 first paragraph; D2 divergences |
| A5 whole origin is the trust base | D1 separate origin; COOP |
| A6 missing threats | ADR-SEC-034 list |
| A7 / B5 Protect | D5 refused, fail closed |
| A8 / B10 consent is user-writable | D6 Worker copy, `legal_consent` |
| A9 / B4 consent shape sandbox-only | D4 development builds only; D9 production test |
| A10 / B8 sandbox toggle | D9 second dev instance, scripted check |
| A11 fail closed | D3 |
| A12 loopback page directives | D8 |
| A13 state in logs | ADR-SEC-034, accepted |
| B1 spike used the CDN (blocker) | Spike note; build step 1 |
| B2 pinned `.htaccess` vs env host (blocker) | D3 host from the key; D5 literal production host |
| B6 happy path only | D1 state list |
| B7 refresh and back | D1 resume and resend |
| B9 D7 sandbox | D7 |
| B11 `site.yml` | D3 |
| B12 legacy API | D3 |
| B13 cookies, forms | D3 `preventDefault` |
| B14 cheaper tests | Tests |

## Prerequisites and open items
- **Terms and Privacy pages** (the sign-up line links to them; Paddle needs them).
- A second Clerk application for page development (D9): free, founder's yes needed.
- Production: `clerk.zaaheen.com` and `account.zaaheen.com` DNS, the licence, the Pro decision.
- Production: Zaaheen's own Google OAuth client (build step 2, "New prerequisite").
- Still to build: the account origin's deploy (its own branch and Hostinger site, real settings,
  `--release` audit); the Worker's `user.created` consent copy (D6); the Rust side (S1-3 `sign_up_page()`,
  D8 loopback page) with the batch's full gates; the end-to-end installer test (build order step 4).
