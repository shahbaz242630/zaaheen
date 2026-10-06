// After a deploy: prove account.zaaheen.com is serving this build, its security
// headers arrived exactly as its .htaccess sets them, and the production Clerk
// instance still meets D4/D9 (AUTH-PAGES-DESIGN D9: "a config-check script
// compares the instance against these").
//
//   node scripts/post-deploy-account.mjs --sha <commit> --htaccess <built .htaccess> [--wait 600] [--origin <url>]
//
// --htaccess is the built file (the artifact's), since its policy line is only
// written at build time. Exit 1 on any problem. Needs no secret. --origin is
// only for a rehearsal against a local copy (serve-account.mjs); the http to
// https check is skipped there.
import fs from 'node:fs';
import { expectedHeaders, headerProblems } from './live-headers.mjs';
import { ACCOUNT_ORIGIN, instanceProblems } from './clerk-invariants.mjs';

const FAPI = 'https://clerk.zaaheen.com';
const PAGES = ['/sign-in/', '/sign-up/', '/sso-callback/', '/delete-account/'];

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};
const SHA = opt('sha', '');
const HTACCESS = opt('htaccess', '');
const WAIT_S = Number(opt('wait', '600'));
const ORIGIN = opt('origin', ACCOUNT_ORIGIN).replace(/\/$/, '');
if (!SHA || !HTACCESS) {
  console.error('post-deploy-account: --sha and --htaccess are required');
  process.exit(1);
}

const problems = [];
const get = (url, init = {}) =>
  fetch(url, { redirect: 'manual', headers: { 'Cache-Control': 'no-cache' }, ...init });

// 1. Wait for Hostinger to pull the account-deploy branch and serve this build.
const deadline = Date.now() + WAIT_S * 1000;
let live = '';
for (;;) {
  try {
    const r = await get(`${ORIGIN}/build.txt?t=${Date.now()}`);
    live = r.ok ? (await r.text()).trim().slice(0, 80) : `HTTP ${r.status}`;
  } catch (e) {
    live = `fetch failed: ${e.message}`;
  }
  if (live === SHA) break;
  if (Date.now() > deadline) {
    console.error(`post-deploy-account: account.zaaheen.com still serves "${live}" after ${WAIT_S}s, expected ${SHA}.`);
    console.error('Check hPanel > Git for the account site: the deployment log, and that auto-deployment is on for account-deploy.');
    process.exit(1);
  }
  await new Promise((r) => setTimeout(r, 10_000));
}
console.log(`post-deploy-account: live build is ${SHA}`);

const expect = async (label, url, check, init) => {
  try {
    const r = await get(url, init);
    const body = r.status < 300 ? await r.text() : '';
    const why = check(r, body);
    if (why) problems.push(`${label}: ${why}`);
    else console.log(`  ok  ${label}`);
  } catch (e) {
    problems.push(`${label}: ${e.message}`);
  }
};

// 2. The pages, the front door, and nothing that should stay private.
for (const path of PAGES) {
  await expect(path, `${ORIGIN}${path}`, (r, b) =>
    r.status !== 200 ? `HTTP ${r.status}` : !b.includes('<meta name="robots" content="noindex, nofollow">') ? 'noindex missing' : '');
}
await expect('the origin forwards to sign-in', `${ORIGIN}/`, (r) =>
  r.status !== 302 ? `HTTP ${r.status}, expected 302` : r.headers.get('location') !== `${ORIGIN}/sign-in/` && r.headers.get('location') !== '/sign-in/'
    ? `Location ${r.headers.get('location')}` : '');
// Crawling allowed so every page's noindex is seen (session 90).
await expect('robots.txt allows crawling', `${ORIGIN}/robots.txt`, (r, b) =>
  r.status !== 200 ? `HTTP ${r.status}` : !/^User-agent: \*\s*\nAllow: \/\s*$/m.test(b) || /^\s*Disallow:\s*\S/im.test(b) ? 'not "Allow: /"' : '');
await expect('missing page is a real 404', `${ORIGIN}/no-such-page-${Date.now()}/`, (r) =>
  r.status !== 404 ? `HTTP ${r.status}, expected 404` : '');
if (ORIGIN === ACCOUNT_ORIGIN) {
  await expect('http redirects to https', 'http://account.zaaheen.com/sign-in/', (r) =>
    ![301, 308].includes(r.status) ? `HTTP ${r.status}, expected 301`
      : !String(r.headers.get('location')).startsWith(`${ORIGIN}/`) ? `Location ${r.headers.get('location')}` : '');
}
for (const path of ['/.htaccess', '/.git/HEAD']) {
  await expect(`${path} is not served`, `${ORIGIN}${path}`, (r) =>
    r.status === 200 ? `HTTP 200: ${path} is publicly readable` : '');
}

// 3. Every header the built .htaccess sets arrives exactly (a CDN between
// Cloudflare and the origin was measured replacing a CSP: live-headers.mjs).
const expected = expectedHeaders(fs.readFileSync(HTACCESS, 'utf8'));
for (const path of ['/sign-in/', '/delete-account/']) {
  try {
    const found = headerProblems(expected, (await get(`${ORIGIN}${path}`)).headers);
    if (found.length) problems.push(...found.map((p) => `${path} headers: ${p}`));
    else console.log(`  ok  ${path} headers`);
  } catch (e) {
    problems.push(`${path} headers: ${e.message}`);
  }
}

// 4. The production Clerk instance (D4, D9), from what it publishes.
try {
  const env = await (await fetch(`${FAPI}/v1/environment`)).json();
  const oauth = await (await fetch(`${FAPI}/.well-known/oauth-authorization-server`)).json();
  const found = instanceProblems(env, oauth);
  if (found.length) problems.push(...found.map((p) => `Clerk instance: ${p}`));
  else console.log('  ok  Clerk instance (MFA and tasks off, DCR and CIMD off, our paths, self-delete on)');
} catch (e) {
  problems.push(`Clerk instance: ${e.message}`);
}

// 5. Allowed subdomains (Clerk: FAPI otherwise answers any *.zaaheen.com, so a
// compromised subdomain could drive sign-in). Measured 2026-09-28: /v1/client
// answers our origin and refuses others with 403 "Subdomain not allowed"; the
// public /v1/environment stays open to all, so it proves nothing here.
for (const [origin, want] of [[ACCOUNT_ORIGIN, 200], [`https://not-allowed-${Date.now()}.zaaheen.com`, 403]]) {
  try {
    const r = await fetch(`${FAPI}/v1/client`, { headers: { Origin: origin } });
    if (r.status !== want) problems.push(`Clerk allowed subdomains: ${origin} got HTTP ${r.status}, expected ${want}`);
    else console.log(`  ok  Clerk answers ${origin} with ${want}`);
  } catch (e) {
    problems.push(`Clerk allowed subdomains: ${e.message}`);
  }
}

if (problems.length) {
  console.error(`\npost-deploy-account: ${problems.length} problem(s) on account.zaaheen.com:`);
  for (const p of problems) console.error(`  - ${p}`);
  process.exit(1);
}
console.log('\npost-deploy-account: all checks pass.');
