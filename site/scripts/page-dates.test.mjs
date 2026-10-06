// The page-date fingerprint reads the page's own words only (SEO audit,
// session 90: a new guide in the Documents menu re-dated every policy).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fingerprint, pageText } from './page-dates.mjs';

const page = ({ menu = 'Getting started', toc = 'Install', words = 'Your memories stay here.' } = {}) =>
  `<title>Privacy · Zaaheen</title><meta name="description" content="How we handle data.">
   <main><nav class="docs-sections" aria-label="Documents"><a href="/docs/">${menu}</a></nav>
   <h1>Privacy</h1><p class="doc-updated">Last updated 1 October 2026</p><p>${words}</p>
   <aside class="docs-toc" aria-label="On this page"><a href="#a">${toc}</a></aside></main>`;

test('a change to the Documents menu leaves the date alone', () => {
  assert.equal(fingerprint(page()), fingerprint(page({ menu: 'Getting started Release notes' })));
});

test('a change to the "On this page" list leaves the date alone', () => {
  assert.equal(fingerprint(page()), fingerprint(page({ toc: 'Install Set up' })));
});

test('a change to the page\'s own words moves the date', () => {
  assert.notEqual(fingerprint(page()), fingerprint(page({ words: 'Your memories stay on your computer.' })));
});

test('the menu and the outline are not part of the page text', () => {
  const t = pageText(page({ menu: 'MENUWORD', toc: 'TOCWORD' }));
  assert.ok(!t.includes('MENUWORD') && !t.includes('TOCWORD'));
  assert.ok(t.includes('Your memories stay here.') && !t.includes('Last updated'));
});
