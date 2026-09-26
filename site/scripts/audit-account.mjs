// Post-build audit for account.zaaheen.com (AUTH-PAGES-DESIGN D1, D3, D5,
// ADR-SEC-034, amendments S1-1 and S2-1). Read-only; exit 1 on any failure.
//
//   node scripts/audit-account.mjs [distDir] [--release]   (default: dist-account)
//
// --release is the run for a build that will be published: it also requires a
// production key (pk_live_) for the production Frontend API host.
// scripts/audit-account.test.mjs proves each rule still catches what it claims.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { readAccountConfig } from '../account/scripts/redirect.js';
import { accountCsp, PRODUCTION_FAPI_HOST } from '../account/scripts/csp.js';
import { CLERK_JS } from '../account/scripts/clerk-pin.js';

const args = process.argv.slice(2);
const RELEASE = args.includes('--release');
const DIST = path.resolve(args.find((a) => !a.startsWith('--')) || 'dist-account');
// account-public/tt.js, the origin's only Trusted Types policy (S1-1), pinned:
// loosening it is a reviewed change to this line. Hashed with LF line endings.
const TT_SHA256 = 'ae1d1042194145e894e023f94d3eeb44ec4f20aa85c626f69da0a95f7b36280c';
// account/htaccess.template, the whole server file with its policy line as
// {{CSP}}, pinned the same way: an added "Header unset", "edit", or a renamed
// <IfModule> that silently drops every header is a change to this line
// (independent review, session 65, finding 4).
const HTACCESS_SHA256 = '8f5c1c70cd666bb721b2d529219d02a8e6a21ff86784cca53a9f7c5c587bc593';
const PAGES = ['sign-in/index.html', 'sign-up/index.html', 'sso-callback/index.html'];
// Patterns no script we serve may contain. innerHTML and friends only in our
// own code: Clerk's file carries its UI library's (unused here, and refused at
// run time by tt.js); eval and new Function in neither.
const FORBIDDEN_OURS = [/\beval\s*\(/, /\bnew\s+Function\b/, /setAttribute\(\s*["']style["']/, /\.innerHTML\b/, /\.outerHTML\s*=/, /insertAdjacentHTML/, /document\.write/];
const FORBIDDEN_CLERK = [/\beval\s*\(/, /\bnew\s+Function\b/, /setAttribute\(\s*["']style["']/];

const errors = [];
const fail = (where, msg) => errors.push(`${where}: ${msg}`);
if (!fs.existsSync(DIST)) {
  console.error(`audit-account: ${DIST} does not exist. Run the account build first.`);
  process.exit(1);
}
const walk = (dir) => fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
  const abs = path.join(dir, e.name);
  return e.isDirectory() ? walk(abs) : [path.relative(DIST, abs).split(path.sep).join('/')];
});
const files = walk(DIST);
const read = (rel) => fs.readFileSync(path.join(DIST, rel), 'utf8');
const sha256 = (buf) => crypto.createHash('sha256').update(buf).digest('hex');

// --- Required and forbidden files -------------------------------------------------
for (const rel of [...PAGES, '404.html', '.htaccess', 'tt.js', 'robots.txt', `clerk/${CLERK_JS.file}`]) {
  if (!files.includes(rel)) fail(rel, 'required file is missing from the build');
}
for (const rel of ['sitemap.xml', 'llms.txt']) {
  if (files.includes(rel)) fail(rel, `the account origin must not publish ${rel} (D1: nothing listed)`);
}
if (files.includes('robots.txt') && !/^User-agent: \*\s*\nDisallow: \/\s*$/m.test(read('robots.txt'))) {
  fail('robots.txt', 'robots.txt must disallow everything');
}

// --- Build settings (D3) -----------------------------------------------------------
const settings = PAGES.filter((rel) => files.includes(rel)).map((rel) => {
  const html = read(rel);
  return { rel, pk: (html.match(/\sdata-pk="([^"]*)"/) || [])[1] || '', clientId: (html.match(/\sdata-client-id="([^"]*)"/) || [])[1] || '' };
});
const first = settings[0] || { pk: '', clientId: '' };
if (settings.some((s) => s.pk !== first.pk || s.clientId !== first.clientId)) fail('pages', 'the pages disagree on the build settings');
const config = readAccountConfig({ publishableKey: first.pk, clientId: first.clientId });
if (!config) fail('pages', 'the build settings are missing or invalid (PUBLIC_CLERK_PUBLISHABLE_KEY, PUBLIC_ZAAHEEN_CLIENT_ID)');
if (RELEASE && (!config || config.dev || config.fapiHost !== PRODUCTION_FAPI_HOST)) {
  fail('pages', `release needs a pk_live_ key for ${PRODUCTION_FAPI_HOST}`);
}

// --- Headers and the CSP (D5, S1-1) ------------------------------------------------
if (files.includes('.htaccess')) {
  const conf = read('.htaccess');
  const header = (name) => [...conf.matchAll(new RegExp(`^\\s*Header\\s+always\\s+set\\s+${name}\\s+"([^"]*)"\\s*$`, 'gim'))].map((m) => m[1]);
  const csps = header('Content-Security-Policy');
  if (!config || csps.length !== 1 || csps[0] !== accountCsp(config.fapiHost)) {
    fail('.htaccess', 'the CSP is not the pinned policy (account/scripts/csp.js)');
  }
  const asTemplate = config ? conf.replace(/\r\n/g, '\n').split(accountCsp(config.fapiHost)).join('{{CSP}}') : '';
  if (sha256(asTemplate) !== HTACCESS_SHA256) {
    fail('.htaccess', 'the server file is not the pinned template (HTACCESS_SHA256 in scripts/audit-account.mjs)');
  }
  const pinned = {
    'X-Robots-Tag': ['noindex', 'X-Robots-Tag noindex'],
    'Cross-Origin-Opener-Policy': ['same-origin', 'Cross-Origin-Opener-Policy same-origin'],
    'X-Frame-Options': ['DENY', 'X-Frame-Options DENY'],
    'Referrer-Policy': ['no-referrer', 'Referrer-Policy no-referrer'],
    'Strict-Transport-Security': ['max-age=31536000; includeSubDomains', 'Strict-Transport-Security with includeSubDomains'],
    'X-Content-Type-Options': ['nosniff', 'X-Content-Type-Options nosniff'],
  };
  for (const [name, [value, label]] of Object.entries(pinned)) {
    const got = header(name);
    if (got.length !== 1 || got[0] !== value) fail('.htaccess', `missing or wrong header: ${label}`);
  }
}

// --- tt.js and Clerk's file (S1-1, D3, S2-1) ----------------------------------------
if (files.includes('tt.js') && sha256(read('tt.js').replace(/\r\n/g, '\n')) !== TT_SHA256) {
  fail('tt.js', 'tt.js is not the pinned policy (TT_SHA256 in scripts/audit-account.mjs)');
}
for (const rel of files.filter((f) => f.startsWith('clerk/'))) {
  if (rel !== `clerk/${CLERK_JS.file}`) fail(rel, 'unexpected file in clerk/ (only the pinned Clerk file is served)');
}
if (files.includes(`clerk/${CLERK_JS.file}`)) {
  const buf = fs.readFileSync(path.join(DIST, 'clerk', CLERK_JS.file));
  if (sha256(buf) !== CLERK_JS.sha256) fail(`clerk/${CLERK_JS.file}`, 'the Clerk file does not match its pinned sha256 (account/scripts/clerk-pin.js)');
  for (const re of FORBIDDEN_CLERK) if (re.test(buf.toString('utf8'))) fail(`clerk/${CLERK_JS.file}`, `forbidden pattern ${re}`);
}
for (const rel of files.filter((f) => f.endsWith('.js') && !f.startsWith('clerk/'))) {
  const src = read(rel);
  for (const re of FORBIDDEN_OURS) if (re.test(src)) fail(rel, `forbidden pattern ${re}`);
}

// --- Every page ------------------------------------------------------------------------
for (const rel of files.filter((f) => f.endsWith('.html'))) {
  const html = read(rel);
  const scripts = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script\b[^>]*>/gi)];
  for (const m of scripts) {
    const src = (m[1].match(/\ssrc="([^"]*)"/i) || [])[1];
    if (!src) fail(rel, 'inline <script> (the CSP allows only files from this origin)');
    else if (!src.startsWith('/') || src.startsWith('//')) fail(rel, `third-party script: ${src}`);
  }
  const srcs = scripts.map((m) => (m[1].match(/\ssrc="([^"]*)"/i) || [])[1]);
  if (srcs[0] !== '/tt.js') fail(rel, 'tt.js must be the first script on every page (S1-1)');
  if (/<style\b/i.test(html)) fail(rel, 'inline <style> (the CSP allows only stylesheet files)');
  if (/\sstyle="/i.test(html)) fail(rel, 'inline style attribute (the CSP allows only stylesheet files)');
  if (/<iframe\b/i.test(html)) fail(rel, 'an <iframe> on the account origin');
  if (!/<meta name="robots" content="noindex, nofollow">/.test(html)) fail(rel, 'robots meta noindex, nofollow is missing');
  for (const m of html.matchAll(/<link\b[^>]*\shref="([^"]*)"/gi)) {
    if (!m[1].startsWith('/') || m[1].startsWith('//')) fail(rel, `third-party resource: ${m[1]}`);
  }
}
for (const rel of PAGES.filter((f) => files.includes(f))) {
  const html = read(rel);
  if (!/\sid="clerk-captcha"/.test(html)) fail(rel, 'the clerk-captcha element is missing (the bot check needs it)');
  const srcs = [...html.matchAll(/<script\b[^>]*\ssrc="([^"]*)"/gi)].map((m) => m[1]);
  if (srcs[1] !== `/clerk/${CLERK_JS.file}`) fail(rel, 'the pinned Clerk file must be the second script');
}

if (errors.length) {
  console.error(`audit-account: ${errors.length} problem(s)\n${errors.map((e) => `  - ${e}`).join('\n')}`);
  process.exit(1);
}
console.log(`audit-account: ${files.length} files checked, all rules pass${RELEASE ? ' (release)' : ''}`);
