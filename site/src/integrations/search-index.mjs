// The Documents search index (founder, session 83: "search activated on
// documents"). Built after every build from the finished pages, so it always
// matches what visitors read: each section row (components/Row.astro) of the
// app's Documents (the guides, /docs/ and the policies) becomes one entry,
// with its page, heading, link and plain text. The fixes list on the
// troubleshooting page is one entry per error message. Written to
// /search-index.json and read by src/scripts/search.js in the visitor's
// browser: no third party, no server, nothing sent anywhere.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

/** The pages searched: the app's Documents, never the Knowledge Centre. */
export const SEARCHED = (rel) =>
  rel === 'docs/index.html' ||
  /^docs\/[^/]+\/index\.html$/.test(rel) ||
  /^(terms|privacy|refunds|ai-and-your-data|security|company|licences)\/index\.html$/.test(rel);

const NAMED = { nbsp: ' ', amp: '&', lt: '<', gt: '>', quot: '"', apos: "'" };
// One pass, so "&amp;lt;" becomes the text "&lt;", never "<".
export const decode = (s) =>
  s.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (all, e) => {
    if (e[0] !== '#') return NAMED[e.toLowerCase()] ?? all;
    const n = e[1] === 'x' || e[1] === 'X' ? parseInt(e.slice(2), 16) : Number(e.slice(1));
    return n > 0 && n <= 0x10ffff ? String.fromCodePoint(n) : all;
  });
export const text = (html) =>
  decode(html.replace(/<script\b[\s\S]*?<\/script\b[^>]*>/gi, ' ').replace(/<[^>]+>/g, ' '))
    .replace(/\s+/g, ' ')
    .trim();

/** The entries of one built page (`url` like `/docs/troubleshooting/`). */
export function entriesOf(url, html) {
  const main = (html.match(/<main\b[\s\S]*?<\/main>/i) || [html])[0];
  const page = text((main.match(/<h1\b[^>]*>([\s\S]*?)<\/h1>/i) || ['', ''])[1]);
  const out = [];
  const rows = [...main.matchAll(/<div id="([^"]+)" class="set-row">/g)];
  rows.forEach((m, i) => {
    const end = i + 1 < rows.length ? rows[i + 1].index : main.length;
    const body = main.slice(m.index, end);
    const title = text((body.match(/<h[23]\b[^>]*class="set-name"[^>]*>([\s\S]*?)<\/h[23]>/i) || ['', ''])[1]);
    // The fixes list: one entry per error message, so a pasted error finds it.
    const fixes = [...body.matchAll(/<dt\b[^>]*>([\s\S]*?)<\/dt>\s*<dd\b[^>]*>([\s\S]*?)<\/dd>/gi)];
    if (fixes.length) {
      for (const f of fixes) {
        // The error's exact words are the title; where it appears goes with the page.
        const words = text((f[1].match(/<code\b[^>]*>([\s\S]*?)<\/code>/i) || ['', f[1]])[1]);
        const where = text((f[1].match(/class="set-where"[^>]*>([\s\S]*?)<\/span>/i) || ['', ''])[1]);
        out.push({ url: `${url}#${m[1]}`, page: where ? `${page} · ${where}` : page, title: words, text: text(f[2]) });
      }
      return;
    }
    out.push({ url: `${url}#${m[1]}`, page, title, text: text(body).slice(title.length).trim().slice(0, 600) });
  });
  return out;
}

export default function searchIndex() {
  return {
    name: 'zaaheen-search-index',
    hooks: {
      'astro:build:done': ({ dir }) => {
        const root = fileURLToPath(dir);
        const walk = (d) =>
          fs.readdirSync(d, { withFileTypes: true }).flatMap((e) => {
            const abs = path.join(d, e.name);
            return e.isDirectory() ? walk(abs) : [path.relative(root, abs).split(path.sep).join('/')];
          });
        const entries = walk(root)
          .filter(SEARCHED)
          .sort()
          .flatMap((rel) => entriesOf(`/${rel.replace(/index\.html$/, '')}`, fs.readFileSync(path.join(root, rel), 'utf8')));
        fs.writeFileSync(path.join(root, 'search-index.json'), JSON.stringify(entries));
      },
    },
  };
}
