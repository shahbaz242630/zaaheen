// Ask Zaaheen's Worker (ADR-117, ADR-SEC-052). The chat page and its files
// are static assets (public/, served by Cloudflare before this code runs);
// only /v1/* reaches the Worker. The daily cron runs the sweep.
import Anthropic from "@anthropic-ai/sdk";
import { handleAsk, type AskEnv } from "./handler";
import { sweep } from "./store";

export default {
  async fetch(request, env): Promise<Response> {
    const { pathname } = new URL(request.url);
    if (pathname !== "/v1/ask") return new Response("Not found", { status: 404 });
    const client = new Anthropic({ apiKey: env.ANTHROPIC_API_KEY, maxRetries: 1 });
    return handleAsk(request, env, {
      model: { create: (params) => client.messages.create(params) },
      fetch: (url, init) => fetch(url, init),
      now: () => Date.now(),
    });
  },

  async scheduled(_controller, env): Promise<void> {
    await sweep(env.DB, Date.now());
  },
} satisfies ExportedHandler<AskEnv>;
