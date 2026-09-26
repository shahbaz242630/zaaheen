// Tests for the account pages' redirect check (account/scripts/redirect.js,
// AUTH-PAGES-DESIGN D3 + D4): which `redirect_url` the sign-in and sign-up
// pages accept, and the build settings it is checked against. The pages only
// ever navigate to this module's output.
//
//   node --test scripts/redirect.test.mjs          (from site/; no build needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readAccountConfig, readRedirect, checkTarget, checkNavigation } from '../account/scripts/redirect.js';

const PROD_HOST = 'clerk.zaaheen.com';
const DEV_HOST = 'example-name-12.clerk.accounts.dev';
const DEV_PORTAL = 'example-name-12.accounts.dev';
const pk = (kind, host) => `pk_${kind}_${Buffer.from(`${host}$`).toString('base64')}`;
const CLIENT = 'appclientid0000';
const PROD = readAccountConfig({ publishableKey: pk('live', PROD_HOST), clientId: CLIENT });
const DEV = readAccountConfig({ publishableKey: pk('test', DEV_HOST), clientId: CLIENT });
// The app's own shape (vault-account signin/mod.rs): 32 random bytes and a
// SHA-256, both base64url without padding, so 43 characters each.
const STATE = 'Abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE';
const CHALLENGE = 'Zyxwvutsrqponmlkjihgfedcba9876543210_-ZYXWV';

const query = (over = {}) => {
  const p = {
    client_id: CLIENT,
    code_challenge: CHALLENGE,
    code_challenge_method: 'S256',
    redirect_uri: 'http://127.0.0.1:53999/callback',
    response_type: 'code',
    scope: 'openid profile email offline_access',
    state: STATE,
    ...over,
  };
  for (const k of Object.keys(p)) if (p[k] === undefined) delete p[k];
  return new URLSearchParams(p).toString();
};
const target = (host = PROD_HOST, path = '/oauth/authorize-with-immediate-redirect', q = query()) => `https://${host}${path}?${q}`;
const page = (raw) => `?${new URLSearchParams({ redirect_url: raw })}`;
const ok = (search, cfg = PROD) => {
  const r = readRedirect(search, cfg);
  assert.equal(r.state, 'ok', `expected ok for ${search}`);
  return r.url;
};
const invalid = (search, cfg = PROD) => assert.equal(readRedirect(search, cfg).state, 'invalid', `expected invalid for ${search}`);

// --- The build settings (D3) --------------------------------------------------

test('config: a live key names its host; a test key is a development build', () => {
  assert.deepEqual(PROD, { fapiHost: PROD_HOST, portalHost: null, clientId: CLIENT, dev: false });
  assert.deepEqual(DEV, { fapiHost: DEV_HOST, portalHost: DEV_PORTAL, clientId: CLIENT, dev: true });
});

test('config: empty, undecodable or malformed settings match nothing', () => {
  for (const publishableKey of [undefined, '', 'pk_live_', 'pk_live_!!!', 'sk_live_' + Buffer.from(`${PROD_HOST}$`).toString('base64'),
    pk('live', PROD_HOST) + ' ', pk('other', PROD_HOST), `pk_live_${Buffer.from(PROD_HOST).toString('base64')}`,
    pk('live', 'clerk.zaaheen.com/evil'), pk('live', 'clerk.zaaheen.com:444'), pk('live', 'Clerk.Zaaheen.com'), pk('live', '')]) {
    assert.equal(readAccountConfig({ publishableKey, clientId: CLIENT }), null, `key ${publishableKey}`);
  }
  for (const clientId of [undefined, '', ' ', 'a b', 'a&b=c']) {
    assert.equal(readAccountConfig({ publishableKey: pk('live', PROD_HOST), clientId }), null, `client ${clientId}`);
  }
  // With no config at all, every link is refused, never waved through.
  assert.equal(readRedirect(page(target()), null).state, 'invalid');
  assert.equal(checkTarget(target(), null), null);
});

test('config: only a development host of the expected shape has a Portal', () => {
  assert.equal(readAccountConfig({ publishableKey: pk('test', 'some.other.host'), clientId: CLIENT }).portalHost, null);
});

