// The Knowledge Centre's coaching data (src/data/coaching.ts) must match the
// coaching app, which owns it. This compares them field by field whenever the
// coaching app ("Training Page") is checked out next to this repo, and fails on
// any difference. Where it is not (CI), it says so and passes: CI cannot see
// another repository.
//
//   node --test scripts/coaching-sync.test.mjs
//   COACHING_REPO=<path> node --test scripts/coaching-sync.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = process.env.COACHING_REPO || path.resolve(here, '../../../Training Page');
const ours = fs.readFileSync(path.join(here, '../src/data/coaching.ts'), 'utf8');
const theirs = (rel) => fs.readFileSync(path.join(repo, rel), 'utf8');
const present = fs.existsSync(path.join(repo, 'src/config/sessions.ts'));

// Ours: one line per session, e.g. { number: 1, slug: '…', shortTitle: '…', priceAed: 1299, hasPrerequisites: false }
const oursSessions = [...ours.matchAll(/\{ number: (\d+), slug: '([^']+)', shortTitle: '([^']+)', priceAed: (\d+), hasPrerequisites: (true|false) \}/g)]
  .map((m) => ({ number: +m[1], slug: m[2], shortTitle: m[3], priceAed: +m[4], hasPrerequisites: m[5] === 'true' }));

test('our mirror parses', () => {
  assert.equal(oursSessions.length, 6, 'expected six sessions in src/data/coaching.ts');
});

test('sessions and prices match the coaching app', { skip: !present && `coaching app not found at ${repo}` }, () => {
  const src = theirs('src/config/sessions.ts');
  const blocks = src.split(/\n  \{\n/).slice(1);
  const their = blocks.map((b) => ({
    number: +(b.match(/code: "S(\d)"/) || [])[1],
    slug: (b.match(/slug: "([^"]+)"/) || [])[1],
    shortTitle: (b.match(/shortTitle: "([^"]+)"/) || [])[1],
    priceAed: +(b.match(/priceFils: aedToFils\((\d+)\)/) || [])[1],
    hasPrerequisites: !/prerequisites: \[\]/.test(b),
    active: /active: true/.test(b),
  })).filter((s) => s.active).map(({ active, ...s }) => s);
  assert.deepEqual(oursSessions, their);
});

test('stages, delivery and steps match the coaching app', { skip: !present && `coaching app not found at ${repo}` }, () => {
  const prog = theirs('src/components/training/Progression.tsx');
  for (const m of ours.matchAll(/\{ name: '([^']+)', description: '([^']+)', sessions: \[([\d, ]+)\] \}/g)) {
    const codes = m[3].split(',').map((n) => `"S${n.trim()}"`).join(', ');
    assert.ok(prog.includes(`name: "${m[1]}"`), `stage ${m[1]}`);
    assert.ok(prog.includes(`description: "${m[2]}"`), `stage ${m[1]} description`);
    assert.ok(prog.includes(`codes: [${codes}]`), `stage ${m[1]} sessions`);
  }
  const site = theirs('src/config/site.ts');
  for (const key of ['availability', 'timezoneLabel']) {
    const v = ours.match(new RegExp(`${key}: '([^']+)'`))[1];
    assert.ok(site.includes(`${key}: "${v}"`), `DELIVERY.${key}`);
  }
  assert.ok(site.includes(`durationMinutes: ${ours.match(/durationMinutes: (\d+)/)[1]}`), 'DELIVERY.durationMinutes');
  const how = theirs('src/components/training/HowItWorks.tsx');
  for (const m of ours.matchAll(/title: '([^']+)',\n\s+body: '([^']+)'/g)) {
    assert.ok(how.includes(`title: "${m[1]}"`), `step "${m[1]}"`);
    assert.ok(how.includes(`body: "${m[2]}"`), `step "${m[1]}" body`);
  }
});
