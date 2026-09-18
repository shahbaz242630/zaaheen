// The daily cron (SIGNIN-DESIGN.md §5, quoted):
//
//   Cron (daily, paged): each run handles one page of subscriptions whose
//   billing period ends within +-48 h and stays under 50 subrequests,
//   PATCHing only changes. At ~30 paying customers move to Workers Paid.
//
// It is the safety net for a lost webhook around a renewal. §8.30: it counts
// its own outbound calls (so no input can push it past the free plan's 50),
// applies the webhook's binding rule (only a record that already names the
// subscription's customer is touched), and one user's failure is logged and
// skipped, never fatal to the run.

import { BillingError, deriveBilling } from "./billing";
import { ClerkClient } from "./clerk";
import type { Config } from "./config";
import { PaddleClient } from "./paddle";
import { applyDerived, parseRecord, recordPatch } from "./record";
import type { RouteDeps } from "./routes/common";
import { HOUR, epochSeconds } from "./time";
import { type Fetch, UpstreamError, isObject } from "./upstream";

/** Outbound calls one run may make: under the free plan's 50. */
export const SUBREQUEST_BUDGET = 45;
/** Calls one user can take: read the user, list the customer, write. */
const CALLS_PER_USER = 3;
const WINDOW = 48 * HOUR;
const USER_ID = /^user_[A-Za-z0-9]{1,64}$/;
const CUSTOMER_ID = /^ctm_[a-z0-9]{26}$/;

export interface SweepResult {
  /** Subscriptions in the window with a usable customer and user. */
  candidates: number;
  updated: number;
  unchanged: number;
  /** User gone, or the record does not name this customer. */
  skipped: number;
  failed: number;
  /** Candidates not reached this run because of the budget. */
  left: number;
}

export async function sweepRenewals(config: Config, deps: RouteDeps): Promise<SweepResult> {
  let calls = 0;
  const counted: Fetch = (input, init) => {
    calls += 1;
    return deps.fetch(input, init);
  };
  const clerk = new ClerkClient(config.clerk, counted);
  const paddle = new PaddleClient(config.paddle, counted);
  const now = deps.now();

  const prices = [config.paddle.prices.monthly, config.paddle.prices.annual];
  const page = await paddle.listOurSubscriptionsFirstPage(prices, ["active", "past_due"]);
  const candidates = inWindow(page, now);
  const result: SweepResult = { candidates: candidates.length, updated: 0, unchanged: 0, skipped: 0, failed: 0, left: 0 };

  for (const [index, c] of candidates.entries()) {
    if (calls + CALLS_PER_USER > SUBREQUEST_BUDGET) {
      result.left = candidates.length - index;
      break;
    }
    try {
      const user = await clerk.getUser(c.userId);
      const record = user === null ? null : parseRecord(user.privateMetadata).record;
      if (record === null || record.paddle_customer_id !== c.customerId) {
        result.skipped += 1;
        continue;
      }
      const derived = deriveBilling(await paddle.listSubscriptions(c.customerId), config.paddle.productId);
      const patch = recordPatch(record, applyDerived(record, derived, now, { live: false }));
      if (patch === null) {
        result.unchanged += 1;
        continue;
      }
      await clerk.mergeRecord(c.userId, patch);
      result.updated += 1;
    } catch (e) {
      result.failed += 1;
      const known = e instanceof UpstreamError || e instanceof BillingError;
      console.warn(JSON.stringify({ route: "sweep", event: known ? "upstream_error" : "unexpected_error" }));
    }
  }
  console.warn(JSON.stringify({ route: "sweep", event: "done", ...result, calls }));
  return result;
}

interface Candidate {
  userId: string;
  customerId: string;
}

/** Readable subscriptions whose period ends within the window, one per user. */
function inWindow(page: readonly unknown[], now: number): Candidate[] {
  const seen = new Set<string>();
  const out: Candidate[] = [];
  for (const s of page) {
    if (!isObject(s)) continue;
    const period = s["current_billing_period"];
    const custom = s["custom_data"];
    const userId = isObject(custom) ? custom["clerk_user_id"] : undefined;
    const customerId = s["customer_id"];
    if (!isObject(period) || typeof userId !== "string" || !USER_ID.test(userId)) continue;
    if (typeof customerId !== "string" || !CUSTOMER_ID.test(customerId)) continue;
    let endsAt: number;
    try {
      endsAt = epochSeconds(period["ends_at"]);
    } catch {
      continue;
    }
    if (Math.abs(endsAt - now) > WINDOW || seen.has(userId)) continue;
    seen.add(userId);
    out.push({ userId, customerId });
  }
  return out;
}
