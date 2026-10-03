// The Documents search index's text clean-up (CodeQL, session 84).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { decode, text } from '../src/integrations/search-index.mjs';

test('entities decode in one pass, never twice', () => {
  assert.equal(decode('&amp;lt;b&amp;gt;'), '&lt;b&gt;');
  assert.equal(decode('a&nbsp;&quot;b&quot; &#39;c&#x27; &apos;d&apos; &#8230;'), 'a "b" \'c\' \'d\' …');
  assert.equal(decode('&unknown; &#0;'), '&unknown; &#0;');
});

test('script blocks are dropped, whatever their end tag looks like', () => {
  assert.equal(text('a<script>x()</script>b'), 'a b');
  assert.equal(text('a<script>x()</script >b'), 'a b');
  assert.equal(text('a<SCRIPT type="m">x()</Script\n foo>b'), 'a b');
});

test('tags become spaces and whitespace collapses', () => {
  assert.equal(text('<p>Copy  the <code>path</code></p>\n<p>then&nbsp;save</p>'), 'Copy the path then save');
});
