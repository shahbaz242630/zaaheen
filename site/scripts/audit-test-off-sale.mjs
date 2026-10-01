// Runs scripts/audit.test.mjs against a NOT-on-sale build, whatever
// RELEASE.available says in src/data/site.ts (launch, session 81).
//
// The negative controls start from a build that is not on sale and add the
// on-sale signs themselves (onSale in audit.test.mjs), so they need that
// starting point. Once the real site is on sale, this makes it: a copy of the
// site's sources in .offsale/ (inside site/, so node_modules still resolves),
// RELEASE.available set false, dated with the copy's own page-dates.json (the
// stamp steps), built, and the copy's own audit tests run there. The real,
// on-sale build is checked by `npm run check -- --release` (site.yml).
//   node scripts/audit-test-off-sale.mjs        (from site/)
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const SITE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const COPY = path.join(SITE, '.offsale');
const ASTRO = path.join(SITE, 'node_modules', 'astro', 'bin', 'astro.mjs');
const SKIP = new Set(['node_modules', 'dist', '.astro', '.offsale']);

fs.rmSync(COPY, { recursive: true, force: true });
fs.mkdirSync(COPY);
for (const entry of fs.readdirSync(SITE)) {
  if (SKIP.has(entry) || entry.startsWith('dist')) continue;
  fs.cpSync(path.join(SITE, entry), path.join(COPY, entry), { recursive: true });
}

const siteTs = path.join(COPY, 'src', 'data', 'site.ts');
const source = fs.readFileSync(siteTs, 'utf8');
const offSale = source.replace(/^(\s*available:\s*)true,/m, '$1false,');
if (!/^\s*available:\s*false,/m.test(offSale)) {
  console.error('audit-test-off-sale: RELEASE.available not found in src/data/site.ts');
  process.exit(1);
}
fs.writeFileSync(siteTs, offSale);

const run = (args) => {
  const r = spawnSync(process.execPath, args, { cwd: COPY, stdio: 'inherit' });
  if (r.status !== 0) process.exit(r.status ?? 1);
};
// The stamp steps (package.json "stamp"), on the copy only.
run([ASTRO, 'build', '--silent']);
run(['scripts/page-dates.mjs', '--write']);
run([ASTRO, 'build', '--silent']);
run(['scripts/audit.test.mjs']);
fs.rmSync(COPY, { recursive: true, force: true });
