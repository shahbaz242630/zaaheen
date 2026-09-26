// Negative controls for scripts/audit-account.mjs (AUTH-PAGES-DESIGN D1, D3,
// D5, S1-1, S2-1). Each case breaks a copy of the real account build in one
// way and asserts the audit exits 1 naming exactly that problem; then the
// untouched build must pass, and a development build must fail --release.
//
//   npm run build:account:test && node scripts/audit-account.test.mjs   (from site/)
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { CLERK_JS } from '../account/scripts/clerk-pin.js';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '..', 'dist-account');
const AUDIT = path.join(HERE, 'audit-account.mjs');
const PAGES = ['sign-in/index.html', 'sign-up/index.html', 'sso-callback/index.html'];

const edit = (rel, fn) => (dir) => {
  const p = path.join(dir, rel);
  const before = fs.readFileSync(p, 'utf8');
  const after = fn(before);
  if (after === before) throw new Error(`audit-account.test: the edit to ${rel} changed nothing`);
  fs.writeFileSync(p, after);
};
const signIn = (fn) => edit('sign-in/index.html', fn);
const htaccess = (fn) => edit('.htaccess', fn);
const firstJs = (dir) => fs.readdirSync(path.join(dir, '_astro')).find((f) => f.endsWith('.js'));

