// POST /v1/cancel end to end, against fake Clerk and Paddle
// (SIGNIN-DESIGN.md §8.48, ADR-SEC-042):
//   never creates anything (no customer, no record write, no transaction)
//   no customer, or none of ours active/past_due   -> {kind:"none"}
//   every one of those already scheduled to cancel -> {kind:"ending"}
//   otherwise the portal session's cancel deep link -> {kind:"portal", url}
//   a deep link missing or off Paddle               -> 503, never a fallback
import { describe, expect, it } from "vitest";

import type { Config } from "../src/config";
import { handleCancel } from "../src/routes/cancel";
import { type Seen, fakeFetch, json, rfcPkcs8 } from "./support";

const T = 1_800_000_000;
const USER = "user_2abcdefghijklmnopqrstuvwxyz0";
const CUSTOMER = "ctm_01h8441jn5pcwrfhwh78jqt8hk";
const PRODUCT = "pro_01aaaaaaaaaaaaaaaaaaaaaaaa";
const SUB_A = "sub_01eeeeeeeeeeeeeeeeeeeeeeee";
const SUB_B = "sub_01ffffffffffffffffffffffff";
const CANCEL_A = "https://sandbox-customer-portal.paddle.com/cpl_01abc?action=cancel_subscription&subscription_id=sub_01eeeeeeeeeeeeeeeeeeeeeeee&token=pga_x";
const CANCEL_B = "https://sandbox-customer-portal.paddle.com/cpl_01abc?action=cancel_subscription&subscription_id=sub_01ffffffffffffffffffffffff&token=pga_x";
const OVERVIEW = "https://sandbox-customer-portal.paddle.com/cpl_01abc?action=overview&token=pga_x";

const config: Config = {
  clerk: { secretKey: "sk_test_SECRET", clientId: "client_ours", webhookSecret: "whsec_placeholder" },
  paddle: {
    apiKey: "pdl_sdbx_apikey_SECRET",
    environment: "sandbox",
    productId: PRODUCT,
    prices: { monthly: "pri_01bbbbbbbbbbbbbbbbbbbbbbbb", annual: "pri_01cccccccccccccccccccccccc" },
    webhookSecret: "pdl_ntfset_test",
  },
  lease: { kid: "primary", pkcs8: rfcPkcs8("primary") },
  trials: { key: "k".repeat(32) },
  killSwitch: false,
};

interface World {
  metadata?: unknown;
  subscriptions?: unknown[] | "down";
  // What the portal session answers under data.urls.subscriptions.
  links?: unknown;
}

function sub(id: string, status: string, opts: { productId?: string; ending?: boolean } = {}): unknown {
  return {
    id,
    status,
    items: [{ price: { product_id: opts.productId ?? PRODUCT } }],
    current_billing_period: { starts_at: "2027-01-01T00:00:00Z", ends_at: "2027-02-01T00:00:00Z" },
    scheduled_change: opts.ending ? { action: "cancel", effective_at: "2027-02-01T00:00:00Z" } : null,
  };
}

const PAYER = { zaaheen_memory: { paddle_customer_id: CUSTOMER } };
const BOTH_LINKS = [
  { id: SUB_A, cancel_subscription: CANCEL_A, update_subscription_payment_method: OVERVIEW },
  { id: SUB_B, cancel_subscription: CANCEL_B, update_subscription_payment_method: OVERVIEW },
];

function world(w: World) {
  const f = fakeFetch((req: Seen) => {
    const { pathname, origin } = req.url;
    if (origin === "https://api.clerk.com") {
      if (pathname.endsWith("/access_tokens/verify")) {
        return json(200, { client_id: "client_ours", subject: USER, revoked: false, expired: false });
      }
      if (pathname === `/v1/users/${USER}`) {
        return json(200, {
          id: USER,
          primary_email_address_id: "idn_1",
          email_addresses: [{ id: "idn_1", email_address: "buyer@example.com" }],
          private_metadata: w.metadata ?? {},
        });
      }
    }
    if (origin === "https://sandbox-api.paddle.com") {
      if (pathname === "/subscriptions") {
        if (w.subscriptions === "down") return json(502, {});
        return json(200, { data: w.subscriptions ?? [], meta: { pagination: { has_more: false } } });
      }
      if (pathname === `/customers/${CUSTOMER}/portal-sessions`) {
        const links = w.links === undefined ? BOTH_LINKS : w.links;
        return json(201, { data: { urls: { general: { overview: OVERVIEW }, subscriptions: links } } });
      }
    }
    return json(418, { unexpected: req.url.href });
  });
  return f;
}

function request(headers: Record<string, string> = { authorization: "Bearer at_TOKEN" }, method = "POST"): Request {
  return new Request("https://api.zaaheen.com/v1/cancel", method === "POST" ? { method, headers, body: "{}" } : { method, headers });
}

async function call(w: World, req: Request = request()) {
  const wd = world(w);
  const response = await handleCancel(req, config, { fetch: wd.fetch, now: () => T });
  return { response, seen: wd.seen };
}

function paddleCalls(seen: Seen[]): string[] {
  return seen.filter((r) => r.url.origin.includes("paddle")).map((r) => `${r.method} ${r.url.pathname}`);
}

