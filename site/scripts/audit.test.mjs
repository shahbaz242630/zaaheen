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

console.log(failed ? `\naudit.test: ${failed} failure(s)` : `\naudit.test: all ${cases.length + 1} passed`);
process.exit(failed ? 1 : 0);
