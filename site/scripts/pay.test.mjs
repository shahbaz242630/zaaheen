// Tests for the /pay page's decisions (src/scripts/pay-plan.js): which config
// is accepted, which transaction is opened, which checkout settings are sent,
// and what the buyer is told after each checkout event. The page itself only
// wires these to the DOM and Paddle.js.
//
//   node --test scripts/pay.test.mjs          (from site/; no build needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { CHECKOUT_SETTINGS, MESSAGES, afterEvent, readConfig, readTransaction } from '../src/scripts/pay-plan.js';

const SANDBOX_TOKEN = `test_${'a'.repeat(27)}`;
const LIVE_TOKEN = `live_${'b'.repeat(27)}`;
const TXN = `txn_${'0123456789abcdefghijklmnop'}`;

test('config: a sandbox token with the sandbox environment is accepted', () => {
  assert.deepEqual(readConfig({ paddleEnv: 'sandbox', paddleToken: SANDBOX_TOKEN }), { env: 'sandbox', token: SANDBOX_TOKEN });
});

test('config: a live token with the production environment is accepted', () => {
  assert.deepEqual(readConfig({ paddleEnv: 'production', paddleToken: LIVE_TOKEN }), { env: 'production', token: LIVE_TOKEN });
});

test('config: a token from the other environment is refused', () => {
  // A sandbox page with a live token (or the reverse) is a build mistake:
  // Paddle.js would talk to the wrong environment.
  assert.equal(readConfig({ paddleEnv: 'sandbox', paddleToken: LIVE_TOKEN }), null);
  assert.equal(readConfig({ paddleEnv: 'production', paddleToken: SANDBOX_TOKEN }), null);
});

test('config: missing or malformed values are refused', () => {
  assert.equal(readConfig({}), null);
  assert.equal(readConfig({ paddleEnv: '', paddleToken: '' }), null);
  assert.equal(readConfig({ paddleEnv: 'staging', paddleToken: SANDBOX_TOKEN }), null);
  assert.equal(readConfig({ paddleEnv: 'sandbox', paddleToken: 'test_short' }), null);
  assert.equal(readConfig({ paddleEnv: 'sandbox', paddleToken: `test_${'a'.repeat(28)}` }), null);
  assert.equal(readConfig({ paddleEnv: 'sandbox', paddleToken: `test_${'a'.repeat(26)}!` }), null);
  assert.equal(readConfig({ paddleEnv: 'sandbox', paddleToken: ` ${SANDBOX_TOKEN}` }), null);
});

test('transaction: exactly one well-formed _ptxn is opened', () => {
  assert.equal(readTransaction(`?_ptxn=${TXN}`), TXN);
  // Other parameters are ignored, not refused.
  assert.equal(readTransaction(`?utm_source=x&_ptxn=${TXN}`), TXN);
});

test('transaction: anything else opens nothing', () => {
  assert.equal(readTransaction(''), null);
  assert.equal(readTransaction('?'), null);
  assert.equal(readTransaction('?_ptxn='), null);
  assert.equal(readTransaction(`?_ptxn=${TXN.toUpperCase()}`), null);
  assert.equal(readTransaction(`?_ptxn=${TXN}x`), null);
  assert.equal(readTransaction(`?_ptxn=${TXN.slice(0, -1)}`), null);
  assert.equal(readTransaction(`?_ptxn=${TXN}%0A`), null);
  assert.equal(readTransaction(`?_ptxn=pri_${'0123456789abcdefghijklmnop'}`), null);
  // Only the transaction route: no price IDs, no items from the URL.
  assert.equal(readTransaction(`?price=pri_${'0123456789abcdefghijklmnop'}`), null);
  // Two of them is ambiguous: Paddle.js and this page could pick different ones.
  assert.equal(readTransaction(`?_ptxn=${TXN}&_ptxn=${TXN}`), null);
});

test('checkout settings: the buyer cannot change the customer', () => {
  // /v1/checkout binds the transaction's Paddle customer to the signed-in user
  // (SIGNIN-DESIGN §8.30, binding rule). A buyer who switched email in the
  // checkout would pay as a different customer, and their payment would never
  // reach their account: a payer locked out.
  assert.equal(CHECKOUT_SETTINGS.allowLogout, false);
  assert.equal(CHECKOUT_SETTINGS.displayMode, 'overlay');
  assert.ok(Object.isFrozen(CHECKOUT_SETTINGS));
});

test('events: loading, completing and closing', () => {
  assert.deepEqual(afterEvent({ completed: false }, 'checkout.loaded'), { completed: false, message: 'open', reopen: false });
  assert.deepEqual(afterEvent({ completed: false }, 'checkout.completed'), { completed: true, message: 'completed', reopen: false });
  assert.deepEqual(afterEvent({ completed: false }, 'checkout.closed'), { completed: false, message: 'closed', reopen: true });
});

test('events: closing the success screen keeps the success message', () => {
  const done = afterEvent({ completed: false }, 'checkout.completed');
  assert.deepEqual(afterEvent(done, 'checkout.closed'), { completed: true, message: 'completed', reopen: false });
  // Nothing after completion undoes it.
  assert.deepEqual(afterEvent(done, 'checkout.error'), { completed: true, message: 'completed', reopen: false });
});

test('events: a checkout error offers no reopen', () => {
  // Paddle's own error (an expired or already-paid transaction): reopening the
  // same transaction would fail the same way. The app starts a new one.
  assert.deepEqual(afterEvent({ completed: false }, 'checkout.error'), { completed: false, message: 'error', reopen: false });
});

test('events: everything else changes nothing', () => {
  for (const name of ['checkout.payment.failed', 'checkout.payment.error', 'checkout.warning', 'checkout.customer.updated', 'unknown', undefined]) {
    assert.equal(afterEvent({ completed: false }, name), null, String(name));
  }
});

test('messages: every key used exists, and none uses a dash the site bans', () => {
  for (const key of ['opening', 'open', 'completed', 'closed', 'error', 'not_configured', 'no_transaction', 'not_loaded', 'failed']) {
    assert.equal(typeof MESSAGES[key], 'string', key);
    assert.ok(MESSAGES[key].length > 0, key);
    assert.doesNotMatch(MESSAGES[key], /—|\s–\s/, key);
  }
  assert.match(MESSAGES.no_transaction, /Open the Zaaheen app and choose Subscribe/);
});
