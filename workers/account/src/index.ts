// The account Worker (SIGNIN-DESIGN.md §5). Routes built so far:
//   POST /v1/lease      the signed entitlement lease
//   POST /v1/checkout   a checkout transaction, or the customer portal
//   POST /v1/cancel     Paddle's cancel step for a paying member (§8.48)
//   POST /v1/web/plan   "Your plan" for the website account page (ADR-SEC-043)
// The website account page may call /v1/web/plan, /v1/checkout and /v1/cancel
// from its one origin (src/cors.ts): preflights are answered before flood
// limiting, from the configuration alone.
//   POST /paddle/webhook  subscription.* notifications from Paddle
//   POST /clerk/webhook   user.deleted from Clerk (cancels billing)
// and a daily cron, the renewal sweep (wrangler.jsonc `triggers.crons`), which
// also cancels billing for a deleted account (ACCOUNT-DELETION-DESIGN D6).
// Everything else is a 404. The /v1 routes are flood-limited per client
// address (src/flood.ts) before anything else runs. A missing or inconsistent
// configuration is a 503 ("try later") for every route, and any unexpected
// failure is a 503 too: the app keeps its lease on a 5xx and never signs
// anyone out for one.

import { readConfig } from "./config";
import { WEB_ROUTES, corsPreflight, withCors } from "./cors";
import { floodCheck } from "./flood";
import { errorResponse } from "./http";
import { handleCancel } from "./routes/cancel";
import { handleCheckout } from "./routes/checkout";
import { handleClerkWebhook } from "./routes/clerk-webhook";
import type { RouteDeps } from "./routes/common";
import { handleLease } from "./routes/lease";
import { handlePaddleWebhook } from "./routes/paddle-webhook";
import { handleWebPlan } from "./routes/web-plan";
import { sweepRenewals } from "./sweep";

type Handler = (request: Request, config: NonNullable<ReturnType<typeof readConfig>>, deps: RouteDeps) => Promise<Response>;

const ROUTES: Record<string, Handler> = {
  "/v1/lease": handleLease,
  "/v1/checkout": handleCheckout,
  "/v1/cancel": handleCancel,
  "/v1/web/plan": handleWebPlan,
  "/paddle/webhook": handlePaddleWebhook,
  "/clerk/webhook": handleClerkWebhook,
};

export default {
  async fetch(request, env): Promise<Response> {
    const path = new URL(request.url).pathname;
    const handler = ROUTES[path];
    if (handler === undefined) return errorResponse(404, "not_found");
    if (request.method === "OPTIONS" && WEB_ROUTES.has(path)) {
      return corsPreflight(request, readConfig(env as unknown as Record<string, unknown>)?.web?.origin);
    }
    const flooded = await floodCheck(request, path, (env as Partial<Env>).FLOOD);
    if (flooded !== null) return flooded;
    const config = readConfig(env as unknown as Record<string, unknown>);
    if (config === null) {
      console.warn(JSON.stringify({ event: "config_incomplete" }));
      return errorResponse(503, "unavailable");
    }
    try {
      const response = await handler(request, config, {
        fetch: (input, init) => fetch(input, init),
        now: () => Math.floor(Date.now() / 1000),
        // Missing, it fails a first lease closed (ACCOUNT-DELETION-DESIGN D8).
        trials: (env as Partial<Env>).TRIALS,
      });
      return WEB_ROUTES.has(path) ? withCors(response, request, config.web?.origin) : response;
    } catch {
      return errorResponse(503, "unavailable");
    }
  },

  async scheduled(_controller, env): Promise<void> {
    const config = readConfig(env as unknown as Record<string, unknown>);
    if (config === null) {
      console.warn(JSON.stringify({ event: "config_incomplete", route: "sweep" }));
      return;
    }
    await sweepRenewals(config, {
      fetch: (input, init) => fetch(input, init),
      now: () => Math.floor(Date.now() / 1000),
    });
  },
} satisfies ExportedHandler<Env>;
