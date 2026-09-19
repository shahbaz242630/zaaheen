// Coarse flood protection for the routes where every request costs an
// upstream call (Clerk's token verify, then Paddle). §8.30 recorded the
// residual ("Flooding /v1/lease with bogus tokens costs Clerk verify calls")
// and left the fix to the deploy; §8.28 found the rate-limit binding on the
// free plan (runtime confirmation at the first deploy).
//
// The binding is "permissive, eventually consistent", counted per Cloudflare
// location, and its period is 10 or 60 s (§8.28), so it only stops floods.
// Exact per-user limits live in the record (`live_fetch_at`).
//
// Keyed by the client address: bogus tokens are all different, so a token key
// would never repeat. Cloudflare advises against address keys because many
// users can share one; the limit (wrangler.jsonc) is set far above what a
// household or office of installs sends (about one lease a day each, a poll
// every 10 s for 10 minutes after a checkout).

import { errorResponse } from "./http";

/** The rate-limit binding's shape (Cloudflare's `RateLimit`). */
export interface Limiter {
  limit(options: { key: string }): Promise<{ success: boolean }>;
}

/**
 * Only these routes are limited. The webhooks are not: a refused Paddle or
 * Clerk notification is a payment update or a cancellation delayed, and their
 * signatures are checked locally before any upstream call.
 */
export const FLOOD_LIMITED_ROUTES: ReadonlySet<string> = new Set(["/v1/lease", "/v1/checkout"]);

/** `null` to go on; otherwise the answer to send (429, or 503 without the binding). */
export async function floodCheck(request: Request, path: string, limiter: Limiter | undefined): Promise<Response | null> {
  if (!FLOOD_LIMITED_ROUTES.has(path)) return null;
  if (limiter === undefined) {
    console.warn(JSON.stringify({ event: "config_incomplete", missing: "FLOOD" }));
    return errorResponse(503, "unavailable");
  }
  const key = request.headers.get("cf-connecting-ip") || "unknown";
  try {
    const { success } = await limiter.limit({ key });
    if (success) return null;
  } catch {
    // Flood protection, not a gate: a limiter fault must not refuse users.
    console.warn(JSON.stringify({ event: "flood_limiter_error" }));
    return null;
  }
  // The address is never logged (§8: the Worker keeps what it learns minimal).
  console.warn(JSON.stringify({ event: "rate_limited", route: path }));
  return errorResponse(429, "rate_limited", { "retry-after": "60" });
}
