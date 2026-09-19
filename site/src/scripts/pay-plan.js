// The /pay page's decisions, kept free of the DOM and of Paddle.js so they can
// be tested in plain Node (scripts/pay.test.mjs). pay.js wires them up.
//
// /pay is Paddle's "default payment link" (SIGNIN-DESIGN §5, §8.30): the app
// opens https://zaaheen.com/pay?_ptxn=txn_... for a transaction our account
// Worker created, and Paddle's own emails link here to update a card. Paddle.js
// opens the transaction in the URL by itself once initialised.

const ENVIRONMENTS = { sandbox: 'test_', production: 'live_' };
// Paddle: client-side tokens are "test_" or "live_" plus 27 of [a-zA-Z0-9].
const TOKEN = /^(test|live)_[a-zA-Z0-9]{27}$/;
// The same check the app makes before building this page's URL (§5).
const TRANSACTION = /^txn_[a-z0-9]{26}$/;

/**
 * The environment and client-side token the page was built with (data
 * attributes on the page), or null when they are missing, malformed, or from
 * different environments.
 */
export function readConfig(dataset) {
  const env = dataset.paddleEnv;
  const token = dataset.paddleToken;
  if (typeof env !== 'string' || !Object.hasOwn(ENVIRONMENTS, env)) return null;
  if (typeof token !== 'string' || !TOKEN.test(token)) return null;
  if (!token.startsWith(ENVIRONMENTS[env])) return null;
  return { env, token };
}

/** The one well-formed `_ptxn` in the query string, or null. */
export function readTransaction(search) {
  const values = new URLSearchParams(search).getAll('_ptxn');
  if (values.length !== 1 || !TRANSACTION.test(values[0])) return null;
  return values[0];
}

// Paddle.js applies these to the checkout it opens from `_ptxn`.
// allowLogout: false keeps the customer our Worker put on the transaction. If
// the buyer could switch email here, they would pay as another Paddle customer,
// and the binding rule (§8.30) would never let that payment reach their account.
export const CHECKOUT_SETTINGS = Object.freeze({
  allowLogout: false,
  displayMode: 'overlay',
});

/**
 * What the page shows after a Paddle.js event: the next state, the message
 * key, and whether to offer "Open the checkout again". null = no change.
 * Once the checkout has completed, nothing undoes the success message
 * (closing the success screen fires `checkout.closed`).
 */
export function afterEvent(state, name) {
  if (state.completed) {
    return ['checkout.closed', 'checkout.error'].includes(name) ? { completed: true, message: 'completed', reopen: false } : null;
  }
  switch (name) {
    case 'checkout.loaded':
      return { completed: false, message: 'open', reopen: false };
    case 'checkout.completed':
      return { completed: true, message: 'completed', reopen: false };
    case 'checkout.closed':
      return { completed: false, message: 'closed', reopen: true };
    case 'checkout.error':
      return { completed: false, message: 'error', reopen: false };
    default:
      return null;
  }
}

export const MESSAGES = Object.freeze({
  opening: 'Opening your secure checkout…',
  open: 'Your secure checkout is open.',
  completed: 'All done, thank you. You can close this page and go back to the Zaaheen app.',
  closed: 'The checkout is closed. You can open it again, or come back later from the Zaaheen app.',
  error:
    'This checkout can no longer be opened. It may have expired or already been paid. ' +
    'Open the Zaaheen app and choose Subscribe to start again.',
  not_configured: 'Checkout is not available right now. Please try again later.',
  no_transaction: 'This page opens the checkout you start from the Zaaheen app. Open the Zaaheen app and choose Subscribe.',
  not_loaded: 'The secure checkout did not load. Check your internet connection and refresh this page.',
  failed: 'Something went wrong opening the checkout. Refresh this page to try again.',
});
