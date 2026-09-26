// Source rules for the account pages' own scripts (AUTH-PAGES-DESIGN D3, D4,
// ADR-SEC-034): no HTML from strings, one way to navigate, handlers that
// never let the browser submit a form by itself. The built bundle is grepped
// again by scripts/audit-account.mjs.
//
//   node --test scripts/account-source.test.mjs     (from site/; no build needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'account', 'scripts');
const SOURCES = fs.readdirSync(DIR).filter((f) => f.endsWith('.js')).map((f) => [f, fs.readFileSync(path.join(DIR, f), 'utf8')]);
// Comments may mention the forbidden names; code may not.
const code = (src) => src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`])\/\/.*$/gm, '$1');
const ACCOUNT = code(fs.readFileSync(path.join(DIR, 'account.js'), 'utf8'));

test('the scripts exist and are the ones this test knows about', () => {
  assert.deepEqual(SOURCES.map(([f]) => f).sort(), ['account-state.js', 'account.js', 'clerk-pin.js', 'csp.js', 'redirect.js']);
});

test('no HTML from strings, no eval, anywhere in our scripts', () => {
  for (const [f, src] of SOURCES) {
    const c = code(src);
    for (const re of [/\.innerHTML\b/, /\.outerHTML\b/, /insertAdjacentHTML/, /document\.write/, /\beval\s*\(/, /\bnew\s+Function\b/, /setAttribute\(\s*['"](style|on\w+|href|src)['"]/, /\.setHTML\b/, /createContextualFragment/]) {
      assert.doesNotMatch(c, re, `${f}: ${re}`);
    }
  }
});

test('the only navigation is location.assign of a URL object from redirect.js', () => {
  const assigns = [...ACCOUNT.matchAll(/location\.assign\(([^)]*)\)/g)].map((m) => m[1].trim());
  assert.ok(assigns.length >= 1);
  for (const arg of assigns) assert.match(arg, /^(built|url)\.href$/, `location.assign(${arg})`);
  // Where built / url come from: checkTarget or checkNavigation, nothing else.
  assert.match(ACCOUNT, /const built = checkTarget\(/);
  assert.match(ACCOUNT, /const url = checkNavigation\(/);
  for (const re of [/location\.href\s*=/, /location\s*=[^=]/, /location\.replace\(/, /window\.open\(/, /\.href\s*=[^=]/, /document\.location/, /window\.location\s*=/]) {
    assert.doesNotMatch(ACCOUNT, re, `forbidden navigation ${re}`);
  }
});

test('every form and button handler calls preventDefault first', () => {
  const handlers = [...ACCOUNT.matchAll(/addEventListener\('(submit|click)',\s*(async\s*)?\(ev\)\s*=>\s*\{\s*([^\n]*)/g)];
  const all = [...ACCOUNT.matchAll(/addEventListener\('(submit|click)'/g)];
  assert.equal(handlers.length, all.length, 'every submit/click handler takes (ev) =>');
  assert.ok(handlers.length >= 6);
  for (const h of handlers) assert.match(h[3], /^ev\.preventDefault\(\);/, `a ${h[1]} handler starts with: ${h[3]}`);
});

test("Clerk's own error message is never shown", () => {
  assert.doesNotMatch(ACCOUNT, /\.message\b/);
  assert.doesNotMatch(ACCOUNT, /longMessage/);
});

test('text reaches the page only through textContent', () => {
  const writes = [...ACCOUNT.matchAll(/\.(textContent|innerText|nodeValue|value)\s*=/g)].map((m) => m[1]);
  assert.ok(writes.includes('textContent'));
  assert.ok(!writes.includes('innerText') && !writes.includes('nodeValue'));
});

test('redirect_url goes to Clerk only as validator output', () => {
  // Every mention of the page's own redirect in a Clerk call is REDIRECT.url (redirect.js output).
  assert.doesNotMatch(ACCOUNT, /searchParams\.get\(['"]redirect_url['"]\)/);
  assert.doesNotMatch(ACCOUNT, /URLSearchParams\(location\.search\)/);
  assert.match(ACCOUNT, /const REDIRECT = readRedirect\(location\.search, CONFIG\);/);
});

// Independent review (session 65), finding 1: a finished Google sign-in is
// navigated by Clerk itself to its redirect props, never through our checked
// navigate function. So those props must be validator output or our own page.
test('every URL Clerk navigates to by itself is validator output or our own sign-in page', () => {
  assert.match(ACCOUNT, /const done = REDIRECT\.state === 'ok' \? REDIRECT\.url\.href : signIn;/);
  assert.match(ACCOUNT, /const signIn = new URL\(`\/sign-in\/\$\{keepQuery\(\)\}`, location\.origin\)\.href;/);
  for (const prop of ['signInFallbackRedirectUrl', 'signUpFallbackRedirectUrl', 'signInForceRedirectUrl', 'signUpForceRedirectUrl']) {
    const uses = [...ACCOUNT.matchAll(new RegExp(`${prop}:\\s*([^,}\\s]+)`, 'g'))].map((m) => m[1]);
    assert.deepEqual(uses, ['done'], `${prop} must be exactly \`done\`, once`);
  }
  // The only other URL handed to Clerk to finish on: Google's redirectUrlComplete.
  const complete = [...ACCOUNT.matchAll(/redirectUrlComplete:\s*([^\n]+)/g)].map((m) => m[1].trim());
  assert.deepEqual(complete, ["REDIRECT.state === 'ok' ? REDIRECT.url.href : new URL('/sign-in/', location.origin).href,"]);
  // No other redirect-carrying option is passed to Clerk anywhere.
  for (const re of [/afterSignInUrl/, /afterSignUpUrl/, /signInUrl:(?!\s*signIn\b)/, /signUpUrl:(?!\s*signUp\b)/, /redirectUrl:(?!\s*new URL\(`\/sso-callback\/)/]) {
    assert.doesNotMatch(ACCOUNT, re, `${re}`);
  }
});

// Independent review, finding 3: the policy text itself, word for word. The
// audit compares .htaccess with csp.js; this pins csp.js, so loosening it is a
// visible change to this test (a reviewed ADR-SEC-034 amendment).
test('the account CSP is the reviewed policy, word for word', async () => {
  const { accountCsp } = await import('../account/scripts/csp.js');
  assert.equal(accountCsp('HOST'), [
    "default-src 'self'", "script-src 'self' https://challenges.cloudflare.com", "connect-src 'self' https://HOST",
    'frame-src https://challenges.cloudflare.com', "img-src 'self' data:", "style-src 'self'", "font-src 'self'",
    "object-src 'none'", "frame-ancestors 'none'", "base-uri 'none'", "form-action 'self'",
    "require-trusted-types-for 'script'", 'trusted-types default',
  ].join('; '));
});
