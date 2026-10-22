// scripts/cloudflare-files.mjs: Cloudflare's files say what the .htaccess files say.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { accountHeaders, accountRedirects, alwaysSet, charsetRules, pageFolders, siteHeaders, siteRedirects } from './cloudflare-files.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT_HT = fs.readFileSync(path.join(HERE, '..', 'public', '.htaccess'), 'utf8');
const PAY_HT = fs.readFileSync(path.join(HERE, '..', 'public', 'pay', '.htaccess'), 'utf8');
const ACCOUNT_HT = fs
  .readFileSync(path.join(HERE, '..', 'account', 'htaccess.template'), 'utf8')
  .replace('{{CSP}}', "default-src 'self'");

/** _headers -> { pattern: [line, ...] }, the way Cloudflare reads it. */
function parse(text) {
  const rules = {};
  let current = null;
  for (const raw of text.split('\n')) {
    if (raw.startsWith('#') || raw.trim() === '') continue;
    if (!raw.startsWith(' ')) {
      current = raw.trim();
      assert.equal(rules[current], undefined, `pattern ${current} appears twice`);
      rules[current] = [];
    } else {
      rules[current].push(raw.trim());
    }
  }
  return rules;
}

/**
 * The headers Cloudflare would send for a path matched by `patterns`, in order:
 * every rule applies, `! Name` drops what earlier rules set, repeats are joined.
 */
function effective(rules, patterns) {
  const out = new Map();
  for (const p of patterns) {
    for (const line of rules[p] ?? []) {
      if (line.startsWith('! ')) {
        out.delete(line.slice(2).toLowerCase());
        continue;
      }
      const i = line.indexOf(': ');
      const name = line.slice(0, i).toLowerCase();
      const value = line.slice(i + 2);
      out.set(name, out.has(name) ? `${out.get(name)}, ${value}` : value);
    }
  }
  return out;
}

const lower = (pairs) => new Map(pairs.map(([n, v]) => [n.toLowerCase(), v]));

test('reads every Header always set line of the real file, HSTS included', () => {
  const names = alwaysSet(ROOT_HT).map(([n]) => n);
  assert.deepEqual(names, [
    'X-Content-Type-Options', 'Referrer-Policy', 'X-Frame-Options', 'Permissions-Policy',
    'Strict-Transport-Security', 'Cross-Origin-Opener-Policy', 'X-Permitted-Cross-Domain-Policies',
    'Content-Security-Policy',
  ]);
});

test('a Header directive it cannot translate stops the build instead of vanishing', () => {
  for (const extra of [
    'Header always unset X-Powered-By',
    'Header always append Vary "Accept"',
    'Header always set X-Test "1" env=HTTPS',
    'Header set Cache-Control "max-age=60"',
    'Header set X-Other "1"',
  ]) {
    assert.throws(() => alwaysSet(`${ROOT_HT}\n  ${extra}\n`), /no Cloudflare translation/, extra);
  }
  assert.doesNotThrow(() => alwaysSet(ROOT_HT));
  assert.doesNotThrow(() => alwaysSet(PAY_HT));
  assert.doesNotThrow(() => alwaysSet(ACCOUNT_HT));
});

test('every page gets exactly the security headers .htaccess promises', () => {
  const got = effective(parse(siteHeaders(ROOT_HT, PAY_HT)), ['/*']);
  for (const [name, value] of lower(alwaysSet(ROOT_HT))) assert.equal(got.get(name), value, name);
  assert.equal(got.get('cache-control'), 'no-cache');
});

test('/pay gets its own CSP and COOP alone, never joined with the site-wide ones', () => {
  const rules = parse(siteHeaders(ROOT_HT, PAY_HT));
  const got = effective(rules, ['/*', '/pay/*']);
  const want = new Map([...lower(alwaysSet(ROOT_HT)), ...lower(alwaysSet(PAY_HT))]);
  for (const [name, value] of want) assert.equal(got.get(name), value, name);
  assert.notEqual(got.get('content-security-policy'), lower(alwaysSet(ROOT_HT)).get('content-security-policy'), 'the pay CSP, not the site one');
  assert.equal(got.get('content-security-policy').split('default-src').length, 2, 'one policy, not two joined');
  assert.equal(got.get('x-robots-tag'), 'noindex');
});

test('without /pay published there is no /pay rule', () => {
  assert.equal(parse(siteHeaders(ROOT_HT, null))['/pay/*'], undefined);
});

test('hashed /_astro/ files are cached for a year, once', () => {
  const got = effective(parse(siteHeaders(ROOT_HT, PAY_HT)), ['/*', '/_astro/*']);
  assert.equal(got.get('cache-control'), 'public, max-age=31536000, immutable');
});

test('a cross-origin read permission is always dropped', () => {
  for (const text of [siteHeaders(ROOT_HT, PAY_HT), accountHeaders(ACCOUNT_HT)]) {
    assert.ok(parse(text)['/*'].includes('! Access-Control-Allow-Origin'));
  }
});

test('the workers.dev test copies are never indexed', () => {
  for (const text of [siteHeaders(ROOT_HT, PAY_HT), accountHeaders(ACCOUNT_HT)]) {
    const rules = parse(text);
    assert.deepEqual(rules['https://:worker.:subdomain.workers.dev/*'], ['! X-Robots-Tag', 'X-Robots-Tag: noindex']);
  }
});

test('the account origin gets exactly its built .htaccess headers', () => {
  const got = effective(parse(accountHeaders(ACCOUNT_HT)), ['/*']);
  const want = lower(alwaysSet(ACCOUNT_HT));
  assert.ok(want.size >= 9);
  for (const [name, value] of want) assert.equal(got.get(name), value, name);
  assert.equal(got.get('cache-control'), 'no-store');
  assert.equal(got.get('content-security-policy'), "default-src 'self'");
});

