// POST /v1/web/plan (AUTH-PAGES-DESIGN S85-1, ADR-SEC-043): "Your plan" on
// the website account page.
//
//   Website session tokens only. READ-ONLY: it never writes the record and
//   never starts a trial (the app's first lease does, ACCOUNT-DELETION-DESIGN
//   D8). With a Paddle customer the plan is derived live, exactly as the lease
//   derives it (deriveBilling + applyDerived on a copy + termsFrom); without
//   one, from the record. Paddle down is a 503, never a made-up plan.
//
//   200 {state:"not_started"}                 never opened the app, never paid
//       {state:"trial", days_left}            a part day counts as a day (§8.44)
//       {state:"active"|"payment_failed", until, ending, paying}
//       {state:"ended"}
//   `paying`: Paddle has a live subscription of ours (active or past_due), so
//   Manage and Cancel have something to open; false for time given without
//   one (comp_until). Review s85, finding 4.

import { BillingError, deriveBilling, ourCancellable } from "../billing";
import { ClerkClient } from "../clerk";
import type { Config } from "../config";
import { termsFrom } from "../decide";
import { bearerToken, errorResponse, jsonResponse } from "../http";
import { PaddleClient } from "../paddle";
import { applyDerived, parseRecord } from "../record";
import { DAY } from "../time";
import { UpstreamError } from "../upstream";
import { type RouteDeps, authenticateSession, log } from "./common";

const SUBSCRIBED = ["active", "past_due"] as const;

export async function handleWebPlan(request: Request, config: Config, deps: RouteDeps): Promise<Response> {
  if (request.method !== "POST") return errorResponse(405, "method_not_allowed", { allow: "POST" });
  if (config.web === undefined) return errorResponse(503, "unavailable");
  const token = bearerToken(request);
  if (token === null) return errorResponse(401, "token_refused");

  const clerk = new ClerkClient(config.clerk, deps.fetch);
  const paddle = new PaddleClient(config.paddle, deps.fetch);
  try {
    const now = deps.now();
    const auth = await authenticateSession(clerk, token, config, now);
    if (auth.kind === "refused") return auth.response;
    const { record: stored, warnings } = parseRecord(auth.user.privateMetadata);
    for (const w of warnings) log("web_plan", "record_warning", w);

    let record = stored;
    let ending = false;
    let paying = false;
    if (stored.paddle_customer_id !== undefined) {
      const subscriptions = await paddle.listSubscriptions(stored.paddle_customer_id);
      // A copy for this answer only: nothing is written back.
      record = applyDerived(stored, deriveBilling(subscriptions, config.paddle.productId), now, { live: false });
      const mine = ourCancellable(subscriptions, config.paddle.productId, SUBSCRIBED);
      ending = mine.ending.length > 0 && mine.renewing.length === 0;
      paying = mine.ending.length + mine.renewing.length > 0;
    }

    const terms = termsFrom(record, now);
    if (terms.state === "trial" && terms.trial_ends_at !== undefined) {
      return jsonResponse(200, { state: "trial", days_left: Math.max(0, Math.ceil((terms.trial_ends_at - now) / DAY)) });
    }
    if ((terms.state === "active" || terms.state === "payment_failed") && terms.active_until !== undefined) {
      return jsonResponse(200, { state: terms.state, until: terms.active_until, ending, paying });
    }
    if (record.trial_started_at === undefined && record.active_until === undefined) {
      return jsonResponse(200, { state: "not_started" });
    }
    return jsonResponse(200, { state: "ended" });
  } catch (e) {
    const known = e instanceof UpstreamError || e instanceof BillingError;
    log("web_plan", known ? "upstream_error" : "unexpected_error", e instanceof Error ? e.message : undefined);
    return errorResponse(503, "unavailable");
  }
}
