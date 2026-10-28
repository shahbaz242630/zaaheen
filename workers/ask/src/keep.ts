// How long an unanswered question is kept (ADR-SEC-052 §3.8, founder s116).

export const KEEP_DAYS = 30;

const DAY_MS = 24 * 60 * 60 * 1000;

/** True once a question stored at `storedAt` must be deleted at `now` (ms). */
export function isExpired(storedAt: number, now: number): boolean {
  // A time in the future is a clock or data error: delete rather than keep forever.
  return storedAt > now || now - storedAt > KEEP_DAYS * DAY_MS;
}
