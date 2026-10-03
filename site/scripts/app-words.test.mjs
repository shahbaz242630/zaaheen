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
    '`Please follow all ${step} steps. If you skip one, ${agent.name} may not use Zaaheen.`',
  ]) {
    assert.ok(APP.includes(title), `app.js no longer says ${title}`);
  }
  for (const title of [
    "'Step 1A: The quick way'",
    "'Step 1B: Or add it yourself'",
    "'Step 1: Add it yourself'",
    "`Step ${(step += 1)}: Check it's connected`",
    "`Step ${(step += 1)}: Tell ${name} to use Zaaheen (don't skip this)`",
    '`Please follow all ${step} steps. If you skip one, ${name} may not use Zaaheen.`',
  ]) {
    assert.ok(WORDS.includes(title), `app-words.ts no longer says ${title}`);
  }
});
