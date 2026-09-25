// The home page's demo window: a 19-second loop through the app's tabs.
// No framework, no third-party code, no inline styles (the CSP allows none):
// this file only builds elements with classes from global.css.
//
// The HTML already holds the first frame, so without scripts, or with reduced
// motion, the picture is simply still. Hovering the window pauses it.
const demo = document.getElementById('hero-demo');
const reduced = window.matchMedia('(prefers-reduced-motion: reduce)');

if (demo && !reduced.matches) {
  const title = document.getElementById('demo-title');
  const pane = document.getElementById('demo-pane');
  const tabs = [...document.querySelectorAll('#demo-tabs .win-tab')];
  const chips = [...demo.querySelectorAll('.demo-chip')];

  const LOOP = 19;
  const PHASES = [['Memories', 0, 7], ['Agents', 7, 12], ['Consolidation', 12, 15.5], ['Settings', 15.5, 19]];
  // The Agents tab shows DEMO_APPS (site.ts).
  const APPS = (demo.dataset.apps || '').split(',').filter(Boolean);
  const START = [
    { type: 'fact', text: 'Prefers short answers in plain English', when: '2 days ago' },
    { type: 'how-to', text: 'Book meetings after 10am, never on Fridays', when: 'last week' },
  ];
  const INCOMING = [
    { at: 1.6, type: 'event', text: 'Launching the new website in March', app: APPS[1] || APPS[0] },
    { at: 4.2, type: 'fact', text: 'Runs a small coaching business', app: APPS[0] },
  ];

  const el = (tag, cls, text, kids = []) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text) n.textContent = text;
    kids.forEach((k) => k && n.appendChild(k));
    return n;
  };
  const note = (text, extra = '') => el('span', `mono-note ${extra}`.trim(), text);
  const dot = (pulse) => el('span', pulse ? 'chip-dot is-pulse' : 'chip-dot');
  // `is-in` rows slide in only inside a pane that is fading in (global.css),
  // so a rebuild within a tab never replays them.
  const row = (left, right, i) => el('div', `prow is-in d${i}`, null, [left, right]);

  const memoriesPane = (t) => {
    const arrived = INCOMING.filter((e) => e.at <= t).reverse();
    const writing = INCOMING.find((e) => t >= e.at - 0.9 && t < e.at);
    const rows = [
      // Only a memory that has just arrived flashes; the pane is rebuilt when
      // the next one starts saving, and an older arrival must not flash again.
      ...arrived.map((e) => ({ ...e, when: 'just now', fresh: t - e.at < 0.9 })),
      ...START,
    ].slice(0, 3).map((m) => el('div', m.fresh ? 'mrow is-new' : 'mrow', null, [
      el('span', 'mrow-ty', m.type), el('span', 'mrow-tx', m.text), el('span', 'mrow-wh', m.when),
    ]));
    const foot = writing
      ? el('div', 'pane-foot is-live', null, [dot(true), note(`${writing.app} is saving a memory…`, 'is-sage')])
      : el('div', 'pane-foot', null, [note(`${START.length + arrived.length} memories · encrypted on this computer`)]);
    return [el('div', 'pane-title', 'Recently remembered'), ...rows, foot];
  };

  const activeApp = (t) => Math.floor(Math.max(0, t - 7)) % APPS.length;
  const agentsPane = (t) => {
    const acts = ['reading memories', 'saving a memory'];
    const on = activeApp(t);
    return [
      el('div', 'pane-title', 'Your AI apps'),
      ...APPS.map((a, i) => row(
        el('span', 'prow-name', null, [dot(i === on), document.createTextNode(a)]),
        note(i === on ? acts[i % acts.length] : 'connected', i === on ? 'is-sage' : ''), i)),
      el('div', 'prow-add is-in d3', null, [el('span', null, 'Other apps that support MCP'), note('+ Connect')]),
    ];
  };

  const consolidationPane = () => [
    el('div', 'pane-title', 'Consolidation'),
    row(el('span', null, 'Near-duplicates merged'), note('2'), 0),
    row(el('span', null, 'Contradictions resolved'), note('1, the newer fact kept'), 1),
    row(el('span', null, 'Runs on'), note('this computer only'), 2),
    el('div', 'sweep-wrap is-in d3', null, [
      note('Tidying up your memories'),
      el('div', 'sweep-track', null, [el('div', 'sweep-bar')]),
    ]),
  ];

  const settingsPane = () => [
    el('div', 'pane-title', 'Settings'),
    row(el('span', null, 'Your memories are kept'), note('on this computer'), 0),
    row(el('span', null, 'Download my memories'), el('span', 'pill-line', 'Download'), 1),
    el('div', 'erase is-in d2', null, [
      el('span', 'erase-note', 'Destroys the encryption key and the files.'),
      el('span', 'pill-line', 'Delete everything'),
    ]),
  ];

  // Re-render only when what is shown changes, so each entrance animation
  // plays once instead of restarting on every tick.
  let shown = '';
  const render = (t) => {
    const phase = PHASES.find((p) => t >= p[1] && t < p[2]) || PHASES[0];
    const tab = phase[0];
    const writing = INCOMING.find((e) => t >= e.at - 0.9 && t < e.at);
    const arrived = INCOMING.filter((e) => e.at <= t).length;
    // The promise chips under the window glow with the tab that shows them:
    // memories and consolidation stay on this computer, Settings has
    // "Delete everything"; the trial takes the Agents tab.
    const lit = { Memories: 'local', Agents: 'trial', Consolidation: 'local', Settings: 'delete' }[tab];
    const key = [tab, arrived, writing ? writing.at : '', tab === 'Agents' ? activeApp(t) : ''].join('|');

    chips.forEach((c) => c.classList.toggle('is-lit', c.dataset.promise === lit));
    if (key === shown) return;
    const tabChanged = shown.split('|')[0] !== tab;
    shown = key;

    title.textContent = `Zaaheen · ${tab}`;
    tabs.forEach((n) => n.classList.toggle('is-on', n.dataset.tab === tab));
    const body = tab === 'Memories' ? memoriesPane(t)
      : tab === 'Agents' ? agentsPane(t)
      : tab === 'Consolidation' ? consolidationPane()
      : settingsPane();
    // A new tab fades in whole; a change within a tab (a memory arriving, the
    // next app lighting up) keeps the pane and only its own rows animate.
    const cls = ['pane', tab === 'Memories' ? '' : 'is-list', tabChanged ? 'is-fade' : ''].filter(Boolean).join(' ');
    pane.replaceChildren(el('div', cls, null, body));
  };

  let t = 0;
  let last = performance.now();
  let paused = false;
  demo.addEventListener('mouseenter', () => { paused = true; });
  demo.addEventListener('mouseleave', () => { paused = false; });
  render(t);
  setInterval(() => {
    const now = performance.now();
    // A hidden tab stops the timers; never jump seconds ahead when it returns.
    const dt = Math.min(0.25, (now - last) / 1000);
    last = now;
    if (paused || document.hidden) return;
    t = (t + dt) % LOOP;
    render(t);
  }, 80);
}