// --- Accepted shapes (D4) ----------------------------------------------------

test('accepts the app link after Clerk: authorize-with-immediate-redirect and authorize', () => {
  assert.equal(ok(page(target())).host, PROD_HOST);
  assert.equal(ok(page(target(PROD_HOST, '/oauth/authorize'))).pathname, '/oauth/authorize');
  // Parameter order is Clerk's business, not ours.
  const reordered = new URLSearchParams([...new URLSearchParams(query())].reverse()).toString();
  ok(page(target(PROD_HOST, '/oauth/authorize', reordered)));
});

test('the accepted URL is the parsed URL, carrying exactly the app parameters', () => {
  const url = ok(page(target()));
  assert.ok(url instanceof URL);
  assert.equal(url.searchParams.get('state'), STATE);
  assert.equal(url.searchParams.get('client_id'), CLIENT);
});

test('no redirect_url is its own state (someone signing in on the website)', () => {
  assert.equal(readRedirect('', PROD).state, 'none');
  assert.equal(readRedirect('?', PROD).state, 'none');
  assert.equal(readRedirect('?utm_source=x', PROD).state, 'none');
  // An empty one is not "none": somebody built a broken link.
  invalid('?redirect_url=');
});

test('development builds only: the Portal consent shape and __clerk_db_jwt', () => {
  const consent = target(DEV_PORTAL, '/oauth-consent', `${query()}&__clerk_db_jwt=dvb_abc123`);
  assert.equal(ok(page(consent), DEV).host, DEV_PORTAL);
  ok(page(target(DEV_HOST, '/oauth/authorize-with-immediate-redirect', `${query()}&__clerk_db_jwt=dvb_abc123`)), DEV);
  ok(page(target(DEV_HOST)), DEV);
  // Production: neither.
  invalid(page(target(PROD_HOST, '/oauth-consent')));
  invalid(page(target(PROD_HOST, '/oauth/authorize', `${query()}&__clerk_db_jwt=dvb_abc123`)));
  // The Portal's other pages are not a way back.
  invalid(page(target(DEV_PORTAL, '/sign-in')), DEV);
  invalid(page(target(DEV_PORTAL, '/oauth/authorize')), DEV);
  // The development-only parameter is still exactly-once and plain.
  invalid(page(target(DEV_HOST, '/oauth/authorize', `${query()}&__clerk_db_jwt=a&__clerk_db_jwt=b`)), DEV);
  invalid(page(target(DEV_HOST, '/oauth/authorize', `${query()}&__clerk_db_jwt=`)), DEV);
  invalid(page(target(DEV_HOST, '/oauth/authorize', `${query()}&__clerk_db_jwt=a%20b`)), DEV);
});

// --- Refused: scheme, host, port, user-info (D4) --------------------------------

test('refuses anything but https to exactly the Frontend API host', () => {
  invalid(page(target().replace('https:', 'http:')));
  invalid(page(target().replace('https://', 'HTTPS://')));
  invalid(page(target('evil.com')));
  invalid(page(target('zaaheen.com')));
  invalid(page(target('accounts.zaaheen.com')));
  invalid(page(target(DEV_HOST)));
});

test('refuses lookalike hosts', () => {
  invalid(page(target('clerk.zaaheen.com.evil.com')));
  invalid(page(`https://evil.com/${PROD_HOST}/oauth/authorize?${query()}`));
  invalid(page(`https://${PROD_HOST}@evil.com/oauth/authorize?${query()}`));
  invalid(page(target(`${PROD_HOST}.`)));
  invalid(page(target(`x${PROD_HOST}`)));
  invalid(page(target(`sub.${PROD_HOST}`)));
  invalid(page(target('clerk.zaaheen.com%2eevil.com')));
});

test('a mixed-case host is normalised, then must match exactly', () => {
  ok(page(target('Clerk.Zaaheen.COM')));
  invalid(page(target('Clerk.Zaaheen.COM.evil.com')));
});

test('refuses user-info and any explicit port', () => {
  invalid(page(`https://user@${PROD_HOST}/oauth/authorize?${query()}`));
  invalid(page(`https://user:pass@${PROD_HOST}/oauth/authorize?${query()}`));
  invalid(page(`https://:@${PROD_HOST}/oauth/authorize?${query()}`));
  invalid(page(`https://${PROD_HOST}:443/oauth/authorize?${query()}`));
  invalid(page(`https://${PROD_HOST}:8443/oauth/authorize?${query()}`));
});

