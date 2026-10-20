// Cloudflare's `_headers` and `_redirects`, written at build time from the
// .htaccess files (session 105, WEB-SECURITY-DESIGN.md ADR-SEC-047).
//
// Why Cloudflare: Hostinger's shared servers answer 429 to "GPTBot/1.x" and
// "meta-externalagent/1.x" by name, and Hostinger cannot change that per account
// (support, s105). So zaaheen.com and account.zaaheen.com are served by
// Cloudflare as static assets on Workers (Pages' successor; the wrangler configs
// in site/cloudflare/); the coaching app stays on Hostinger.
//
// The .htaccess files stay the one source of truth: every audit, test and live
// check written for them keeps working, and the Hostinger copy stays a working
// fallback while both are published. These functions only translate. The audits
// rebuild each file from the built .htaccess and fail on any difference, so a
// hand edit to either side cannot drift.
//
// Cloudflare applies EVERY matching `_headers` rule, joining repeated names with
// a comma, so a folder's stricter or looser value must first detach the
// site-wide one with `! Name`. Two Content-Security-Policy values would both be
// enforced, and /pay's Paddle checkout would break.

import fs from 'node:fs';
import path from 'node:path';

const SET = /^\s*Header\s+always\s+set\s+([A-Za-z0-9-]+)\s+"([^"]*)"\s*$/;
// The two per-file cache lines of the root .htaccess, which siteHeaders models
// with its own /* and /_astro/* rules.
const CACHE_LINES = [
  'Header set Cache-Control "no-cache"',
  'Header set Cache-Control "public, max-age=31536000, immutable"',
];

/**
 * Every `Header always set` line, HSTS included, as [name, value] in file order.
 * Any other Header directive (unset, append, edit, a condition, a new cache
 * line) throws: it would otherwise be dropped on Cloudflare in silence, and the
 * audits, which translate with this same function, could not see it.
 */
export function alwaysSet(htaccess) {
  const out = [];
  for (const line of htaccess.split(/\r?\n/)) {
    if (!/^\s*Header\b/i.test(line)) continue;
    const m = SET.exec(line);
    if (m) out.push([m[1], m[2]]);
    else if (!CACHE_LINES.includes(line.trim())) {
      throw new Error(`cloudflare-files: no Cloudflare translation for "${line.trim()}" (scripts/cloudflare-files.mjs)`);
    }
  }
  return out;
}

// In case Cloudflare adds it: the sites are not an API, so no other origin
// needs to read them.
const DROPPED = ['Access-Control-Allow-Origin'];

// The workers.dev copies (used only to test before a switch; off in the
// wrangler configs afterwards) must never be indexed beside the real site.
const PREVIEWS = ['https://:worker.:subdomain.workers.dev/*'];

const HEAD = '# Generated from .htaccess by scripts/cloudflare-files.mjs at build time. Do not edit.';

const block = (pattern, lines) => [pattern, ...lines.map((l) => `  ${l}`)];

function previewBlocks() {
  return PREVIEWS.flatMap((p) => block(p, ['! X-Robots-Tag', 'X-Robots-Tag: noindex']));
}

// AddCharset lines: Cloudflare does not add a charset to text files, so the
// rule becomes an explicit Content-Type (measured s105: llms.txt, which carries
// "·", went out as bare text/plain on both hosts). An extension without a
// known base type throws, like any other directive with no translation.
const BASE_TYPES = { txt: 'text/plain', xml: 'application/xml' };

/** `AddCharset <charset> .ext ...` -> one `/*.ext` rule per extension. */
export function charsetRules(htaccess) {
  const out = [];
  for (const line of htaccess.split(/\r?\n/)) {
    const m = /^\s*AddCharset\s+(\S+)\s+(.+?)\s*$/i.exec(line);
    if (!m) continue;
    for (const raw of m[2].split(/\s+/)) {
      const ext = raw.replace(/^\./, '').toLowerCase();
      if (!BASE_TYPES[ext]) throw new Error(`cloudflare-files: no Cloudflare translation for "${line.trim()}" (scripts/cloudflare-files.mjs)`);
      out.push(...block(`/*.${ext}`, ['! Content-Type', `Content-Type: ${BASE_TYPES[ext]}; charset=${m[1].toLowerCase()}`]));
    }
  }
  return out;
}

/** Every file under `dir`, as forward-slash paths relative to it. */
export function builtFiles(dir) {
  const walk = (d) => fs.readdirSync(d, { withFileTypes: true }).flatMap((e) => {
    const abs = path.join(d, e.name);
    return e.isDirectory() ? walk(abs) : [path.relative(dir, abs).split(path.sep).join('/')];
  });
  return walk(dir);
}

