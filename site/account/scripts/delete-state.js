// What the delete page shows (ACCOUNT-DELETION-DESIGN D3, D4), kept free of
// the DOM and of Clerk so it can be tested in plain Node
// (scripts/delete-state.test.mjs). account.js wires these to the page.
//
// The security control is Clerk's own: it refuses to delete a user whose
// session's first factor is older than the reverification window (measured in
// the spike: 403 session_reverification_required). These views only make that
// refusal pleasant: one in-place code, one retry, never a loop.

export const DELETE_PATH = '/delete-account/';
// Where Clerk sends a Google address with no Zaaheen account (D4: with
// `transferable: false` it goes to the sign-in URL instead of creating one).
export const NO_ACCOUNT_PATH = '/delete-account/?google=none';
export const DELETE_FAILED_LINE = "We couldn't delete your account. Please try again, or email customerservice@zaaheen.com.";
export const NO_REVERIFY_LINE = "We can't confirm it's you on this page, so we can't delete this account here. Please email customerservice@zaaheen.com and we'll delete it for you.";
export const NO_ACCOUNT_LINE ='There is no Zaaheen account for this Google address.';

const REVERIFY = 'session_reverification_required';

// Exactly one parameter, with exactly this value: anything else is not ours.
function exactly(search, key, value) {
  if (typeof search !== 'string') return false;
  const params = [...new URLSearchParams(search)];
  return params.length === 1 && params[0][0] === key && params[0][1] === value;
}

/** `/sso-callback/?after=delete`: Google's return from the delete page (D4). */
export const deleteMarker = (search) => exactly(search, 'after', 'delete');

/** `/delete-account/?google=none`: Clerk found no account for that Google address. */
export const noGoogleAccount = (search) => exactly(search, 'google', 'none');

/**
 * On load. `redirect` is readRedirect's state: the delete page takes no way
 * back, so any redirect_url at all is a bad link. A session goes straight to
 * confirm (Clerk decides at the delete whether it is fresh enough).
 */
export function onDeleteLoad({ redirect, signedIn, signIn = null, noAccount = false }) {
  if (redirect !== 'none') return { view: 'bad-link' };
  if (signedIn) return { view: 'confirm' };
  if (signIn && signIn.status === 'needs_first_factor') return { view: 'enter-code', resumed: true };
  return { view: 'start', note: noAccount ? NO_ACCOUNT_LINE : null };
}

/** DELETE, typed (spaces around it ignored). A guard against slips, not a security control. */
export function confirmed(typed) {
  return typeof typed === 'string' && typed.trim() === 'DELETE';
}

/** After `user.delete()`: null code is success; one re-verify; else the fixed line. */
export function afterDelete(code, reverified) {
  if (code === null) return { view: 'deleted' };
  if (code === REVERIFY && !reverified) return { view: 'reverify' };
  return { view: 'error', line: DELETE_FAILED_LINE };
}

/** The email address id to send the re-verify code to, or null. */
export function reverifyFactor(verification) {
  const factors = verification && Array.isArray(verification.supportedFirstFactors) ? verification.supportedFirstFactors : [];
  const f = factors.find((x) => x && x.strategy === 'email_code' && typeof x.emailAddressId === 'string');
  return f ? f.emailAddressId : null;
}

/**
 * What to do when Clerk asks for a fresh sign-in: send an email code, or, for
 * an account with no email-code factor (Google only), a line that does not
 * promise a retry that cannot work (session 67 review, m2).
 */
export function reverifyPlan(verification) {
  const emailAddressId = reverifyFactor(verification);
  return emailAddressId ? { view: 'send-code', emailAddressId } : { view: 'error', line: NO_REVERIFY_LINE };
}

/** After the re-verify code: complete means try the delete once more. */
export function afterReverify(status) {
  return status === 'complete' ? { view: 'retry' } : { view: 'error', line: DELETE_FAILED_LINE };
}
