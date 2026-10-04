// POST /v1/cancel (SIGNIN-DESIGN.md §8.48, ADR-SEC-042):
//
//   Bearer, same authentication as /v1/checkout; the body is ignored. It
//   never creates anything: no customer, no record write, no transaction, so
//   a cancel click can never start a payment. A live Paddle fetch decides:
//   no stored customer, or none of ours active/past_due -> {kind:"none"};
//   every one of those already scheduled to cancel -> {kind:"ending"};
//   otherwise the portal session's cancel_subscription deep link for the
//   first one still renewing -> {kind:"portal", url}. Paddle's page then
//   shows the cancellation survey and cancels at the end of the paid period.

import { BillingError, ourCancellable } from "../billing";
import { ClerkClient } from "../clerk";
import type { Config } from "../config";
import { bearerToken, errorResponse, jsonResponse } from "../http";
import { PaddleClient } from "../paddle";
import { parseRecord } from "../record";
import { UpstreamError } from "../upstream";
import { type RouteDeps, authenticate, log } from "./common";

const SUBSCRIBED = ["active", "past_due"] as const;

export async function handleCancel(request: Request, config: Config, deps: RouteDeps): Promise<Response> {
  if (request.method !== "POST") return errorResponse(405, "method_not_allowed", { allow: "POST" });
  const token = bearerToken(request);
  if (token === null) return errorResponse(401, "token_refused");

  const clerk = new ClerkClient(config.clerk, deps.fetch);
  const paddle = new PaddleClient(config.paddle, deps.fetch);
  try {
    const auth = await authenticate(clerk, token);
    if (auth.kind === "refused") return auth.response;
    const { record, warnings } = parseRecord(auth.user.privateMetadata);
    for (const w of warnings) log("cancel", "record_warning", w);

    if (record.paddle_customer_id === undefined) return jsonResponse(200, { kind: "none" });
    const subscriptions = await paddle.listSubscriptions(record.paddle_customer_id);
    const { ending, renewing } = ourCancellable(subscriptions, config.paddle.productId, SUBSCRIBED);
    const first = renewing[0];
    if (first === undefined) return jsonResponse(200, { kind: ending.length > 0 ? "ending" : "none" });

    const url = await paddle.createCancelLink(record.paddle_customer_id, first);
    return jsonResponse(200, { kind: "portal", url });
  } catch (e) {
    const known = e instanceof UpstreamError || e instanceof BillingError;
    log("cancel", known ? "upstream_error" : "unexpected_error", e instanceof Error ? e.message : undefined);
    return errorResponse(503, "unavailable");
  }
}
