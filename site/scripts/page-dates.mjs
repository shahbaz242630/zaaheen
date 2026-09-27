// Each listed page's "last updated" date, moved only when the words on that
// page change (SEO audit, session 72).
//
//   npm run stamp        build, record every changed page's text, build again
//
// The date feeds the sitemap's <lastmod>, the JSON-LD dateModified and the
// "Last updated" line on the documents. Google and Bing use <lastmod> only
// while it is "consistently and verifiably accurate", and a policy's "Last
// updated" is a statement to its readers. A git date of the page's source files
// was neither: every page reads shared data (src/data/site.ts) and a shared
// layout, so an edit to the home page FAQ, or the footer, re-dated the Terms.
//
// So the date follows a fingerprint of what a reader and a crawler actually
// get: the title, the meta description and the text of <main>, less the "Last
// updated" line itself. src/data/page-dates.json holds the fingerprint and date
// of every listed page. scripts/audit.mjs fails the build when a page's text no
// longer matches its fingerprint, so a changed page cannot keep an old date and
// an unchanged one cannot gain a new one. Markup-only changes (a heading level,
// a class, a link target) leave the date alone: they change no words.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const DATES_FILE = path.resolve(HERE, '..', 'src', 'data', 'page-dates.json');

const decode = (s) => s
  .replace(/&#x([0-9a-f]+);/gi, (_, h) => String.fromCodePoint(parseInt(h, 16)))
  .replace(/&#(\d+);/g, (_, d) => String.fromCodePoint(Number(d)))
  .replace(/&quot;/g, '"').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&nbsp;/g, ' ').replace(/&amp;/g, '&');

/** The words a page shows: title, meta description and <main> text, whitespace collapsed. */
export function pageText(html) {
  const title = (html.match(/<title>([^<]*)<\/title>/i) || [])[1] || '';
  const desc = (html.match(/<meta\b[^>]*\sname="description"[^>]*\scontent="([^"]*)"/i) || [])[1] || '';
  const main = ((html.match(/<main\b[^>]*>([\s\S]*)<\/main>/i) || [])[1] || '')
    .replace(/<p class="doc-updated">[\s\S]*?<\/p>/gi, '')
    .replace(/<script\b[\s\S]*?<\/script\b[^>]*>/gi, '')
    .replace(/<[^>]+>/g, ' ');
  return decode(`${title}\n${desc}\n${main}`).replace(/\s+/g, ' ').trim();
}

export const fingerprint = (html) => crypto.createHash('sha256').update(pageText(html)).digest('hex').slice(0, 16);

export function readDates() {
  return fs.existsSync(DATES_FILE) ? JSON.parse(fs.readFileSync(DATES_FILE, 'utf8')) : {};
}

/** The built HTML file for a listed page path such as '/terms/'. */
export const fileFor = (dist, url) => path.join(dist, url.replace(/^\//, ''), 'index.html');

/**
 * Pages whose recorded fingerprint is missing or no longer matches the build,
 * and recorded pages that are no longer built.
 */
export function staleDates(dist, urls, dates = readDates()) {
  const changed = urls.filter((u) => dates[u]?.text !== fingerprint(fs.readFileSync(fileFor(dist, u), 'utf8')));
  const gone = Object.keys(dates).filter((u) => !urls.includes(u));
  return { changed, gone };
}

// The listed pages are the sitemap's, so this and the audit agree on the set.
function listedUrls(dist) {
  const xml = fs.readFileSync(path.join(dist, 'sitemap.xml'), 'utf8');
  return [...xml.matchAll(/<loc>https:\/\/zaaheen\.com([^<]*)<\/loc>/g)].map((m) => m[1]);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const dist = path.resolve(HERE, '..', 'dist');
  const urls = listedUrls(dist);
  const dates = readDates();
  const { changed, gone } = staleDates(dist, urls, dates);
  if (!process.argv.includes('--write')) {
    for (const u of changed) console.error(`page-dates: ${u} changed since its date`);
    for (const u of gone) console.error(`page-dates: ${u} is recorded but no longer built`);
    process.exit(changed.length || gone.length ? 1 : 0);
  }
  // Seconds precision, UTC: stable, and what sitemaps and schema.org both accept.
  const now = new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
  for (const u of changed) dates[u] = { text: fingerprint(fs.readFileSync(fileFor(dist, u), 'utf8')), date: now };
  for (const u of gone) delete dates[u];
  const sorted = Object.fromEntries(Object.keys(dates).sort().map((u) => [u, dates[u]]));
  fs.writeFileSync(DATES_FILE, `${JSON.stringify(sorted, null, 2)}\n`);
  console.log(changed.length ? `page-dates: re-dated ${changed.length} page(s): ${changed.join(', ')}` : 'page-dates: no page text changed');
  if (gone.length) console.log(`page-dates: dropped ${gone.join(', ')}`);
}
