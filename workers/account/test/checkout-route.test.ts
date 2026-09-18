// POST /v1/checkout end to end, against fake Clerk and Paddle
// (SIGNIN-DESIGN.md §5 "/v1/checkout", §8.30):
//   body {plan: "monthly"|"annual"} -> allowlisted price
//   already subscribed (active or past_due, live fetch) -> {kind:"portal", url}
//   otherwise find-or-create the customer, store paddle_customer_id and
//   checkout_at, create a transaction with custom_data.clerk_user_id
//   -> {kind:"checkout", txn}
import { describe, expect, it } from "vitest";

import type { Config } from "../src/config";
import { handleCheckout } from "../src/routes/checkout";
import { DAY } from "../src/time";
import { type Seen, fakeFetch, json, rfcPkcs8 } from "./support";

const T = 1_800_000_000;
const USER = "user_2abcdefghijklmnopqrstuvwxyz0";
const CUSTOMER = "ctm_01h8441jn5pcwrfhwh78jqt8hk";
const PRODUCT = "pro_01aaaaaaaaaaaaaaaaaaaaaaaa";
const MONTHLY = "pri_01bbbbbbbbbbbbbbbbbbbbbbbb";
const ANNUAL = "pri_01cccccccccccccccccccccccc";
const TXN = "txn_01dddddddddddddddddddddddd";
const PORTAL = "https://sandbox-customer-portal.paddle.com/cpl_01abc?action=overview&token=pga_x";

const config: Config = {
  clerk: { secretKey: "sk_test_SECRET", clientId: "client_ours", webhookSecret: "whsec_placeholder" },
  paddle: { apiKey: "pdl_sdbx_apikey_SECRET", environment: "sandbox", productId: PRODUCT, prices: { monthly: MONTHLY, annual: ANNUAL }, webhookSecret: "pdl_ntfset_test" },
  lease: { kid: "primary", pkcs8: rfcPkcs8("primary") },
  killSwitch: false,
};

interface World {
  metadata?: unknown;
  email?: string | null;
  subscriptions?: unknown[] | "down";
  existingCustomer?: boolean;
  writeFails?: boolean;
  txnFails?: boolean;
}

function sub(status: string, productId = PRODUCT): unknown {
  return {
    id: "sub_01eeeeeeeeeeeeeeeeeeeeeeee",
    status,
    items: [{ price: { product_id: productId } }],
    current_billing_period: { starts_at: "2027-01-01T00:00:00Z", ends_at: "2027-02-01T00:00:00Z" },
    scheduled_change: null,
  };
}

function world(w: World) {
  const writes: unknown[] = [];
  const f = fakeFetch((req: Seen) => {
    const { pathname, origin } = req.url;
    if (origin === "https://api.clerk.com") {
      if (pathname.endsWith("/access_tokens/verify")) {
        return json(200, { client_id: "client_ours", subject: USER, revoked: false, expired: false });
      }
      if (pathname === `/v1/users/${USER}`) {
        const email = w.email === undefined ? "buyer@example.com" : w.email;
        return json(200, {
          id: USER,
          primary_email_address_id: email === null ? null : "idn_1",
          email_addresses: email === null ? [] : [{ id: "idn_1", email_address: email }],
          private_metadata: w.metadata ?? {},
        });
      }
      if (pathname === `/v1/users/${USER}/metadata`) {
        if (w.writeFails) return json(500, {});
        writes.push(JSON.parse(req.body));
        return json(200, { id: USER });
      }
    }
    if (origin === "https://sandbox-api.paddle.com") {
      if (pathname === "/subscriptions") {
        if (w.subscriptions === "down") return json(502, {});
        return json(200, { data: w.subscriptions ?? [], meta: { pagination: { has_more: false } } });
      }
      if (pathname === "/customers" && req.method === "GET") {
        return json(200, { data: w.existingCustomer ? [{ id: CUSTOMER }] : [], meta: {} });
      }
      if (pathname === "/customers" && req.method === "POST") return json(201, { data: { id: CUSTOMER } });
      if (pathname === `/customers/${CUSTOMER}/portal-sessions`) {
        return json(201, { data: { urls: { general: { overview: PORTAL } } } });
      }
      if (pathname === "/transactions") return w.txnFails ? json(500, {}) : json(201, { data: { id: TXN } });
    }
    return json(418, { unexpected: req.url.href });
  });
  return { fetch: f.fetch, seen: f.seen, writes };
}

function request(body: unknown, headers: Record<string, string> = {}): Request {
  return new Request("https://api.zaaheen.com/v1/checkout", {
    method: "POST",
    headers: { authorization: "Bearer at_TOKEN", "content-type": "application/json", ...headers },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });
}

async function call(w: World, req: Request = request({ plan: "monthly" })) {
  const wd = world(w);
  const response = await handleCheckout(req, config, { fetch: wd.fetch, now: () => T });
  return { response, ...wd };
}

function paddleCalls(seen: Seen[]): string[] {
  return seen.filter((r) => r.url.origin.includes("paddle")).map((r) => `${r.method} ${r.url.pathname}`);
}

