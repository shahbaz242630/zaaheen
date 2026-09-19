// The account Worker (SIGNIN-DESIGN.md §5). Routes built so far:
//   POST /v1/lease      the signed entitlement lease
//   POST /v1/checkout   a checkout transaction, or the customer portal
//   POST /paddle/webhook  subscription.* notifications from Paddle
//   POST /clerk/webhook   user.deleted from Clerk (cancels billing)
// and a daily cron, the renewal sweep (wrangler.jsonc `triggers.crons`).
// Everything else is a 404. The two /v1 routes are flood-limited per client
// address (src/flood.ts) before anything else runs. A missing or inconsistent
// configuration is a 503 ("try later") for every route, and any unexpected
// failure is a 503 too: the app keeps its lease on a 5xx and never signs
// anyone out for one.

import { readConfig } from "./config";
import { floodCheck } from "./flood";
import { errorResponse } from "./http";
import { handleCheckout } from "./routes/checkout";
import { handleClerkWebhook } from "./routes/clerk-webhook";
import type { RouteDeps } from "./routes/common";
import { handleLease } from "./routes/lease";
import { handlePaddleWebhook } from "./routes/paddle-webhook";
import { sweepRenewals } from "./sweep";

type Handler = (request: Request, config: NonNullable<ReturnType<typeof readConfig>>, deps: RouteDeps) => Promise<Response>;

const ROUTES: Record<string, Handler> = {
  "/v1/lease": handleLease,
  "/v1/checkout": handleCheckout,
  "/paddle/webhook": handlePaddleWebhook,
  "/clerk/webhook": handleClerkWebhook,
};

export default {
  async fetch(request, env): Promise<Response> {
    const path = new URL(request.url).pathname;
    const handler = ROUTES[path];
    if (handler === undefined) return errorResponse(404, "not_found");
    const flooded = await floodCheck(request, path, (env as Partial<Env>).FLOOD);
    if (flooded !== null) return flooded;
    const config = readConfig(env as unknown as Record<string, unknown>);
    if (config === null) {
      console.warn(JSON.stringify({ event: "config_incomplete" }));
      return errorResponse(503, "unavailable");
    }
    try {
      return await handler(request, config, {
        fetch: (input, init) => fetch(input, init),
        now: () => Math.floor(Date.now() / 1000),
      });
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
