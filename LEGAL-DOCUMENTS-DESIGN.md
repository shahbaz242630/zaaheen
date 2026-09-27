# Legal documents and the app's Documents link

**Status:** live, session 69 (2026-09-27). Holds ADR-SEC-036 (the app's Documents link). The legal document set itself (research reports A and B in `C:\Projects\MemoryVault-artifacts\legal-research-s69\`, the founder's decisions and the pages built from them) is added here as it is decided.

## ADR-SEC-036 — Settings › Documents opens our own pages by name

**Founder, session 69:** *"we also need to give a link in our app which takes user to documents page.. we can add it to our settings page .. Documents with drop down menu with link to all our documents"*; after the browser preview: *"I looked at the links in browser its perfect keep it"*.

**Decision.** Settings gains a **Documents** section: two dropdowns, "Using Zaaheen" (the five `/docs/` sections) and "Policies" (the six policy pages), as on the website's Documents menu. Clicking one opens that page on `https://zaaheen.com` in the person's browser.

**The security rule.** The page sends a document's **name** from a closed list, never an address. `vault_app::external_link::Document` is that list: a `serde` enum (kebab-case names), so any other value is refused by Tauri before the command runs. Its `path()` is fixed in the app; `ExternalLink::document` joins it to the fixed site, refuses anything that is not a plain `https` page on `zaaheen.com` with no query, and passes the same `checked` gate as every other link. This keeps ADR-030's rule (no user-controlled input reaches a process launcher) and SIGNIN-DESIGN §8.26 §5 (*"Nothing from the server reaches the OS unchecked"*) true for this path: nothing from the page or any server chooses where the browser goes.

**Gated.** `open_document` is on `GATED_COMMANDS`, not the locked allowlist: Settings is only reached past the lock, and widening the allowlist is a founder decision this feature does not need. A locked person still reaches every document on the website.

**One list, three places, held together by tests:**
- `vault-app` `external_link/tests.rs`: every document builds an `https://zaaheen.com` link with no query; guides are `/docs/#<id>`, policies their own page; only the listed names deserialize (addresses, `file:` paths, other spellings refused); every path is a plain site path (no `//`, `?`, `@`).
- `vault-tauri` `frontend_contract.rs`: the screen's `data-doc` names are exactly `Document::ALL`, in order; every document's page is listed in `site/src/data/site.ts` (`DOCS_SECTIONS` id or `POLICIES` path); the handler sends only `{ doc: button.dataset.doc }` and names no address.
- `vault-tauri` `commands/documents.rs`: the command takes only the entitlement and the `Document`; `guard.rs` holds that it asks `require()` first and builds no guard of its own.

**Adding a document** (the research may add some): a `Document` variant and its path, its button in `dist/index.html` in the same order, and its page in `site.ts`. The tests fail until all three agree.

**Residual.** The links only work once zaaheen.com is live, which the go-live order puts before the public download. A Rust test reads `site/src/data/site.ts`, so a website-only change that drops a page is caught at the next app CI run, not the website's own (ADR-110 skips the app checks on website-only pull requests).

## The document set (session 69)

