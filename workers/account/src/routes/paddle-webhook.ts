// POST /paddle/webhook (SIGNIN-DESIGN.md §5, quoted):
//
//   subscribed to subscription.* only. Verify Paddle-Signature (any h1, 5 s
//   tolerance), derive synchronously (re-fetch that customer's
//   subscriptions), write, then 200. Any failure -> 5xx so Paddle retries.
//
// §8.30: the user comes from the subscription's custom_data.clerk_user_id,
// which a buyer's own browser can set (Paddle.js customData), so a record is
// only ever updated when it already names this Paddle customer: that link
// is made only by our authenticated /v1/checkout. Everything else that is
// genuine but changes nothing answers 200, so Paddle stops retrying.
//
// Audit s104 PDL-1 widens the subscription: adjustment.created and
// adjustment.updated too, for chargebacks (handleAdjustment below).

import { BillingError, deriveBilling, hasOurProduct } from "../billing";
import { ClerkClient } from "../clerk";
import type { Config } from "../config";
import { errorResponse, jsonResponse, readCapped } from "../http";
import { PaddleClient } from "../paddle";
import { applyDerived, parseRecord, recordPatch } from "../record";
import { verifyPaddleSignature } from "../signatures";
import { UpstreamError, isObject } from "../upstream";
import { type RouteDeps, log, errorDetail } from "./common";

const MAX_BODY_BYTES = 256 * 1024;
const USER_ID = /^user_[A-Za-z0-9]{1,64}$/;
const CUSTOMER_ID = /^ctm_[a-z0-9]{26}$/;

export async function handlePaddleWebhook(request: Request, config: Config, deps: RouteDeps): Promise<Response> {
  if (request.method !== "POST") return errorResponse(405, "method_not_allowed", { allow: "POST" });
  const raw = await readCapped(request, MAX_BODY_BYTES);
  if (raw === null) return errorResponse(413, "too_large");
  const now = deps.now();
  if (!(await verifyPaddleSignature(request.headers.get("paddle-signature"), raw, config.paddle.webhookSecret, now))) {
    log("paddle_webhook", "bad_signature");
    return errorResponse(401, "bad_signature");
  }

  let event: unknown;
  try {
    event = JSON.parse(raw);
  } catch {
    return errorResponse(400, "bad_request");
  }
  if (!isObject(event) || typeof event["event_type"] !== "string" || !isObject(event["data"])) {
    return errorResponse(400, "bad_request");
  }
  if (event["event_type"] === "adjustment.created" || event["event_type"] === "adjustment.updated") {
    return handleAdjustment(event["data"], config, deps);
  }
  if (!event["event_type"].startsWith("subscription.")) return done("ignored_event_type");
  const data = event["data"];
  const customerId = data["customer_id"];
  const custom = data["custom_data"];
  const userId = isObject(custom) ? custom["clerk_user_id"] : undefined;
  if (typeof customerId !== "string" || !CUSTOMER_ID.test(customerId) || typeof userId !== "string" || !USER_ID.test(userId)) {
    return done("unmapped_subscription");
  }

  const clerk = new ClerkClient(config.clerk, deps.fetch);
  const paddle = new PaddleClient(config.paddle, deps.fetch);
  try {
    const user = await clerk.getUser(userId);
    if (user === null) return done("user_gone");
    const { record, warnings } = parseRecord(user.privateMetadata);
    for (const w of warnings) log("paddle_webhook", "record_warning", w);
    if (record.paddle_customer_id !== customerId) return done("customer_not_bound");

    const derived = deriveBilling(await paddle.listSubscriptions(customerId, userId), config.paddle.productId);
    const patch = recordPatch(record, applyDerived(record, derived, now, { live: false }));
    if (patch !== null) await clerk.mergeRecord(userId, patch);
    return jsonResponse(200, { ok: true });
  } catch (e) {
    const known = e instanceof UpstreamError || e instanceof BillingError;
    log("paddle_webhook", known ? "upstream_error" : "unexpected_error", errorDetail(e));
    return errorResponse(503, "unavailable");
  }
}

/*
  A chargeback ends the subscription (audit s104 PDL-1). Paddle records a
  dispute the bank decided, or an early dispute alert it refunded, as an
  adjustment on the transaction, naming the subscription; it does not say it
  cancels the subscription, which then renews and invites a second
  chargeback. So it is cancelled here, at once. Paddle's own
  subscription.canceled then reaches the branch below and access ends, by the
  one path that already derives access.

  Refunds are not acted on: a refund made in the dashboard is a person's
  choice, made where the subscription can be cancelled too, and is often
  goodwill. A won dispute (chargeback_reverse) changes nothing: the
  subscription stays cancelled and the customer can subscribe again.

  The adjustment is Paddle's, signed, and names only the subscription; the
  subscription is read again and cancelled only if it is ours and still live.
*/
const CHARGEBACK_ACTIONS = new Set(["chargeback", "chargeback_warning"]);
const SUBSCRIPTION_ID = /^sub_[a-z0-9]{26}$/;

async function handleAdjustment(data: Record<string, unknown>, config: Config, deps: RouteDeps): Promise<Response> {
  if (typeof data["action"] !== "string" || !CHARGEBACK_ACTIONS.has(data["action"])) return done("ignored_adjustment");
  if (data["status"] === "rejected" || data["status"] === "reversed") return done("ignored_adjustment");
  const subscriptionId = data["subscription_id"];
  if (typeof subscriptionId !== "string" || !SUBSCRIPTION_ID.test(subscriptionId)) {
    return done("chargeback_without_subscription");
  }
  const paddle = new PaddleClient(config.paddle, deps.fetch);
  try {
    const subscription = await paddle.getSubscription(subscriptionId);
    // Unreadable items are Paddle's answer gone wrong, not "not ours": 503, so Paddle sends it again (review s104).
    if (subscription !== null && !Array.isArray(subscription["items"])) throw new BillingError("a charged-back subscription has no readable items");
    if (subscription === null || !hasOurProduct(subscription, config.paddle.productId)) {
      return done("chargeback_not_ours");
    }
    if (subscription["status"] === "canceled") return done("chargeback_already_canceled");
    await paddle.cancelSubscription(subscriptionId);
    // Money a bank took back: the subscription is cancelled, and access ends with it.
    log("paddle_webhook", "chargeback_canceled", String(data["action"]));
    return jsonResponse(200, { ok: true });
  } catch (e) {
    const known = e instanceof UpstreamError || e instanceof BillingError;
    log("paddle_webhook", known ? "upstream_error" : "unexpected_error", errorDetail(e));
    return errorResponse(503, "unavailable");
  }
}

/** A genuine event that changes nothing: 200, logged by kind. */
function done(kind: string): Response {
  log("paddle_webhook", kind);
  return jsonResponse(200, { ok: true });
}
