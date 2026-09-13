// After a deploy: prove zaaheen.com is serving this build and is reachable the
// way crawlers see it, then tell IndexNow (Bing, and through Bing, Copilot and
// ChatGPT search) which pages changed.
//
//   node scripts/post-deploy.mjs --sha <commit> [--changed <file>] [--wait 600] [--dry-run]
//
// --changed: newline-separated dist paths that changed in this deploy.
// Exit 1 if the live site is wrong. An IndexNow failure only warns: a search
// ping must never fail an otherwise-good deploy.
import fs from 'node:fs';

const ORIGIN = 'https://zaaheen.com';
const HOST = 'zaaheen.com';
const KEY = 'dc7e96914b463f8b38a2ca7309b9a25f';

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};
const SHA = opt('sha', '');
const CHANGED = opt('changed', '');
const WAIT_S = Number(opt('wait', '600'));
const DRY = args.includes('--dry-run');
if (!SHA) {
  console.error('post-deploy: --sha is required');
  process.exit(1);
}

const problems = [];
const get = (url, init = {}) =>
  fetch(url, { redirect: 'manual', headers: { 'Cache-Control': 'no-cache' }, ...init });

// 1. Wait for Hostinger to pull the site-deploy branch and serve this build.
const deadline = Date.now() + WAIT_S * 1000;
let live = '';
for (;;) {
  try {
    const r = await get(`${ORIGIN}/build.txt?t=${Date.now()}`);
    // Trimmed: a parking page answers every path with a whole HTML document.
    live = r.ok ? (await r.text()).trim().slice(0, 80) : `HTTP ${r.status}`;
  } catch (e) {
    live = `fetch failed: ${e.message}`;
  }
  if (live === SHA) break;
  if (Date.now() > deadline) {
    console.error(`post-deploy: zaaheen.com still serves "${live}" after ${WAIT_S}s, expected ${SHA}.`);
    console.error('Check hPanel > Git: the deployment log, and that auto-deployment is on for site-deploy.');
    process.exit(1);
  }
  await new Promise((r) => setTimeout(r, 10_000));
}
console.log(`post-deploy: live build is ${SHA}`);

// 2. Smoke checks, as a crawler would see the site.
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

await expect('home page', `${ORIGIN}/`, (r, b) =>
  r.status !== 200 ? `HTTP ${r.status}` : !b.includes(`<link rel="canonical" href="${ORIGIN}/">`) ? 'canonical missing' : '');
await expect('robots.txt', `${ORIGIN}/robots.txt`, (r, b) =>
  r.status !== 200 ? `HTTP ${r.status}`
    : /Cloudflare Managed/i.test(b) ? 'Cloudflare is injecting its managed robots.txt again (turn it off: SEO-HANDOFF section 7)'
    : /^\s*Disallow:\s*\S/im.test(b) ? 'contains a Disallow rule' : '');
for (const path of ['/sitemap.xml', '/llms.txt', '/favicon.ico', '/favicon-192.png', '/og.png', `/${KEY}.txt`]) {
  await expect(path, `${ORIGIN}${path}`, (r) => (r.status !== 200 ? `HTTP ${r.status}` : ''));
}
await expect('missing page is a real 404', `${ORIGIN}/no-such-page-${Date.now()}/`, (r) =>
  r.status !== 404 ? `HTTP ${r.status}, expected 404` : '');
await expect('www redirects to the apex', 'https://www.zaaheen.com/', (r) =>
  ![301, 308].includes(r.status) ? `HTTP ${r.status}, expected 301`
    : r.headers.get('location') !== `${ORIGIN}/` ? `Location ${r.headers.get('location')}` : '');
await expect('http redirects to https', 'http://zaaheen.com/', (r) =>
  ![301, 308].includes(r.status) ? `HTTP ${r.status}, expected 301`
    : !String(r.headers.get('location')).startsWith(`${ORIGIN}/`) ? `Location ${r.headers.get('location')}` : '');
await expect('repository metadata is not served', `${ORIGIN}/.git/HEAD`, (r) =>
  r.status === 200 ? 'HTTP 200: .git is publicly readable' : '');
for (const ua of ['Googlebot/2.1 (+http://www.google.com/bot.html)', 'OAI-SearchBot/1.0; +https://openai.com/searchbot', 'Claude-SearchBot/1.0']) {
  await expect(`reachable as ${ua.split('/')[0]}`, `${ORIGIN}/`, (r) => (r.status !== 200 ? `HTTP ${r.status}` : ''),
    { headers: { 'User-Agent': `Mozilla/5.0 (compatible; ${ua})`, 'Cache-Control': 'no-cache' } });
}

if (problems.length) {
  console.error(`\npost-deploy: ${problems.length} problem(s) on the live site:`);
  for (const p of problems) console.error(`  - ${p}`);
  process.exit(1);
}

// 3. IndexNow: only pages whose HTML changed, and only pages in the sitemap.
const changedFiles = CHANGED && fs.existsSync(CHANGED)
  ? fs.readFileSync(CHANGED, 'utf8').split('\n').map((s) => s.trim()).filter(Boolean)
  : [];
const sitemap = await (await get(`${ORIGIN}/sitemap.xml`)).text();
const listed = new Set([...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => m[1]));
const urls = changedFiles
  .filter((f) => f.endsWith('.html'))
  .map((f) => (f === 'index.html' ? `${ORIGIN}/` : `${ORIGIN}/${f.replace(/index\.html$/, '')}`))
  .filter((u) => listed.has(u));

if (!urls.length) {
  console.log('\nIndexNow: no listed page changed; nothing to submit.');
  process.exit(0);
}
console.log(`\nIndexNow: ${urls.length} changed page(s):\n${urls.map((u) => `  - ${u}`).join('\n')}`);
if (DRY) {
  console.log('(dry run: nothing sent)');
  process.exit(0);
}
try {
  const r = await fetch('https://api.indexnow.org/indexnow', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json; charset=utf-8' },
    body: JSON.stringify({ host: HOST, key: KEY, keyLocation: `${ORIGIN}/${KEY}.txt`, urlList: urls }),
  });
  if (r.status === 200 || r.status === 202) console.log(`IndexNow: accepted (HTTP ${r.status}).`);
  else console.log(`::warning::IndexNow returned HTTP ${r.status}; the pages are live, the ping can be resent.`);
} catch (e) {
  console.log(`::warning::IndexNow request failed: ${e.message}`);
}
