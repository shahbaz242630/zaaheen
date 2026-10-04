// /v1/checkout and /v1/cancel from the website account page (AUTH-PAGES-
// DESIGN S85-1, ADR-SEC-043): a session token works there too, checked
// without a network call; it is never sent to the OAuth verify, and the
// app's opaque token is never parsed as a JWT.
import { beforeAll, describe, expect, it } from "vitest";

import type { Config } from "../src/config";
import { handleCancel } from "../src/routes/cancel";
import { handleCheckout } from "../src/routes/checkout";
import { type SessionKit, sessionKit } from "./session-support";
import { type Seen, fakeFetch, json, rfcPkcs8 } from "./support";

const T = 1_800_000_000;
const USER = "user_2abcdefghijklmnopqrstuvwxyz0";
const CUSTOMER = "ctm_01h8441jn5pcwrfhwh78jqt8hk";
const PRODUCT = "pro_01aaaaaaaaaaaaaaaaaaaaaaaa";
const TXN = "txn_01dddddddddddddddddddddddd";
const SUB = "sub_01eeeeeeeeeeeeeeeeeeeeeeee";
const CANCEL = "https://sandbox-customer-portal.paddle.com/cpl_01abc?action=cancel_subscription&token=pga_x";

let kit: SessionKit;
let config: Config;

beforeAll(async () => {
  kit = await sessionKit();
  config = {
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
    web: kit.web,
  };
});

function world(metadata: unknown, subscriptions: unknown[]) {
  return fakeFetch((req: Seen) => {
    const { pathname, origin } = req.url;
    if (origin === "https://api.clerk.com") {
      if (pathname.endsWith("/access_tokens/verify")) {
        return json(200, { client_id: "client_ours", subject: USER, revoked: false, expired: false });
      }
      if (pathname === `/v1/users/${USER}` && req.method === "GET") {
        return json(200, { id: USER, primary_email_address_id: "idn_1", email_addresses: [{ id: "idn_1", email_address: "a@example.com" }], private_metadata: metadata });
      }
      if (pathname === `/v1/users/${USER}/metadata`) return json(200, { id: USER });
    }
    if (origin === "https://sandbox-api.paddle.com") {
      if (pathname === "/subscriptions") return json(200, { data: subscriptions, meta: { pagination: { has_more: false } } });
      if (pathname === "/customers" && req.method === "GET") return json(200, { data: [{ id: CUSTOMER }], meta: {} });
      if (pathname === "/transactions") return json(201, { data: { id: TXN } });
      if (pathname === `/customers/${CUSTOMER}/portal-sessions`) {
        return json(201, { data: { urls: { general: { overview: CANCEL }, subscriptions: [{ id: SUB, cancel_subscription: CANCEL }] } } });
      }
    }
    return json(418, { unexpected: req.url.href });
  });
}

const PAYER = { zaaheen_memory: { paddle_customer_id: CUSTOMER } };
const ACTIVE = [{ id: SUB, status: "active", items: [{ price: { product_id: PRODUCT } }], scheduled_change: null }];
const verifyCalls = (seen: Seen[]) => seen.filter((r) => r.url.pathname.endsWith("/access_tokens/verify"));

