// The Knowledge Centre's booking card: a 10.5-second loop through choosing a
// session, picking a slot and the booked summary. Illustrative only: the
// sessions and prices are read from the HTML the page was built with
// (data/coaching.ts), and the slots are examples, never real availability.
// No inline styles (the CSP allows none); hovering pauses it; with reduced
// motion it shows the booked frame and stops.
const demo = document.getElementById('booking-demo');

if (demo) {
  const pane = document.getElementById('bk-pane');
  const stepLabel = document.getElementById('bk-step');
  const dots = [...document.querySelectorAll('#bk-dots span')];
  const SESSIONS = [...pane.querySelectorAll('.bk-row')].map((r) => [
    r.children[0].textContent, r.children[1].textContent,
  ]);
  const PICK = 2; // the third session, as in the design
  const SLOTS = ['Mon 19:00', 'Tue 20:30', 'Wed 19:00', 'Thu 21:00', 'Sat 11:00', 'Sat 14:00'];
  const SLOT_PICK = 2;
  const LOOP = 10.5;

  const el = (tag, cls, text, kids = []) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text) n.textContent = text;
    kids.forEach((k) => k && n.appendChild(k));
    return n;
  };
  const note = (text, extra = '') => el('span', `mono-note ${extra}`.trim(), text);

  const choose = (t) => {
    const on = t > 1.6 ? PICK : Math.min(PICK, Math.floor(t / 0.55));
    return [
      el('div', 'bk-title', 'Choose a session'),
      ...SESSIONS.map(([name, price], i) => el('div',
        ['bk-row', 'is-in', `d${i}`, i === on ? 'is-on' : '', i === on && t > 1.6 ? 'is-picked' : ''].filter(Boolean).join(' '),
        null, [el('span', null, name), note(price, i === on ? 'is-sage' : '')])),
    ];
  };

  const slot = (t) => {
    const s = t - 3.5;
    const picked = s > 1.4 ? SLOT_PICK : -1;
    return [
      el('div', 'bk-title', 'Pick an evening slot'),
      el('div', 'bk-sub', null, [note(`${SESSIONS[PICK][0]} · 90 minutes`)]),
      el('div', 'bk-slots', null, SLOTS.map((x, i) => el('div',
        ['bk-slot', 'is-in', `d${i}`, i === picked ? 'is-picked' : ''].filter(Boolean).join(' '), x))),
      el('div', 'bk-sub', null, [note('Times shown in Gulf Standard Time (UTC+4)')]),
      s > 2.2 ? el('div', 'bk-live', null, [el('span', 'chip-dot is-pulse'), note('Securing payment…', 'is-sage')]) : null,
    ];
  };

  const done = () => {
    const row = (k, v, i) => el('div', `prow is-in d${i + 1}`, null, [el('span', null, k), note(v)]);
    return [
      el('div', 'bk-done', null, [el('span', 'bk-check', '✓'), el('div', 'bk-title', 'Your session is booked')]),
      row('Session', SESSIONS[PICK][0], 0),
      row('When', `${SLOTS[SLOT_PICK]} GST`, 1),
      row('Where', 'Microsoft Teams', 2),
      row('Length', '90 minutes', 3),
      el('div', 'bk-sub', null, [note('Confirmed once payment is verified.', 'is-sage')]),
    ];
  };

  let shown = '';
  const render = (t) => {
    const step = t < 3.5 ? 0 : t < 7 ? 1 : 2;
    // Re-render only when what is shown changes, so entrance animations run once.
    const key = step === 0 ? `0|${t > 1.6 ? 'p' : Math.min(PICK, Math.floor(t / 0.55))}`
      : step === 1 ? `1|${t - 3.5 > 1.4}|${t - 3.5 > 2.2}` : '2';
    if (key === shown) return;
    const stepChanged = shown.split('|')[0] !== String(step);
    shown = key;
    stepLabel.textContent = `Step ${step + 1} of 3`;
    dots.forEach((d, i) => d.classList.toggle('is-on', i === step));
    const body = step === 0 ? choose(t) : step === 1 ? slot(t) : done();
    pane.replaceChildren(el('div', stepChanged ? 'pane is-list is-fade' : 'pane is-list', null, body));
  };

  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
    render(8);
  } else {
    let t = 0;
    let last = performance.now();
    let paused = false;
    demo.addEventListener('mouseenter', () => { paused = true; });
    demo.addEventListener('mouseleave', () => { paused = false; });
    shown = '0|0'; // the HTML already shows the first frame
    setInterval(() => {
      const now = performance.now();
      const dt = Math.min(0.25, (now - last) / 1000);
      last = now;
      if (paused || document.hidden) return;
      t = (t + dt) % LOOP;
      render(t);
    }, 80);
  }
}