describe("a new subscriber", () => {
  it("gets a transaction for the chosen plan, tagged with their Clerk id, after the record is saved", async () => {
    const { response, seen, writes } = await call({ metadata: { zaaheen_memory: { trial_started_at: T - 10 * DAY } } });
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("no-store");
    expect(await response.json()).toEqual({ kind: "checkout", txn: TXN });
    expect(paddleCalls(seen)).toEqual(["GET /customers", "POST /customers", "POST /transactions"]);
    expect(writes).toEqual([{ private_metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER, checkout_at: T } } }]);
    const txn = seen.find((r) => r.url.pathname === "/transactions");
    expect(JSON.parse(txn?.body ?? "")).toEqual({
      items: [{ price_id: MONTHLY, quantity: 1 }],
      customer_id: CUSTOMER,
      custom_data: { clerk_user_id: USER },
    });
    // The record is written before the transaction exists.
    const order = seen.map((r) => r.url.pathname);
    expect(order.indexOf(`/v1/users/${USER}/metadata`)).toBeLessThan(order.indexOf("/transactions"));
  });

  it("the annual plan maps to the annual price", async () => {
    const { seen } = await call({}, request({ plan: "annual" }));
    const txn = seen.find((r) => r.url.pathname === "/transactions");
    expect(JSON.parse(txn?.body ?? "")).toMatchObject({ items: [{ price_id: ANNUAL, quantity: 1 }] });
  });

  it("an existing Paddle customer with that email is reused, not duplicated", async () => {
    const { seen } = await call({ existingCustomer: true });
    expect(paddleCalls(seen)).toEqual(["GET /customers", "POST /transactions"]);
  });

  it("a customer already in the record is used as-is, after checking they are not subscribed", async () => {
    const { seen, writes } = await call({ metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER } } });
    expect(paddleCalls(seen)).toEqual(["GET /subscriptions", "POST /transactions"]);
    expect(writes).toEqual([{ private_metadata: { zaaheen_memory: { checkout_at: T } } }]);
  });

  it("a subscription to another product does not count as subscribed", async () => {
    const { response } = await call({ metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER } }, subscriptions: [sub("active", "pro_01ffffffffffffffffffffffff")] });
    expect(await response.json()).toEqual({ kind: "checkout", txn: TXN });
  });

  it("paused or cancelled subscriptions do not count as subscribed", async () => {
    for (const status of ["paused", "canceled"]) {
      const { response } = await call({ metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER } }, subscriptions: [sub(status)] });
      expect(await response.json()).toEqual({ kind: "checkout", txn: TXN });
    }
  });
});

describe("someone already subscribed", () => {
  for (const status of ["active", "past_due"]) {
    it(`(${status}) is sent to the customer portal, and no transaction is created`, async () => {
      const { response, seen, writes } = await call({ metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER } }, subscriptions: [sub(status)] });
      expect(response.status).toBe(200);
      expect(await response.json()).toEqual({ kind: "portal", url: PORTAL });
      expect(paddleCalls(seen)).toEqual(["GET /subscriptions", `POST /customers/${CUSTOMER}/portal-sessions`]);
      const portal = seen.find((r) => r.url.pathname.endsWith("/portal-sessions"));
      expect(JSON.parse(portal?.body ?? "")).toEqual({ subscription_ids: ["sub_01eeeeeeeeeeeeeeeeeeeeeeee"] });
      expect(writes).toEqual([]);
    });
  }
});

describe("failures never start a checkout that cannot be recorded", () => {
  it("the record cannot be saved: 503, and no transaction", async () => {
    const { response, seen } = await call({ writeFails: true });
    expect(response.status).toBe(503);
    expect(paddleCalls(seen)).not.toContain("POST /transactions");
  });

  it("Paddle down while checking an existing customer: 503", async () => {
    expect((await call({ metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER } }, subscriptions: "down" })).response.status).toBe(503);
  });

  it("the transaction fails: 503", async () => {
    expect((await call({ txnFails: true })).response.status).toBe(503);
  });

  it("a user with no email address: 422, and nothing is created", async () => {
    const { response, seen } = await call({ email: null });
    expect(response.status).toBe(422);
    expect(await response.json()).toEqual({ error: "no_email" });
    expect(paddleCalls(seen)).toEqual([]);
  });
});

describe("the request itself", () => {
  it("only POST", async () => {
    expect((await call({}, new Request("https://api.zaaheen.com/v1/checkout"))).response.status).toBe(405);
  });

  it("no token is 401, without calling anyone", async () => {
    const { response, seen } = await call({}, request({ plan: "monthly" }, { authorization: "" }));
    expect(response.status).toBe(401);
    expect(seen).toHaveLength(0);
  });

  for (const body of [{}, { plan: "weekly" }, { plan: "Monthly" }, { plan: 1 }, "[]", "{"]) {
    it(`a bad body (${JSON.stringify(body)}) is 400, without calling anyone`, async () => {
      const { response, seen } = await call({}, request(body));
      expect(response.status).toBe(400);
      expect(seen).toHaveLength(0);
    });
  }
});