function clerkWrites(seen: Seen[]): Seen[] {
  return seen.filter((r) => r.url.origin === "https://api.clerk.com" && r.method !== "GET" && !r.url.pathname.endsWith("/verify"));
}

describe("a paying member", () => {
  it("gets Paddle's cancel step for their subscription, and nothing is created or written", async () => {
    const { response, seen } = await call({ metadata: PAYER, subscriptions: [sub(SUB_A, "active")], links: [BOTH_LINKS[0]] });
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("no-store");
    expect(await response.json()).toEqual({ kind: "portal", url: CANCEL_A });
    expect(paddleCalls(seen)).toEqual(["GET /subscriptions", `POST /customers/${CUSTOMER}/portal-sessions`]);
    const portal = seen.find((r) => r.url.pathname.endsWith("/portal-sessions"));
    expect(JSON.parse(portal?.body ?? "")).toEqual({ subscription_ids: [SUB_A] });
    expect(clerkWrites(seen)).toEqual([]);
  });

  it("whose last payment failed can still cancel", async () => {
    const { response } = await call({ metadata: PAYER, subscriptions: [sub(SUB_A, "past_due")], links: [BOTH_LINKS[0]] });
    expect(await response.json()).toEqual({ kind: "portal", url: CANCEL_A });
  });

  it("is sent to the subscription that is not already ending", async () => {
    const { response, seen } = await call({
      metadata: PAYER,
      subscriptions: [sub(SUB_A, "active", { ending: true }), sub(SUB_B, "active")],
      links: [BOTH_LINKS[1]],
    });
    expect(await response.json()).toEqual({ kind: "portal", url: CANCEL_B });
    const portal = seen.find((r) => r.url.pathname.endsWith("/portal-sessions"));
    expect(JSON.parse(portal?.body ?? "")).toEqual({ subscription_ids: [SUB_B] });
  });

  it("picks the link by subscription id, not by its place in Paddle's list", async () => {
    const { response } = await call({ metadata: PAYER, subscriptions: [sub(SUB_B, "active")], links: BOTH_LINKS });
    expect(await response.json()).toEqual({ kind: "portal", url: CANCEL_B });
  });
});

describe("nothing to cancel", () => {
  it("someone who never paid: no Paddle customer, and Paddle is never asked", async () => {
    const { response, seen } = await call({ metadata: { zaaheen_memory: { trial_started_at: T } } });
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ kind: "none" });
    expect(paddleCalls(seen)).toEqual([]);
    expect(clerkWrites(seen)).toEqual([]);
  });

  it("a customer whose subscriptions are all canceled or paused", async () => {
    const { response, seen } = await call({ metadata: PAYER, subscriptions: [sub(SUB_A, "canceled"), sub(SUB_B, "paused")] });
    expect(await response.json()).toEqual({ kind: "none" });
    expect(paddleCalls(seen)).toEqual(["GET /subscriptions"]);
  });

  it("a subscription to another product does not count", async () => {
    const other = sub(SUB_A, "active", { productId: "pro_01zzzzzzzzzzzzzzzzzzzzzzzz" });
    const { response } = await call({ metadata: PAYER, subscriptions: [other] });
    expect(await response.json()).toEqual({ kind: "none" });
  });
});

describe("already cancelled", () => {
  it("every current subscription already has a scheduled cancel: no portal session", async () => {
    const { response, seen } = await call({ metadata: PAYER, subscriptions: [sub(SUB_A, "active", { ending: true })] });
    expect(await response.json()).toEqual({ kind: "ending" });
    expect(paddleCalls(seen)).toEqual(["GET /subscriptions"]);
  });
});

describe("refused or unavailable, never a fallback", () => {
  it("a cancel link off Paddle's host is a 503", async () => {
    const links = [{ id: SUB_A, cancel_subscription: "https://paddle.com.evil.test/cancel" }];
    const { response } = await call({ metadata: PAYER, subscriptions: [sub(SUB_A, "active")], links });
    expect(response.status).toBe(503);
  });

  it("a plain-http cancel link is a 503", async () => {
    const links = [{ id: SUB_A, cancel_subscription: CANCEL_A.replace("https:", "http:") }];
    const { response } = await call({ metadata: PAYER, subscriptions: [sub(SUB_A, "active")], links });
    expect(response.status).toBe(503);
  });

  it("no cancel link for the subscription is a 503, not the overview", async () => {
    const { response } = await call({ metadata: PAYER, subscriptions: [sub(SUB_A, "active")], links: [BOTH_LINKS[1]] });
    expect(response.status).toBe(503);
  });

  it("a portal answer with no subscription links is a 503", async () => {
    const { response } = await call({ metadata: PAYER, subscriptions: [sub(SUB_A, "active")], links: null });
    expect(response.status).toBe(503);
  });

  it("Paddle down is a 503", async () => {
    const { response } = await call({ metadata: PAYER, subscriptions: "down" });
    expect(response.status).toBe(503);
  });

  it("no token is a 401 and nobody is asked", async () => {
    const { response, seen } = await call({ metadata: PAYER }, request({}));
    expect(response.status).toBe(401);
    expect(seen).toEqual([]);
  });

  it("only POST", async () => {
    const { response } = await call({ metadata: PAYER }, request({ authorization: "Bearer at_TOKEN" }, "GET"));
    expect(response.status).toBe(405);
  });
});