**Research:** two independent reports, same brief (`legal-research-s69-A.md`, `-B.md` in `C:\Projects\MemoryVault-artifacts\legal-research-s69\`). They converged on: one Terms of Service for the app (licence and website use inside it, no separate EULA or website terms); fix the statements made untrue by Delete my account (7969e58), the backup wording (the export is a readable, unencrypted Markdown file and there is no import: checked in `vault-tauri/src/commands/export.rs`, `vault_app::export::to_markdown`), "Paddle is the Merchant of Record for all our orders" and the shared coaching account; add a General section, sanctions, price-change notice, discontinuation, IP complaints; new `/licences/` (also in the app's Documents list), `/.well-known/security.txt`, an optional accessibility statement; coaching gets its own Terms, cancellation and refund policy, privacy notice, terms tickbox and EU withdrawal page on coaching.zaaheen.com, kept off Paddle's reviewed domain. App pages first (they gate zaaheen.com going live), coaching after.

**Founder principle (session 69):** *"we want to be transparent with users and no gaming around but we also need to make sure we are not exploited"*. Every page is honest about what the product does, and every money rule is closed against gaming.

### Decision L1: the liability cap (founder: *"yes go with 12 months partner.. also we need to make sure this is properly tightened up"*)

Replaces the session 67 one-month cap (both reports: UK CRA 2015 s.47/s.57 and UAE Cabinet Resolution 66/2023 Art 34 would strike a cap below the price paid, leaving none). Approved text:

> **How much we are responsible for.** As far as the law allows, our total responsibility to you for all claims connected with Zaaheen is limited to the greater of the total amount you paid for Zaaheen in the 12 months before the event that caused the claim, or USD 50.
>
> This limit is about claims for loss or damage. It is not a refund and gives you no right to money back. Refunds are covered only by our Refund Policy.
>
> This limit never reduces your legal right to a repair, a replacement, a price reduction or a refund where Zaaheen does not work as described.

### Decision L2: refund anti-abuse (approved with L1)

The rule stays: the latest payment in full if asked within 14 days of the charge, both plans; a refund ends the subscription at once. Added (closing loopholes neither report covered):
- **One refund per person:** "We refund one payment per person. If you have had a refund before, on this account or another one, later payments are not refundable, except where the law gives you a right to a refund."
- **No part refunds:** "When you cancel, you keep access until the end of the time you paid for. We do not refund the unused part of a month or a year."
- **Abuse:** "We refuse a refund when it is being used to get Zaaheen without paying, such as repeated sign-ups and refunds across accounts."
- **Bank reversals:** "If you ask your bank to reverse a payment, your subscription ends when the reversal is made."
- **Scope:** "This policy covers the Zaaheen app only. Coaching has its own cancellation rules."
- **Paddle:** "We confirm the refund and instruct Paddle to send it."
- **The legal exception:** "After 14 days we do not refund a payment, except where the law gives you a right to one."

### Decision L3: no physical address or business phone yet (founder: *"we dont need business number and virtual office address..our affiliate website me and you set up also works without it .. once we have the numbers or users grow we can set it up"*)

Pages publish email only (`customerservice@zaaheen.com`). **Accepted risk:** UAE Decree-Law 14/2023 Art 6(7)-(9) (address, contact numbers), Paddle's handbook (support phone), UK/EU distance-selling address for coaching. **Operational rule:** no marketing email (tips and product news) to US recipients until a postal address exists (CAN-SPAM); the tips consent tickbox can still collect consent.

### Decision L4: no EU/UK Art 27 representatives yet (founder: *"we dont require these since we dont have revenue.. once we have clients in these countries ... we will offcourse set up every thin"*; contact: *"we can use customerservice@zaaheen.com for now"*)

The Privacy Policy names `customerservice@zaaheen.com` as the privacy contact for everyone, EU and UK included, and claims no exemption. **Accepted risk, recorded as such:** GDPR / UK GDPR Art 27 has no grace period; the duty starts with the first EU/UK customer. Revisit when EU or UK sales are regular (founder: traction known in 1 to 6 months). `team@zaaheen.com` (the Microsoft 365 sign-in) is never published.

**Founder's compliance stance (session 69):** *"we are startup .. no one requires us to be 100% compliant from day one and if we do hit sales and threshold we will offcourse set up every thin"*; *"we will know in 1-6 months if our product is getting traction ... if yes we will offcourse invest in all the requirements"*. Paid compliance items wait for traction; free ones (wording, honesty, anti-abuse) are done now.

### Decision L5: if we stop offering Zaaheen (founder: *"yes go with that clause partner"*)

> **If we stop offering Zaaheen.** If we decide to stop offering Zaaheen, we will tell you at least 60 days before, stop taking payments, and refund the unused part of any yearly plan. *Download my memories* keeps working, so you can take everything with you.

The L2 "no part refunds" line gains "except if we stop offering Zaaheen". Report B's optional "final version that works without our servers" promise is declined (an engineering commitment to deliver during a shutdown).

### Decision L6: tax shown on the Pricing page (founder: *"yes add that line partner"*)

Paddle prices are tax-exclusive (`tax_mode: external`, read from the sandbox catalogue; production set up the same way, unread this session: the live Paddle connector failed). Paddle collects and remits the tax (founder: *"vat and tax is collected and paid by paddle this is why we are using them"*); the page states it under each price:

> *Plus VAT or sales tax where it applies, collected by our reseller Paddle. You see the exact total before you pay.*

### Pages approved (founder, session 69: *"yes all look good partner"*)

`/terms/` rewritten (both reports' findings plus L1, L5; indemnity omitted, as report B advised: a consumer indemnity is a classic unfair term and gives little real protection); `/refunds/` (L2); `/pricing/` (L6). Reply times approved: complaints and refunds *"within 5 working days"*, rights complaints *"within 14 days"*. The Terms name "features may change" and term changes that affect rights apply *from the next renewal* after 30 days' notice, so no change opens a route to a part refund.

### Decision L7: open-source licences (founder: *"yes option 1 partner.."*)

The MIT / Apache 2.0 (and similar) licences of the code and AI models we ship require their notices to accompany the app. Option 1: the full notices ship with the app as a file in the install folder, generated at build time (**to build in the big-build batch; the public installer must not ship without it**); `/licences/` names no component (the white-label rule keeps the stack off public pages) and says the notices ship with the app and a copy is sent on request. `/licences/` is in `POLICIES`, the footer's Legal column, the audit, and the app's Settings › Documents (`Document::Licences`).

### The Pricing page comparison and FAQ (founder, session 69)

Founder: *"our pricing page should have FAQ at the bottom.. answering questions.. comparisons.. and also we should add cost comparison between us and our competitors"*; then *"I dont see big names like obsedian . memai ... add column where it shows where memories are kept ... encrypted ... used for training yes or no"*. Data in `site/src/data/pricing.ts` (`COMPARISON`, `COMPARISON_SOURCES`, `CHECKED_ON`, `PRICING_FAQ`). Research: `C:\Projects\MemoryVault-artifacts\pricing-research-s69\` (`competitor-pricing.md`, `competitor-privacy-facts.md`); every price and the two training policies re-read in a browser on 2026-09-27 with screenshots in `evidence\`. Rules (UK CAP Code 3.7, 3.33 to 3.44; EU Directive 2006/114/EC Art 4): same need only (Supermemory, Mem0, Notion left out); the price the buyer pays; competitors' own words in quotes; "Not stated" where they say nothing, never a guessed "No"; no "cheapest" or "most private"; dated, with links. **Re-check every row before any change to the table and at least every three months.** Honest finding: Obsidian Sync costs the same as Zaaheen and is end-to-end encrypted; the difference shown is that AI apps reach Obsidian only through community-made plugins. FAQ is plain HTML (SEO-HANDOFF §3a rule 18: no FAQPage schema).
