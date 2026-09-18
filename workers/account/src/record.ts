// The per-user billing record, kept in Clerk at
// `private_metadata.zaaheen_memory` (SIGNIN-DESIGN.md §5):
//
//   { trial_started_at, paddle_customer_id, active_until, payment_failed,
//     comp_until, checkout_at, synced_at }            (+ live_fetch_at, §8.28)
//
// Written only by the Worker, except `comp_until`, which the founder sets by
// hand in the Clerk dashboard (beta comp, §5). "Writes never clear
// trial_started_at (tested); a PATCH is skipped when nothing changed."
//
// Amendment 3 (§8.29): a malformed field is dropped with a warning naming
// the field (never its value); `comp_until` may be typed as a YYYY-MM-DD
// date, free through the end of that day (UTC); the write back carries only
// the fields that changed, so it never overwrites the founder's comp_until.

import type { Derived } from "./billing";
import { endOfDaySeconds } from "./time";

export const RECORD_KEY = "zaaheen_memory";

export interface BillingRecord {
  trial_started_at?: number;
  paddle_customer_id?: string;
  active_until?: number;
  payment_failed?: boolean;
  comp_until?: number;
  checkout_at?: number;
  synced_at?: number;
  live_fetch_at?: number;
}

/** The fields the Worker writes. `comp_until` is the founder's. */
const WORKER_FIELDS = [
  "trial_started_at",
  "paddle_customer_id",
  "active_until",
  "payment_failed",
  "checkout_at",
  "synced_at",
  "live_fetch_at",
] as const;
const TIMES = ["trial_started_at", "active_until", "checkout_at", "synced_at", "live_fetch_at"] as const;

const PADDLE_CUSTOMER = /^ctm_[a-z0-9]{26}$/;

export type RecordPatch = Partial<Record<(typeof WORKER_FIELDS)[number], number | string | boolean | null>>;

/** Read the record out of a user's whole `private_metadata`. */
export function parseRecord(privateMetadata: unknown): { record: BillingRecord; warnings: string[] } {
  const warnings: string[] = [];
  const record: BillingRecord = {};
  if (!isObject(privateMetadata) || privateMetadata[RECORD_KEY] === undefined) return { record, warnings };
  const raw = privateMetadata[RECORD_KEY];
  if (!isObject(raw)) {
    warnings.push(`${RECORD_KEY} is not an object; treated as empty`);
    return { record, warnings };
  }
  const drop = (field: string) => warnings.push(`${RECORD_KEY}.${field} is malformed; ignored`);

  for (const field of TIMES) {
    const v = raw[field];
    if (v === undefined) continue;
    if (isPositiveTime(v)) record[field] = v;
    else drop(field);
  }
  if (raw["paddle_customer_id"] !== undefined) {
    const v = raw["paddle_customer_id"];
    if (typeof v === "string" && PADDLE_CUSTOMER.test(v)) record.paddle_customer_id = v;
    else drop("paddle_customer_id");
  }
  if (raw["payment_failed"] !== undefined) {
    const v = raw["payment_failed"];
    if (typeof v === "boolean") record.payment_failed = v;
    else drop("payment_failed");
  }
  if (raw["comp_until"] !== undefined) {
    const v = raw["comp_until"];
    if (isPositiveTime(v)) record.comp_until = v;
    else if (typeof v === "string") {
      try {
        record.comp_until = endOfDaySeconds(v);
      } catch {
        drop("comp_until");
      }
    } else drop("comp_until");
  }
  return { record, warnings };
}

/**
 * Fold a fresh derivation from Paddle into the record. When nothing counts
 * any more, an end already past is kept (the app tells a former payer
 * "subscription ended", §8.27) and an end still in the future is cut to
 * `now` (refunded or cancelled at once).
 */
export function applyDerived(
  record: BillingRecord,
  derived: Derived,
  now: number,
  opts: { live: boolean },
): BillingRecord {
  const next: BillingRecord = { ...record, payment_failed: derived.payment_failed, synced_at: now };
  if (derived.active_until !== null) next.active_until = derived.active_until;
  else if (record.active_until !== undefined) next.active_until = Math.min(record.active_until, now);
  if (opts.live) next.live_fetch_at = now;
  return next;
}

/**
 * What to send back to Clerk: the Worker's fields that changed, a removed
 * one as `null`, or `null` when there is nothing to write. An existing
 * trial start is never changed or removed; `comp_until` is never written.
 */
export function recordPatch(before: BillingRecord, after: BillingRecord): RecordPatch | null {
  const patch: RecordPatch = {};
  for (const field of WORKER_FIELDS) {
    if (field === "trial_started_at" && before.trial_started_at !== undefined) continue;
    if (before[field] !== after[field]) patch[field] = after[field] ?? null;
  }
  return Object.keys(patch).length === 0 ? null : patch;
}

function isPositiveTime(v: unknown): v is number {
  return Number.isSafeInteger(v) && (v as number) > 0;
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
