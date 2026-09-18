// The Paddle calls behind /v1/checkout (SIGNIN-DESIGN.md §5, §8.30):
// find-or-create the customer, a portal session, a checkout transaction.
import { describe, expect, it } from "vitest";

import { PaddleClient } from "../src/paddle";
import { UpstreamError } from "../src/upstream";
import { fakeFetch, json } from "./support";

const KEY = "pdl_sdbx_apikey_PLACEHOLDER_not_a_key";
const BASE = "https://sandbox-api.paddle.com";
const CUSTOMER = "ctm_01h8441jn5pcwrfhwh78jqt8hk";
const PRICE = "pri_01aaaaaaaaaaaaaaaaaaaaaaaa";
const TXN = "txn_01h8441jn5pcwrfhwh78jqt8hk";

function client(answer: Parameters<typeof fakeFetch>[0]) {
  const f = fakeFetch(answer);
  return { paddle: new PaddleClient({ apiKey: KEY, environment: "sandbox" }, f.fetch), seen: f.seen };
}

describe("finding or creating the customer", () => {
  it("finds an existing customer by exact email", async () => {
    const { paddle, seen } = client(() => json(200, { data: [{ id: CUSTOMER, email: "a@example.com", status: "active" }], meta: {} }));
    expect(await paddle.findOrCreateCustomer("a@example.com")).toBe(CUSTOMER);
    expect(seen).toHaveLength(1);
    expect(seen[0]?.method).toBe("GET");
    expect(`${seen[0]?.url.origin}${seen[0]?.url.pathname}`).toBe(`${BASE}/customers`);
    expect(seen[0]?.url.searchParams.get("email")).toBe("a@example.com");
    expect(seen[0]?.headers.get("authorization")).toBe(`Bearer ${KEY}`);
  });

  it("creates the customer when none exists", async () => {
    const { paddle, seen } = client((req) =>
      req.method === "GET" ? json(200, { data: [], meta: {} }) : json(201, { data: { id: CUSTOMER, email: "a@example.com" } }),
    );
    expect(await paddle.findOrCreateCustomer("a@example.com")).toBe(CUSTOMER);
    expect(seen.map((r) => r.method)).toEqual(["GET", "POST"]);
    expect(JSON.parse(seen[1]?.body ?? "")).toEqual({ email: "a@example.com" });
  });

  it("a creation race (409) re-reads instead of failing", async () => {
    let lists = 0;
    const { paddle } = client((req) => {
      if (req.method === "POST") return json(409, { error: { code: "customer_already_exists" } });
      lists += 1;
      return json(200, { data: lists === 1 ? [] : [{ id: CUSTOMER }], meta: {} });
    });
    expect(await paddle.findOrCreateCustomer("a@example.com")).toBe(CUSTOMER);
  });

  it("an email Paddle's list filter would split is refused before any call", async () => {
    const { paddle, seen } = client(() => json(200, { data: [], meta: {} }));
    await expect(paddle.findOrCreateCustomer('"a,b"@example.com')).rejects.toThrow(UpstreamError);
    expect(seen).toHaveLength(0);
  });

  it("an id that is not Paddle's shape is an upstream failure", async () => {
    const { paddle } = client(() => json(200, { data: [{ id: "cus_123" }], meta: {} }));
    await expect(paddle.findOrCreateCustomer("a@example.com")).rejects.toThrow(UpstreamError);
  });
});

describe("the customer portal", () => {
  it("creates a session for the given subscriptions and returns the overview link", async () => {
    const url = "https://sandbox-customer-portal.paddle.com/cpl_01abc?action=overview&token=pga_x";
    const { paddle, seen } = client(() => json(201, { data: { id: "cpls_1", customer_id: CUSTOMER, urls: { general: { overview: url }, subscriptions: [] } } }));
    expect(await paddle.createPortalSession(CUSTOMER, ["sub_01h8441jn5pcwrfhwh78jqt8hk"])).toBe(url);
    expect(seen[0]?.method).toBe("POST");
    expect(seen[0]?.url.href).toBe(`${BASE}/customers/${CUSTOMER}/portal-sessions`);
    expect(JSON.parse(seen[0]?.body ?? "")).toEqual({ subscription_ids: ["sub_01h8441jn5pcwrfhwh78jqt8hk"] });
  });

  it("refuses a link that is not https on a Paddle host", async () => {
    for (const bad of ["http://customer-portal.paddle.com/x", "https://evil.example/x", "https://paddle.com.evil.example/x", "not a url"]) {
      const { paddle } = client(() => json(201, { data: { urls: { general: { overview: bad } } } }));
      await expect(paddle.createPortalSession(CUSTOMER, [])).rejects.toThrow(UpstreamError);
    }
  });
});

describe("the checkout transaction", () => {
  it("creates it for one item with the Clerk user in custom_data", async () => {
    const { paddle, seen } = client(() => json(201, { data: { id: TXN, status: "draft", checkout: { url: "https://x/pay?_ptxn=" + TXN } } }));
    expect(await paddle.createTransaction(CUSTOMER, PRICE, "user_2abc")).toBe(TXN);
    expect(seen[0]?.method).toBe("POST");
    expect(seen[0]?.url.href).toBe(`${BASE}/transactions`);
    expect(JSON.parse(seen[0]?.body ?? "")).toEqual({
      items: [{ price_id: PRICE, quantity: 1 }],
      customer_id: CUSTOMER,
      custom_data: { clerk_user_id: "user_2abc" },
    });
  });

  it("an id that is not a transaction id is an upstream failure", async () => {
    const { paddle } = client(() => json(201, { data: { id: "txn_SHORT" } }));
    await expect(paddle.createTransaction(CUSTOMER, PRICE, "user_2abc")).rejects.toThrow(UpstreamError);
  });

  it("any error status is an upstream failure", async () => {
    for (const status of [400, 403, 429, 500]) {
      const { paddle } = client(() => json(status, {}));
      await expect(paddle.createTransaction(CUSTOMER, PRICE, "user_2abc")).rejects.toThrow(UpstreamError);
    }
  });
});