test('refuses other schemes, relative and protocol-relative links', () => {
  for (const raw of [
    'javascript:alert(1)', 'JaVaScRiPt:alert(1)', 'data:text/html,<script>x</script>', 'vbscript:x',
    `/oauth/authorize?${query()}`, `oauth/authorize?${query()}`, `//${PROD_HOST}/oauth/authorize?${query()}`,
    `https:/${PROD_HOST}/oauth/authorize?${query()}`, `https:${PROD_HOST}/oauth/authorize?${query()}`,
    `https:///${PROD_HOST}/oauth/authorize?${query()}`,
  ]) invalid(page(raw));
});

// --- Refused: path tricks (D4) -------------------------------------------------

test('refuses any other path, and encoded or dot-segment tricks toward the allowed ones', () => {
  for (const path of [
    '/oauth/authorize/', '/oauth/token', '/', '', '/OAUTH/AUTHORIZE', '/oauth//authorize',
    '/x/../oauth/authorize', '/oauth/./authorize', '/oauth/%2e%2e/oauth/authorize', '/oauth%2fauthorize',
    '/oauth/authorize%2F', '/oauth/authorize;x', '/oauth/authorize-with-immediate-redirect/..',
    '/oauth/%61uthorize',
  ]) invalid(page(target(PROD_HOST, path)));
});

test('refuses backslashes, whitespace and control characters anywhere in the link', () => {
  for (const bad of ['\\', '\t', '\n', '\r', ' ', '\u0000', '\u007f', ' ']) {
    invalid(page(target().replace('/oauth', `${bad}/oauth`)));
    invalid(page(`${target()}${bad}`));
    invalid(page(`${bad}${target()}`));
  }
  invalid(page(`https:\\\\${PROD_HOST}/oauth/authorize?${query()}`));
});

test('refuses a fragment, even an empty one', () => {
  invalid(page(`${target()}#x`));
  invalid(page(`${target()}#`));
});

// --- Refused: the page's own query (D4) ---------------------------------------

test('refuses two redirect_url parameters, even two identical good ones', () => {
  const one = new URLSearchParams({ redirect_url: target() }).toString();
  invalid(`?${one}&${one}`);
});

// --- Refused: the app parameters (D4) ------------------------------------------

test('refuses a wrong, missing or repeated client_id', () => {
  invalid(page(target(PROD_HOST, '/oauth/authorize', query({ client_id: 'someoneelse0000' }))));
  invalid(page(target(PROD_HOST, '/oauth/authorize', query({ client_id: undefined }))));
  invalid(page(target(PROD_HOST, '/oauth/authorize', `${query()}&client_id=${CLIENT}`)));
  invalid(page(target(PROD_HOST, '/oauth/authorize', query({ client_id: CLIENT.toUpperCase() }))));
});

test('refuses any redirect_uri but the app loopback', () => {
  for (const redirect_uri of [
    'http://localhost:53999/callback', 'https://127.0.0.1:53999/callback', 'http://127.0.0.1:53999/other',
    'http://127.0.0.1:53999/callback/', 'http://127.0.0.1/callback', 'http://127.0.0.1:0/callback',
    'http://127.0.0.1:053999/callback', 'http://127.0.0.1:123456/callback', 'http://127.0.0.2:53999/callback',
    'http://127.0.0.1:53999/callback?x=1', 'http://127.0.0.1:53999/callback#x', 'http://evil.com/callback',
    'http://127.0.0.1.evil.com:53999/callback', 'http://[::1]:53999/callback', '',
  ]) invalid(page(target(PROD_HOST, '/oauth/authorize', query({ redirect_uri }))));
  // The whole range of real ports is fine.
  for (const port of [1, 1024, 53999, 65535]) ok(page(target(PROD_HOST, '/oauth/authorize', query({ redirect_uri: `http://127.0.0.1:${port}/callback` }))));
  invalid(page(target(PROD_HOST, '/oauth/authorize', query({ redirect_uri: 'http://127.0.0.1:65536/callback' }))));
});

