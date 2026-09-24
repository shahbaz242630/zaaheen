// The /pay page: initialise Paddle.js for the transaction in the URL and keep
// the status line honest. Every decision is in pay-plan.js (tested); this file
// only connects it to the page. Paddle.js itself is the classic script loaded
// before this module, the one third-party script the site allows (audit.mjs).
import { CHECKOUT_SETTINGS, MESSAGES, afterEvent, readConfig, readTransaction } from './pay-plan.js';

const root = document.querySelector('[data-paddle-env]');
const status = document.getElementById('pay-status');
const reopen = document.getElementById('pay-reopen');

const say = (key) => {
  if (status) status.textContent = MESSAGES[key];
};

function start() {
  const config = root ? readConfig(root.dataset) : null;
  if (!config) return say('not_configured');
  const transactionId = readTransaction(window.location.search);
  // Without a valid transaction Paddle.js is never initialised, so it cannot
  // open whatever else the URL carries.
  if (!transactionId) return say('no_transaction');
  const paddle = window.Paddle;
  if (!paddle) return say('not_loaded');

  let state = { completed: false };
  // Before Initialize, so a quick checkout.loaded is never overwritten.
  say('opening');
  try {
    if (config.env === 'sandbox') paddle.Environment.set('sandbox');
    paddle.Initialize({
      token: config.token,
      checkout: { settings: CHECKOUT_SETTINGS },
      eventCallback: (event) => {
        const next = afterEvent(state, event && event.name);
        if (!next) return;
        state = { completed: next.completed };
        say(next.message);
        if (reopen) reopen.hidden = !next.reopen;
      },
    });
  } catch {
    return say('failed');
  }

  if (reopen) {
    reopen.addEventListener('click', () => {
      reopen.hidden = true;
      try {
        paddle.Checkout.open({ transactionId, settings: CHECKOUT_SETTINGS });
      } catch {
        say('failed');
      }
    });
  }
}

start();
