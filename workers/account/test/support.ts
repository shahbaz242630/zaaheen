// Test helpers: a fake `fetch` that records every request and answers from
// a handler, so the Clerk and Paddle clients are tested without a network.
import type { Fetch } from "../src/upstream";

export interface Seen {
  method: string;
  url: URL;
  headers: Headers;
  body: string;
}

export type Answer = Response | "network-error";

export function fakeFetch(answer: (req: Seen) => Answer | Promise<Answer>): { fetch: Fetch; seen: Seen[] } {
  const seen: Seen[] = [];
  const fetch: Fetch = async (input, init) => {
    const request = new Request(input, init);
    const req: Seen = {
      method: request.method,
      url: new URL(request.url),
      headers: request.headers,
      body: request.method === "GET" ? "" : await request.text(),
    };
    seen.push(req);
    const a = await answer(req);
    if (a === "network-error") throw new TypeError("fetch failed");
    return a;
  };
  return { fetch, seen };
}

export function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

/** RFC 8032 section 7.1 TEST 1 (primary) and TEST 2 (backup): published test seeds. */
export const RFC8032_SEEDS: Record<string, string> = {
  primary: "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
  backup: "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
};

/** The PKCS#8 (base64) form of an RFC 8032 test seed, built at test time. */
export function rfcPkcs8(kid: string): string {
  const hex = "302e020100300506032b657004220420" + (RFC8032_SEEDS[kid] ?? "");
  const bytes = Uint8Array.from(hex.match(/../g) ?? [], (h) => parseInt(h, 16));
  return btoa(String.fromCharCode(...bytes));
}
