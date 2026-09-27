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
//
// ACCOUNT-DELETION-DESIGN D6 (quoted): "the daily sweep, for each
// subscription it reads (those renewing within +-48 h) whose
// custom_data.clerk_user_id Clerk answers with 404, cancels it immediately,
// so a webhook that failed past Svix's retries is caught before any charge.
// Logged with the existing wording pattern (a count, never an ID or email)."
// Only a 404 cancels: any other Clerk failure is a failure, and cancels
// nothing. As with the webhook, a forged tag only cancels the forger's own
// subscription. A gone user's cancels come out of the same 3 calls per user
// (one read, then at most 2 cancels); a third renewing subscription of the
// same deleted account waits for the next run, still inside the window.
// Before the first cancel the key must prove it is our instance's (its OAuth
// application list names our client id); otherwise nothing is cancelled.

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
/** Calls one user can take: read the user, list the customer, write (or read, cancel, cancel). */
const CALLS_PER_USER = 3;
/** Cancels for one deleted user in one run: what is left of CALLS_PER_USER after the read. */
const CANCELS_PER_USER = CALLS_PER_USER - 1;
/**
 * Gone accounts one run may cancel for (session 67). A Clerk key from the wrong
 * instance makes every user answer 404; real deletions arrive one or two a
 * day. More than this in one run cancels none of them and logs `gone_guard`.
 */
export const MAX_GONE_PER_RUN = 3;
/**
 * The one call that proves the key is our instance's before any cancel
 * (session 68, review M1): at launch volume the count guard never trips.
 * Always reserved, so the check fits whatever the page holds.
 */
const INSTANCE_CHECK_CALLS = 1;
const WINDOW = 48 * HOUR;
const USER_ID = /^user_[A-Za-z0-9]{1,64}$/;
const CUSTOMER_ID = /^ctm_[a-z0-9]{26}$/;
const SUBSCRIPTION_ID = /^sub_[a-z0-9]{26}$/;

export interface SweepResult {
  /** Subscriptions in the window with a usable customer and user. */
  candidates: number;
  updated: number;
  unchanged: number;
  /** The record does not name this customer. */
  skipped: number;
  /** Subscriptions cancelled because their Clerk user is gone (D6). */
  cancelled: number;
  /** Gone accounts not cancelled for: more than MAX_GONE_PER_RUN, or the key failed the instance check. */
  held: number;
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
  const result: SweepResult = { candidates: candidates.length, updated: 0, unchanged: 0, skipped: 0, cancelled: 0, held: 0, failed: 0, left: 0 };
  // Gone accounts are collected first and cancelled after the loop, so the
  // guard can see how many there are before any cancel is sent.
  const gone: Candidate[] = [];
  // Their cancels still to send, kept in the budget check below.
  let pendingCancels = 0;

  for (const [index, c] of candidates.entries()) {
    if (calls + pendingCancels + INSTANCE_CHECK_CALLS + CALLS_PER_USER > SUBREQUEST_BUDGET) {
      result.left = candidates.length - index;
      break;
    }
    try {
      const user = await clerk.getUser(c.userId);
      if (user === null) {
        // D6: the account is gone (404), so its billing goes too (below).
        gone.push(c);
        pendingCancels += Math.min(c.subscriptionIds.length, CANCELS_PER_USER);
        continue;
      }
      const record = parseRecord(user.privateMetadata).record;
      if (record.paddle_customer_id !== c.customerId) {
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
  if (gone.length > 0 && (gone.length > MAX_GONE_PER_RUN || !(await confirmsInstance(clerk)))) {
    result.held = gone.length;
    console.warn(JSON.stringify({ route: "sweep", event: "gone_guard", held: gone.length }));
  } else {
    // Each gone user's cancels were reserved in pendingCancels above.
    for (const c of gone) {
      if (c.subscriptionIds.length === 0) {
        // Nothing this run can cancel for a gone account: say so.
        result.failed += 1;
        continue;
      }
      try {
        for (const id of c.subscriptionIds.slice(0, CANCELS_PER_USER)) {
          await paddle.cancelSubscription(id);
          result.cancelled += 1;
        }
      } catch (e) {
        result.failed += 1;
        const known = e instanceof UpstreamError || e instanceof BillingError;
        console.warn(JSON.stringify({ route: "sweep", event: known ? "upstream_error" : "unexpected_error" }));
      }
    }
  }
  console.warn(JSON.stringify({ route: "sweep", event: "done", ...result, calls }));
  return result;
}

/** The key's instance lists our OAuth application; any failure is a no. */
async function confirmsInstance(clerk: ClerkClient): Promise<boolean> {
  try {
    return await clerk.instanceHasOurApp();
  } catch {
    return false;
  }
}

interface Candidate {
  userId: string;
  customerId: string;
  /** This user's subscriptions in the window, to cancel if the user is gone (D6). */
  subscriptionIds: string[];
}

/** Readable subscriptions whose period ends within the window, one per user. */
function inWindow(page: readonly unknown[], now: number): Candidate[] {
  const byUser = new Map<string, Candidate>();
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
    if (Math.abs(endsAt - now) > WINDOW) continue;
    const id = s["id"];
    const candidate = byUser.get(userId) ?? { userId, customerId, subscriptionIds: [] };
    if (typeof id === "string" && SUBSCRIPTION_ID.test(id)) candidate.subscriptionIds.push(id);
    byUser.set(userId, candidate);
  }
  return [...byUser.values()];
}
