// Talking to Clerk and Paddle.
//
// UpstreamError means "the answer could not be had": transport failures,
// 5xx, 429, our own credentials being refused, or a body we cannot read.
// It is never a verdict about the user. The §5 rules turn it into a signed
// `active` for a former payer or a 503, never a signed `ended` and never a
// 401. Messages are fixed text: no secret, token or upstream body is ever
// quoted.

export type Fetch = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

export class UpstreamError extends Error {
  override name = "UpstreamError";
}

/** Longest upstream body read (Paddle lists 200 subscriptions per page). */
const MAX_UPSTREAM_BYTES = 2_000_000;

/** `fetch`, with transport failures turned into UpstreamError. */
export async function send(fetch: Fetch, what: string, input: string, init: RequestInit): Promise<Response> {
  try {
    return await fetch(input, { ...init, redirect: "manual" });
  } catch {
    throw new UpstreamError(`${what}: no response`);
  }
}

/** The body as JSON, or UpstreamError. */
export async function readJson(what: string, response: Response): Promise<unknown> {
  const text = await response.text().catch(() => {
    throw new UpstreamError(`${what}: body unreadable`);
  });
  if (text.length > MAX_UPSTREAM_BYTES) throw new UpstreamError(`${what}: body too large`);
  try {
    return JSON.parse(text) as unknown;
  } catch {
    throw new UpstreamError(`${what}: body is not JSON`);
  }
}

export function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
