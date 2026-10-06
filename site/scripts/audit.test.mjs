// Negative controls for scripts/audit.mjs. Each case breaks a copy of the real
// build in one way and asserts the audit exits 1 naming exactly that problem,
// then the untouched build must pass. A clean audit run proves nothing on its
// own; this proves the audit still catches what it claims to.
//
//   npm run build && npm test        (from site/)
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { expectedOffers } from './site-facts.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '..', 'dist');
const AUDIT = path.join(HERE, 'audit.mjs');

const edit = (rel, fn) => (dir) => {
  const p = path.join(dir, rel);
  fs.writeFileSync(p, fn(fs.readFileSync(p, 'utf8')));
};
const inject = (html) => edit('index.html', (s) => s.replace('</main>', `${html}</main>`));
// Points the header anchor carrying `cls` somewhere else; throws if the build
// has no such anchor, so a case can never pass by testing nothing.
const headerHref = (cls, href) => edit('index.html', (s) => {
  const out = s.replace(new RegExp(`<a\\b[^>]*\\b${cls}\\b[^>]*>`), (tag) => tag.replace(/\shref="[^"]*"/, ` href="${href}"`));
  if (out === s) throw new Error(`audit.test: no header anchor with class ${cls}`);
  return out;
});
// Turns the header's "Coming soon" label (download off) back into a "Get
// started" link to `href`; throws if the build has no such label.
// The label, with its long and short wordings inside (one shown per width).
const SOON_LABEL = /<span class="bar-start\b[^"]*">(?:<span[^>]*>[^<]*<\/span>)*<\/span>/;
const startLink = (href) => edit('index.html', (s) => {
  const out = s.replace(SOON_LABEL, `<a class="bar-start" href="${href}">Get started</a>`);
  if (out === s) throw new Error('audit.test: no header "Coming soon" label');
  return out;
});
// Removes the footer link to `href` from the Documents page; throws if the
// footer has no such link, so the case can never pass by testing nothing.
const footerLink = (href) => edit('docs/index.html', (s) => {
  const out = s.replace(/<footer\b[\s\S]*?<\/footer>/, (f) => f.replace(new RegExp(`<a\\b[^>]*\\shref="${href}"[^>]*>[\\s\\S]*?<\\/a>`), ''));
  if (out === s) throw new Error(`audit.test: no footer link to ${href}`);
  return out;
});
// Edits the Knowledge Centre's footer; throws if the edit changed nothing.
const kcFooter = (fn) => edit('knowledge-centre/index.html', (s) => {
  const out = s.replace(/<footer\b[\s\S]*?<\/footer>/, fn);
  if (out === s) throw new Error('audit.test: the Knowledge Centre footer edit changed nothing');
  return out;
});
// Sets the footer variant marker on `rel`; throws if the page has none.
const footerVariant = (rel, from, to) => edit(rel, (s) => {
  const out = s.replace(`data-footer="${from}"`, `data-footer="${to}"`);
  if (out === s) throw new Error(`audit.test: ${rel} has no data-footer="${from}"`);
  return out;
});
// The /pay page's build-time Paddle config, as pay.astro renders it. An unset
// value renders as a bare attribute (no ="..."), so both forms are replaced,
// and a build that matched neither would throw rather than test nothing.
const payConfig = (env, token) => edit('pay/index.html', (s) => {
  const out = s
    .replace(/\sdata-paddle-env(="[^"]*")?(?=[\s>])/, ` data-paddle-env="${env}"`)
    .replace(/\sdata-paddle-token(="[^"]*")?(?=[\s>])/, ` data-paddle-token="${token}"`);
  if (!out.includes(`data-paddle-env="${env}"`) || !out.includes(`data-paddle-token="${token}"`)) {
    throw new Error('audit.test: the pay page no longer carries data-paddle-env / data-paddle-token');
  }
  return out;
});

// Rewrites the JSON-LD graph of `rel`; throws if the page has none or the edit
// changed nothing, so a case can never pass by testing nothing.
const ldEdit = (rel, fn) => edit(rel, (s) => {
  const out = s.replace(/(<script type="application\/ld\+json">)([\s\S]*?)(<\/script>)/, (_, open, json, close) => {
    const doc = JSON.parse(json);
    fn(doc['@graph']);
    return `${open}${JSON.stringify(doc)}${close}`;
  });
  if (out === s) throw new Error(`audit.test: the JSON-LD edit on ${rel} changed nothing`);
  return out;
});
const appNode = (graph) => {
  const app = graph.find((n) => n['@type'] === 'SoftwareApplication');
  if (!app) throw new Error('audit.test: the home page JSON-LD has no SoftwareApplication');
  return app;
};
// The offers as lib/seo.ts writes them, built from PRICES (scripts/site-facts.mjs).
const offersFor = (list) => list.map((o) => ({
  '@type': 'Offer', price: String(o.price), priceCurrency: o.priceCurrency,
  priceSpecification: { '@type': 'UnitPriceSpecification', price: String(o.price), priceCurrency: o.priceCurrency, billingDuration: o.billingDuration },
}));
const KC_TERMS = 'knowledge-centre/terms/index.html';
const crumbList = (graph) => {
  const list = graph.find((n) => n['@type'] === 'BreadcrumbList');
  if (!list) throw new Error('audit.test: the page has no BreadcrumbList');
  return list;
};

// The app on sale: an installer link on the home page (RELEASE.available in
// src/data/site.ts). Only then must a release build take live payments. The
// link carries no words, so the page's text, and so its date, stay the same.
const installerLink = inject('<a href="https://dl.zaaheen.com/Zaaheen_x.msi" aria-label="x"></a><a href="https://dl.zaaheen.com/Zaaheen_x.dmg" aria-label="x"></a>');
// On sale with the Windows installer only, no Mac disk image.
const windowsOnlyLink = inject('<a href="https://dl.zaaheen.com/Zaaheen_x.msi" aria-label="x"></a>');
// An on-sale build also shows "Get started" in every page's header.
const onSale = (d) => {
  installerLink(d);
  const pages = fs.readdirSync(d, { recursive: true }).filter((f) => String(f).endsWith('.html'));
  for (const rel of pages) {
    edit(String(rel), (s) => s.replace(SOON_LABEL, '<a class="bar-start" href="https://account.zaaheen.com/sign-up/">Get started</a>'))(d);
  }
};
const onSaleWith = (env, token) => (d) => {
  onSale(d);
  payConfig(env, token)(d);
};

// The first Learn article (s91), for the Learn cases.
const LEARN_1 = 'learn/share-memory-between-chatgpt-and-claude/index.html';
const cases = [
  ['robots.txt blocks crawlers', edit('robots.txt', (s) => s.replace('Allow: /', 'Disallow: /')), /contains a Disallow rule/],
  ['home page noindex', edit('index.html', (s) => s.replace('<title>', '<meta name="robots" content="noindex"><title>')), /robots meta contains "noindex"/],
  ['home page nosnippet', edit('index.html', (s) => s.replace('<title>', '<meta name="robots" content="nosnippet"><title>')), /robots meta contains "nosnippet"/],
  ['canonical removed', edit('index.html', (s) => s.replace(/<link rel="canonical"[^>]*>/, '')), /exactly one canonical, found 0/],
  ['link filled in by script', inject('<a href="#">x</a>'), /anchor without a real href/],
  ['third-party script', edit('index.html', (s) => s.replace('</head>', '<script src="https://cdn.example.com/x.js"></script></head>')), /loads a third-party resource/],
  ['inline script', inject('<script>alert(1)</script>'), /inline <script> would be blocked/],
  // Browsers also end a script at "</script >". Last in the page, so no later
  // "</script>" can close the match for it.
  ['inline script, spaced end tag', edit('index.html', (s) => s.replace('</body>', '<script>alert(1)</script ></body>')), /inline <script> would be blocked/],
  ['glued inline text', inject('<p>Choose<em>More info</em></p>'), /missing space before <em>/],
  ['private doc published', (d) => fs.writeFileSync(path.join(d, 'SEO-HANDOFF.md'), '# x'), /private or build-internal file/],
  ['page missing from sitemap', edit('sitemap.xml', (s) => s.replace(/<url>[\s\S]*?<\/url>/, '')), /does not list the indexable page \//],
  ['fake rating in JSON-LD', edit('index.html', (s) => s.replace('"softwareVersion"', '"aggregateRating":{"ratingValue":5},"softwareVersion"')), /claims ratings or reviews/],
  ['broken internal link', inject('<a href="/nope/">x</a>'), /broken internal link or asset: \/nope\//],
  ['favicon missing', (d) => fs.rmSync(path.join(d, 'favicon-192.png')), /favicon-192\.png: required file is missing/],
  ['description too long', edit('index.html', (s) => s.replace(/(<meta name="description" content=")/, `$1${'x'.repeat(170)} `)), /meta description is \d+ chars/],
  ['em dash in copy', inject('<p>Private — and yours</p>'), /em dash in published text/],
  ['spaced en dash in copy', inject('<p>Private – and yours</p>'), /em dash in published text/],
  // The marker proves this exact injection was caught, not some placeholder
  // the real build already carries.
  ['placeholder on a release build', inject('<p>[Placeholder] audit-test-marker</p>'), /placeholder text would be published: "\[Placeholder\] audit-test-marker/, ['--release']],
  // /pay: Paddle's default payment link, the one page with a third-party script.
  ['pay page indexable', edit('pay/index.html', (s) => s.replace(/<meta name="robots" content="noindex">/, '')), /pay\/index\.html: the pay page must be noindex/],
  ['pay page with a canonical', edit('pay/index.html', (s) => s.replace('<title>', '<link rel="canonical" href="https://zaaheen.com/pay/"><title>')), /the pay page must not declare a canonical/],
  ['pay page in the sitemap', edit('sitemap.xml', (s) => s.replace('</urlset>', '<url><loc>https://zaaheen.com/pay/</loc></url></urlset>')), /lists https:\/\/zaaheen\.com\/pay\/, which is not a built indexable page/],
  ['Paddle.js on another page', edit('index.html', (s) => s.replace('</head>', '<script src="https://cdn.paddle.com/paddle/v2/paddle.js"></script></head>')), /index\.html: loads a third-party resource: https:\/\/cdn\.paddle\.com/],
  ['another third party on the pay page', edit('pay/index.html', (s) => s.replace('</head>', '<script src="https://cdn.example.com/x.js"></script></head>')), /pay\/index\.html: loads a third-party resource: https:\/\/cdn\.example\.com/],
  // The exception is for a <script> only, not the same URL in another tag.
  ['Paddle URL in a non-script tag', edit('pay/index.html', (s) => s.replace('</head>', '<link rel="preload" href="https://cdn.paddle.com/paddle/v2/paddle.js"></head>')), /pay\/index\.html: loads a third-party resource: https:\/\/cdn\.paddle\.com/],
  ['pay page without its CSP', (d) => fs.rmSync(path.join(d, 'pay', '.htaccess')), /pay\/\.htaccess: required file is missing/],
  ['pay CSP widened', edit('pay/.htaccess', (s) => s.replace("script-src 'self' https://cdn.paddle.com", "script-src 'self' https://cdn.paddle.com https://cdn.example.com")), /pay\/\.htaccess: the checkout CSP is not the pinned policy/],
  ['pay page indexable by header', edit('pay/.htaccess', (s) => s.replace(/^.*X-Robots-Tag.*$/m, '')), /pay\/\.htaccess: missing the X-Robots-Tag noindex header/],
  ['sandbox checkout on a release build', onSaleWith('sandbox', `test_${'a'.repeat(27)}`), /pay\/index\.html: checkout is not set up for live payments/, ['--release']],
  ['live environment with a sandbox token', onSaleWith('production', `test_${'a'.repeat(27)}`), /pay\/index\.html: checkout is not set up for live payments/, ['--release']],
  ['sandbox environment with a live token', onSaleWith('sandbox', `live_${'b'.repeat(27)}`), /pay\/index\.html: checkout is not set up for live payments/, ['--release']],
  // The header's account buttons and the /sign-in, /sign-up forwards (AUTH-PAGES-DESIGN D1).
  ['header Account not on the account origin', headerHref('bar-signin', '/#how'), /index\.html: header "Account" must link to https:\/\/account\.zaaheen\.com\/account\//],
  // "Get started" only while the app is on sale (founder, session 75).
  ['header Get started while not on sale', startLink('https://account.zaaheen.com/sign-up/'), /index\.html: header "Get started" must not be a link while the app is not on sale/],
  ['header sign-up link while not on sale', edit('index.html', (s) => s.replace('</header>', '<a href="https://account.zaaheen.com/sign-up/">Join</a></header>')), /index\.html: the header links to sign-up while the app is not on sale/],
  ['header Get started missing when on sale', installerLink, /index\.html: header "Get started" must link to https:\/\/account\.zaaheen\.com\/sign-up\/ \(found no link\)/],
  ['header Get started elsewhere when on sale', (d) => { onSale(d); headerHref('bar-start', 'https://evil.example/sign-up/')(d); }, /index\.html: header "Get started" must link to https:\/\/account\.zaaheen\.com\/sign-up\/ \(found https:\/\/evil\.example/],
  // Windows and Mac together (founder, session 78).
  ['header says Windows only while not on sale', edit('index.html', (s) => { const out = s.replace('Coming soon for Windows and Mac', 'Coming soon for Windows'); if (out === s) throw new Error('audit.test: no two-platform label'); return out; }), /index\.html: the header must say "Coming soon for Windows and Mac" while the app is not on sale/],
  ['on sale without the Mac download', (d) => { windowsOnlyLink(d); }, /index\.html: the app is on sale but the home page does not link the Mac disk image \(\.dmg\)/],
  ['header Account removed', edit('index.html', (s) => s.replace(/<a\b[^>]*\bbar-signin\b[^>]*>[\s\S]*?<\/a>/, '')), /index\.html: header "Account" must link to/],
  ['sign-in forward missing', edit('.htaccess', (s) => s.replace(/^.*account\.zaaheen\.com.*$/gm, '')), /\.htaccess: \/sign-in and \/sign-up must forward to the account origin/],
  ['sign-in forward keeps the query', edit('.htaccess', (s) => s.replace('account.zaaheen.com/sign-$1/?', 'account.zaaheen.com/sign-$1/')), /\.htaccess: \/sign-in and \/sign-up must forward to the account origin/],
  // The policy pages: each one built, and linked from every page's footer
  // (Paddle's domain review wants them "clearly accessible via navigation").
  ['footer Privacy link removed', footerLink('/privacy/'), /docs\/index\.html: footer must link to the Privacy Policy \(\/privacy\/\)/],
  ['footer Terms link removed', footerLink('/terms/'), /docs\/index\.html: footer must link to the Terms of Service \(\/terms\/\)/],
  ['footer Refunds link removed', footerLink('/refunds/'), /docs\/index\.html: footer must link to the Refund Policy \(\/refunds\/\)/],
  // The Knowledge Centre's own footer (founder, session 70): coaching's
  // documents only, never the app's.
  ['app refund policy in the coaching footer', kcFooter((f) => f.replace('</nav>', '<a href="/refunds/">Refunds</a></nav>')), /knowledge-centre\/index\.html: the coaching footer must not link to the app's Refund Policy \(\/refunds\/\)/],
  ['coaching footer without company information', kcFooter((f) => f.replace(/<a\b[^>]*\shref="\/company\/"[^>]*>[\s\S]*?<\/a>/, '')), /knowledge-centre\/index\.html: the coaching footer must link to Company Information \(\/company\/\)/],
  ['coaching footer without the Booking and Refund Policy', kcFooter((f) => f.replace(/<a\b[^>]*\shref="\/knowledge-centre\/booking-and-refunds\/"[^>]*>[\s\S]*?<\/a>/, '')), /knowledge-centre\/index\.html: the coaching footer must link to the Booking and Refund Policy/],
  ['coaching terms page missing', (d) => fs.rmSync(path.join(d, 'knowledge-centre', 'terms'), { recursive: true }), /knowledge-centre\/terms\/index\.html: required file is missing/],
  ['app footer on the Knowledge Centre', footerVariant('knowledge-centre/index.html', 'coaching', 'app'), /knowledge-centre\/index\.html: the Knowledge Centre must use the coaching footer/],
  ['coaching footer on an app page', footerVariant('docs/index.html', 'app', 'coaching'), /docs\/index\.html: the coaching footer is only for the Knowledge Centre/],
  ['refunds page missing', (d) => fs.rmSync(path.join(d, 'refunds'), { recursive: true }), /refunds\/index\.html: required file is missing/],
  ['privacy page missing', (d) => fs.rmSync(path.join(d, 'privacy'), { recursive: true }), /privacy\/index\.html: required file is missing/],
  ['security.txt missing', (d) => fs.rmSync(path.join(d, '.well-known'), { recursive: true }), /\.well-known\/security\.txt: required file is missing/],
  ['security.txt expired', edit('.well-known/security.txt', (s) => s.replace(/^Expires: .*$/m, 'Expires: 2020-01-01T00:00:00.000Z')), /security\.txt: Expires must be between 30 days and a year from now/],
  // The app's prices in JSON-LD must be PRICES in src/data/site.ts (SEO-HANDOFF §1 step 2).
  ['on sale with no prices in JSON-LD', onSale, /index\.html: the app is on sale but its JSON-LD states no prices/],
  ['free, in pounds (the old offer)', ldEdit('index.html', (g) => { appNode(g).offers = { '@type': 'Offer', price: '0', priceCurrency: 'GBP' }; }), /index\.html: JSON-LD offers do not match PRICES/],
  ['wrong currency', ldEdit('index.html', (g) => { appNode(g).offers = offersFor(expectedOffers().map((o) => ({ ...o, priceCurrency: 'GBP' }))); }), /index\.html: JSON-LD offers do not match PRICES/],
  ['monthly and yearly swapped', ldEdit('index.html', (g) => { appNode(g).offers = offersFor(expectedOffers().reverse()); }), /index\.html: JSON-LD offers do not match PRICES/],
  ['yearly plan missing', ldEdit('index.html', (g) => { appNode(g).offers = offersFor(expectedOffers().slice(0, 1)); }), /index\.html: JSON-LD offers do not match PRICES/],
  ['price specification disagrees', ldEdit('index.html', (g) => { const o = offersFor(expectedOffers()); o[0].priceSpecification.price = '1'; appNode(g).offers = o; }), /index\.html: JSON-LD offers do not match PRICES/],
  // Breadcrumbs on nested pages only, the JSON-LD identical to the visible trail (step 3).
  ['nested page without a visible trail', edit(KC_TERMS, (s) => s.replace(/<nav\b[^>]*aria-label="Breadcrumb"[\s\S]*?<\/nav>/, '')), /knowledge-centre\/terms\/index\.html: a nested or policy page must show one breadcrumb trail/],
  ['nested page without BreadcrumbList', ldEdit(KC_TERMS, (g) => { g.splice(g.indexOf(crumbList(g)), 1); }), /knowledge-centre\/terms\/index\.html: a nested or policy page must carry one BreadcrumbList/],
  ['breadcrumb name differs', ldEdit(KC_TERMS, (g) => { crumbList(g).itemListElement[1].name = 'Coaching'; }), /knowledge-centre\/terms\/index\.html: the breadcrumb JSON-LD does not match the visible trail/],
  ['breadcrumb URL differs', ldEdit(KC_TERMS, (g) => { crumbList(g).itemListElement[1].item = 'https://zaaheen.com/docs/'; }), /knowledge-centre\/terms\/index\.html: the breadcrumb JSON-LD does not match the visible trail/],
  ['breadcrumb item dropped', ldEdit(KC_TERMS, (g) => { crumbList(g).itemListElement.pop(); }), /knowledge-centre\/terms\/index\.html: the breadcrumb JSON-LD does not match the visible trail/],
  ['trail not ending on the page', edit(KC_TERMS, (s) => { const out = s.replace(/\saria-current="page"(?=[^<]*>[^<]*<\/span>\s*<\/li>\s*<\/ol>)/, ''); if (out === s) throw new Error('audit.test: no current crumb'); return out; }), /knowledge-centre\/terms\/index\.html: the breadcrumb trail must run from Home/],
  // The policies' trail is Home › Documents › the page (founder, session 71).
  ['policy page without a visible trail', edit('terms/index.html', (s) => s.replace(/<nav\b[^>]*aria-label="Breadcrumb"[\s\S]*?<\/nav>/, '')), /terms\/index\.html: a nested or policy page must show one breadcrumb trail/],
  ['policy page without BreadcrumbList', ldEdit('privacy/index.html', (g) => { g.splice(g.indexOf(crumbList(g)), 1); }), /privacy\/index\.html: a nested or policy page must carry one BreadcrumbList/],
  ['policy crumb under the wrong parent', ldEdit('refunds/index.html', (g) => { crumbList(g).itemListElement[1].item = 'https://zaaheen.com/knowledge-centre/'; }), /refunds\/index\.html: the breadcrumb JSON-LD does not match the visible trail/],
  // Full snippets and large image previews on every listed page (step 4).
  ['robots meta removed', edit('pricing/index.html', (s) => { const out = s.replace(/<meta name="robots"[^>]*>/, ''); if (out === s) throw new Error('audit.test: no robots meta'); return out; }), /pricing\/index\.html: robots meta must allow "max-snippet:-1"/],
  ['snippet length capped', edit('index.html', (s) => { const out = s.replace('max-snippet:-1', 'max-snippet:50'); if (out === s) throw new Error('audit.test: no max-snippet'); return out; }), /index\.html: robots meta must allow "max-snippet:-1"/],
  ['small image previews', edit('index.html', (s) => { const out = s.replace('max-image-preview:large', 'max-image-preview:standard'); if (out === s) throw new Error('audit.test: no max-image-preview'); return out; }), /index\.html: robots meta must allow "max-image-preview:large"/],
  // Guides are Articles by the company, headline = H1 (step 6).
  ['guide without an Article', ldEdit('docs/connect-claude/index.html', (g) => { const i = g.findIndex((n) => n['@type'] === 'Article'); if (i < 0) throw new Error('audit.test: no Article'); g.splice(i, 1); }), /docs\/connect-claude\/index\.html: a guide must carry one Article/],
  ['Article headline differs from the H1', ldEdit('docs/connect-cursor/index.html', (g) => { g.find((n) => n['@type'] === 'Article').headline = 'Cursor memory'; }), /docs\/connect-cursor\/index\.html: the Article headline "Cursor memory" is not the page's H1/],
  ['Article by someone else', ldEdit('docs/troubleshooting/index.html', (g) => { g.find((n) => n['@type'] === 'Article').author = { '@type': 'Person', name: 'x' }; }), /docs\/troubleshooting\/index\.html: the Article author and publisher must be the company/],
  ['guide missing', (d) => fs.rmSync(path.join(d, 'docs', 'getting-started'), { recursive: true }), /broken internal link or asset: \/docs\/getting-started\//],
  // <main> carries attributes since session 90 (id="main" for the skip link):
  // match the tag, and throw if it is missing so the case never tests nothing.
  ['breadcrumb on a top-level page', edit('docs/index.html', (s) => { const out = s.replace(/<main\b[^>]*>/, (m) => `${m}<nav aria-label="Breadcrumb"><ol><li><a href="/">Home</a></li></ol></nav>`); if (out === s) throw new Error('audit.test: no <main>'); return out; }), /docs\/index\.html: a top-level page must not carry a breadcrumb trail/],
  ['security.txt wrong contact', edit('.well-known/security.txt', (s) => s.replace('customerservice@', 'someone@')), /security\.txt: must have "Contact: mailto:customerservice@zaaheen\.com"/],
  // SEO audit, session 72.
  ['guide heading skips a level', edit('docs/connect-claude/index.html', (s) => { const out = s.replace(/<h2 class="set-name">([\s\S]*?)<\/h2>/, '<h3 class="set-name">$1</h3>'); if (out === s) throw new Error('audit.test: no guide row heading'); return out; }), /docs\/connect-claude\/index\.html: heading level skipped: an <h3> follows an <h1>/],
  ['page text changed, date kept', edit('terms/index.html', (s) => s.replace('</main>', '<p>A new clause.</p></main>')), /terms\/index\.html: the page text changed since its date: run npm run stamp/],
  ['new listed page with no date', (d) => {
    fs.mkdirSync(path.join(d, 'extra'));
    fs.writeFileSync(path.join(d, 'extra', 'index.html'), fs.readFileSync(path.join(d, 'terms', 'index.html'), 'utf8'));
  }, /extra\/index\.html: the page has no recorded date: run npm run stamp/],
  ['sitemap date not the recorded one', edit('sitemap.xml', (s) => { const out = s.replace(/<lastmod>[^<]*<\/lastmod>/, '<lastmod>2020-01-01T00:00:00Z</lastmod>'); if (out === s) throw new Error('audit.test: no lastmod'); return out; }), /sitemap\.xml: lastmod for \/ is 2020-01-01T00:00:00Z, not its recorded date/],
  ['absolute security claim', inject('<p>Zaaheen is unhackable.</p>'), /index\.html: absolute security claim "unhackable"/],
  ['absolute security claim in llms.txt', edit('llms.txt', (s) => `${s}\nYour memories are 100% secure.\n`), /llms\.txt: absolute security claim "100% secure"/],
  // Learn articles (s91): a chart, an animation, no stock phrases, an Article,
  // and a link from outside /learn/. Each edit throws if it changed nothing.
  ['Learn article without an animation', edit(LEARN_1, (s) => { const out = s.replace(/<figure class="flow"[\s\S]*?<\/figure>/, ''); if (out === s) throw new Error('audit.test: no animation'); return out; }), /learn\/share-memory-between-chatgpt-and-claude\/index\.html: a Learn article needs an animation/],
  ['Learn article without a chart', edit(LEARN_1, (s) => { const out = s.replace(/<figure class="chart"[\s\S]*?<\/figure>/, '').replace(/<p class="placeholder">[\s\S]*?<\/p>/, ''); if (out === s) throw new Error('audit.test: no chart or chart placeholder'); return out; }), /learn\/share-memory-between-chatgpt-and-claude\/index\.html: a Learn article needs a chart/],
  ['stock phrase in a Learn article', edit(LEARN_1, (s) => s.replace('</main>', '<p>Let us delve into your memory.</p></main>')), /learn\/share-memory-between-chatgpt-and-claude\/index\.html: stock phrase "delve"/],
  ['Learn article without an Article', ldEdit(LEARN_1, (g) => { const i = g.findIndex((n) => n['@type'] === 'Article'); if (i < 0) throw new Error('audit.test: no Article'); g.splice(i, 1); }), /learn\/share-memory-between-chatgpt-and-claude\/index\.html: a guide must carry one Article/],
  ['Learn article without People also read', edit(LEARN_1, (s) => { const out = s.replace(/<nav class="learn-more" aria-label="People also read">[\s\S]*?<\/nav>/, ''); if (out === s) throw new Error('audit.test: no People also read'); return out; }), /learn\/share-memory-between-chatgpt-and-claude\/index\.html: a Learn article needs a People also read list/],
  ['Learn article linked only from /learn/', (d) => {
    const url = '/learn/share-memory-between-chatgpt-and-claude/';
    let removed = 0;
    const walk = (dir) => fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(path.join(dir, e.name)) : [path.join(dir, e.name)]));
    for (const f of walk(d).filter((f) => f.endsWith('.html') && !path.relative(d, f).startsWith('learn'))) {
      const s = fs.readFileSync(f, 'utf8');
      const out = s.replaceAll(`href="${url}"`, 'href="/learn/"');
      if (out !== s) { removed++; fs.writeFileSync(f, out); }
    }
    if (!removed) throw new Error('audit.test: nothing outside /learn/ links the article');
  }, /learn\/share-memory-between-chatgpt-and-claude\/index\.html: no page outside \/learn\/ links to this article/],
];

if (!fs.existsSync(DIST)) {
  console.error('audit.test: dist/ does not exist. Run the build first.');
  process.exit(1);
}

let failed = 0;
for (const [name, breakIt, expected, extraArgs = []] of cases) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zaaheen-audit-'));
  try {
    fs.cpSync(DIST, dir, { recursive: true });
    breakIt(dir);
    const r = spawnSync(process.execPath, [AUDIT, dir, ...extraArgs], { encoding: 'utf8' });
    const out = `${r.stdout}${r.stderr}`;
    const caught = r.status === 1 && expected.test(out);
    if (!caught) failed++;
    console.log(`${caught ? 'ok  ' : 'FAIL'}  ${name}${caught ? '' : `\n      exit=${r.status}\n${out}`}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

const clean = spawnSync(process.execPath, [AUDIT, DIST], { encoding: 'utf8' });
if (clean.status !== 0) failed++;
console.log(`${clean.status === 0 ? 'ok  ' : 'FAIL'}  untouched build passes${clean.status === 0 ? '' : `\n${clean.stdout}${clean.stderr}`}`);

// The other side of the release cases above: with the app on sale, a live
// config is accepted. (Checks only that the pay page is not among the problems,
// so other release findings cannot mask it.)
{
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zaaheen-audit-'));
  try {
    fs.cpSync(DIST, dir, { recursive: true });
    onSaleWith('production', `live_${'b'.repeat(27)}`)(dir);
    const r = spawnSync(process.execPath, [AUDIT, dir, '--release'], { encoding: 'utf8' });
    const out = `${r.stdout}${r.stderr}`;
    const accepted = !/pay\/index\.html/.test(out);
    if (!accepted) failed++;
    console.log(`${accepted ? 'ok  ' : 'FAIL'}  live checkout config accepted on a release build${accepted ? '' : `\n${out}`}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

// While the app is not on sale the deploy leaves /pay out, so a release build
// with a sandbox (or no) checkout config must not fail on it.
{
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zaaheen-audit-'));
  try {
    fs.cpSync(DIST, dir, { recursive: true });
    edit('index.html', (h) => h.replaceAll('https://dl.zaaheen.com/', 'https://example.invalid/'))(dir);
    payConfig('sandbox', `test_${'a'.repeat(27)}`)(dir);
    const r = spawnSync(process.execPath, [AUDIT, dir, '--release'], { encoding: 'utf8' });
    const out = `${r.stdout}${r.stderr}`;
    const accepted = !/pay\/index\.html/.test(out);
    if (!accepted) failed++;
    console.log(`${accepted ? 'ok  ' : 'FAIL'}  sandbox checkout ignored while the app is not on sale${accepted ? '' : `\n${out}`}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

// The other side of the price cases: on sale, with the offers lib/seo.ts writes
// from PRICES, the audit raises nothing about the offers.
{
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zaaheen-audit-'));
  try {
    fs.cpSync(DIST, dir, { recursive: true });
    onSale(dir);
    ldEdit('index.html', (g) => { appNode(g).offers = offersFor(expectedOffers()); })(dir);
    const r = spawnSync(process.execPath, [AUDIT, dir], { encoding: 'utf8' });
    const out = `${r.stdout}${r.stderr}`;
    const accepted = r.status === 0;
    if (!accepted) failed++;
    console.log(`${accepted ? 'ok  ' : 'FAIL'}  on sale with the offers from PRICES passes${accepted ? '' : `\n${out}`}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

// The other side of the date cases: a markup-only change (a class, a heading
// level) changes no words, so it keeps the page's date and passes.
{
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zaaheen-audit-'));
  try {
    fs.cpSync(DIST, dir, { recursive: true });
    edit('terms/index.html', (s) => { const out = s.replace('class="set-row"', 'class="set-row is-new"'); if (out === s) throw new Error('audit.test: no set-row'); return out; })(dir);
    const r = spawnSync(process.execPath, [AUDIT, dir], { encoding: 'utf8' });
    const accepted = r.status === 0;
    if (!accepted) failed++;
    console.log(`${accepted ? 'ok  ' : 'FAIL'}  a markup-only change keeps its date${accepted ? '' : `\n${r.stdout}${r.stderr}`}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

// The other side of the claim cases: the honest, negated wording is allowed.
// (Checks only that no claim is reported: the added words re-date the page.)
{
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zaaheen-audit-'));
  try {
    fs.cpSync(DIST, dir, { recursive: true });
    inject('<p>No system is 100% secure, and nothing online is completely safe.</p>')(dir);
    const r = spawnSync(process.execPath, [AUDIT, dir], { encoding: 'utf8' });
    const out = `${r.stdout}${r.stderr}`;
    const accepted = !/absolute security claim/.test(out);
    if (!accepted) failed++;
    console.log(`${accepted ? 'ok  ' : 'FAIL'}  a negated security claim is allowed${accepted ? '' : `\n${out}`}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

console.log(failed ? `\naudit.test: ${failed} failure(s)` : `\naudit.test: all ${cases.length + 6} passed`);
process.exit(failed ? 1 : 0);
