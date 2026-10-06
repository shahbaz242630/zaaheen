// The published download fingerprints (security audit s89): each installer's
// SHA-256 is on the Getting started page and in llms.txt so people can check
// their download. A release that forgets to update one would publish a wrong
// fingerprint, so its shape is checked here and its file name must match the
// version. The value itself is read back from dl.zaaheen.com at release time.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';

const SITE_TS = new URL('../src/data/site.ts', import.meta.url);
const text = fs.readFileSync(SITE_TS, 'utf8');
const release = text.slice(text.indexOf('export const RELEASE'), text.indexOf('} as const;', text.indexOf('export const RELEASE')));

function field(block, name) {
  const m = block.match(new RegExp(`\\b${name}: '([^']*)'`));
  assert.ok(m, `RELEASE is missing ${name}`);
  return m[1];
}

test('each installer has a well-formed SHA-256 fingerprint', () => {
  const version = field(release, 'version');
  for (const os of ['windows', 'mac']) {
    const start = release.indexOf(`${os}: {`);
    assert.ok(start >= 0, `RELEASE.${os} not found`);
    const block = release.slice(start, release.indexOf('},', start));
    assert.match(field(block, 'sha256'), /^[0-9a-f]{64}$/, `RELEASE.${os}.sha256`);
    assert.ok(field(block, 'file').includes(`_${version}_`), `RELEASE.${os}.file names ${version}`);
  }
});

test('the two fingerprints differ (one was not copied over the other)', () => {
  const all = [...release.matchAll(/sha256: '([0-9a-f]{64})'/g)].map((m) => m[1]);
  assert.equal(all.length, 2);
  assert.notEqual(all[0], all[1]);
});
