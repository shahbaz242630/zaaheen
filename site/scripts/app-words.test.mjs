// The connect guides take their wording from the desktop app itself
// (src/data/app-words.ts reads the CONNECT WORDING block of
// crates/vault-tauri/dist/app.js). Two things the site repeats instead of
// reading are pinned here, so a change in the app fails the site's tests
// until the site follows (session 82): the block's markers, and the step
// titles the app builds in renderAgentCards.
//
//   node --test scripts/app-words.test.mjs     (from site/; no build needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const APP = fs.readFileSync(path.resolve(HERE, '..', '..', 'crates', 'vault-tauri', 'dist', 'app.js'), 'utf8');
const WORDS = fs.readFileSync(path.resolve(HERE, '..', 'src', 'data', 'app-words.ts'), 'utf8');

test('the app marks its connect wording for the site, once each way', () => {
  assert.equal(APP.split('// >>> CONNECT WORDING').length, 2);
  assert.equal(APP.split('// <<< END CONNECT WORDING').length, 2);
  assert.ok(APP.indexOf('// >>> CONNECT WORDING') < APP.indexOf('// <<< END CONNECT WORDING'));
});

test("the guides' step titles are the app's, word for word", () => {
  for (const title of [
    '"Step 1A: The quick way"',
    '"Step 1B: Or add it yourself"',
    '"Step 1: Add it yourself"',
    "`Step ${step}: Check it's connected`",
    "`Step ${step}: Tell ${agent.name} to use Zaaheen (don't skip this)`",
    '`Please follow ${stepsWord(step)}. If you skip one, ${appCalled(agent.name)} may not use Zaaheen.`',
  ]) {
    assert.ok(APP.includes(title), `app.js no longer says ${title}`);
  }
  for (const title of [
    "'Step 1A: The quick way'",
    "'Step 1B: Or add it yourself'",
    "'Step 1: Add it yourself'",
    "`Step ${(step += 1)}: Check it's connected`",
    "`Step ${(step += 1)}: Tell ${name} to use Zaaheen (don't skip this)`",
    '`Please follow ${words.stepsWord(step)}. If you skip one, ${words.appCalled(name)} may not use Zaaheen.`',
  ]) {
    assert.ok(WORDS.includes(title), `app-words.ts no longer says ${title}`);
  }
});

// --- One app, one guide (session 83, founder: "make sure ... they dont mingle
// up again"). Every app in the app's Agents list has exactly one guide page,
// each guide page shows exactly one app, the Documents menu and llms.txt list
// that page, and the wording helpers live in the app's block, not the site.
const BLOCK = (() => {
  const src = APP.replace(/\r\n/g, '\n');
  return src.slice(src.indexOf('// >>> CONNECT WORDING'), src.indexOf('// <<< END CONNECT WORDING'));
})();
const appNames = (isMac) =>
  new Function('IS_MAC', `${BLOCK}\nreturn AGENTS.map((a) => a.name);`)(isMac);
const DOCS = path.resolve(HERE, '..', 'src', 'pages', 'docs');
const GUIDE_FILES = fs.readdirSync(DOCS).filter((f) => f.startsWith('connect-') && f.endsWith('.astro'));
// The app each guide shows: its NAME constant, or a name="..." on ConnectSteps.
const shownBy = (file) => {
  const src = fs.readFileSync(path.join(DOCS, file), 'utf8');
  const names = [
    ...[...src.matchAll(/const NAME = '([^']+)';/g)].map((m) => m[1]),
    ...[...src.matchAll(/<ConnectSteps name="([^"]+)"/g)].map((m) => m[1]),
  ];
  const steps = (src.match(/<ConnectSteps\b/g) || []).length;
  return { names, steps };
};

test('every app in the Agents list has exactly one guide, and each guide shows one app', () => {
  const owners = new Map();
  for (const file of GUIDE_FILES) {
    const { names, steps } = shownBy(file);
    assert.equal(steps, 1, `${file} shows ${steps} apps' steps; each app needs its own guide`);
    assert.equal(names.length, 1, `${file} must name exactly one app`);
    assert.ok(!owners.has(names[0]), `${names[0]} has two guides: ${owners.get(names[0])} and ${file}`);
    owners.set(names[0], file);
  }
  for (const isMac of [false, true]) {
    for (const name of appNames(isMac)) {
      assert.ok(owners.has(name), `the app lists ${name} but no guide in src/pages/docs shows it`);
    }
  }
  for (const name of owners.keys()) {
    assert.ok(appNames(false).includes(name), `a guide shows ${name}, which the app does not list`);
  }
});

test('every guide is in the Documents menu and in llms.txt', () => {
  const site = fs.readFileSync(path.resolve(HERE, '..', 'src', 'data', 'site.ts'), 'utf8');
  const llms = fs.readFileSync(path.resolve(HERE, '..', 'src', 'pages', 'llms.txt.ts'), 'utf8');
  for (const file of GUIDE_FILES) {
    const url = `/docs/${file.replace(/\.astro$/, '')}/`;
    assert.ok(site.includes(`{ path: '${url}', label: `), `${url} is missing from GUIDES (the Documents menu)`);
    assert.ok(site.includes(`    path: '${url}',`), `${url} has no PAGES entry`);
    assert.ok(llms.includes(`'${url}'`), `${url} is missing from llms.txt's guide list`);
  }
});

test('the step wording comes from the app, never a copy on the site', () => {
  assert.ok(BLOCK.includes('function stepsWord(n) {') && BLOCK.includes('function appCalled(name) {'));
  assert.ok(!/'both steps'|"both steps"/.test(WORDS), "app-words.ts must call the app's stepsWord, not repeat it");
  assert.ok(!WORDS.includes("'your app'"), "app-words.ts must call the app's appCalled, not repeat it");
  const helpers = new Function('IS_MAC', `${BLOCK}\nreturn { stepsWord, appCalled };`)(false);
  assert.equal(helpers.stepsWord(2), 'both steps');
  assert.equal(helpers.stepsWord(3), 'all 3 steps');
  assert.equal(helpers.appCalled('Another app'), 'your app');
  assert.equal(helpers.appCalled('Codex'), 'Codex');
});
