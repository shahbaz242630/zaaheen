// The post-deploy header check (live-headers.mjs). The case that matters is the
// one measured on the coaching staging site: a CDN between Cloudflare and the
// origin replacing the whole Content-Security-Policy.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expectedHeaders, headerProblems, mergeHeaders } from './live-headers.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT_HT = fs.readFileSync(path.join(HERE, '..', 'public', '.htaccess'), 'utf8');
const PAY_HT = fs.readFileSync(path.join(HERE, '..', 'public', 'pay', '.htaccess'), 'utf8');
const headers = (map) => ({ get: (name) => map[name.toLowerCase()] ?? null });

test('reads every security header the real .htaccess sets, and nothing else', () => {
  const expected = expectedHeaders(ROOT_HT);
  assert.deepEqual(Object.keys(expected).sort(), [
    'content-security-policy', 'permissions-policy', 'referrer-policy', 'x-content-type-options', 'x-frame-options',
  ]);
  assert.equal(expected['x-frame-options'], 'DENY');
  assert.match(expected['content-security-policy'], /^default-src 'self'; script-src 'self';/);
});

test('leaves HSTS out by default (it is set at the edge), but can include it', () => {
  assert.equal(expectedHeaders(ROOT_HT)['strict-transport-security'], undefined);
  assert.equal(expectedHeaders(ROOT_HT, { skip: [] })['strict-transport-security'], 'max-age=31536000');
});

test('the pay page keeps its own policy on top of the site-wide headers', () => {
  const pay = mergeHeaders(expectedHeaders(ROOT_HT), expectedHeaders(PAY_HT));
  assert.match(pay['content-security-policy'], /https:\/\/cdn\.paddle\.com/);
  assert.equal(pay['x-robots-tag'], 'noindex');
  assert.equal(pay['x-frame-options'], 'DENY');
});

test('passes when every header arrived exactly', () => {
  const expected = expectedHeaders(ROOT_HT);
  assert.deepEqual(headerProblems(expected, headers(expected)), []);
});

// Measured on the coaching staging site, 2026-09-27.
test('fails when a CDN has replaced the policy', () => {
  const expected = expectedHeaders(ROOT_HT);
  const rewritten = { ...expected, 'content-security-policy': 'upgrade-insecure-requests' };
  const problems = headerProblems(expected, headers(rewritten));
  assert.equal(problems.length, 1);
  assert.match(problems[0], /content-security-policy was changed on the way/);
});

test('fails when a header is missing', () => {
  const expected = expectedHeaders(ROOT_HT);
  const { 'x-frame-options': _dropped, ...rest } = expected;
  assert.match(headerProblems(expected, headers(rest))[0], /x-frame-options is missing/);
});
