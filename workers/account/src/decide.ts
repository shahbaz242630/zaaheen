// What `/v1/lease` signs (SIGNIN-DESIGN.md §5, quoted):
//
//   state: active if now < max(active_until, comp_until) (payment_failed flag
//   -> state payment_failed); live Paddle re-derive first when (a) the record
//   is active-flavoured but now >= active_until, or (b) paddle_customer_id
//   exists and the result would be ended, or (c) checkout_at is within 24 h
//   and the result is not active. Rate-limited to one live fetch per user per
//   10 min. Cases (a) and (b) are skipped when synced_at > active_until; case
//   (c), the just-paid check, never is; else trial if now < trial_started_at
//   + 30 d; else ended.
//   Paddle or Clerk error for a record that was active-flavoured -> sign
//   active until the stored active_until + 3 days. Any other upstream error
//   -> 503 (never a signed ended).
//   Kill switch: NEVER_END_PAYERS=1 -> anyone with a paddle_customer_id gets
//   active with active_until = now + 3 days.
//
// §4 adds "every 30 s per user while checkout_at is within 1 h"; §8.28 keeps
// both limits in the record's `live_fetch_at`. Amendment 3 (§8.29):
// "active-flavoured" means the record has an `active_until`; "synced since
// it ended" is `synced_at >= active_until` (a derivation that finds nothing
// sets both to the same second); a failed fetch still stamps
// `live_fetch_at`; and a trial whose start could not be saved is never
// signed (Clerk refusing that write is an upstream error for a record that
// was not active-flavoured: 503).

import { deriveBilling } from "./billing";
import type { LeaseState } from "./lease";
import { type BillingRecord, type RecordPatch, applyDerived, recordPatch } from "./record";
import { DAY, HOUR, MINUTE, TRIAL_SECONDS } from "./time";

const UPSTREAM_GRACE = 3 * DAY;
const KILL_SWITCH_GRANT = 3 * DAY;
const JUST_PAID_WINDOW = 24 * HOUR;
const FAST_POLL_WINDOW = HOUR;
const FAST_POLL_SPACING = 30;
const LIVE_FETCH_SPACING = 10 * MINUTE;

/** The state part of a lease; the handler adds v, kid, sub and the times. */
export interface LeaseTerms {
  state: LeaseState;
  trial_ends_at?: number;
  active_until?: number;
}

export interface LeaseDeps {
  now: number;
  killSwitch: boolean;
  productId: string;
  /** All of this customer's Paddle subscriptions; throws on any failure. */
  fetchSubscriptions(customerId: string): Promise<unknown[]>;
  /** Merge `patch` into the user's record in Clerk; throws on any failure. */
  writeRecord(patch: RecordPatch): Promise<void>;
}

export type LeaseDecision = { kind: "lease"; terms: LeaseTerms } | { kind: "unavailable" };

/** The state `record` means at `now`, from the record alone. */
export function termsFrom(record: BillingRecord, now: number): LeaseTerms {
  const trialEnds = record.trial_started_at !== undefined ? record.trial_started_at + TRIAL_SECONDS : undefined;
  const withTrial = (terms: LeaseTerms): LeaseTerms =>
    trialEnds === undefined ? terms : { ...terms, trial_ends_at: trialEnds };

  const paidUntil = maxDefined(record.active_until, record.comp_until);
  if (paidUntil !== undefined && now < paidUntil) {
    return withTrial({ state: record.payment_failed === true ? "payment_failed" : "active", active_until: paidUntil });
  }
  if (trialEnds !== undefined && now < trialEnds) return { state: "trial", trial_ends_at: trialEnds };
  // "For a signed ended, include active_until if the user ever paid" (§8.27).
  const ended: LeaseTerms = { state: "ended" };
  if (record.active_until !== undefined) ended.active_until = record.active_until;
  return withTrial(ended);
}

/** Whether to ask Paddle before answering: cases (a)-(c), within the rate limit. */
export function liveFetchWanted(record: BillingRecord, now: number): boolean {
  const state = termsFrom(record, now).state;
  // With no paid period there is no end to have synced since: keep asking
  // (inside the rate limit), since a payment may have landed with its
  // webhook lost (§8.29).
  const syncedSinceEnd =
    record.synced_at !== undefined && record.active_until !== undefined && record.synced_at >= record.active_until;
  const sinceCheckout = record.checkout_at !== undefined ? now - record.checkout_at : undefined;

  const a = record.active_until !== undefined && now >= record.active_until && !syncedSinceEnd;
  const b = record.paddle_customer_id !== undefined && state === "ended" && !syncedSinceEnd;
  const c = sinceCheckout !== undefined && sinceCheckout < JUST_PAID_WINDOW && state !== "active";
  if (!(a || b || c)) return false;

  const spacing = sinceCheckout !== undefined && sinceCheckout < FAST_POLL_WINDOW ? FAST_POLL_SPACING : LIVE_FETCH_SPACING;
  const last = record.live_fetch_at;
  return last === undefined || last > now || now - last >= spacing;
}

/** The lease signed when Paddle or Clerk failed, or `null` for a 503. */
export function termsOnUpstreamError(record: BillingRecord): LeaseTerms | null {
  if (record.active_until === undefined) return null;
  const until = maxDefined(record.active_until + UPSTREAM_GRACE, record.comp_until) ?? record.active_until;
  const terms: LeaseTerms = { state: "active", active_until: until };
  if (record.trial_started_at !== undefined) terms.trial_ends_at = record.trial_started_at + TRIAL_SECONDS;
  return terms;
}

/** The whole `/v1/lease` decision for one user's stored record. */
export async function decideLease(stored: BillingRecord, deps: LeaseDeps): Promise<LeaseDecision> {
  const { now } = deps;
  // "first desktop call sets trial_started_at" (§5).
  let record: BillingRecord = stored.trial_started_at === undefined ? { ...stored, trial_started_at: now } : stored;
  let terms: LeaseTerms | null;

  if (deps.killSwitch && record.paddle_customer_id !== undefined) {
    terms = { state: "active", active_until: now + KILL_SWITCH_GRANT };
    if (record.trial_started_at !== undefined) terms.trial_ends_at = record.trial_started_at + TRIAL_SECONDS;
  } else if (record.paddle_customer_id !== undefined && liveFetchWanted(record, now)) {
    try {
      const subscriptions = await deps.fetchSubscriptions(record.paddle_customer_id);
      record = applyDerived(record, deriveBilling(subscriptions, deps.productId), now, { live: true });
      terms = termsFrom(record, now);
    } catch {
      record = { ...record, live_fetch_at: now };
      terms = termsOnUpstreamError(stored);
    }
  } else {
    terms = termsFrom(record, now);
  }

  const patch = recordPatch(stored, record);
  if (patch !== null) {
    try {
      await deps.writeRecord(patch);
    } catch {
      terms = termsOnUpstreamError(stored);
    }
  }
  return terms === null ? { kind: "unavailable" } : { kind: "lease", terms };
}

function maxDefined(a: number | undefined, b: number | undefined): number | undefined {
  if (a === undefined) return b;
  if (b === undefined) return a;
  return Math.max(a, b);
}
