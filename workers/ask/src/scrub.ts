// Before an unanswered question is kept (ADR-SEC-052 §3.8), anything shaped
// like an email address, a phone number or a card number is blanked. Kept
// questions carry no identity; this removes the identity people type in.

export const REMOVED = "[removed]";

const EMAIL = /[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g;

// A run of digits joined by spaces, dots, dashes or brackets. Phone and card
// numbers have at least 7 digits; versions (0.3.8), prices and years do not.
const NUMBER_RUN = /\+?\(?\d[\d\s().-]*\d\)?/g;
const MIN_DIGITS = 7;

export function scrub(text: string): string {
  return text
    .replace(EMAIL, REMOVED)
    .replace(NUMBER_RUN, (run) => (run.replace(/\D/g, "").length >= MIN_DIGITS ? REMOVED : run));
}
