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

import { BillingError, deriveBilling } from "../billing";
import { ClerkClient } from "../clerk";
import type { Config } from "../config";
import { errorResponse, jsonResponse, readCapped } from "../http";
import { PaddleClient } from "../paddle";
import { applyDerived, parseRecord, recordPatch } from "../record";
import { verifyPaddleSignature } from "../signatures";
import { UpstreamError, isObject } from "../upstream";
import { type RouteDeps, log } from "./common";

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

    const derived = deriveBilling(await paddle.listSubscriptions(customerId), config.paddle.productId);
    const patch = recordPatch(record, applyDerived(record, derived, now, { live: false }));
    if (patch !== null) await clerk.mergeRecord(userId, patch);
    return jsonResponse(200, { ok: true });
  } catch (e) {
    const known = e instanceof UpstreamError || e instanceof BillingError;
    log("paddle_webhook", known ? "upstream_error" : "unexpected_error", e instanceof Error ? e.message : undefined);
    return errorResponse(503, "unavailable");
  }
}

/** A genuine event that changes nothing: 200, logged by kind. */
function done(kind: string): Response {
  log("paddle_webhook", kind);
  return jsonResponse(200, { ok: true });
}
