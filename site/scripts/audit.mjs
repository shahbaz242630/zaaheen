// Post-build audit for zaaheen.com. Read-only. Exit 1 on any failure, so CI
// refuses to deploy a build that would hurt search or AI visibility.
//
//   node scripts/audit.mjs [distDir] [--release]      (default: dist)
//
// --release is the run for a build that will be published: it also refuses
// placeholder copy. Without it placeholders are only counted, so unwritten
// pages can still be built, previewed and reviewed.
//
// Each rule cites the policy it enforces in SEO-HANDOFF.md. Plain regex over our
// own generated HTML: no dependencies, and the markup is ours to keep simple.
import fs from 'node:fs';
import path from 'node:path';

const ORIGIN = 'https://zaaheen.com';
const ACCOUNT_ORIGIN = 'https://account.zaaheen.com';
const args = process.argv.slice(2);
const RELEASE = args.includes('--release');
const DIST = path.resolve(args.find((a) => !a.startsWith('--')) || 'dist');
const INDEXNOW_KEY = 'dc7e96914b463f8b38a2ca7309b9a25f';
// Pages that are never listed in search: no canonical, no JSON-LD, noindex,
// never in the sitemap (section 3, rule 2 names the 404 page as the only
// exception; /pay, Paddle's checkout page, is the second, SIGNIN-DESIGN §5).
const UNLISTED = { '404.html': 'the 404 page', 'pay/index.html': 'the pay page' };
// Section 3, rule 8: no third-party requests. The single exception is the pay
// page loading Paddle.js, which Paddle requires on its default payment link.
const THIRD_PARTY_ALLOWED = { 'pay/index.html': ['https://cdn.paddle.com/paddle/v2/paddle.js'] };
// The pay page's own CSP (public/pay/.htaccess), pinned: widening it is a
// reviewed change to this line, never a quiet edit to the server file. Why each
// Paddle source is there: the comment in public/pay/.htaccess (measured live).
const PAY_CSP =
  "default-src 'self'; script-src 'self' https://cdn.paddle.com; style-src 'self' 'unsafe-inline' https://*.paddle.com; img-src 'self' data:; " +
  "font-src 'self'; connect-src 'self' https://*.paddle.com; frame-src https://*.paddle.com; object-src 'none'; " +
  "frame-ancestors 'none'; base-uri 'self'; form-action 'self'";
const errors = [];
const fail = (where, msg) => errors.push(`${where}: ${msg}`);

if (!fs.existsSync(DIST)) {
  console.error(`audit: ${DIST} does not exist. Run the build first.`);
  process.exit(1);
}

const walk = (dir) =>
  fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const abs = path.join(dir, e.name);
    return e.isDirectory() ? walk(abs) : [path.relative(DIST, abs).split(path.sep).join('/')];
  });
const files = walk(DIST);
const read = (rel) => fs.readFileSync(path.join(DIST, rel), 'utf8');

// --- Required files (SEO-HANDOFF section 6) -------------------------------------
for (const rel of [
  'index.html', '404.html', 'robots.txt', 'sitemap.xml', 'llms.txt', '.htaccess',
  'favicon.ico', 'favicon.svg', 'favicon-192.png', 'apple-touch-icon.png',
  'icon-192.png', 'icon-512.png', 'manifest.webmanifest', 'og.png', `${INDEXNOW_KEY}.txt`,
  'pay/index.html', 'pay/.htaccess',
]) {
  if (!files.includes(rel)) fail(rel, 'required file is missing from the build');
}

