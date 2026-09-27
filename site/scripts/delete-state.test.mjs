// The delete page's decisions (ACCOUNT-DELETION-DESIGN D3, D4), in plain Node.
//
//   node --test scripts/delete-state.test.mjs     (from site/; no build needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  DELETE_PATH, NO_ACCOUNT_PATH, DELETE_FAILED_LINE, NO_ACCOUNT_LINE,
  deleteMarker, noGoogleAccount, onDeleteLoad, confirmed, afterDelete, reverifyFactor, reverifyPlan, afterReverify, NO_REVERIFY_LINE,
} from '../account/scripts/delete-state.js';

test('the two fixed addresses', () => {
  assert.equal(DELETE_PATH, '/delete-account/');
  assert.equal(NO_ACCOUNT_PATH, '/delete-account/?google=none');
});

test("Google's return carries only the fixed marker, nothing else", () => {
  assert.equal(deleteMarker('?after=delete'), true);
  for (const bad of ['', '?', '?after=Delete', '?after=delete&x=1', '?after=delete&after=delete', '?redirect_url=%2Fdelete-account%2F',
    '?after=delete&redirect_url=https%3A%2F%2Fevil.example%2F', '?after=deletes', '?After=delete', null, undefined]) {
    assert.equal(deleteMarker(bad), false, String(bad));
  }
});

test('the "no account for this Google address" note is one exact flag', () => {
  assert.equal(noGoogleAccount('?google=none'), true);
  for (const bad of ['', '?google=None', '?google=none&x=1', '?google=', '?x=none']) assert.equal(noGoogleAccount(bad), false, bad);
});

test('on load: a redirect of any kind is refused; a session goes to confirm; else sign in', () => {
  assert.deepEqual(onDeleteLoad({ redirect: 'ok', signedIn: true }), { view: 'bad-link' });
  assert.deepEqual(onDeleteLoad({ redirect: 'invalid', signedIn: false }), { view: 'bad-link' });
  assert.deepEqual(onDeleteLoad({ redirect: 'none', signedIn: true }), { view: 'confirm' });
  assert.deepEqual(onDeleteLoad({ redirect: 'none', signedIn: false }), { view: 'start', note: null });
  assert.deepEqual(onDeleteLoad({ redirect: 'none', signedIn: false, noAccount: true }), { view: 'start', note: NO_ACCOUNT_LINE });
  // A sign-in that was waiting for its code when the page reloaded.
  assert.deepEqual(onDeleteLoad({ redirect: 'none', signedIn: false, signIn: { status: 'needs_first_factor' } }), { view: 'enter-code', resumed: true });
  // Signed in wins over a stale note: a fresh session never shows "no account".
  assert.deepEqual(onDeleteLoad({ redirect: 'none', signedIn: true, noAccount: true }), { view: 'confirm' });
});

test('the button needs DELETE typed exactly', () => {
  assert.equal(confirmed('DELETE'), true);
  assert.equal(confirmed('  DELETE  '), true);
  for (const bad of ['delete', 'Delete', 'DELET', 'DELETE!', 'DEL ETE', '', null, undefined]) assert.equal(confirmed(bad), false, String(bad));
});

test('after the delete call: done; one in-place re-verify; never a loop', () => {
  assert.deepEqual(afterDelete(null, false), { view: 'deleted' });
  assert.deepEqual(afterDelete('session_reverification_required', false), { view: 'reverify' });
  // A second refusal after re-verifying ends at the fixed line, not another code.
  assert.deepEqual(afterDelete('session_reverification_required', true), { view: 'error', line: DELETE_FAILED_LINE });
  for (const code of ['user_delete_self_not_enabled', 'too_many_requests', 'something_new', undefined]) {
    assert.deepEqual(afterDelete(code, false), { view: 'error', line: DELETE_FAILED_LINE }, String(code));
  }
});

test('re-verify uses the email code factor Clerk offers, or nothing', () => {
  assert.equal(reverifyFactor({ supportedFirstFactors: [{ strategy: 'email_code', emailAddressId: 'idn_test' }] }), 'idn_test');
  assert.equal(reverifyFactor({ supportedFirstFactors: [{ strategy: 'oauth_google' }, { strategy: 'email_code', emailAddressId: 'idn_test' }] }), 'idn_test');
  assert.equal(reverifyFactor({ supportedFirstFactors: [{ strategy: 'oauth_google' }] }), null);
  assert.equal(reverifyFactor({ supportedFirstFactors: [{ strategy: 'email_code' }] }), null);
  assert.equal(reverifyFactor({}), null);
  assert.equal(reverifyFactor(null), null);
});

test('after the code: complete retries the delete once; anything else stops', () => {
  assert.deepEqual(afterReverify('complete'), { view: 'retry' });
  for (const s of ['needs_first_factor', 'needs_second_factor', undefined]) {
    assert.deepEqual(afterReverify(s), { view: 'error', line: DELETE_FAILED_LINE }, String(s));
  }
});

test('the fixed lines are our words, with the support address', () => {
  assert.equal(DELETE_FAILED_LINE, "We couldn't delete your account. Please try again, or email customerservice@zaaheen.com.");
  assert.equal(NO_ACCOUNT_LINE, 'There is no Zaaheen account for this Google address.');
});

// Session 67's review, m2: an account with no email-code factor (Google only)
// cannot re-verify here, so "Please try again" would be a false promise.
test('re-verify: the email code factor, or a distinct line that promises no retry', () => {
  assert.deepEqual(reverifyPlan({ supportedFirstFactors: [{ strategy: 'email_code', emailAddressId: 'idn_test' }] }), { view: 'send-code', emailAddressId: 'idn_test' });
  for (const v of [{ supportedFirstFactors: [{ strategy: 'oauth_google' }] }, {}, null]) {
    assert.deepEqual(reverifyPlan(v), { view: 'error', line: NO_REVERIFY_LINE });
  }
  assert.notEqual(NO_REVERIFY_LINE, DELETE_FAILED_LINE);
  assert.doesNotMatch(NO_REVERIFY_LINE, /try again/i);
  assert.match(NO_REVERIFY_LINE, /customerservice@zaaheen\.com/);
});
