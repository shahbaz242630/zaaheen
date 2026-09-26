// Tests for the account pages' states (account/scripts/account-state.js,
// AUTH-PAGES-DESIGN D1 + amendment S1-2): what the sign-in, sign-up and Google
// return pages show on load and after each Clerk answer, the resend cooldown,
// and the fixed error lines. The pages only wire these to the DOM and Clerk.
//
//   node --test scripts/signin-state.test.mjs      (from site/; no build needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  FIXED_LINE, BAD_LINK_LINE, RESEND_SECONDS, onLoad, afterSignIn, afterSignUp, resend, messageFor,
} from '../account/scripts/account-state.js';

const OK = 'ok';
const load = (over) => onLoad({ page: 'sign-in', redirect: OK, signedIn: false, signIn: null, signUp: null, ...over });

// --- On load -----------------------------------------------------------------

test('a bad link is refused before anything else, even when signed in', () => {
  for (const page of ['sign-in', 'sign-up', 'sso-callback']) {
    assert.deepEqual(load({ page, redirect: 'invalid' }), { view: 'bad-link' });
    assert.deepEqual(load({ page, redirect: 'invalid', signedIn: true }), { view: 'bad-link' });
  }
});

test('already signed in on load: Continue with a valid link, "You are signed in" without one', () => {
  assert.deepEqual(load({ signedIn: true }), { view: 'signed-in', canContinue: true });
  assert.deepEqual(load({ page: 'sign-up', signedIn: true }), { view: 'signed-in', canContinue: true });
  assert.deepEqual(load({ signedIn: true, redirect: 'none' }), { view: 'signed-in', canContinue: false });
});

test('a fresh visit starts at the form', () => {
  assert.deepEqual(load({}), { view: 'start' });
  assert.deepEqual(load({ page: 'sign-up' }), { view: 'start' });
  assert.deepEqual(load({ redirect: 'none' }), { view: 'start' });
  assert.deepEqual(load({ signIn: { status: null } }), { view: 'start' });
});

test('the Google return page hands over to Clerk, nothing else', () => {
  assert.deepEqual(load({ page: 'sso-callback' }), { view: 'callback' });
  assert.deepEqual(load({ page: 'sso-callback', redirect: 'none' }), { view: 'callback' });
});

test('resume after a reload: a sign-in waiting for its code', () => {
  assert.deepEqual(load({ signIn: { status: 'needs_first_factor' } }), { view: 'enter-code', resumed: true });
});

test('resume after a reload: a sign-up waiting for its email code', () => {
  const signUp = { status: 'missing_requirements', missingFields: [], unverifiedFields: ['email_address'] };
  assert.deepEqual(load({ page: 'sign-up', signUp }), { view: 'enter-code', resumed: true });
});

test('S1-2: Google sign-up missing the Terms (and maybe names) asks for exactly those', () => {
  const signUp = (missingFields) => ({ status: 'missing_requirements', missingFields, unverifiedFields: [] });
  assert.deepEqual(load({ page: 'sign-up', signUp: signUp(['legal_accepted']) }), { view: 'more-details', fields: ['legal_accepted'] });
  assert.deepEqual(load({ page: 'sign-up', signUp: signUp(['first_name', 'last_name']) }), { view: 'more-details', fields: ['first_name', 'last_name'] });
  assert.deepEqual(
    load({ page: 'sign-up', signUp: signUp(['last_name', 'legal_accepted', 'first_name']) }),
    { view: 'more-details', fields: ['first_name', 'last_name', 'legal_accepted'] },
  );
});

test('a sign-up missing anything we do not ask for gets the fixed line', () => {
  const signUp = { status: 'missing_requirements', missingFields: ['phone_number'], unverifiedFields: [] };
  assert.deepEqual(load({ page: 'sign-up', signUp }), { view: 'error', line: FIXED_LINE });
  const mixed = { status: 'missing_requirements', missingFields: ['legal_accepted', 'username'], unverifiedFields: [] };
  assert.deepEqual(load({ page: 'sign-up', signUp: mixed }), { view: 'error', line: FIXED_LINE });
  const unverifiedPhone = { status: 'missing_requirements', missingFields: [], unverifiedFields: ['phone_number'] };
  assert.deepEqual(load({ page: 'sign-up', signUp: unverifiedPhone }), { view: 'error', line: FIXED_LINE });
  // Even when what is missing is something we do ask for.
  const phoneAndTerms = { status: 'missing_requirements', missingFields: ['legal_accepted'], unverifiedFields: ['phone_number'] };
  assert.deepEqual(load({ page: 'sign-up', signUp: phoneAndTerms }), { view: 'error', line: FIXED_LINE });
});

test('an in-progress attempt of the other kind does not hijack the page', () => {
  // A half-finished sign-up is not shown on the sign-in page, and the reverse.
  assert.deepEqual(load({ signUp: { status: 'missing_requirements', missingFields: ['legal_accepted'], unverifiedFields: [] } }), { view: 'start' });
  assert.deepEqual(load({ page: 'sign-up', signIn: { status: 'needs_first_factor' } }), { view: 'start' });
});