const rules = (text) => text.split('\n').filter((l) => l && !l.startsWith('#'));

test("sign-in and sign-up forward to the account origin, temporarily, with a fixed query that replaces the visitor's", () => {
  assert.deepEqual(rules(siteRedirects([])), [
    '/sign-in https://account.zaaheen.com/sign-in/?from=website 302',
    '/sign-in/ https://account.zaaheen.com/sign-in/?from=website 302',
    '/sign-up https://account.zaaheen.com/sign-up/?from=website 302',
    '/sign-up/ https://account.zaaheen.com/sign-up/?from=website 302',
  ]);
});

test("the account origin's front door is sign-in", () => {
  assert.deepEqual(rules(accountRedirects([])), ['/ /sign-in/ 302']);
});

test('every page gets one permanent hop to its canonical address', () => {
  const folders = pageFolders(['index.html', 'learn/index.html', 'learn/a-b/index.html', '404.html', 'robots.txt', '_astro/x.js']);
  assert.deepEqual(folders, ['', 'learn/', 'learn/a-b/']);
  assert.deepEqual(rules(siteRedirects(folders)).slice(4), [
    '/index.html / 301',
    '/learn/index.html /learn/ 301',
    '/learn /learn/ 301',
    '/learn/a-b/index.html /learn/a-b/ 301',
    '/learn/a-b /learn/a-b/ 301',
  ]);
  assert.deepEqual(rules(accountRedirects(['sign-in/'])), ['/ /sign-in/ 302', '/sign-in/index.html /sign-in/ 301', '/sign-in /sign-in/ 301']);
});

test('text files are declared UTF-8, as AddCharset says', () => {
  const got = effective(parse(siteHeaders(ROOT_HT, PAY_HT)), ['/*', '/*.txt']);
  assert.equal(got.get('content-type'), 'text/plain; charset=utf-8');
  assert.equal(effective(parse(siteHeaders(ROOT_HT, PAY_HT)), ['/*', '/*.xml']).get('content-type'), 'application/xml; charset=utf-8');
  assert.deepEqual(charsetRules('AddCharset UTF-8 .txt'), ['/*.txt', '  ! Content-Type', '  Content-Type: text/plain; charset=utf-8']);
  assert.throws(() => charsetRules('AddCharset UTF-8 .json'), /no Cloudflare translation/);
});

/** JSONC to JSON: drops `//` comments outside strings (the files have no block comments). */
function parseJsonc(text) {
  let out = '';
  let inString = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (inString) {
      out += c;
      if (c === '\\') out += text[++i];
      else if (c === '"') inString = false;
    } else if (c === '"') {
      inString = true;
      out += c;
    } else if (c === '/' && text[i + 1] === '/') {
      while (i < text.length && text[i] !== '\n') i++;
      out += '\n';
    } else {
      out += c;
    }
  }
  return JSON.parse(out);
}

const config = (name) => parseJsonc(fs.readFileSync(path.join(HERE, '..', 'cloudflare', name), 'utf8'));

test('each site answers on its own real address only', () => {
  for (const [file, name, host, dir] of [
    ['site.jsonc', 'zaaheen-site', 'zaaheen.com', '../dist'],
    ['account.jsonc', 'zaaheen-account-site', 'account.zaaheen.com', '../dist-account'],
  ]) {
    const c = config(file);
    assert.equal(c.name, name);
    assert.deepEqual(c.routes, [{ pattern: host, custom_domain: true }], file);
    assert.equal(c.workers_dev, false, `${file}: no workers.dev copy`);
    assert.equal(c.preview_urls, false, `${file}: no preview URLs`);
    assert.equal(c.assets.directory, dir);
  }
});

/*
  Session 108: account.jsonc once used the account API's Worker name, so every
  website publish replaced the API (code, secrets, api.zaaheen.com). A site
  Worker may never share a name with any Worker under workers/, in any env.
*/
test('no site Worker shares a name with a Worker under workers/', () => {
  const workersDir = path.join(HERE, '..', '..', 'workers');
  const taken = new Set();
  for (const dir of fs.readdirSync(workersDir)) {
    const file = path.join(workersDir, dir, 'wrangler.jsonc');
    if (!fs.existsSync(file)) continue;
    const w = parseJsonc(fs.readFileSync(file, 'utf8'));
    taken.add(w.name);
    for (const env of Object.values(w.env ?? {})) if (env.name) taken.add(env.name);
  }
  assert.ok(taken.has('zaaheen-account'), 'the account API is found');
  for (const file of ['site.jsonc', 'account.jsonc']) {
    const name = config(file).name;
    assert.ok(!taken.has(name), `${file}: ${name} is a Worker under workers/`);
  }
});

test('static files only: no Worker code, real 404s, canonical trailing slashes', () => {
  for (const file of ['site.jsonc', 'account.jsonc']) {
    const c = config(file);
    assert.equal(c.main, undefined, `${file}: no Worker script`);
    assert.equal(c.assets.binding, undefined, `${file}: no assets binding`);
    assert.equal(c.assets.run_worker_first, undefined, file);
    assert.equal(c.assets.not_found_handling, '404-page', file);
    assert.equal(c.assets.html_handling, 'auto-trailing-slash', file);
    for (const key of ['vars', 'kv_namespaces', 'r2_buckets', 'services', 'durable_objects', 'triggers', 'env']) {
      assert.equal(c[key], undefined, `${file}: ${key}`);
    }
  }
});
