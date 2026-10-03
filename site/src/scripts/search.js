// Documents search (founder, session 83). The index is /search-index.json,
// built from the finished pages (src/integrations/search-index.mjs) and
// fetched from this site only when someone starts typing; the search itself
// runs here, in the browser. Nothing typed is sent anywhere. Results are
// built with textContent, never as HTML. Without scripts the box stays
// hidden and the Documents menu works as before.
const form = document.querySelector('[data-search]');
const input = form?.querySelector('input');
const list = form?.querySelector('ul');

const MAX = 8;
let index = null;
let loading = null;

const load = () => {
  loading ??= fetch('/search-index.json')
    .then((r) => (r.ok ? r.json() : []))
    .then((data) => { index = Array.isArray(data) ? data : []; })
    .catch(() => { index = []; });
  return loading;
};

const words = (q) => q.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter((w) => w.length > 1);

// Every word must appear somewhere; the heading counts most, then the page,
// then the text, and the whole phrase in the heading counts most of all.
function search(q) {
  const ws = words(q);
  if (!ws.length || !index) return [];
  const phrase = q.trim().toLowerCase();
  return index
    .map((e) => {
      const title = e.title.toLowerCase();
      const page = e.page.toLowerCase();
      const body = e.text.toLowerCase();
      let score = 0;
      for (const w of ws) {
        const hit = (title.includes(w) ? 3 : 0) + (page.includes(w) ? 2 : 0) + (body.includes(w) ? 1 : 0);
        if (!hit) return null;
        score += hit;
      }
      if (title.includes(phrase)) score += 10;
      else if (body.includes(phrase)) score += 4;
      return { e, score };
    })
    .filter(Boolean)
    .sort((a, b) => b.score - a.score)
    .slice(0, MAX)
    .map((r) => r.e);
}

// The start of the text when the heading already matched; otherwise around
// the first word found in the text.
function snippet(textIn, q, title) {
  const first = words(q)[0] ?? '';
  const at = title.toLowerCase().includes(first) ? 0 : textIn.toLowerCase().indexOf(first);
  const start = Math.max(0, at - 40);
  const s = textIn.slice(start, start + 140);
  return (start > 0 ? '…' : '') + s + (start + 140 < textIn.length ? '…' : '');
}

function show(q) {
  list.replaceChildren();
  if (!q.trim()) { list.hidden = true; return; }
  const found = search(q);
  list.hidden = false;
  if (!found.length) {
    const li = document.createElement('li');
    li.className = 'docs-search-none';
    li.append('Nothing found. Try other words, or ');
    const a = document.createElement('a');
    a.href = '/docs/contact/';
    a.textContent = 'contact us';
    li.append(a, '.');
    list.append(li);
    return;
  }
  for (const e of found) {
    const li = document.createElement('li');
    const a = document.createElement('a');
    a.href = e.url;
    const t = document.createElement('span');
    t.className = 'docs-search-title';
    t.textContent = e.title || e.page;
    const p = document.createElement('span');
    p.className = 'docs-search-page';
    p.textContent = e.page;
    const s = document.createElement('span');
    s.className = 'docs-search-snippet';
    s.textContent = snippet(e.text, q, e.title);
    a.append(t, p, s);
    li.append(a);
    list.append(li);
  }
}

if (form && input && list) {
  form.hidden = false;
  input.addEventListener('focus', load, { once: true });
  input.addEventListener('input', async () => {
    await load();
    show(input.value);
  });
  input.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') { input.value = ''; show(''); }
  });
  // Enter opens the best result.
  form.addEventListener('submit', (ev) => {
    ev.preventDefault();
    const first = list.querySelector('a');
    if (first) window.location.href = first.href;
  });
}
