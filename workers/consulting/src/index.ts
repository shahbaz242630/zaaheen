// The enquiry form's Worker (ADR-121, ADR-SEC-057). The form page and its
// files are static assets (public/, served before this code runs); only
// /v1/* reaches the Worker. The daily cron runs the sweep.
import { handleEnquiry, type EnquiryEnv } from "./handler";
import { sweep } from "./store";

export default {
  async fetch(request, env): Promise<Response> {
    const { pathname } = new URL(request.url);
    if (pathname !== "/v1/enquiry") return new Response("Not found", { status: 404 });
    return handleEnquiry(request, env, { fetch: (url, init) => fetch(url, init), now: () => Date.now() });
  },

  async scheduled(_controller, env): Promise<void> {
    await sweep(env.DB, Date.now());
  },
} satisfies ExportedHandler<EnquiryEnv>;
