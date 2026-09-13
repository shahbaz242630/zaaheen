// Download tabs. No framework, no tracking, no third-party scripts.
//
// Progressive enhancement only: every link and every word is already in the
// HTML, because crawlers and AI page readers do not run scripts. This file only
// switches which platform panel is showing.
const tabs = document.querySelectorAll('.tab');
const panels = document.querySelectorAll('.panel');

function show(os) {
  tabs.forEach((t) => t.setAttribute('aria-selected', String(t.dataset.os === os)));
  panels.forEach((p) => p.classList.toggle('hidden', p.dataset.os !== os));
}

tabs.forEach((t) => t.addEventListener('click', () => show(t.dataset.os)));

// Preselect the visitor's platform. A Mac visitor should land on the Mac tab
// and see "not available yet", not on Windows, concluding it is Windows-only.
const ua = navigator.userAgent;
const guess = /Macintosh|Mac OS X/i.test(ua) ? 'mac'
  : /Linux|X11/i.test(ua) && !/Android/i.test(ua) ? 'linux'
  : 'win';
show(guess);
