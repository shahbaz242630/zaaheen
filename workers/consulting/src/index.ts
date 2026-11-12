// The enquiry form's Worker (ADR-121, ADR-SEC-057). The form page and its
// files are static assets (public/, served before this code runs); only
// /v1/* reaches the Worker. The daily cron sends the nightly alert, then
// runs the sweep.
import type { Fetcher } from "./gate";
import { handleEnquiry, type EnquiryEnv } from "./handler";
import { NIGHTLY_SUBJECT, nightlyAlertText, sendMail } from "./mail";
import { DAILY_STORED, alertCounts, sweep } from "./store";

/**
 * The nightly job. One alert email, counts only, when enquiries in the last
 * 24 hours were not emailed or yesterday hit the form's limit; then the
 * sweep. Each part logs a reason-only line if it fails, so a broken sweep
 * (enquiries kept past 12 months) is visible in the Worker's log.
 */
export async function nightly(env: EnquiryEnv, fetcher: Fetcher, now: number): Promise<void> {
  try {
    const { unmailed, storedYesterday } = await alertCounts(env.DB, now);
    if (unmailed > 0 || storedYesterday >= DAILY_STORED) {
      if (!env.MS_TENANT_ID || !env.MS_CLIENT_ID || !env.MS_CLIENT_SECRET) {
        console.error("consulting_alert_failed", "secret_missing");
      } else if (!(await sendMail(env, fetcher, NIGHTLY_SUBJECT, nightlyAlertText(unmailed, storedYesterday, DAILY_STORED)))) {
        console.error("consulting_alert_failed", "send");
      }
    }
  } catch {
    console.error("consulting_alert_failed", "store");
  }
  try {
    await sweep(env.DB, now);
  } catch {
    console.error("consulting_sweep_failed");
  }
}

export default {
  async fetch(request, env): Promise<Response> {
    const { pathname } = new URL(request.url);
    if (pathname !== "/v1/enquiry") return new Response("Not found", { status: 404 });
    return handleEnquiry(request, env, { fetch: (url, init) => fetch(url, init), now: () => Date.now() });
  },

  async scheduled(_controller, env): Promise<void> {
    await nightly(env, (url, init) => fetch(url, init), Date.now());
  },
} satisfies ExportedHandler<EnquiryEnv>;
