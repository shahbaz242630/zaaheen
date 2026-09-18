// POST /clerk/webhook (SIGNIN-DESIGN.md §5, quoted):
//
//   (Svix-verified): user.deleted -> page through subscriptions with status
//   active/past_due/paused and our price IDs, match
//   custom_data.clerk_user_id, cancel immediately. Failure -> 5xx (Svix
//   retries). Runbook backup.
//
// §1: the user.deleted payload is only {id, object, deleted}, and Clerk has
// already deleted the user (and our record with it), so matching on the
// subscriptions' custom_data is the only link left. A buyer can set that
// field on their own checkout, so the worst a forged tag does is cancel
// the forger's own subscription when someone else deletes their account.

import type { Config } from "../config";
import { errorResponse, jsonResponse, readCapped } from "../http";
import { PaddleClient } from "../paddle";
import { verifySvixSignature } from "../signatures";
import { UpstreamError, isObject } from "../upstream";
import { type RouteDeps, log } from "./common";

const MAX_BODY_BYTES = 64 * 1024;
const USER_ID = /^user_[A-Za-z0-9]{1,64}$/;
const LIVE_STATUSES = ["active", "past_due", "paused"] as const;

export async function handleClerkWebhook(request: Request, config: Config, deps: RouteDeps): Promise<Response> {
  if (request.method !== "POST") return errorResponse(405, "method_not_allowed", { allow: "POST" });
  const raw = await readCapped(request, MAX_BODY_BYTES);
  if (raw === null) return errorResponse(413, "too_large");
  const headers = {
    id: request.headers.get("svix-id"),
    timestamp: request.headers.get("svix-timestamp"),
    signature: request.headers.get("svix-signature"),
  };
  if (!(await verifySvixSignature(headers, raw, config.clerk.webhookSecret, deps.now()))) {
    log("clerk_webhook", "bad_signature");
    return errorResponse(401, "bad_signature");
  }

  let event: unknown;
  try {
    event = JSON.parse(raw);
  } catch {
    return errorResponse(400, "bad_request");
  }
  if (!isObject(event) || event["type"] !== "user.deleted") return done("ignored_event_type");
  const data = event["data"];
  const userId = isObject(data) ? data["id"] : undefined;
  if (typeof userId !== "string" || !USER_ID.test(userId)) return done("unmapped_user");

  const paddle = new PaddleClient(config.paddle, deps.fetch);
  try {
    const prices = [config.paddle.prices.monthly, config.paddle.prices.annual];
    const theirs = (await paddle.listOurSubscriptions(prices, LIVE_STATUSES)).filter(
      (s) => isObject(s) && isObject(s["custom_data"]) && s["custom_data"]["clerk_user_id"] === userId,
    );
    for (const s of theirs) {
      const id = isObject(s) ? s["id"] : undefined;
      await paddle.cancelSubscription(typeof id === "string" ? id : "");
    }
    log("clerk_webhook", "user_deleted", `cancelled ${theirs.length}`);
    return jsonResponse(200, { ok: true });
  } catch (e) {
    log("clerk_webhook", e instanceof UpstreamError ? "upstream_error" : "unexpected_error", e instanceof Error ? e.message : undefined);
    return errorResponse(503, "unavailable");
  }
}

function done(kind: string): Response {
  log("clerk_webhook", kind);
  return jsonResponse(200, { ok: true });
}
