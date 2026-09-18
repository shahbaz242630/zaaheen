// Deriving what a customer has paid for from their Paddle subscriptions
// (SIGNIN-DESIGN.md §5, "Derived billing record", quoted):
//
//   Only subscriptions whose items belong to our allowlisted Paddle product
//   ID count (checked on the fetched items, so a future price change cannot
//   lock new subscribers out).
//   status=active, no scheduled cancel -> active_until =
//     current_billing_period.ends_at + 3 days (renewal gap).
//   status=active with a scheduled cancel -> active_until = scheduled change
//     effective_at.
//   status=past_due -> active_until = current_billing_period.starts_at +
//     7 days, payment_failed = true.
//   paused/canceled -> nothing. The latest active_until over all counting
//   subscriptions wins.
//
// Amendment 3 (§8.29): a scheduled *pause* ends access like a cancel;
// `trialing` and unknown statuses count for nothing; a tie goes to the
// subscription that is paying. Unreadable data on a subscription of ours is
// a BillingError (an upstream error), never a silent "nothing": dropping a
// payer's subscription would sign them "ended".

import { DAY, epochSeconds } from "./time";

const RENEWAL_GAP = 3 * DAY;
const PAST_DUE_GRACE = 7 * DAY;

export interface Derived {
  active_until: number | null;
  payment_failed: boolean;
}

export class BillingError extends Error {
  override name = "BillingError";
}

export function deriveBilling(subscriptions: readonly unknown[], productId: string): Derived {
  let best: Derived = { active_until: null, payment_failed: false };
  for (const sub of subscriptions) {
    const one = deriveOne(sub, productId);
    if (one.active_until === null) continue;
    const current = best.active_until;
    const later = current === null || one.active_until > current;
    const tieButPaying = one.active_until === current && !one.payment_failed;
    if (later || tieButPaying) best = one;
  }
  return best;
}

const SUBSCRIPTION_ID = /^sub_[a-z0-9]{26}$/;

/**
 * Ids of our subscriptions whose status is one of `statuses` (for
 * `/v1/checkout`'s "already subscribed" check, §5). One of ours without a
 * readable id is a BillingError, like any unreadable data of ours.
 */
export function ourSubscriptionIds(subscriptions: readonly unknown[], productId: string, statuses: readonly string[]): string[] {
  const ids: string[] = [];
  for (const sub of subscriptions) {
    if (!isOurs(sub, productId)) continue;
    if (typeof sub["status"] !== "string" || !statuses.includes(sub["status"])) continue;
    const id = sub["id"];
    if (typeof id !== "string" || !SUBSCRIPTION_ID.test(id)) throw new BillingError("a subscription of ours has no readable id");
    ids.push(id);
  }
  return ids;
}

function isOurs(sub: unknown, productId: string): sub is Record<string, unknown> {
  if (!isObject(sub)) throw new BillingError("a subscription is not an object");
  const items = sub["items"];
  if (!Array.isArray(items)) throw new BillingError("a subscription's items are not a list");
  return items.some((item) => isObject(item) && isObject(item["price"]) && item["price"]["product_id"] === productId);
}

function deriveOne(sub: unknown, productId: string): Derived {
  if (!isOurs(sub, productId)) return { active_until: null, payment_failed: false };

  try {
    switch (sub["status"]) {
      case "active": {
        const change = sub["scheduled_change"];
        if (isObject(change) && (change["action"] === "cancel" || change["action"] === "pause")) {
          return { active_until: epochSeconds(change["effective_at"]), payment_failed: false };
        }
        return { active_until: epochSeconds(period(sub)["ends_at"]) + RENEWAL_GAP, payment_failed: false };
      }
      case "past_due":
        return { active_until: epochSeconds(period(sub)["starts_at"]) + PAST_DUE_GRACE, payment_failed: true };
      default:
        return { active_until: null, payment_failed: false };
    }
  } catch (e) {
    if (e instanceof BillingError) throw e;
    throw new BillingError("a subscription of ours has an unreadable date");
  }
}

function period(sub: Record<string, unknown>): Record<string, unknown> {
  const p = sub["current_billing_period"];
  if (!isObject(p)) throw new BillingError("a subscription of ours has no billing period");
  return p;
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
