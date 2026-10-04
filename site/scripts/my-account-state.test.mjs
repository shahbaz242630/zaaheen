// The website account page's decisions (AUTH-PAGES-DESIGN S85-1, ADR-SEC-043),
// in plain Node: which address it calls, what each plan says and offers, the
// cancel answers, and what it will ever open.
import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  ACCOUNT_PATH, CANCEL_LINES, PRODUCTION_API_HOST, accountMarker, apiHostFor, cancelAnswer, planView,
  checkoutTarget, manageTarget, portalTarget,
} from '../account/scripts/my-account-state.js';

test('the account service address comes from the sign-in address in production', () => {
  assert.equal(apiHostFor({ dev: false, fapiHost: 'clerk.zaaheen.com' }), 'api.zaaheen.com');
  assert.equal(PRODUCTION_API_HOST, 'api.zaaheen.com');
  assert.equal(apiHostFor({ dev: false, fapiHost: 'auth.zaaheen.com' }), null);
  assert.equal(apiHostFor(null), null);
});

test('a development build names its API host itself, or has none', () => {
  assert.equal(apiHostFor({ dev: true, fapiHost: 'x.clerk.accounts.dev' }, 'api-sandbox.zaaheen.com'), 'api-sandbox.zaaheen.com');
  assert.equal(apiHostFor({ dev: true, fapiHost: 'x.clerk.accounts.dev' }), null);
  assert.equal(apiHostFor({ dev: true, fapiHost: 'x.clerk.accounts.dev' }, 'https://evil.test/'), null);
});

test('the way back from Google is one fixed marker', () => {
  assert.equal(ACCOUNT_PATH, '/account/');
  assert.equal(accountMarker('?after=account'), true);
  for (const s of ['', '?after=delete', '?after=account&x=1', '?after=accounts', '?redirect_url=https://evil.test']) {
    assert.equal(accountMarker(s), false, s);
  }
});

test('each plan says what the app says and offers the right buttons', () => {
  assert.deepEqual(planView({ state: 'not_started' }), {
    line: 'Your 30-day free trial starts when you first open the Zaaheen app.', plans: true, manage: false, cancel: false,
  });
  assert.deepEqual(planView({ state: 'trial', days_left: 20 }), { line: 'Free trial, 20 days left', plans: true, manage: false, cancel: false });
  assert.equal(planView({ state: 'trial', days_left: 1 }).line, 'Free trial, 1 day left');
  assert.equal(planView({ state: 'trial', days_left: 0 }).line, 'Free trial, less than a day left');
  const until = Date.UTC(2026, 10, 4) / 1000;
  assert.deepEqual(planView({ state: 'active', until, ending: false, paying: true }), { line: 'Subscribed, renews on 4 November 2026', plans: false, manage: true, cancel: true });
  assert.deepEqual(planView({ state: 'active', until, ending: true, paying: true }), { line: 'Subscribed, ends on 4 November 2026', plans: false, manage: true, cancel: false });
  // Time given without a subscription (review s85, finding 4).
  assert.deepEqual(planView({ state: 'active', until, ending: false, paying: false }), { line: 'Your plan runs until 4 November 2026', plans: false, manage: false, cancel: false });
  assert.deepEqual(planView({ state: 'payment_failed', until, ending: false, paying: true }), {
    line: "Your last payment didn't go through. Update your card under Manage subscription.", plans: false, manage: true, cancel: true,
  });
  assert.deepEqual(planView({ state: 'ended' }), { line: 'Your plan has ended. Subscribe to keep using Zaaheen.', plans: true, manage: false, cancel: false });
});

test('an answer of no known shape is never shown as a plan', () => {
  for (const bad of [null, {}, { state: 'free' }, { state: 'trial' }, { state: 'active' }, { state: 'trial', days_left: 'x' }, { state: 'active', until: 1, ending: false }]) {
    assert.equal(planView(bad), null, JSON.stringify(bad));
  }
});

test('the cancel answers, worded as in the app (SIGNIN-DESIGN 8.48)', () => {
  assert.deepEqual(cancelAnswer({ kind: 'none' }), { line: CANCEL_LINES.nothing_to_cancel, open: null });
  assert.deepEqual(cancelAnswer({ kind: 'ending' }), { line: CANCEL_LINES.already_ending, open: null });
  const url = 'https://customer-portal.paddle.com/cpl_1?action=cancel_subscription';
  assert.deepEqual(cancelAnswer({ kind: 'portal', url }), { line: CANCEL_LINES.opened, open: url });
  assert.equal(cancelAnswer({ kind: 'portal', url: 'https://evil.test/' }), null);
  assert.equal(cancelAnswer({ kind: 'checkout', txn: 'txn_01h8xce4qhqk1w0dm2rc4v0dxa' }), null);
  assert.equal(CANCEL_LINES.nothing_to_cancel, "You don't have a paid subscription, so there's nothing to cancel.");
});

test('only a checked pay page or a Paddle portal is ever opened', () => {
  assert.equal(checkoutTarget({ kind: 'checkout', txn: 'txn_01h8xce4qhqk1w0dm2rc4v0dxa' }), 'https://zaaheen.com/pay/?_ptxn=txn_01h8xce4qhqk1w0dm2rc4v0dxa');
  assert.equal(checkoutTarget({ kind: 'portal', url: 'https://customer-portal.paddle.com/cpl_1' }), 'https://customer-portal.paddle.com/cpl_1');
  for (const bad of [
    { kind: 'checkout', txn: 'txn_SHORT' },
    { kind: 'checkout', txn: 'txn_01h8xce4qhqk1w0dm2rc4v0dxa&x=1' },
    { kind: 'portal', url: 'https://paddle.com.evil.test/' },
    { kind: 'portal', url: 'https://paddle.com@evil.test/' },
    { kind: 'portal', url: 'http://customer-portal.paddle.com/' },
    { kind: 'portal', url: 'javascript:alert(1)' },
    { kind: 'none' },
    null,
  ]) {
    assert.equal(checkoutTarget(bad), null, JSON.stringify(bad));
  }
  assert.equal(portalTarget('https://sandbox-customer-portal.paddle.com/x'), 'https://sandbox-customer-portal.paddle.com/x');
  assert.equal(portalTarget('https://notpaddle.com/x'), null);
});

test('Manage opens only the subscription page, never a checkout (review s85, finding 4)', () => {
  assert.equal(manageTarget({ kind: 'portal', url: 'https://customer-portal.paddle.com/cpl_1' }), 'https://customer-portal.paddle.com/cpl_1');
  assert.equal(manageTarget({ kind: 'checkout', txn: 'txn_01h8xce4qhqk1w0dm2rc4v0dxa' }), null);
  assert.equal(manageTarget({ kind: 'none' }), null);
  assert.equal(manageTarget({ kind: 'portal', url: 'https://evil.test/' }), null);
});
