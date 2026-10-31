// changed-pages.sh decides which pages IndexNow hears about. Every page links
// shared assets whose names carry a content hash, so one styling edit renames
// them on all 153 pages: the search engines must not hear "153 pages changed".
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT = path.join(path.dirname(fileURLToPath(import.meta.url)), 'changed-pages.sh');

const page = (body, css = 'Base.AAAA1111', js = 'Download.astro_astro_type_script_index_0_lang.BBBB-22_') =>
  `<link rel="stylesheet" href="/_astro/${css}.css"><script type="module" src="/_astro/${js}.js"></script><p>${body}</p>\n`;

// A throwaway repository with one deploy commit on top of another, the shape
// the publish job has when the script runs. Returns the script's output lines.
function changedBetween(before, after) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'changed-pages-'));
  try {
    const git = (...args) => execFileSync('git', args, { cwd: dir, encoding: 'utf8' });
    const write = (files) => {
      for (const name of fs.readdirSync(dir)) {
        if (name !== '.git') fs.rmSync(path.join(dir, name), { recursive: true });
      }
      for (const [name, text] of Object.entries(files)) {
        fs.mkdirSync(path.dirname(path.join(dir, name)), { recursive: true });
        fs.writeFileSync(path.join(dir, name), text);
      }
      git('add', '-A');
      git('-c', 'user.name=t', '-c', 'user.email=t@example.invalid', 'commit', '-q', '--allow-empty', '-m', 'deploy');
    };
    git('init', '-q');
    git('config', 'core.autocrlf', 'false');
    write(before);
    write(after);
    return execFileSync('bash', [SCRIPT.replaceAll('\\', '/')], { cwd: dir, encoding: 'utf8' })
      .split('\n')
      .filter(Boolean)
      .sort();
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test('a renamed shared stylesheet alone is not a changed page', () => {
  const out = changedBetween(
    { 'index.html': page('home'), 'pricing/index.html': page('price') },
    { 'index.html': page('home', 'Base.CCCC3333'), 'pricing/index.html': page('price', 'Base.CCCC3333') },
  );
  assert.deepEqual(out, []);
});

test('a renamed shared script alone is not a changed page', () => {
  const out = changedBetween(
    { 'index.html': page('home') },
    { 'index.html': page('home', undefined, 'Download.astro_astro_type_script_index_0_lang.DDDD_44-') },
  );
  assert.deepEqual(out, []);
});

test('a page whose words changed is listed, even when the assets were renamed too', () => {
  const out = changedBetween(
    { 'index.html': page('home'), 'pricing/index.html': page('price') },
    { 'index.html': page('home', 'Base.CCCC3333'), 'pricing/index.html': page('new price', 'Base.CCCC3333') },
  );
  assert.deepEqual(out, ['pricing/index.html']);
});

test('a new page is listed', () => {
  const out = changedBetween(
    { 'index.html': page('home') },
    { 'index.html': page('home'), 'learn/new/index.html': page('new') },
  );
  assert.deepEqual(out, ['learn/new/index.html']);
});

test('a deleted page does not stop the run', () => {
  const out = changedBetween(
    { 'index.html': page('home'), 'old/index.html': page('old') },
    { 'index.html': page('home') },
  );
  assert.deepEqual(out, ['old/index.html']);
});

test('files that are not pages are passed through as before', () => {
  const out = changedBetween(
    { 'index.html': page('home'), 'build.txt': 'one\n' },
    { 'index.html': page('home'), 'build.txt': 'two\n' },
  );
  assert.deepEqual(out, ['build.txt']);
});

test('an asset name outside /_astro/ still counts as a change', () => {
  const out = changedBetween(
    { 'index.html': '<img src="/og-v3.png">\n' },
    { 'index.html': '<img src="/og-v4.png">\n' },
  );
  assert.deepEqual(out, ['index.html']);
});
