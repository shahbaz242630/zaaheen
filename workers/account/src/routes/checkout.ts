// POST /v1/checkout (SIGNIN-DESIGN.md §5, quoted):
//
//   body { plan: "monthly"|"annual" } mapped to allowlisted price IDs. A
//   live fetch decides "already subscribed" (active or past_due) ->
//   { kind:"portal", url } (portal session). Otherwise find-or-create the
//   customer, store paddle_customer_id and checkout_at, create a
//   transaction with custom_data.clerk_user_id server-side ->
//   { kind:"checkout", txn }.
//
// §8.30: the record is written before the transaction exists, so a
// checkout that could not be recorded is never started (503). Without a
// stored customer, the user's primary email finds or creates one; with no
// email the answer is 422 and nothing is created.

import { BillingError, ourSubscriptionIds } from "../billing";
import { ClerkClient } from "../clerk";
import type { Config } from "../config";
import { bearerToken, errorResponse, jsonResponse } from "../http";
import { PaddleClient } from "../paddle";
import { parseRecord, recordPatch } from "../record";
import { UpstreamError } from "../upstream";
import { type RouteDeps, authenticate, log, readJsonObject } from "./common";

const MAX_BODY_BYTES = 1024;
const SUBSCRIBED = ["active", "past_due"] as const;

export async function handleCheckout(request: Request, config: Config, deps: RouteDeps): Promise<Response> {
  if (request.method !== "POST") return errorResponse(405, "method_not_allowed", { allow: "POST" });
  const token = bearerToken(request);
  if (token === null) return errorResponse(401, "token_refused");
  const body = await readJsonObject(request, MAX_BODY_BYTES);
  const plan = body?.["plan"];
  if (plan !== "monthly" && plan !== "annual") return errorResponse(400, "bad_request");
  const priceId = config.paddle.prices[plan];

  const clerk = new ClerkClient(config.clerk, deps.fetch);
  const paddle = new PaddleClient(config.paddle, deps.fetch);
  try {
    const auth = await authenticate(clerk, token);
    if (auth.kind === "refused") return auth.response;
    const { record, warnings } = parseRecord(auth.user.privateMetadata);
    for (const w of warnings) log("checkout", "record_warning", w);

    // Already subscribed? Decided by a live fetch, never by the record.
    if (record.paddle_customer_id !== undefined) {
      const subscriptions = await paddle.listSubscriptions(record.paddle_customer_id);
      const current = ourSubscriptionIds(subscriptions, config.paddle.productId, SUBSCRIBED);
      if (current.length > 0) {
        const url = await paddle.createPortalSession(record.paddle_customer_id, current);
        return jsonResponse(200, { kind: "portal", url });
      }
    }

    let customerId = record.paddle_customer_id;
    if (customerId === undefined) {
      if (auth.user.primaryEmail === null) return errorResponse(422, "no_email");
      customerId = await paddle.findOrCreateCustomer(auth.user.primaryEmail);
    }

    const patch = recordPatch(record, { ...record, paddle_customer_id: customerId, checkout_at: deps.now() });
    if (patch !== null) await clerk.mergeRecord(auth.sub, patch);

    const txn = await paddle.createTransaction(customerId, priceId, auth.sub);
    return jsonResponse(200, { kind: "checkout", txn });
  } catch (e) {
    const known = e instanceof UpstreamError || e instanceof BillingError;
    log("checkout", known ? "upstream_error" : "unexpected_error", e instanceof Error ? e.message : undefined);
    return errorResponse(503, "unavailable");
  }
}
