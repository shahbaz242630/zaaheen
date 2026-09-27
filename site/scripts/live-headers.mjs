// The security headers our own server config promises, and whether a live
// response kept them exactly (security audit, session 72).
//
// Hostinger's CDN sits between Cloudflare and the origin, and on the coaching
// staging site it was measured replacing the whole Content-Security-Policy with
// `upgrade-insecure-requests`. A header set in .htaccess is only a promise; the
// post-deploy check (post-deploy.mjs) holds the live site to it.
//
// Only `Header always set` lines at the top level of <IfModule mod_headers.c>
// count: the per-file Cache-Control lines are not security headers. HSTS is
// left out by default because it is set at the Cloudflare edge, which may
// legitimately send a stronger value than the origin's.

const SET = /^\s*Header\s+always\s+set\s+([A-Za-z0-9-]+)\s+"([^"]*)"\s*$/;

/** name (lower case) -> value, for every `Header always set` line. */
export function expectedHeaders(htaccess, { skip = ['strict-transport-security'] } = {}) {
  const out = {};
  for (const line of htaccess.split(/\r?\n/)) {
    const m = SET.exec(line);
    if (m && !skip.includes(m[1].toLowerCase())) out[m[1].toLowerCase()] = m[2];
  }
  return out;
}

/** A subfolder's .htaccess on top of the root's: the subfolder's lines win. */
export function mergeHeaders(root, folder) {
  return { ...root, ...folder };
}

/** Every header that did not arrive exactly as promised. `actual` is a Headers-like get(). */
export function headerProblems(expected, actual) {
  const problems = [];
  for (const [name, want] of Object.entries(expected)) {
    const got = actual.get(name);
    if (got === null || got === undefined) problems.push(`${name} is missing (expected "${want}")`);
    else if (got !== want) problems.push(`${name} was changed on the way: expected "${want}", got "${got}"`);
  }
  return problems;
}
