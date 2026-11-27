// The consulting page's two moving pictures (pages/consulting.astro). The HTML
// already holds a finished state of both, so without script or with reduced
// motion nothing moves and nothing is missing. No inline styles in markup (the
// CSP allows none); motion is set through the DOM here.
//
// 1. The desk: one slip of everyday work at a time comes onto the desk, then
//    goes to its verdict tray. Each tray shows only its latest slip and a
//    count, so the desk never fills up, and it loops for ever (founder s129:
//    "keep it in a loop so its never full of clutter"). Pauses on hover, off
//    screen, in a hidden tab, and with the Pause button.
// 2. The example report page: its verdicts fill in row by row the first time
//    it scrolls into view.
const reduced = window.matchMedia('(prefers-reduced-motion: reduce)');
const desk = document.getElementById('cd-desk');

if (desk && !reduced.matches) {
  const slot = desk.querySelector('.cd-inbox-slot');
  const btn = desk.querySelector('.cd-pause');
  const queue = [...desk.querySelectorAll('.cd-queue .cd-slip')];
  const trays = {};
  desk.querySelectorAll('.cd-tray').forEach((t) => {
    trays[t.dataset.tray] = { el: t, list: t.querySelector('.cd-tray-list'), count: t.querySelector('.cd-count'), n: 0 };
  });
  // One of each verdict in turn, so the trays fill evenly.
  const ORDER = [0, 2, 4, 6, 1, 3, 5, 7].map((i) => queue[i]).filter(Boolean);
  const EASE = 'cubic-bezier(.22,.8,.24,1)';
  const SHOW = 1300; // how long a slip sits on the desk before it is sorted
  const GAP = 900; // the pause after it lands, before the next comes in

  let next = 0;
  let timer = null;
  let pending = null;
  let userPaused = false;
  let hover = false;
  let visible = true;
  const paused = () => userPaused || hover || !visible || document.hidden;

  const later = (fn, ms) => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      if (paused()) pending = fn; else fn();
    }, ms);
  };
  const resume = () => {
    if (!paused() && pending) { const f = pending; pending = null; f(); }
  };

  const tasks = (n) => (n === 1 ? '1 task' : `${n} tasks`);
  const resetCounts = () => Object.values(trays).forEach((t) => { t.n = 0; t.count.textContent = ''; });

  // A slip comes onto the desk.
  const bringIn = () => {
    if (next === 0) resetCounts();
    const slip = ORDER[next].cloneNode(true);
    next = (next + 1) % ORDER.length;
    slot.replaceChildren(slip);
    void slip.offsetWidth;
    slip.classList.add('is-in');
    later(() => sort(slip), SHOW);
  };

  // It goes to its tray (FLIP: measure, move, animate from where it was); the
  // tray's old slip fades out underneath.
  const sort = (slip) => {
    const tray = trays[slip.dataset.tray];
    if (!tray) { later(bringIn, GAP); return; }
    const first = slip.getBoundingClientRect();
    const old = [...tray.list.children];
    old.forEach((o) => o.classList.add('is-leaving'));
    slip.classList.remove('is-in');
    tray.list.appendChild(slip);
    const last = slip.getBoundingClientRect();
    slip.style.transition = 'none';
    slip.style.opacity = '1';
    slip.style.transform = `translate(${first.left - last.left}px,${first.top - last.top}px)`;
    void slip.offsetWidth;
    slip.classList.add('is-moving');
    slip.style.transition = `transform 700ms ${EASE}`;
    slip.style.transform = '';
    setTimeout(() => {
      slip.classList.remove('is-moving');
      old.forEach((o) => o.remove());
      tray.n += 1;
      tray.count.textContent = tasks(tray.n);
      tray.el.classList.add('is-hit');
      setTimeout(() => tray.el.classList.remove('is-hit'), 600);
    }, 700);
    later(bringIn, 700 + GAP);
  };

  const start = () => {
    clearTimeout(timer);
    pending = null;
    next = 0;
    desk.classList.add('is-live');
    later(bringIn, 600);
  };

  btn.hidden = false;
  btn.addEventListener('click', () => {
    userPaused = !userPaused;
    btn.setAttribute('aria-pressed', String(userPaused));
    btn.textContent = userPaused ? 'Play' : 'Pause';
    resume();
  });
  desk.addEventListener('mouseenter', () => { hover = true; });
  desk.addEventListener('mouseleave', () => { hover = false; resume(); });
  document.addEventListener('visibilitychange', resume);
  if ('IntersectionObserver' in window) {
    new IntersectionObserver((entries) => {
      visible = entries[0].isIntersecting;
      resume();
    }, { threshold: 0.25 }).observe(desk);
  }

  start();
}

const sheet = document.getElementById('cd-sheet');
if (sheet && !reduced.matches && 'IntersectionObserver' in window) {
  const rows = [...sheet.querySelectorAll('.cd-vrow')];
  sheet.classList.add('cd-anim');
  const io = new IntersectionObserver((entries) => {
    if (!entries[0].isIntersecting) return;
    io.disconnect();
    rows.forEach((row, i) => setTimeout(() => row.classList.add('is-on'), 250 + i * 380));
  }, { threshold: 0.35 });
  io.observe(sheet);
}