/** Every built page's folder, '' for the home page, from a build's file list. */
export function pageFolders(files) {
  return files
    .filter((f) => f === 'index.html' || f.endsWith('/index.html'))
    .map((f) => f.slice(0, -'index.html'.length))
    .sort();
}

/**
 * One permanent hop to each page's canonical address: /x and /x/index.html go
 * to /x/, /index.html to /. Cloudflare's own trailing-slash handling answers
 * 307 (temporary); explicit _redirects lines win over it (measured s105), so
 * search engines get the 301 the old server sent.
 */
function canonicalRedirects(folders) {
  const out = [];
  for (const dir of folders) {
    const page = `/${dir}`;
    out.push(`${page}index.html ${page} 301`);
    if (dir) out.push(`${page.slice(0, -1)} ${page} 301`);
  }
  return out;
}

/**
 * zaaheen.com: the root .htaccess for every path, /pay's own on top of it
 * (null when /pay is not published), and the cache rules of its FilesMatch
 * blocks: pages, robots.txt, sitemap.xml and llms.txt revalidate; /_astro/
 * files carry a content hash in their name and are cached for a year.
 */
export function siteHeaders(rootHtaccess, payHtaccess) {
  const root = alwaysSet(rootHtaccess);
  const lines = [HEAD];
  lines.push(...block('/*', [
    ...root.map(([n, v]) => `${n}: ${v}`),
    'Cache-Control: no-cache',
    ...DROPPED.map((n) => `! ${n}`),
  ]));
  lines.push(...block('/_astro/*', ['! Cache-Control', 'Cache-Control: public, max-age=31536000, immutable']));
  lines.push(...charsetRules(rootHtaccess));
  if (payHtaccess !== null) {
    const rootNames = new Set(root.map(([n]) => n.toLowerCase()));
    const pay = alwaysSet(payHtaccess);
    lines.push(...block('/pay/*', [
      ...pay.filter(([n]) => rootNames.has(n.toLowerCase())).map(([n]) => `! ${n}`),
      ...pay.map(([n, v]) => `${n}: ${v}`),
    ]));
  }
  lines.push(...previewBlocks());
  return `${lines.join('\n')}\n`;
}

/** account.zaaheen.com: one policy for the whole origin (its built .htaccess). */
export function accountHeaders(builtHtaccess) {
  const lines = [HEAD];
  lines.push(...block('/*', [
    ...alwaysSet(builtHtaccess).map(([n, v]) => `${n}: ${v}`),
    ...DROPPED.map((n) => `! ${n}`),
  ]));
  lines.push(...charsetRules(builtHtaccess));
  lines.push(...previewBlocks());
  return `${lines.join('\n')}\n`;
}

/**
 * zaaheen.com's redirects. Sign-in and sign-up live on the account origin
 * (AUTH-PAGES-DESIGN D1), as the .htaccess RewriteRule says: temporary, and the
 * visitor's query string is never carried onto the sign-in page, so no
 * redirect_url can arrive there from this site. Cloudflare appends the visitor's
 * query to a target that has none (measured s105: ?redirect_url=... arrived);
 * a target with its own query replaces it. Hence the fixed ?from=website, which
 * the account pages ignore (they read only redirect_url and their own markers).
 * scripts/post-deploy.mjs proves it on the served site. The www form is
 * redirected by a Cloudflare rule, since the static assets see only their own
 * hosts. Then each page's canonical redirects (`folders`: pageFolders()).
 */
export function siteRedirects(folders) {
  return [
    '# Generated by scripts/cloudflare-files.mjs at build time. Do not edit.',
    '/sign-in https://account.zaaheen.com/sign-in/?from=website 302',
    '/sign-in/ https://account.zaaheen.com/sign-in/?from=website 302',
    '/sign-up https://account.zaaheen.com/sign-up/?from=website 302',
    '/sign-up/ https://account.zaaheen.com/sign-up/?from=website 302',
    ...canonicalRedirects(folders),
    '',
  ].join('\n');
}

/**
 * account.zaaheen.com's front door is sign-in (its .htaccess: RewriteRule ^$
 * /sign-in/), then each page's canonical redirects.
 */
export function accountRedirects(folders) {
  return [
    '# Generated by scripts/cloudflare-files.mjs at build time. Do not edit.',
    '/ /sign-in/ 302',
    ...canonicalRedirects(folders),
    '',
  ].join('\n');
}
