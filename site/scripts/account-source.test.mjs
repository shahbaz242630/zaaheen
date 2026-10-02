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
  assert.deepEqual(SOURCES.map(([f]) => f).sort(), ['account-state.js', 'account.js', 'clerk-pin.js', 'csp.js', 'delete-state.js', 'redirect.js']);
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
  // ...or, on Google's return from the delete page, the one fixed delete address
  // (ACCOUNT-DELETION-DESIGN D4): a constant on our own origin, never a URL
  // read from the address bar.
  assert.match(ACCOUNT, /const done = AFTER_DELETE \? DELETE_URL : REDIRECT\.state === 'ok' \? REDIRECT\.url\.href : signIn;/);
  assert.match(ACCOUNT, /const signIn = AFTER_DELETE \? NO_ACCOUNT_URL : new URL\(`\/sign-in\/\$\{keepQuery\(\)\}`, location\.origin\)\.href;/);
  assert.match(ACCOUNT, /const DELETE_URL = new URL\(DELETE_PATH, location\.origin\)\.href;/);
  assert.match(ACCOUNT, /const NO_ACCOUNT_URL = new URL\(NO_ACCOUNT_PATH, location\.origin\)\.href;/);
  assert.match(ACCOUNT, /const AFTER_DELETE = PAGE === 'sso-callback' && deleteMarker\(location\.search\);/);
  for (const prop of ['signInFallbackRedirectUrl', 'signUpFallbackRedirectUrl', 'signInForceRedirectUrl', 'signUpForceRedirectUrl']) {
    const uses = [...ACCOUNT.matchAll(new RegExp(`${prop}:\\s*([^,}\\s]+)`, 'g'))].map((m) => m[1]);
    assert.deepEqual(uses, ['done'], `${prop} must be exactly \`done\`, once`);
  }
  // The only other URL handed to Clerk to finish on: Google's redirectUrlComplete.
  const complete = [...ACCOUNT.matchAll(/redirectUrlComplete:\s*([^\n]+)/g)].map((m) => m[1].trim());
  assert.deepEqual(complete, ["DELETING ? DELETE_URL : REDIRECT.state === 'ok' ? REDIRECT.url.href : new URL('/sign-in/', location.origin).href,"]);
  // No other redirect-carrying option is passed to Clerk anywhere.
  for (const re of [/afterSignInUrl/, /afterSignUpUrl/, /signInUrl:(?!\s*signIn\b)/, /signUpUrl:(?!\s*signUp\b)/, /redirectUrl:(?!\s*new URL\(`\/sso-callback\/)/]) {
    assert.doesNotMatch(ACCOUNT, re, `${re}`);
  }
});

// ACCOUNT-DELETION-DESIGN D3, D4 and its review: Google from the delete page
// never creates an account; nothing signs out (that would also end coaching's
// session); a failed delete can never read as success; one Clerk delete call.
test('the delete path: no account creation, no sign-out, no false success', () => {
  const transfer = [...ACCOUNT.matchAll(/transferable\s*:[^}\n]*/g)].map((m) => m[0].trim());
  assert.deepEqual(transfer, ['transferable: false']);
  assert.match(ACCOUNT, /\.\.\.\(AFTER_DELETE \? \{ transferable: false \} : \{\}\)/);
  assert.match(ACCOUNT, /redirectUrl: new URL\(`\/sso-callback\/\$\{DELETING \? '\?after=delete' : keepQuery\(\)\}`, location\.origin\)\.href,/);
  assert.doesNotMatch(ACCOUNT, /signOut\s*\(/);
  assert.equal([...ACCOUNT.matchAll(/\.delete\(\)/g)].length, 1, 'one user.delete() call');
  assert.match(ACCOUNT, /await clerk\.user\.delete\(\);\s*\n\s*next = afterDelete\(null, reverified\);/);
  assert.match(ACCOUNT, /next = afterDelete\(codeOf\(e\) \|\| 'unknown', reverified\);/);
});

// Session 67's review of the delete build: m1 (an existing session on the
// delete page goes to confirm, not the "signed in" dead end), m3 (the fixed
// line stays the default; only the delete and re-verify paths say "couldn't
// delete"), m2 (no email-code factor is its own line, not "try again").
test('the delete page: its error lines go only where they belong', () => {
  assert.match(ACCOUNT, /if \(code === 'session_exists'\) return DELETING \? showConfirm\(\) : showSignedIn\(\);/);
  assert.match(ACCOUNT, /function fatal\(line = FIXED_LINE\) \{/);
  assert.doesNotMatch(ACCOUNT, /line = DELETING/);
  assert.match(ACCOUNT, /const plan = reverifyPlan\(verification\);\s*\n\s*if \(plan\.view === 'error'\) return fatal\(plan\.line\);/);
  assert.doesNotMatch(ACCOUNT, /reverifyFactor\(/);
});

// Session 82: Clerk runs its bot check before sending a sign-up to Google, which
// can take seconds; without feedback people click again and restart it.
test('the Google button says it is working, once, and comes back on an error or Back', () => {
  const GOOGLE = fs.readFileSync(path.resolve(DIR, '..', 'components', 'GoogleButton.astro'), 'utf8');
  assert.match(GOOGLE, /<span id="google-label">Continue with Google<\/span>/);
  const h = /\$\('google'\)\.addEventListener\('click', async \(ev\) => \{([\s\S]*?)\n {2}\}\);/.exec(ACCOUNT);
  assert.ok(h, 'the Google handler');
  const body = h[1];
  // A second click while it is working does nothing.
  assert.match(body, /^\s*ev\.preventDefault\(\);\s*if \(\$\('google'\)\.disabled\) return;/);
  // Greyed out and relabelled before Clerk is asked anything.
  const working = body.indexOf('googleWorking(true)');
  assert.ok(working !== -1 && working < body.indexOf('authenticateWithRedirect'), 'working before the redirect');
  // An error puts it back.
  assert.match(body, /catch \(e\) \{\s*googleWorking\(false\);\s*failWith\(e\);/);
  assert.match(ACCOUNT, /function googleWorking\(on\) \{\s*\$\('google'\)\.disabled = on;\s*\$\('google-label'\)\.textContent = on \? 'Opening Google…' : 'Continue with Google';\s*\}/);
  // The browser's Back button restores the page from its cache: put it back.
  assert.match(ACCOUNT, /addEventListener\('pageshow', \(\) => googleWorking\(false\)\)/);
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