test('refuses a missing or wrong flow: response_type, S256, state, code_challenge, scope', () => {
  for (const over of [
    { response_type: undefined }, { response_type: 'token' }, { response_type: 'code id_token' },
    { code_challenge_method: undefined }, { code_challenge_method: 'plain' }, { code_challenge_method: 's256' },
    { state: undefined }, { state: '' }, { state: STATE.slice(1) }, { state: `${STATE}A` }, { state: STATE.replace('A', '+') },
    { code_challenge: undefined }, { code_challenge: '' }, { code_challenge: CHALLENGE.replace('Z', '=') },
    { scope: undefined }, { scope: 'openid' }, { scope: 'openid profile email offline_access admin' },
    { scope: 'email openid offline_access profile' },
  ]) invalid(page(target(PROD_HOST, '/oauth/authorize', query(over))));
});

test('refuses any extra parameter, and any app parameter given twice', () => {
  for (const extra of ['prompt=none', 'nonce=x', 'redirect_url=x', 'client_id2=x', 'x=', 'STATE=x']) {
    invalid(page(target(PROD_HOST, '/oauth/authorize', `${query()}&${extra}`)));
  }
  for (const k of ['state', 'code_challenge', 'redirect_uri', 'response_type', 'scope', 'code_challenge_method']) {
    const q = new URLSearchParams(query());
    q.append(k, q.get(k));
    invalid(page(target(PROD_HOST, '/oauth/authorize', q.toString())));
  }
});

// --- The second check, after Clerk builds the final URL (D4) --------------------

test('checkTarget re-validates a built URL and returns a URL object', () => {
  const url = checkTarget(target(), PROD);
  assert.ok(url instanceof URL);
  assert.equal(url.href, new URL(target()).href);
  assert.equal(checkTarget(`${target()}&extra=1`, PROD), null);
  assert.equal(checkTarget(target('evil.com'), PROD), null);
  assert.equal(checkTarget(123, PROD), null);
  assert.equal(checkTarget(null, PROD), null);
  // Clerk's development handshake adds __clerk_db_jwt: fine in development only.
  assert.ok(checkTarget(`${target(DEV_HOST)}&__clerk_db_jwt=dvb_x`, DEV));
  assert.equal(checkTarget(`${target()}&__clerk_db_jwt=dvb_x`, PROD), null);
});

// --- Where Clerk may send the browser after Google (D3, D4) ----------------------
// handleRedirectCallback navigates by itself; the page gives it a navigate
// function that only follows URLs the page itself handed over (same origin,
// exact string) or a way back that passes checkTarget.

test('checkNavigation: our own handed-over URLs, exactly, on our own origin', () => {
  const own = 'https://account.zaaheen.com';
  const handed = [`${own}/sign-up/?continue=1`, `${own}/sign-in/`];
  assert.equal(checkNavigation(`${own}/sign-up/?continue=1`, handed, PROD, own).href, `${own}/sign-up/?continue=1`);
  // Relative forms Clerk may use resolve against our origin first.
  assert.equal(checkNavigation('/sign-in/', handed, PROD, own).href, `${own}/sign-in/`);
  assert.equal(checkNavigation(`${own}/sign-up/?continue=2`, handed, PROD, own), null);
  assert.equal(checkNavigation(`${own}/sign-up/`, handed, PROD, own), null);
  assert.equal(checkNavigation('https://evil.com/sign-in/', handed, PROD, own), null);
  assert.equal(checkNavigation('//evil.com/sign-in/', handed, PROD, own), null);
  assert.equal(checkNavigation('javascript:alert(1)', handed, PROD, own), null);
});

test('checkNavigation: otherwise only a way back that passes checkTarget', () => {
  const own = 'https://account.zaaheen.com';
  assert.ok(checkNavigation(target(), [], PROD, own));
  assert.equal(checkNavigation(target('evil.com'), [], PROD, own), null);
  assert.equal(checkNavigation(`${target()}&x=1`, [], PROD, own), null);
  assert.equal(checkNavigation(target(), [], null, own), null);
  assert.equal(checkNavigation(undefined, [], PROD, own), null);
});
