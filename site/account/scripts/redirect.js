// Where the account pages may send the browser (AUTH-PAGES-DESIGN D3 + D4),
// kept free of the DOM and of Clerk so it can be tested in plain Node
// (scripts/redirect.test.mjs). The pages navigate only to this module's output.
//
// The one way back is the Zaaheen app's own sign-in request: Clerk's
// authorize endpoint (after Clerk wraps it) carrying exactly the parameters
// vault-account sends (signin/mod.rs), for our client and a loopback listener.
// A development build (a pk_test_ key) also accepts the Portal's consent page
// and Clerk's development handshake parameter, measured in build step 1 (M8).

const APP_SCOPE = 'openid profile email offline_access';
// vault-account: 32 random bytes / a SHA-256, base64url without padding.
const TOKEN_43 = /^[A-Za-z0-9_-]{43}$/;
const LOOPBACK = /^http:\/\/127\.0\.0\.1:([1-9][0-9]{0,4})\/callback$/;
const HOST = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/;
const CLIENT_ID = /^[A-Za-z0-9_-]{1,128}$/;
const DEV_HANDSHAKE = /^[A-Za-z0-9_.-]{1,512}$/;
const FAPI_PATHS = ['/oauth/authorize-with-immediate-redirect', '/oauth/authorize'];
const CONSENT_PATH = '/oauth-consent';
const DEV_FAPI_SUFFIX = '.clerk.accounts.dev';

/**
 * The build settings (D3): the publishable key names the Frontend API host
 * (a `pk_` key is base64 of `<host>$`), so the key and the host can never
 * disagree. `pk_live_` is a production build; `pk_test_` a development one.
 * Anything missing, undecodable or odd is null: null matches no link at all.
 */
export function readAccountConfig({ publishableKey, clientId } = {}) {
  if (typeof publishableKey !== 'string' || typeof clientId !== 'string') return null;
  const m = /^pk_(live|test)_([A-Za-z0-9+/]+={0,2})$/.exec(publishableKey);
  if (!m || !CLIENT_ID.test(clientId)) return null;
  let decoded;
  try {
    decoded = decodeBase64(m[2]);
  } catch {
    return null;
  }
  if (!decoded.endsWith('$')) return null;
  const fapiHost = decoded.slice(0, -1);
  if (!HOST.test(fapiHost)) return null;
  const dev = m[1] === 'test';
  const portalHost = dev && fapiHost.endsWith(DEV_FAPI_SUFFIX)
    ? `${fapiHost.slice(0, -DEV_FAPI_SUFFIX.length)}.accounts.dev`
    : null;
  return { fapiHost, portalHost, clientId, dev };
}

/**
 * The page's own `redirect_url`: `{ state: 'none' }` when there is none (a
 * sign-in on the website itself), `{ state: 'ok', url }` for an accepted way
 * back, `{ state: 'invalid' }` for anything else, which the page refuses on
 * load before any email is typed.
 */
export function readRedirect(search, config) {
  const values = new URLSearchParams(typeof search === 'string' ? search : '').getAll('redirect_url');
  if (values.length === 0) return { state: 'none' };
  if (values.length !== 1) return { state: 'invalid' };
  const url = checkTarget(values[0], config);
  return url ? { state: 'ok', url } : { state: 'invalid' };
}

/**
 * One link, checked: the parsed URL when it is an accepted way back, else
 * null. Used on load and again on the URL Clerk builds before navigating.
 */
export function checkTarget(raw, config) {
  if (!config || typeof raw !== 'string') return null;
  // The URL parser silently drops tabs and newlines and turns backslashes
  // into slashes; refuse them (and every other oddity) before it sees them.
  if (!/^https:\/\/[\x21-\x7e]+$/.test(raw) || raw.includes('\\') || raw.includes('#')) return null;
  // The authority as written: host only. The parser drops an empty "user:@"
  // and a default ":443", so those are refused here, not after parsing.
  const pathStart = raw.indexOf('/', 'https://'.length);
  const queryStart = raw.indexOf('?');
  if (pathStart === -1 || (queryStart !== -1 && queryStart < pathStart)) return null;
  if (/[@:%]/.test(raw.slice('https://'.length, pathStart))) return null;
  let url;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (url.protocol !== 'https:' || url.username || url.password || url.port || url.hash) return null;
  // The path exactly as written: no dot-segments or encodings that the parser
  // would normalise into an allowed path.
  const rawPath = raw.slice(pathStart, queryStart === -1 ? undefined : queryStart);
  if (rawPath !== url.pathname) return null;

  const fapi = url.hostname === config.fapiHost && FAPI_PATHS.includes(url.pathname);
  const consent = config.dev && config.portalHost !== null
    && url.hostname === config.portalHost && url.pathname === CONSENT_PATH;
  if (!fapi && !consent) return null;
  return appParamsOk(url.searchParams, config) ? url : null;
}

/**
 * Where Clerk may send the browser during Google's return (the navigate
 * function handed to handleRedirectCallback): one of the URLs the page handed
 * it, exactly, on our own origin, or a way back that passes checkTarget.
 * Else null. A FINISHED sign-in does not come through here (Clerk navigates to
 * its redirect props itself); account.js makes those props validator output.
 */
export function checkNavigation(to, handed, config, ownOrigin) {
  if (typeof to !== 'string' || typeof ownOrigin !== 'string') return null;
  let url;
  try {
    url = new URL(to, ownOrigin);
  } catch {
    return null;
  }
  if (url.origin === ownOrigin && Array.isArray(handed) && handed.includes(url.href)) return url;
  return checkTarget(to, config);
}

function appParamsOk(params, config) {
  const seen = new Map();
  for (const [k, v] of params) {
    if (seen.has(k)) return false;
    seen.set(k, v);
  }
  const required = {
    client_id: (v) => v === config.clientId,
    redirect_uri: (v) => {
      const m = LOOPBACK.exec(v);
      return m !== null && Number(m[1]) <= 65535;
    },
    response_type: (v) => v === 'code',
    code_challenge_method: (v) => v === 'S256',
    state: (v) => TOKEN_43.test(v),
    code_challenge: (v) => TOKEN_43.test(v),
    scope: (v) => v === APP_SCOPE,
  };
  for (const [k, ok] of Object.entries(required)) {
    if (!seen.has(k) || !ok(seen.get(k))) return false;
  }
  for (const [k, v] of seen) {
    if (Object.hasOwn(required, k)) continue;
    if (config.dev && k === '__clerk_db_jwt' && DEV_HANDSHAKE.test(v)) continue;
    return false;
  }
  return true;
}

// atob in browsers, Buffer in Node: the same strict decode either way.
function decodeBase64(b64) {
  const text = typeof atob === 'function' ? atob(b64) : Buffer.from(b64, 'base64').toString('latin1');
  if (btoaSafe(text) !== b64) throw new Error('not canonical base64');
  return text;
}

function btoaSafe(text) {
  return typeof btoa === 'function' ? btoa(text) : Buffer.from(text, 'latin1').toString('base64');
}
