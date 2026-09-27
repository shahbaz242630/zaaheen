# Website security — design (ADR-SEC-037)

The security of zaaheen.com, account.zaaheen.com and the account API, from the session-72
research, audit and fixes. The coaching booking app (separate repository) records its own
controls in its `SECURITY.md`.

**Evidence (local, not in this repo):** two independent research reports from primary sources,
reconciled into a 40-item checklist; two independent read-only audits against it, merged
(`MemoryVault-artifacts\security-research-s72\`, `...\security-audit-s72\`). Neither audit found
anything Critical or High, any secret in either repository's history or any built bundle, or an
exploitable XSS, injection, open redirect or webhook-forgery path.

## ADR-SEC-037 — what the websites promise, and how we hold them to it

**1. Headers are checked where visitors receive them, not only where they are written.**
Hostinger runs its own CDN between Cloudflare and the origin, and on the coaching staging site
it was measured replacing the whole Content-Security-Policy with `upgrade-insecure-requests`.
`site/scripts/post-deploy.mjs` now fails the deploy unless every `Header always set` line in
`site/public/.htaccess` (and `site/public/pay/.htaccess` for `/pay`) arrives exactly
(`site/scripts/live-headers.mjs`, 6 tests including that measured rewrite). HSTS is left out:
it is set once, at the Cloudflare edge, which covers every proxied host (`max-age=31536000` now,
`includeSubDomains` after four clean weeks, no preload: hard to undo, ASVS L3 only).

**2. Sign-in does not say whether an email has an account.** Founder decision:
*"yes sign in message is fine neutral"*. The real protection is Clerk's user enumeration
protection in production, which makes Clerk answer every email alike, so our pages behave alike;
our line for `form_identifier_not_found` is neutral as a fallback
(`site/account/scripts/account-state.js`, pinned by a test). The sign-up page's "There is
already an account" line depends on the same Clerk setting; verified in the production live test.

**3. The account API is protected from floods at the edge.** The Workers rate-limit binding
does not refuse on the free plan (measured), and a request it refuses still counts against the
100,000-a-day quota. The fix is the free Cloudflare rate-limiting rule (IP, 10 s) plus a WAF rule
allowing only the known routes, both before the Worker. **Not done, deliberately:** a
token-shape pre-check before calling Clerk. Clerk's access tokens are opaque; one measured
sample is not a format, so a check would refuse real tokens or pass well-shaped fakes.

**4. Dependencies are watched, Rust excepted.** `.github/dependabot.yml` opens weekly update
pull requests for the website, the Worker and the pinned Actions, two days after release.
Cargo is excluded: BRD §4 pins every crate to an exact reviewed version, each bump runs the
hour-long Rust matrix, and `cargo-deny` in CI already refuses a crate with a RustSec advisory.

**5. Never switched on:** Cloudflare Bot Fight Mode (it cannot be exempted and would challenge
Paddle, Clerk and Stripe webhooks and the desktop app), Scrape Shield email obfuscation (it
injects a script our CSP refuses; found ON for zaaheen.com and to be turned off), Rocket Loader,
Web Analytics auto-injection, SRI on Paddle.js (Paddle updates it in place), HSTS preload, COEP
`require-corp`.

**Owed at launch (founder's accounts, with Claude):** phishing-resistant MFA on the registrar,
Cloudflare, GitHub, Hostinger, Microsoft 365, Clerk, Paddle, Stripe and Supabase; Cloudflare
Always Use HTTPS, SSL Full (strict), minimum TLS 1.2, the rate-limit and WAF rules, DNSSEC with
the DS record at the registrar, CAA; DMARC `p=none` → `quarantine` → `reject`; Hostinger's CDN
off once Cloudflare proxies the site; Clerk's enumeration protection; a written view from
Paddle that, as merchant of record, it carries the PCI duty for `/pay`.