describe("with a website session token", () => {
  it("cancel opens Paddle's cancel step, and Clerk's OAuth verify is never called", async () => {
    const f = world(PAYER, ACTIVE);
    const req = new Request("https://api.example.test/v1/cancel", { method: "POST", headers: { authorization: `Bearer ${await kit.token(USER, T)}` }, body: "{}" });
    const response = await handleCancel(req, config, { fetch: f.fetch, now: () => T });
    expect(await response.json()).toEqual({ kind: "portal", url: CANCEL });
    expect(verifyCalls(f.seen)).toEqual([]);
  });

  it("checkout starts a transaction for the signed-in user", async () => {
    const f = world({ zaaheen_memory: { trial_started_at: T - 1000 } }, []);
    const req = new Request("https://api.example.test/v1/checkout", {
      method: "POST",
      headers: { authorization: `Bearer ${await kit.token(USER, T)}`, "content-type": "application/json" },
      body: JSON.stringify({ plan: "annual" }),
    });
    const response = await handleCheckout(req, config, { fetch: f.fetch, now: () => T });
    expect(await response.json()).toEqual({ kind: "checkout", txn: TXN });
    expect(verifyCalls(f.seen)).toEqual([]);
  });

  it("an expired one is refused before anything else is asked", async () => {
    const f = world(PAYER, ACTIVE);
    const stale = await kit.token(USER, T - 3600);
    const req = new Request("https://api.example.test/v1/cancel", { method: "POST", headers: { authorization: `Bearer ${stale}` } });
    const response = await handleCancel(req, config, { fetch: f.fetch, now: () => T });
    expect(response.status).toBe(401);
    expect(f.seen).toEqual([]);
  });

  it("a JWT is refused when the Worker has no website settings, never sent to the OAuth verify", async () => {
    const f = world(PAYER, ACTIVE);
    const { web: _web, ...noWeb } = config;
    const req = new Request("https://api.example.test/v1/cancel", { method: "POST", headers: { authorization: `Bearer ${await kit.token(USER, T)}` } });
    const response = await handleCancel(req, noWeb, { fetch: f.fetch, now: () => T });
    expect(response.status).toBe(401);
    expect(verifyCalls(f.seen)).toEqual([]);
  });
});

describe("Manage from the website (portal_only, review s85 finding 4)", () => {
  it("with no current subscription answers none, and creates and writes nothing", async () => {
    const f = world({ zaaheen_memory: { paddle_customer_id: CUSTOMER } }, []);
    const req = new Request("https://api.example.test/v1/checkout", {
      method: "POST",
      headers: { authorization: `Bearer ${await kit.token(USER, T)}`, "content-type": "application/json" },
      body: JSON.stringify({ plan: "monthly", portal_only: true }),
    });
    const response = await handleCheckout(req, config, { fetch: f.fetch, now: () => T });
    expect(await response.json()).toEqual({ kind: "none" });
    expect(f.seen.filter((r) => r.method !== "GET")).toEqual([]);
  });

  it("for a subscriber is the subscription page, as before", async () => {
    const f = world(PAYER, ACTIVE);
    const req = new Request("https://api.example.test/v1/checkout", {
      method: "POST",
      headers: { authorization: `Bearer ${await kit.token(USER, T)}`, "content-type": "application/json" },
      body: JSON.stringify({ plan: "monthly", portal_only: true }),
    });
    const response = await handleCheckout(req, config, { fetch: f.fetch, now: () => T });
    expect(((await response.json()) as Record<string, unknown>)["kind"]).toBe("portal");
  });
});

describe("a website token on the app's lease route (review s85)", () => {
  it("is refused: the lease only takes the app's token", async () => {
    const { handleLease } = await import("../src/routes/lease");
    const f = fakeFetch((req: Seen) =>
      req.url.pathname.endsWith("/access_tokens/verify") ? json(404, {}) : json(418, { unexpected: req.url.href }),
    );
    const req = new Request("https://api.example.test/v1/lease", {
      method: "POST",
      headers: { authorization: `Bearer ${await kit.token(USER, T)}`, "content-type": "application/json" },
      body: JSON.stringify({ client_now: T, app_version: "0.3.2" }),
    });
    const response = await handleLease(req, config, { fetch: f.fetch, now: () => T });
    expect(response.status).toBe(401);
    expect(f.seen.map((r) => r.url.pathname)).toEqual(["/v1/oauth_applications/access_tokens/verify"]);
  });
});

describe("the app's opaque token", () => {
  it("still goes to Clerk's OAuth verify, as before", async () => {
    const f = world(PAYER, ACTIVE);
    const req = new Request("https://api.example.test/v1/cancel", { method: "POST", headers: { authorization: "Bearer at_TOKEN" } });
    const response = await handleCancel(req, config, { fetch: f.fetch, now: () => T });
    expect(await response.json()).toEqual({ kind: "portal", url: CANCEL });
    expect(verifyCalls(f.seen).length).toBe(1);
  });
});
