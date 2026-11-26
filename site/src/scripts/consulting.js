// The consulting page's two moving pictures (pages/consulting.astro). The HTML
// already holds the finished state of both, so without script or with reduced
// motion nothing moves and nothing is missing. No inline styles in markup (the
// CSP allows none); positions are set through the DOM here.
//
// 1. The desk: a pile of everyday work sorts itself into the four verdict
//    trays, one slip at a time, then gathers and sorts again. The pile is
//    tidy columns (four wide, two on a phone), so every slip stays readable
//    (founder s129).
//    Pauses on hover, off screen, in a hidden tab, and with the Pause button.
// 2. The example report page: its verdicts fill in row by row the first time
//    it scrolls into view.
const reduced = window.matchMedia('(prefers-reduced-motion: reduce)');
const desk = document.getElementById('cd-desk');

if (desk && !reduced.matches) {
  const pile = desk.querySelector('.cd-pile');
  const btn = desk.querySelector('.cd-pause');
  const lists = {};
  desk.querySelectorAll('[data-tray-list]').forEach((ul) => { lists[ul.dataset.trayList] = ul; });
  const slips = [...desk.querySelectorAll('.cd-slip')];
  // One of each verdict in turn, so the trays fill evenly.
  const ORDER = [0, 2, 4, 6, 1, 3, 5, 7].map((i) => slips[i]).filter(Boolean);
  // A small tilt per pile spot, so it still reads as a pile.
  const TILT = [-2.5, 2, 1.5, -2, -1.5, 2.5, 2, -1];
  const PAD = 10;
  const GAP = 8;
  const EASE = 'cubic-bezier(.22,.8,.24,1)';
  const face = (s) => s.querySelector('.cd-slip-face');

  let queue = [];
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

  // Four columns on a wide desk, two on a phone, filled row by row; the pile
  // grows to fit its tallest rows.
  const pileGroup = (group) => {
    const cols = pile.clientWidth >= 640 ? 4 : 2;
    const colWidth = Math.floor((pile.clientWidth - PAD * 2 - GAP * (cols - 1)) / cols);
    group.forEach((s) => {
      s.classList.add('is-piled');
      s.style.width = `${colWidth}px`;
      pile.appendChild(s);
    });
    let top = PAD;
    for (let i = 0; i < group.length; i += cols) {
      const row = group.slice(i, i + cols);
      const h = Math.max(...row.map((s) => s.offsetHeight));
      row.forEach((s, j) => {
        s.style.left = `${PAD + j * (colWidth + GAP)}px`;
        s.style.top = `${top}px`;
        face(s).style.transform = `rotate(${TILT[(i + j) % TILT.length]}deg)`;
      });
      top += h + GAP;
    }
    pile.style.height = `${top + 18}px`;
  };
  const toTray = (s) => {
    s.classList.remove('is-piled');
    s.style.width = '';
    s.style.left = '';
    s.style.top = '';
    lists[s.dataset.tray].appendChild(s);
    face(s).style.transform = '';
  };

  // FLIP for a group: measure every slip first, move them all, then animate
  // each from where it was, so slips that shift because of another never jump.
  const flip = (group, move, stagger = 0) => {
    const first = group.map((s) => s.getBoundingClientRect());
    move();
    group.forEach((s, i) => {
      const last = s.getBoundingClientRect();
      s.style.transition = 'none';
      s.style.transform = `translate(${first[i].left - last.left}px,${first[i].top - last.top}px)`;
    });
    void desk.offsetWidth;
    group.forEach((s, i) => {
      s.classList.add('is-moving');
      s.style.transition = `transform 760ms ${EASE} ${i * stagger}ms`;
      s.style.transform = '';
      const done = () => { s.classList.remove('is-moving'); s.removeEventListener('transitionend', done); };
      s.addEventListener('transitionend', done);
    });
  };

  const sortNext = () => {
    const s = queue.shift();
    if (!s) { later(gather, 5200); return; }
    flip([s], () => toTray(s));
    const tray = s.closest('.cd-tray');
    if (tray) {
      tray.classList.add('is-hit');
      setTimeout(() => tray.classList.remove('is-hit'), 650);
    }
    later(sortNext, 780);
  };

  const gather = () => {
    flip(ORDER, () => pileGroup(ORDER), 45);
    queue = [...ORDER];
    later(sortNext, 1700);
  };

  // Hold each tray's sorted height so the desk never changes size.
  const start = () => {
    Object.values(lists).forEach((ul) => { ul.style.minHeight = ''; });
    slips.forEach((s) => { s.style.transition = 'none'; s.style.transform = ''; s.classList.remove('is-moving'); toTray(s); });
    desk.classList.add('is-live');
    Object.values(lists).forEach((ul) => { ul.style.minHeight = `${ul.offsetHeight}px`; });
    pileGroup(ORDER);
    queue = [...ORDER];
    later(sortNext, 900);
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

  let width = window.innerWidth;
  let resizeTimer = null;
  window.addEventListener('resize', () => {
    if (window.innerWidth === width) return;
    width = window.innerWidth;
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => { clearTimeout(timer); pending = null; start(); }, 200);
  });

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
