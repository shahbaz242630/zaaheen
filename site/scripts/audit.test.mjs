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

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '..', 'dist');
const AUDIT = path.join(HERE, 'audit.mjs');

const edit = (rel, fn) => (dir) => {
  const p = path.join(dir, rel);
  fs.writeFileSync(p, fn(fs.readFileSync(p, 'utf8')));
};
const inject = (html) => edit('index.html', (s) => s.replace('</main>', `${html}</main>`));
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

const cases = [
  ['robots.txt blocks crawlers', edit('robots.txt', (s) => s.replace('Allow: /', 'Disallow: /')), /contains a Disallow rule/],
  ['home page noindex', edit('index.html', (s) => s.replace('<title>', '<meta name="robots" content="noindex"><title>')), /robots meta contains "noindex"/],
  ['home page nosnippet', edit('index.html', (s) => s.replace('<title>', '<meta name="robots" content="nosnippet"><title>')), /robots meta contains "nosnippet"/],
  ['canonical removed', edit('index.html', (s) => s.replace(/<link rel="canonical"[^>]*>/, '')), /exactly one canonical, found 0/],
  ['link filled in by script', inject('<a href="#">x</a>'), /anchor without a real href/],
  ['third-party script', edit('index.html', (s) => s.replace('</head>', '<script src="https://cdn.example.com/x.js"></script></head>')), /loads a third-party resource/],
  ['inline script', inject('<script>alert(1)</script>'), /inline <script> would be blocked/],
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
  ['sandbox checkout on a release build', payConfig('sandbox', `test_${'a'.repeat(27)}`), /pay\/index\.html: checkout is not set up for live payments/, ['--release']],
  ['live environment with a sandbox token', payConfig('production', `test_${'a'.repeat(27)}`), /pay\/index\.html: checkout is not set up for live payments/, ['--release']],
  ['sandbox environment with a live token', payConfig('sandbox', `live_${'b'.repeat(27)}`), /pay\/index\.html: checkout is not set up for live payments/, ['--release']],
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

// The other side of the two release cases above: a live config is accepted.
// (A release run of today's build still fails on its placeholder copy, so this
// checks only that the pay page is not among the problems.)
{
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'zaaheen-audit-'));
  try {
    fs.cpSync(DIST, dir, { recursive: true });
    payConfig('production', `live_${'b'.repeat(27)}`)(dir);
    const r = spawnSync(process.execPath, [AUDIT, dir, '--release'], { encoding: 'utf8' });
    const out = `${r.stdout}${r.stderr}`;
    const accepted = !/pay\/index\.html/.test(out);
    if (!accepted) failed++;
    console.log(`${accepted ? 'ok  ' : 'FAIL'}  live checkout config accepted on a release build${accepted ? '' : `\n${out}`}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

console.log(failed ? `\naudit.test: ${failed} failure(s)` : `\naudit.test: all ${cases.length + 2} passed`);
process.exit(failed ? 1 : 0);
