// POST /v1/enquiry (ADR-121, ADR-SEC-057): our form page only (same origin,
// so no CORS header is ever sent); every secret present; a capped body; the
// fields; a passed Turnstile check; stored; then consulting@ told, within a
// daily cap. Once stored, the visitor always hears "received": a failed
// email is logged by reason only and the enquiry waits in the database.
import { verifyTurnstile, type Fetcher } from "./gate";
import { MAIL_DAILY_CAP, sendEnquiryMail, type MailEnv } from "./mail";
import { claimMail, dayOf, markMailed, storeEnquiry } from "./store";
import { validateEnquiry } from "./validate";

/** The form page's own address: the only Origin allowed to send. */
export const ORIGIN = "https://consulting.zaaheen.com";

/** A real enquiry (2,000-character message at most) is far below this. */
export const MAX_BODY_BYTES = 16 * 1024;

export interface EnquiryEnv extends MailEnv {
  DB: D1Database;
  TURNSTILE_SECRET: string;
}

export interface Deps {
  fetch: Fetcher;
  now: () => number;
}

export async function handleEnquiry(request: Request, env: EnquiryEnv, deps: Deps): Promise<Response> {
  if (request.method !== "POST") return reply(405, { kind: "invalid" });
  if (request.headers.get("Origin") !== ORIGIN) return reply(403, { kind: "forbidden" });
  if (!env.TURNSTILE_SECRET || !env.MS_TENANT_ID || !env.MS_CLIENT_ID || !env.MS_CLIENT_SECRET) {
    console.warn("consulting_unavailable", "secret_missing");
    return reply(503, { kind: "unavailable" });
  }

  if (Number(request.headers.get("Content-Length")) > MAX_BODY_BYTES) return reply(413, { kind: "invalid" });
  const raw = await readCapped(request, MAX_BODY_BYTES);
  if (raw === null) return reply(413, { kind: "invalid" });
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return reply(400, { kind: "invalid" });
  }
  const checked = validateEnquiry(parsed);
  if (!checked.ok) return reply(400, { kind: "invalid" });

  const address = request.headers.get("CF-Connecting-IP") ?? "";
  if (!(await verifyTurnstile(checked.turnstile, address, env.TURNSTILE_SECRET, deps.fetch))) {
    console.warn("consulting_refused", "check", "not_accepted");
    return reply(401, { kind: "challenge" });
  }

  const now = deps.now();
  let id: number;
  try {
    id = await storeEnquiry(env.DB, now, checked.enquiry);
  } catch {
    console.error("consulting_unavailable", "store");
    return reply(503, { kind: "unavailable" });
  }

  if (await claimMail(env.DB, dayOf(now), MAIL_DAILY_CAP)) {
    if (await sendEnquiryMail(env, deps.fetch, id, checked.enquiry, now)) await markMailed(env.DB, id);
  } else {
    console.warn("consulting_mail_skipped", "daily_cap");
  }
  return reply(200, { kind: "received" });
}

/** The body as text, or null as soon as it passes `max` bytes: an over-large body is never read whole. */
async function readCapped(request: Request, max: number): Promise<string | null> {
  if (!request.body) return "";
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > max) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.byteLength;
  }
  return new TextDecoder().decode(bytes);
}

function reply(status: number, body: Record<string, unknown>): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    },
  });
}