// --- /pay: Paddle's default payment link (SIGNIN-DESIGN §5) -----------------------
if (files.includes('pay/.htaccess')) {
  const conf = read('pay/.htaccess');
  const csps = [...conf.matchAll(/^\s*Header\s+always\s+set\s+Content-Security-Policy\s+"([^"]*)"\s*$/gim)].map((m) => m[1]);
  if (csps.length !== 1 || csps[0] !== PAY_CSP) fail('pay/.htaccess', 'the checkout CSP is not the pinned policy (PAY_CSP in scripts/audit.mjs)');
  if (!/^\s*Header\s+always\s+set\s+X-Robots-Tag\s+"noindex"\s*$/im.test(conf)) fail('pay/.htaccess', 'missing the X-Robots-Tag noindex header');
}
// A published build must take live payments: production, with a live token.
// Anything else (no config, the sandbox, a mixed pair) fails on release only,
// so previews and sandbox tests still build. Only while the app is on sale:
// with no installer link on the home page (RELEASE.available false in
// src/data/site.ts), the deploy leaves /pay out of the published site
// (site.yml, the same test), so its checkout settings do not matter yet.
// "On sale" = the home page has a link whose href starts with the installer
// host. An anchored attribute match, not a substring, so a mention of the host
// in text or inside another URL does not count.
const ON_SALE = files.includes('index.html') && /\shref="https:\/\/dl\.zaaheen\.com\//.test(read('index.html'));
if (RELEASE && ON_SALE && files.includes('pay/index.html')) {
  const html = read('pay/index.html');
  const env = (html.match(/\sdata-paddle-env="([^"]*)"/) || [])[1];
  const token = (html.match(/\sdata-paddle-token="([^"]*)"/) || [])[1] || '';
  if (env !== 'production' || !/^live_[a-zA-Z0-9]{27}$/.test(token)) {
    fail('pay/index.html', 'checkout is not set up for live payments (PUBLIC_PADDLE_ENVIRONMENT=production and a live_ client-side token)');
  }
}
// --- The account pages' entry points (AUTH-PAGES-DESIGN D1) -------------------------
// Sign-in and sign-up live on account.zaaheen.com (ACCOUNT in src/data/site.ts).
// Every page's header links there, and /sign-in, /sign-up forward there with
// any query string dropped: a temporary redirect, so it can be moved later.
const ACCOUNT_LINKS = { 'bar-signin': ['Sign in', `${ACCOUNT_ORIGIN}/sign-in/`], 'bar-start': ['Get started', `${ACCOUNT_ORIGIN}/sign-up/`] };
for (const rel of files.filter((f) => f.endsWith('.html'))) {
  const html = read(rel);
  if (!/\bbar-actions\b/.test(html)) continue;
  for (const [cls, [label, want]] of Object.entries(ACCOUNT_LINKS)) {
    const tag = (html.match(new RegExp(`<a\\b[^>]*\\b${cls}\\b[^>]*>`)) || [])[0];
    const href = tag && (tag.match(/\shref="([^"]*)"/) || [])[1];
    if (href !== want) fail(rel, `header "${label}" must link to ${want} (found ${href ?? 'no link'})`);
  }
}
if (files.includes('.htaccess')) {
  const forward = 'RewriteRule ^sign-(in|up)/?$ https://account.zaaheen.com/sign-$1/? [R=302,L]';
  const lines = read('.htaccess').split(/\r?\n/).map((l) => l.trim());
  const mentions = lines.filter((l) => !l.startsWith('#') && /\baccount\.zaaheen\.com\b/.test(l));
  if (mentions.length !== 1 || mentions[0] !== forward) {
    fail('.htaccess', `/sign-in and /sign-up must forward to the account origin, exactly: ${forward}`);
  }
}

if (files.includes(`${INDEXNOW_KEY}.txt`) && read(`${INDEXNOW_KEY}.txt`).trim() !== INDEXNOW_KEY) {
  fail(`${INDEXNOW_KEY}.txt`, 'IndexNow key file does not contain its own key');
}

// --- Nothing private or build-internal is published ------------------------------
for (const rel of files) {
  const base = path.posix.basename(rel);
  if (/\.(md|map|mjs|ts|py|log)$/i.test(base) || /^(\.env.*|package(-lock)?\.json)$/i.test(base)) {
    fail(rel, 'private or build-internal file would be published');
  }
}

// --- robots.txt: never block anyone (section 3, rule 1) -------------------------
if (files.includes('robots.txt')) {
  const robots = read('robots.txt');
  if (!/^User-agent:\s*\*\s*$/im.test(robots)) fail('robots.txt', 'no "User-agent: *" group');
  if (/^\s*Disallow:\s*\S/im.test(robots)) fail('robots.txt', 'contains a Disallow rule; the policy is to allow every crawler');
  if (!robots.includes(`Sitemap: ${ORIGIN}/sitemap.xml`)) fail('robots.txt', 'missing the absolute Sitemap line');
}

// --- Pages -----------------------------------------------------------------------
const htmlFiles = files.filter((f) => f.endsWith('.html'));
const urlFor = (rel) => (rel === 'index.html' ? '/' : rel.endsWith('/index.html') ? `/${rel.slice(0, -10)}` : `/${rel}`);
const all = (html, re) => [...html.matchAll(re)];
const attr = (tag, name) => (tag.match(new RegExp(`\\s${name}="([^"]*)"`, 'i')) || [])[1];
const metaContent = (html, key, val) =>
  all(html, /<meta\b[^>]*>/gi).map((m) => m[0]).filter((t) => attr(t, key) === val).map((t) => attr(t, 'content'));

