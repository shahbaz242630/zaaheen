// The Documents page: highlight the section (left) and the topic (right) the
// reader is on. Only adds and removes classes from global.css; every link
// works without it, and without scripts nothing is highlighted.
const sections = [...document.querySelectorAll('.docs-section')];
const rows = [...document.querySelectorAll('.docs-body .set-row')];
const left = [...document.querySelectorAll('.docs-sections a')];
const groups = [...document.querySelectorAll('.docs-toc-group')];
const topics = [...document.querySelectorAll('.docs-toc a[data-topic]')];

// The last element whose top has passed a line just below the page top.
const current = (els) => {
  let found = els[0];
  for (const el of els) {
    if (el.getBoundingClientRect().top <= 140) found = el;
  }
  return found;
};

let queued = false;
const update = () => {
  queued = false;
  const section = current(sections);
  const row = current(rows);
  const id = section ? section.id : '';
  left.forEach((a) => a.classList.toggle('is-on', a.dataset.section === id));
  groups.forEach((g) => g.classList.toggle('is-on', g.dataset.section === id));
  topics.forEach((a) => a.classList.toggle('is-on', row !== undefined && a.dataset.topic === row.id));
};

if (sections.length) {
  document.documentElement.classList.add('docs-js');
  window.addEventListener('scroll', () => {
    if (!queued) {
      queued = true;
      requestAnimationFrame(update);
    }
  }, { passive: true });
  update();
}