// --- After a Clerk answer ------------------------------------------------------

test('sign-in: complete finishes, a code is asked for, everything else is the fixed line', () => {
  assert.deepEqual(afterSignIn('complete'), { view: 'finish' });
  assert.deepEqual(afterSignIn('needs_first_factor'), { view: 'enter-code', resumed: false });
  // No MFA enrolment, no passwords: these cannot arise, and if they do we stop (D1).
  for (const status of ['needs_second_factor', 'needs_client_trust', 'needs_new_password', 'needs_identifier', 'needs_protect_check', '', undefined, 'surprise']) {
    assert.deepEqual(afterSignIn(status), { view: 'error', line: FIXED_LINE }, `status ${status}`);
  }
});

test('sign-up: complete finishes; code, then details, else the fixed line', () => {
  assert.deepEqual(afterSignUp({ status: 'complete', missingFields: [], unverifiedFields: [] }), { view: 'finish' });
  assert.deepEqual(
    afterSignUp({ status: 'missing_requirements', missingFields: [], unverifiedFields: ['email_address'] }),
    { view: 'enter-code', resumed: false },
  );
  assert.deepEqual(
    afterSignUp({ status: 'missing_requirements', missingFields: ['legal_accepted'], unverifiedFields: [] }),
    { view: 'more-details', fields: ['legal_accepted'] },
  );
  for (const status of ['abandoned', 'needs_protect_check', '', undefined, 'surprise']) {
    assert.deepEqual(afterSignUp({ status, missingFields: [], unverifiedFields: [] }), { view: 'error', line: FIXED_LINE }, `status ${status}`);
  }
  // Missing lists that are not lists are not trusted.
  assert.deepEqual(afterSignUp({ status: 'missing_requirements' }), { view: 'error', line: FIXED_LINE });
  assert.deepEqual(afterSignUp(null), { view: 'error', line: FIXED_LINE });
});

test('an unverified email wins over missing details: the code comes first', () => {
  assert.deepEqual(
    afterSignUp({ status: 'missing_requirements', missingFields: ['legal_accepted'], unverifiedFields: ['email_address'] }),
    { view: 'enter-code', resumed: false },
  );
});

// --- Resend cooldown (D1) ------------------------------------------------------

test('resend: 30 seconds after the last send, counting down in whole seconds', () => {
  assert.equal(RESEND_SECONDS, 30);
  const t = 1_000_000;
  assert.deepEqual(resend(t, t), { allowed: false, wait: 30 });
  assert.deepEqual(resend(t, t + 100), { allowed: false, wait: 30 });
  assert.deepEqual(resend(t, t + 29_001), { allowed: false, wait: 1 });
  assert.deepEqual(resend(t, t + 30_000), { allowed: true, wait: 0 });
  assert.deepEqual(resend(t, t + 90_000), { allowed: true, wait: 0 });
  // Never sent: allowed. A clock that went backwards: wait the full time.
  assert.deepEqual(resend(null, t), { allowed: true, wait: 0 });
  assert.deepEqual(resend(t, t - 5_000), { allowed: false, wait: 30 });
});

// --- Error lines (D3: Clerk's own message is never shown) -----------------------

test('known Clerk error codes get our own words', () => {
  const codes = [
    'form_code_incorrect', 'verification_expired', 'form_identifier_not_found', 'form_identifier_exists',
    'form_param_format_invalid', 'form_param_nil', 'form_param_missing', 'too_many_requests', 'user_locked',
    'captcha_invalid', 'captcha_missing_token', 'session_exists',
  ];
  for (const code of codes) {
    const line = messageFor(code);
    assert.equal(typeof line, 'string');
    assert.notEqual(line, FIXED_LINE, `${code} should have its own line`);
  }
});

test('anything unknown, empty or odd gets the fixed line', () => {
  for (const code of ['', undefined, null, 'surprise', 'constructor', '__proto__', 'toString', 42, {}]) {
    assert.equal(messageFor(code), FIXED_LINE, `code ${String(code)}`);
  }
});

test('the words: plain, no long dashes, no provider or vendor names', () => {
  const lines = [FIXED_LINE, BAD_LINK_LINE, ...[
    'form_code_incorrect', 'verification_expired', 'form_identifier_not_found', 'form_identifier_exists',
    'form_param_format_invalid', 'form_param_nil', 'too_many_requests', 'captcha_invalid', 'session_exists',
  ].map(messageFor)];
  for (const line of lines) {
    assert.doesNotMatch(line, /[–—]/, `long dash in: ${line}`);
    assert.doesNotMatch(line, /clerk|cloudflare|turnstile|captcha|oauth|api/i, `vendor or tech word in: ${line}`);
    assert.match(line, /^[A-Z].*[.]$/, `a sentence: ${line}`);
  }
  assert.equal(BAD_LINK_LINE, "This sign-in link isn't valid. Open the Zaaheen app and press Sign in again.");
  assert.equal(FIXED_LINE, 'Something went wrong. Open the Zaaheen app and press Sign in again.');
});
