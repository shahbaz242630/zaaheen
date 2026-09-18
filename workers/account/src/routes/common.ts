// What every authenticated route shares: its dependencies, the capped JSON
// body, the Clerk check, and one log format.

import { ClerkClient, type ClerkUser } from "../clerk";
import { errorResponse, readCapped } from "../http";
import type { Fetch } from "../upstream";

export interface RouteDeps {
  fetch: Fetch;
  /** Server time, whole epoch seconds. */
  now(): number;
}

/** The request body as a JSON object, or `null` (too big, not JSON, not an object). */
export async function readJsonObject(request: Request, maxBytes: number): Promise<Record<string, unknown> | null> {
  const text = await readCapped(request, maxBytes);
  if (text === null) return null;
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return null;
  }
  return typeof raw === "object" && raw !== null && !Array.isArray(raw) ? (raw as Record<string, unknown>) : null;
}

export type Authenticated = { kind: "user"; sub: string; user: ClerkUser } | { kind: "refused"; response: Response };

/**
 * The Clerk user behind `token`: refused (401) for a token Clerk refuses or
 * a user who no longer exists. Upstream failures throw (the caller answers
 * 503).
 */
export async function authenticate(clerk: ClerkClient, token: string): Promise<Authenticated> {
  const check = await clerk.verifyAccessToken(token);
  if (check.kind === "refused") return { kind: "refused", response: errorResponse(401, "token_refused") };
  const user = await clerk.getUser(check.sub);
  if (user === null) return { kind: "refused", response: errorResponse(401, "token_refused") };
  return { kind: "user", sub: check.sub, user };
}

/** One structured log line. Never a token, key, user id, email or upstream body. */
export function log(route: string, event: string, detail?: string): void {
  console.warn(JSON.stringify({ route, event, ...(detail === undefined ? {} : { detail }) }));
}