const indexable = [];
for (const rel of htmlFiles) {
  const html = read(rel);
  const url = urlFor(rel);
  const unlisted = UNLISTED[rel];

  const titles = all(html, /<title>([^<]*)<\/title>/gi).map((m) => m[1].trim());
  if (titles.length !== 1) fail(rel, `expected exactly one <title>, found ${titles.length}`);
  else if (titles[0].length < 10 || titles[0].length > 65) fail(rel, `title is ${titles[0].length} chars (keep 10-65)`);

  const descs = metaContent(html, 'name', 'description');
  if (descs.length !== 1) fail(rel, `expected exactly one meta description, found ${descs.length}`);
  else if (descs[0].length < 50 || descs[0].length > 160) fail(rel, `meta description is ${descs[0].length} chars (keep 50-160)`);

  const h1s = all(html, /<h1\b/gi).length;
  if (h1s !== 1) fail(rel, `expected exactly one <h1>, found ${h1s}`);

  const robotsMeta = metaContent(html, 'name', 'robots').join(',').toLowerCase();
  const canonicals = all(html, /<link\b[^>]*rel="canonical"[^>]*>/gi).map((m) => attr(m[0], 'href'));

  if (unlisted) {
    if (!robotsMeta.includes('noindex')) fail(rel, `${unlisted} must be noindex`);
    if (canonicals.length) fail(rel, `${unlisted} must not declare a canonical`);
  } else {
    indexable.push(url);
    // Section 3, rule 2: nothing that removes a page from search or AI answers.
    for (const bad of ['noindex', 'nosnippet', 'noarchive', 'nocache', 'none']) {
      if (robotsMeta.split(/[\s,]+/).includes(bad)) fail(rel, `robots meta contains "${bad}"`);
    }
    if (/data-nosnippet/i.test(html)) fail(rel, 'data-nosnippet hides text from snippets and AI answers');
    if (canonicals.length !== 1) fail(rel, `expected exactly one canonical, found ${canonicals.length}`);
    else if (canonicals[0] !== `${ORIGIN}${url}`) fail(rel, `canonical ${canonicals[0]} is not ${ORIGIN}${url}`);

    for (const prop of ['og:title', 'og:description', 'og:url', 'og:image']) {
      const v = metaContent(html, 'property', prop);
      if (v.length !== 1 || !v[0]) fail(rel, `missing ${prop}`);
      else if ((prop === 'og:url' || prop === 'og:image') && !v[0].startsWith(`${ORIGIN}/`)) fail(rel, `${prop} is not an absolute ${ORIGIN} URL`);
    }

    const ld = all(html, /<script type="application\/ld\+json">([\s\S]*?)<\/script>/gi);
    if (ld.length !== 1) fail(rel, `expected one JSON-LD block, found ${ld.length}`);
    else {
      try {
        const graph = JSON.parse(ld[0][1])['@graph'];
        if (!Array.isArray(graph) || !graph.length) fail(rel, 'JSON-LD has no @graph');
        const types = graph.map((n) => n['@type']);
        for (const t of ['WebSite', 'Organization', 'WebPage']) if (!types.includes(t)) fail(rel, `JSON-LD lacks ${t}`);
        if (JSON.stringify(graph).match(/"(aggregateRating|review)"/)) fail(rel, 'JSON-LD claims ratings or reviews; a beta has none (section 3, rule 5)');
      } catch (e) {
        fail(rel, `JSON-LD does not parse: ${e.message}`);
      }
    }
  }

  // Section 3, rule 3: links must be real in the HTML, never filled in by script.
  for (const m of all(html, /<a\b[^>]*>/gi)) {
    const href = attr(m[0], 'href');
    if (href === undefined || href === '' || href === '#') fail(rel, `anchor without a real href: ${m[0]}`);
  }

  // Section 3, rule 6: no third-party requests. Anchors may point anywhere;
  // anything the browser loads by itself must come from our own origin.
  for (const m of all(html, /<(script|link|img|iframe|source)\b[^>]*>/gi)) {
    const tag = m[0];
    if (/rel="canonical"/i.test(tag)) continue;
    const ref = attr(tag, 'src') ?? attr(tag, 'href');
    if (!ref || !/^(https?:)?\/\//i.test(ref)) continue;
    const allowed = m[1].toLowerCase() === 'script' && (THIRD_PARTY_ALLOWED[rel] || []).includes(ref);
    if (!allowed) fail(rel, `loads a third-party resource: ${ref}`);
  }

  for (const m of all(html, /<img\b[^>]*>/gi)) {
    if (attr(m[0], 'alt') === undefined) fail(rel, `image without alt: ${m[0]}`);
  }

  // The CSP in public/.htaccess allows only same-origin script and style files.
  // Anything inline would be silently blocked in the browser, so refuse it here.
  // "</script >" and "</script foo>" end a script too, so the end tag may
  // carry anything up to its ">".
  for (const m of all(html, /<script\b([^>]*)>([\s\S]*?)<\/script\b[^>]*>/gi)) {
    const isData = /type="application\/ld\+json"/i.test(m[1]);
    if (!isData && !/\ssrc="/i.test(m[1])) fail(rel, 'inline <script> would be blocked by the CSP');
  }
  if (/<style\b/i.test(html)) fail(rel, 'inline <style> would be blocked by the CSP');
  if (/\sstyle="/i.test(html)) fail(rel, 'inline style attribute would be blocked by the CSP');

  // Astro 7's whitespace handling once rendered "ChooseMore info". Catch a word
  // glued to an inline element's text.
  for (const m of all(html, /[A-Za-z]<(em|strong|a|span|code)\b/g)) {
    const at = m.index ?? 0;
    fail(rel, `missing space before <${m[1]}>: "${html.slice(Math.max(0, at - 20), at + 12)}"`);
  }

  // Internal links and assets must exist in the build.
  for (const m of all(html, /\s(?:href|src)="([^"]+)"/gi)) {
    let ref = m[1];
    if (/^(https?:|mailto:|tel:|data:|#)/i.test(ref)) continue;
    ref = ref.split('#')[0].split('?')[0];
    if (!ref) continue;
    const target = ref.startsWith('/') ? ref.slice(1) : path.posix.join(path.posix.dirname(rel), ref);
    const candidates = [target, `${target.replace(/\/$/, '')}/index.html`];
    if (target === '') candidates.push('index.html');
    if (!candidates.some((c) => files.includes(c))) fail(rel, `broken internal link or asset: ${m[1]}`);
  }
}

// --- sitemap.xml lists exactly the indexable pages ------------------------------
if (files.includes('sitemap.xml')) {
  const locs = all(read('sitemap.xml'), /<loc>([^<]+)<\/loc>/g).map((m) => m[1]);
  for (const loc of locs) {
    if (!loc.startsWith(`${ORIGIN}/`)) fail('sitemap.xml', `non-absolute or foreign <loc>: ${loc}`);
    else if (!indexable.includes(loc.slice(ORIGIN.length))) fail('sitemap.xml', `lists ${loc}, which is not a built indexable page`);
  }
  for (const url of indexable) {
    if (!locs.includes(`${ORIGIN}${url}`)) fail('sitemap.xml', `does not list the indexable page ${url}`);
  }
  for (const m of all(read('sitemap.xml'), /<lastmod>([^<]+)<\/lastmod>/g)) {
    if (Number.isNaN(Date.parse(m[1]))) fail('sitemap.xml', `unparseable lastmod ${m[1]}`);
    else if (Date.parse(m[1]) > Date.now() + 60_000) fail('sitemap.xml', `lastmod in the future: ${m[1]}`);
  }
}

// --- No em dashes in anything a visitor or crawler reads (section 3a, rule 6) ----
// The founder's copy rule: an em dash, or a spaced en dash standing in for one,
// reads as "AI wrote this". An en dash inside a number range (45–65) is fine.
// Page text, titles, descriptions, JSON-LD and llms.txt are all read, so all count.
for (const rel of files.filter((f) => /\.(html|txt|xml)$/i.test(f))) {
  const text = read(rel);
  for (const m of text.matchAll(/—|\s–\s/g)) {
    const at = m.index ?? 0;
    const around = text.slice(Math.max(0, at - 30), at + 30).replace(/\s+/g, ' ');
    fail(rel, `em dash in published text; rewrite the sentence: "${around}"`);
  }
}

// --- Placeholder copy never goes live (section 3, rule 6) ------------------------
// Unwritten copy carries visible "[Placeholder]" text (components/Placeholder.astro).
// A crawler would cache it as fact, so a build that will be published refuses it.
const placeholders = [];
for (const rel of files.filter((f) => /\.(html|txt|xml)$/i.test(f))) {
  const text = read(rel);
  for (const m of text.matchAll(/\[Placeholder\]/g)) {
    const at = m.index ?? 0;
    // The placeholder's own text: up to the next tag, capped for a readable line.
    const snippet = text.slice(at).split('<')[0].replace(/\s+/g, ' ').slice(0, 80).trim();
    placeholders.push({ rel, snippet });
  }
}
if (RELEASE) {
  for (const { rel, snippet } of placeholders) fail(rel, `placeholder text would be published: "${snippet}"`);
}

if (errors.length) {
  console.error(`audit: ${errors.length} problem(s) in ${DIST}\n`);
  for (const e of errors) console.error(`  - ${e}`);
  process.exit(1);
}
console.log(`audit: OK. ${htmlFiles.length} page(s), ${files.length} file(s), ${indexable.length} in the sitemap.`);
if (placeholders.length) {
  console.log(`audit: ${placeholders.length} placeholder(s) left in ${new Set(placeholders.map((p) => p.rel)).size} file(s). Fine for a preview; a --release run refuses them.`);
}
