// The Ask Zaaheen bubble (components/AskBubble.astro; ADR-SEC-052 §3.5).
// Shows the button, and on the first click creates the frame that holds the
// chat on ask.zaaheen.com. Nothing crosses between the two pages except the
// chat's "close" message, accepted only from that exact origin.

const ASK = 'https://ask.zaaheen.com';

const bubble = document.querySelector('.ask-bubble');
if (bubble) {
  const button = bubble.querySelector('.ask-open');
  const box = bubble.querySelector('.ask-box');
  let frame = null;

  const open = () => {
    if (!frame) {
      frame = document.createElement('iframe');
      const url = new URL('/chat/', ASK);
      url.searchParams.set('from', window.location.origin);
      url.searchParams.set('lang', bubble.dataset.askLang || 'en');
      frame.src = url.href;
      frame.title = bubble.dataset.askTitle || 'Ask Zaaheen';
      // Its own origin, so allow-same-origin keeps it apart from ours; popups
      // only for the links to our pages it opens in a new tab.
      frame.setAttribute('sandbox', 'allow-scripts allow-same-origin allow-forms allow-popups allow-popups-to-escape-sandbox');
      frame.setAttribute('referrerpolicy', 'no-referrer');
      frame.className = 'ask-frame';
      box.appendChild(frame);
    }
    box.hidden = false;
    bubble.classList.add('is-open');
    button.setAttribute('aria-expanded', 'true');
  };

  const close = () => {
    box.hidden = true;
    bubble.classList.remove('is-open');
    button.setAttribute('aria-expanded', 'false');
    button.focus();
  };

  button.addEventListener('click', () => (box.hidden ? open() : close()));
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && !box.hidden) close();
  });
  window.addEventListener('message', (event) => {
    if (event.origin !== ASK || event.source !== frame?.contentWindow) return;
    if (event.data && event.data.type === 'zaaheen-ask:close') close();
  });

  bubble.hidden = false;
}
