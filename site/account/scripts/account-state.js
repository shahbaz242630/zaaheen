// What the account pages show (AUTH-PAGES-DESIGN D1 + amendment S1-2), kept
// free of the DOM and of Clerk so it can be tested in plain Node
// (scripts/signin-state.test.mjs). account.js wires these to the page.
//
// Views: 'bad-link' | 'signed-in' | 'start' | 'enter-code' | 'more-details'
// | 'callback' | 'finish' | 'error'. Anything not listed here ends at the
// fixed line: the page never guesses what Clerk meant.

export const FIXED_LINE = 'Something went wrong. Open the Zaaheen app and press Sign in again.';
export const BAD_LINK_LINE = "This sign-in link isn't valid. Open the Zaaheen app and press Sign in again.";
export const RESEND_SECONDS = 30;

// The only details the sign-up page can ask for after the email: the names
// (Google may not give them) and the Terms (S1-2: a Google sign-up started
// from the sign-in page has no Terms box). In the order the page shows them.
const DETAILS = ['first_name', 'last_name', 'legal_accepted'];
const ERROR = Object.freeze({ view: 'error', line: FIXED_LINE });

/**
 * On load. `redirect` is readRedirect's state; `signIn` / `signUp` are the
 * in-progress attempts Clerk restored (status and field lists), or null.
 */
export function onLoad({ page, redirect, signedIn, signIn, signUp }) {
  if (redirect === 'invalid') return { view: 'bad-link' };
  if (page === 'sso-callback') return { view: 'callback' };
  if (signedIn) return { view: 'signed-in', canContinue: redirect === 'ok' };
  if (page === 'sign-in' && signIn && signIn.status === 'needs_first_factor') {
    return { view: 'enter-code', resumed: true };
  }
  if (page === 'sign-up' && signUp && signUp.status === 'missing_requirements') {
    return signUpNext(signUp, true);
  }
  return { view: 'start' };
}

/** After a sign-in answer from Clerk (create, or the code). */
export function afterSignIn(status) {
  if (status === 'complete') return { view: 'finish' };
  if (status === 'needs_first_factor') return { view: 'enter-code', resumed: false };
  // needs_second_factor, needs_client_trust, needs_new_password,
  // needs_protect_check and anything new: no MFA enrolment and no passwords
  // here, so these cannot arise; if one does, stop (D1, D5).
  return ERROR;
}

/** After a sign-up answer from Clerk (create, the code, or the details). */
export function afterSignUp(signUp) {
  if (!signUp) return ERROR;
  if (signUp.status === 'complete') return { view: 'finish' };
  if (signUp.status === 'missing_requirements') return signUpNext(signUp, false);
  return ERROR;
}

function signUpNext({ missingFields, unverifiedFields }, resumed) {
  if (!Array.isArray(missingFields) || !Array.isArray(unverifiedFields)) return ERROR;
  const unverified = unverifiedFields.filter((f) => f !== 'email_address');
  if (unverified.length > 0) return ERROR;
  if (unverifiedFields.includes('email_address')) return { view: 'enter-code', resumed };
  if (missingFields.length === 0 || !missingFields.every((f) => DETAILS.includes(f))) return ERROR;
  return { view: 'more-details', fields: DETAILS.filter((f) => missingFields.includes(f)) };
}

/**
 * "Send a new code": allowed once RESEND_SECONDS have passed since the last
 * send (milliseconds), else how many whole seconds are left.
 */
export function resend(lastSentAt, now) {
  if (lastSentAt === null || lastSentAt === undefined) return { allowed: true, wait: 0 };
  const elapsed = now - lastSentAt;
  if (elapsed < 0) return { allowed: false, wait: RESEND_SECONDS };
  const left = RESEND_SECONDS * 1000 - elapsed;
  if (left <= 0) return { allowed: true, wait: 0 };
  return { allowed: false, wait: Math.ceil(left / 1000) };
}

// Our own words for Clerk's error codes (D3: Clerk's `message` is never shown).
const LINES = Object.freeze({
  form_code_incorrect: "That code isn't right. Check the email we sent and try again.",
  verification_expired: 'That code has expired. Send a new one.',
  form_identifier_not_found: "We couldn't find an account with that email. Check it, or create an account.",
  form_identifier_exists: 'There is already an account with that email. Sign in instead.',
  form_param_format_invalid: "That email address doesn't look right.",
  form_param_nil: 'Please fill in every field.',
  form_param_missing: 'Please fill in every field.',
  too_many_requests: 'Too many tries. Please wait a few minutes and try again.',
  user_locked: 'Too many tries. Please wait a few minutes and try again.',
  captcha_invalid: "We couldn't check that you're a person. Refresh the page and try again.",
  captcha_missing_token: "We couldn't check that you're a person. Refresh the page and try again.",
  session_exists: "You're already signed in.",
});

/** The line for a Clerk error code; the fixed line for anything else. */
export function messageFor(code) {
  return typeof code === 'string' && Object.hasOwn(LINES, code) ? LINES[code] : FIXED_LINE;
}
