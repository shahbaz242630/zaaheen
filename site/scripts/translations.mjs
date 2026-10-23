// Keeps the translations in step with the English (ADR-113 D6, notes/I18N-DESIGN.md).
//
//   npm run translations              check (scripts/audit.mjs runs this too)
//   npm run translations -- --write   record the English each translation now matches
//
// src/i18n/translated-from.json holds, for every translated page, the text
// fingerprint (scripts/page-dates.mjs) of the English page it was translated
// from ("en") and of the translation itself ("own"). The check fails when:
//   - a page listed in TRANSLATED (src/i18n/locales.ts) is not built in a language;
//   - its English page's words changed since that translation was recorded;
//   - a translated page does not name all its language versions (hreflang);
//   - a shared phrase's translation lost a {placeholder} or a tag (src/i18n/ui/*.ts).
// So an English wording change cannot merge until its four translations are
// updated in the same change, and only then is --write run. --write refuses a
// page whose English changed while its own words did not: recording alone
// cannot pass the check (independent review, s109). Shared words need no
// record: src/i18n/ui.ts keys them by their English, so a changed phrase has
// no translation and the build itself fails.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { fingerprint, fileFor } from './page-dates.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SITE = path.resolve(HERE, '..');
export const RECORD_FILE = path.join(SITE, 'src', 'i18n', 'translated-from.json');
const LOCALES_TS = path.join(SITE, 'src', 'i18n', 'locales.ts');
const ORIGIN = 'https://zaaheen.com';

/** TRANSLATIONS and TRANSLATED from src/i18n/locales.ts. A reshaped declaration fails loudly. */
export function readLocales() {
  const src = fs.readFileSync(LOCALES_TS, 'utf8');
  const langs = src.match(/export const TRANSLATIONS = \[([^\]]*)\]/);
  const pages = src.match(/export const TRANSLATED: readonly string\[\] = \[([^\]]*)\]/);
  if (!langs || !pages) throw new Error(`translations: cannot read TRANSLATIONS / TRANSLATED from ${LOCALES_TS}; update the pattern with the declaration`);
  const strings = (s) => [...s.matchAll(/'([^']*)'/g)].map((m) => m[1]);
  return { langs: strings(langs[1]), pages: strings(pages[1]) };
}

/**
 * Shared phrases whose translation lost or gained a {placeholder} or an HTML
 * tag: t() would leave the value out, or the markup would break. Reads the
 * dictionaries themselves (Node strips their types).
 */
export async function phraseProblems(langs = readLocales().langs) {
  const problems = [];
  const marks = (s) => [...s.matchAll(/\{\w+\}|<\/?[a-z]+\b/g)].map((m) => m[0]).sort().join(' ');
  for (const lang of langs) {
    const file = path.join(SITE, 'src', 'i18n', 'ui', `${lang}.ts`);
    const dict = (await import(pathToFileURL(file).href)).default;
    for (const [english, translated] of Object.entries(dict)) {
      if (marks(english) !== marks(translated)) {
        problems.push([`src/i18n/ui/${lang}.ts`, `"${english.slice(0, 60)}": the translation's placeholders or tags (${marks(translated) || 'none'}) differ from the English (${marks(english) || 'none'})`]);
      }
    }
  }
  return problems;
}

export function readRecord() {
  return fs.existsSync(RECORD_FILE) ? JSON.parse(fs.readFileSync(RECORD_FILE, 'utf8')) : {};
}

const exists = (dist, url) => fs.existsSync(fileFor(dist, url));

/** Every problem with the translations in a build, as [where, message] pairs. */
export function translationProblems(dist, { langs, pages } = readLocales(), record = readRecord()) {
  const problems = [];
  for (const en of pages) {
    if (!exists(dist, en)) {
      problems.push([en, 'is listed in TRANSLATED (src/i18n/locales.ts) but has no English page']);
      continue;
    }
    const current = fingerprint(fs.readFileSync(fileFor(dist, en), 'utf8'));
    for (const lang of langs) {
      const url = `/${lang}${en}`;
      if (!exists(dist, url)) {
        problems.push([url, `is missing: ${en} must exist in every language (src/pages/${lang}${en === '/' ? '/index' : en.replace(/\/$/, '')}.astro)`]);
        continue;
      }
      const html = fs.readFileSync(fileFor(dist, url), 'utf8');
      const rec = record[url];
      if (!rec) problems.push([url, 'has no recorded English original: once it is translated, run npm run translations -- --write']);
      else if (rec.en !== current) {
        problems.push([url, rec.own === fingerprint(html)
          ? `the English page ${en} has changed since this translation, and the translation has not: update it, then run npm run translations -- --write`
          : `the English page ${en} and this translation both changed: run npm run translations -- --write`]);
      } else if (rec.own !== fingerprint(html)) {
        problems.push([url, 'its words changed since they were recorded: run npm run translations -- --write']);
      }
      // The page's hreflang alternates, by a fixed pattern; compared as strings.
      const alternates = new Set([...html.matchAll(/<link rel="alternate" hreflang="[^"]+" href="([^"]+)">/g)].map((m) => m[1]));
      for (const l of ['en', ...langs]) {
        const href = `${ORIGIN}${l === 'en' ? en : `/${l}${en}`}`;
        if (!alternates.has(href)) {
          problems.push([url, `does not name its ${l} version (${href}) in a hreflang alternate`]);
        }
      }
    }
  }
  for (const url of Object.keys(record)) {
    const m = url.match(/^\/([a-z]{2})(\/.*)$/);
    if (!m || !langs.includes(m[1]) || !pages.includes(m[2])) problems.push([url, 'is recorded in src/i18n/translated-from.json but is not a translated page: run npm run translations -- --write']);
  }
  return problems;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const dist = path.join(SITE, 'dist');
  const locales = readLocales();
  if (process.argv.includes('--write')) {
    // --fresh: start over (scripts/audit-test-off-sale.mjs, whose copy changes
    // the English on purpose); never for a real change.
    const old = process.argv.includes('--fresh') ? {} : readRecord();
    const record = {};
    for (const en of locales.pages) {
      const current = fingerprint(fs.readFileSync(fileFor(dist, en), 'utf8'));
      for (const lang of locales.langs) {
        const url = `/${lang}${en}`;
        if (!exists(dist, url)) continue;
        const own = fingerprint(fs.readFileSync(fileFor(dist, url), 'utf8'));
        // The English moved on and this translation did not: keep the old
        // record, so the check still fails until the page is translated again.
        if (old[url] && old[url].en !== current && old[url].own === own) {
          record[url] = old[url];
          console.error(`translations: not recording ${url}: its English (${en}) changed but it did not`);
          continue;
        }
        record[url] = { en: current, own };
      }
    }
    const sorted = Object.fromEntries(Object.keys(record).sort().map((k) => [k, record[k]]));
    fs.writeFileSync(RECORD_FILE, `${JSON.stringify(sorted, null, 2)}\n`);
    console.log(`translations: recorded ${Object.keys(sorted).length} translated page(s)`);
  }
  const problems = [...translationProblems(dist, locales), ...(await phraseProblems(locales.langs))];
  for (const [where, message] of problems) console.error(`translations: ${where} ${message}`);
  if (!problems.length) console.log(`translations: OK. ${locales.pages.length} page(s) in ${locales.langs.length} language(s), all current.`);
  process.exitCode = problems.length ? 1 : 0;
}