const cases = [
  // Headers and the CSP (D5, S1-1).
  ['CSP widened', htaccess((s) => s.replace("style-src 'self'", "style-src 'self' 'unsafe-inline'")), /CSP is not the pinned policy/],
  ['Trusted Types dropped', htaccess((s) => s.replace("; require-trusted-types-for 'script'", '')), /CSP is not the pinned policy/],
  ['second CSP header', htaccess((s) => s.replace('Header always set Cache-Control', 'Header always set Content-Security-Policy "default-src *"\n  Header always set Cache-Control')), /CSP is not the pinned policy/],
  ['noindex header missing', htaccess((s) => s.replace(/^.*X-Robots-Tag.*$/m, '')), /X-Robots-Tag noindex/],
  ['COOP missing', htaccess((s) => s.replace(/^.*Cross-Origin-Opener-Policy.*$/m, '')), /Cross-Origin-Opener-Policy/],
  ['framing allowed', htaccess((s) => s.replace('"DENY"', '"SAMEORIGIN"')), /X-Frame-Options/],
  ['referrer sent', htaccess((s) => s.replace('"no-referrer"', '"origin"')), /Referrer-Policy/],
  ['HSTS without subdomains', htaccess((s) => s.replace('max-age=31536000; includeSubDomains', 'max-age=31536000')), /Strict-Transport-Security/],
  ['CSP unset afterwards', htaccess((s) => s.replace(/(Header always set Cache-Control[^\n]*\n)/, '$1  Header always unset Content-Security-Policy\n')), /not the pinned template/],
  ['headers switched off', htaccess((s) => s.replace('<IfModule mod_headers.c>', '<IfModule mod_nothing.c>')), /not the pinned template/],
  ['rewrite rule added', htaccess((s) => s.replace('RewriteEngine On', 'RewriteEngine On\n  RewriteRule ^x$ https://evil.example/ [R=302,L]')), /not the pinned template/],
  // The Trusted Types policy file (S1-1).
  ['tt.js loosened', edit('tt.js', (s) => s.replace('if (url === BOT_CHECK) return url;', 'return url;')), /tt\.js is not the pinned policy/],
  ['tt.js not first', signIn((s) => s.replace('<script src="/tt.js"></script>', '').replace('</head>', '<script src="/tt.js"></script></head>')), /tt\.js must be the first script/],
  // Clerk's file (D3, S2-1).
  ['Clerk file changed', (d) => fs.appendFileSync(path.join(d, 'clerk', CLERK_JS.file), '\n//x'), /Clerk file does not match its pinned sha256/],
  ['extra file in clerk/', (d) => fs.writeFileSync(path.join(d, 'clerk', 'chunk.js'), 'x'), /unexpected file in clerk\//],
  ['Clerk from a CDN', signIn((s) => s.replace(`src="/clerk/${CLERK_JS.file}"`, 'src="https://x.clerk.accounts.dev/npm/@clerk/clerk-js@6/dist/clerk.browser.js"')), /third-party script/],
  // Pages (D1, D5).
  ['third-party script', signIn((s) => s.replace('</head>', '<script src="https://cdn.example.com/x.js"></script></head>')), /third-party script/],
  ['inline script', signIn((s) => s.replace('</body>', '<script>alert(1)</script></body>')), /inline <script>/],
  ['inline script, spaced end tag', signIn((s) => s.replace('</body>', '<script>alert(1)</script ></body>')), /inline <script>/],
  ['style block', signIn((s) => s.replace('</head>', '<style>p{}</style></head>')), /inline <style>/],
  ['style attribute', signIn((s) => s.replace('<main class="acct">', '<main class="acct" style="color:red">')), /inline style attribute/],
  ['iframe', signIn((s) => s.replace('</main>', '<iframe src="/x"></iframe></main>')), /<iframe>/],
  ['indexable page', signIn((s) => s.replace('content="noindex, nofollow"', 'content="index"')), /robots meta noindex/],
  ['captcha element missing', edit('sso-callback/index.html', (s) => s.replace('id="clerk-captcha"', 'id="gone"')), /clerk-captcha/],
  ['sitemap published', (d) => fs.writeFileSync(path.join(d, 'sitemap.xml'), '<urlset/>'), /must not publish sitemap\.xml/],
  ['robots allows crawling', edit('robots.txt', (s) => s.replace('Disallow: /', 'Allow: /')), /robots\.txt must disallow everything/],
  ['page missing', (d) => fs.rmSync(path.join(d, 'sso-callback', 'index.html')), /sso-callback\/index\.html: required file is missing/],
  // Build settings (D3).
  ['key and client missing', signIn((s) => s.replace(/data-pk="[^"]*"/, 'data-pk=""')), /build settings are missing or invalid/],
  ['pages disagree on the key', edit('sign-up/index.html', (s) => s.replace(/data-client-id="[^"]*"/, 'data-client-id="other"')), /pages disagree on the build settings/],
  // The bundle (ADR-SEC-034 supply chain).
  ['eval in our bundle', (d) => fs.appendFileSync(path.join(d, '_astro', firstJs(d)), '\neval("1")'), /forbidden pattern/],
  ['new Function in our bundle', (d) => fs.appendFileSync(path.join(d, '_astro', firstJs(d)), '\nnew Function("x")'), /forbidden pattern/],
  ['innerHTML in our bundle', (d) => fs.appendFileSync(path.join(d, '_astro', firstJs(d)), '\ndocument.body.innerHTML="x"'), /forbidden pattern/],
  ['eval in the Clerk file', (d) => fs.appendFileSync(path.join(d, 'clerk', CLERK_JS.file), '\neval("1")'), /forbidden pattern|sha256/],
];

const run = (dir, ...flags) => spawnSync(process.execPath, [AUDIT, dir, ...flags], { encoding: 'utf8' });
const copy = () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'audit-account-'));
  fs.cpSync(DIST, dir, { recursive: true });
  return dir;
};

if (!fs.existsSync(DIST)) {
  console.error('audit-account.test: dist-account/ is missing. Run npm run build:account:test first.');
  process.exit(1);
}
let failures = 0;
for (const [name, breakIt, expect] of cases) {
  const dir = copy();
  try {
    breakIt(dir);
    const r = run(dir);
    const out = `${r.stdout}${r.stderr}`;
    if (r.status !== 1 || !expect.test(out)) {
      failures++;
      console.error(`FAIL ${name}: exit ${r.status}, expected ${expect}\n${out}`);
    } else {
      console.log(`ok   ${name}`);
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}
const clean = run(DIST);
if (clean.status !== 0) {
  failures++;
  console.error(`FAIL the untouched build must pass:\n${clean.stdout}${clean.stderr}`);
} else console.log('ok   the untouched build passes');
// This test build is a development build: --release must refuse it (D3).
const release = run(DIST, '--release');
if (release.status !== 1 || !/release needs a pk_live_ key for clerk\.zaaheen\.com/.test(`${release.stdout}${release.stderr}`)) {
  failures++;
  console.error(`FAIL --release must refuse a development build:\n${release.stdout}${release.stderr}`);
} else console.log('ok   --release refuses a development build');
console.log(failures ? `\n${failures} failing` : `\nall ${cases.length + 2} passed`);
process.exit(failures ? 1 : 0);
