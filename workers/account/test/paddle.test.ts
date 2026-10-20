// The Paddle API client (SIGNIN-DESIGN.md §5, §8.30): one customer's
// subscriptions, all pages, never following a link off Paddle's own host.
import { describe, expect, it } from "vitest";

import { PaddleClient } from "../src/paddle";
import { UpstreamError } from "../src/upstream";
import { fakeFetch, json } from "./support";

const KEY = "pdl_sdbx_apikey_PLACEHOLDER_not_a_key";
const CUSTOMER = "ctm_01h8441jn5pcwrfhwh78jqt8hk";
const USER = "user_2abcdefghijklmnopqrstuvwxyz0";
const BASE = "https://sandbox-api.paddle.com";

function page(ids: string[], next: string | null): Response {
  return json(200, {
    data: ids.map((id) => ({ id, status: "active", custom_data: { clerk_user_id: USER } })),
    meta: { request_id: "r", pagination: { per_page: 200, next, has_more: next !== null, estimated_total: 9 } },
  });
}

function client(answer: Parameters<typeof fakeFetch>[0]) {
  const f = fakeFetch(answer);
  return { paddle: new PaddleClient({ apiKey: KEY, environment: "sandbox" }, f.fetch), seen: f.seen };
}

describe("listing a customer's subscriptions", () => {
  it("sends the documented request", async () => {
    const { paddle, seen } = client(() => page(["sub_1"], null));
    expect(await paddle.listSubscriptions(CUSTOMER, USER)).toEqual([{ id: "sub_1", status: "active", custom_data: { clerk_user_id: USER } }]);
    expect(seen).toHaveLength(1);
    expect(seen[0]?.method).toBe("GET");
    expect(seen[0]?.url.origin).toBe(BASE);
    expect(seen[0]?.url.pathname).toBe("/subscriptions");
    expect(seen[0]?.url.searchParams.get("customer_id")).toBe(CUSTOMER);
    expect(seen[0]?.url.searchParams.get("per_page")).toBe("200");
    expect(seen[0]?.headers.get("authorization")).toBe(`Bearer ${KEY}`);
  });

  /*
    Audit s104 PDL-2: a Paddle customer is one email. A user who takes over an
    email another user gave up must not inherit that user's subscriptions.
  */
  it("keeps only the subscriptions this user bought", async () => {
    const { paddle } = client(() =>
      json(200, {
        data: [
          { id: "sub_mine", custom_data: { clerk_user_id: USER } },
          { id: "sub_theirs", custom_data: { clerk_user_id: "user_2someoneelse0000000000000" } },
          { id: "sub_unmarked", custom_data: null },
          { id: "sub_bare" },
        ],
        meta: { pagination: { has_more: false } },
      }),
    );
    expect((await paddle.listSubscriptions(CUSTOMER, USER)).map((s) => (s as { id: string }).id)).toEqual(["sub_mine"]);
  });

  it("follows every page", async () => {
    const { paddle, seen } = client((req) =>
      req.url.searchParams.get("after") === "sub_2" ? page(["sub_3"], null) : page(["sub_1", "sub_2"], `${BASE}/subscriptions?customer_id=${CUSTOMER}&per_page=200&after=sub_2`),
    );
    expect((await paddle.listSubscriptions(CUSTOMER, USER)).map((s) => (s as { id: string }).id)).toEqual(["sub_1", "sub_2", "sub_3"]);
    expect(seen).toHaveLength(2);
  });

  it("never sends the key to a host other than Paddle's API", async () => {
    const { paddle, seen } = client(() => page(["sub_1"], "https://evil.example/subscriptions?after=sub_1"));
    await expect(paddle.listSubscriptions(CUSTOMER, USER)).rejects.toThrow(UpstreamError);
    expect(seen).toHaveLength(1);
  });

  it("gives up rather than derive from a partial list when there are too many pages", async () => {
    let n = 0;
    const { paddle, seen } = client(() => {
      n += 1;
      return page([`sub_${n}`], `${BASE}/subscriptions?customer_id=${CUSTOMER}&per_page=200&after=sub_${n}`);
    });
    await expect(paddle.listSubscriptions(CUSTOMER, USER)).rejects.toThrow(UpstreamError);
    expect(seen.length).toBeLessThanOrEqual(5);
  });

  it("uses the live API for the live environment", async () => {
    const f = fakeFetch(() => page([], null));
    await new PaddleClient({ apiKey: "pdl_live_apikey_X", environment: "live" }, f.fetch).listSubscriptions(CUSTOMER, USER);
    expect(f.seen[0]?.url.origin).toBe("https://api.paddle.com");
  });

  const failures: Array<[string, () => Response | "network-error"]> = [
    ["an error status", () => json(500, {})],
    ["rate limiting", () => json(429, {})],
    ["a refused key", () => json(403, {})],
    ["no network", () => "network-error"],
    ["a body with no data list", () => json(200, { meta: {} })],
    ["a page claiming more with no link", () => json(200, { data: [], meta: { pagination: { has_more: true, next: null } } })],
  ];
  for (const [name, answer] of failures) {
    it(`treats ${name} as an upstream failure`, async () => {
      const { paddle } = client(answer);
      await expect(paddle.listSubscriptions(CUSTOMER, USER)).rejects.toThrow(UpstreamError);
    });
  }

  it("refuses a customer id that is not Paddle's shape, without calling Paddle", async () => {
    const { paddle, seen } = client(() => page([], null));
    await expect(paddle.listSubscriptions("ctm_x&customer_id=ctm_other", USER)).rejects.toThrow(UpstreamError);
    expect(seen).toHaveLength(0);
  });

  it("never puts the key in an error message", async () => {
    const { paddle } = client(() => json(500, { echo: KEY }));
    const err = await paddle.listSubscriptions(CUSTOMER, USER).then(
      () => null,
      (e: unknown) => e as Error,
    );
    expect(String(err?.message)).not.toContain(KEY);
  });
});
