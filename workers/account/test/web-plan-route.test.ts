// POST /v1/web/plan (AUTH-PAGES-DESIGN S85-1, ADR-SEC-043): the website
// account page's "Your plan". Session tokens only. Read-only: it never writes
// the record and never starts a trial (only the app's first lease does,
// ACCOUNT-DELETION-DESIGN D8). Derived as the lease derives it, live from
// Paddle when there is a customer.
import { beforeAll, describe, expect, it } from "vitest";

import type { Config } from "../src/config";
import { handleWebPlan } from "../src/routes/web-plan";
import { DAY } from "../src/time";
import { type SessionKit, sessionKit } from "./session-support";
import { type Seen, fakeFetch, json, rfcPkcs8 } from "./support";

const T = 1_800_000_000;
const USER = "user_2abcdefghijklmnopqrstuvwxyz0";
const CUSTOMER = "ctm_01h8441jn5pcwrfhwh78jqt8hk";
const PRODUCT = "pro_01aaaaaaaaaaaaaaaaaaaaaaaa";

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

function sub(status: string, opts: { ending?: boolean; endsAt?: string; startedDaysAgo?: number } = {}): unknown {
  const ends = opts.endsAt ?? new Date((T + 20 * DAY) * 1000).toISOString();
  const starts = new Date((T - (opts.startedDaysAgo ?? 10) * DAY) * 1000).toISOString();
  return {
    id: "sub_01eeeeeeeeeeeeeeeeeeeeeeee",
    status,
    items: [{ price: { product_id: PRODUCT } }],
    current_billing_period: { starts_at: starts, ends_at: ends },
    scheduled_change: opts.ending ? { action: "cancel", effective_at: ends } : null,
  };
}

interface World {
  metadata?: unknown;
  subscriptions?: unknown[] | "down";
}

function world(w: World) {
  return fakeFetch((req: Seen) => {
    const { pathname, origin } = req.url;
    if (origin === "https://api.clerk.com" && pathname === `/v1/users/${USER}` && req.method === "GET") {
      return json(200, { id: USER, primary_email_address_id: "idn_1", email_addresses: [{ id: "idn_1", email_address: "a@example.com" }], private_metadata: w.metadata ?? {} });
    }
    if (origin === "https://sandbox-api.paddle.com" && pathname === "/subscriptions") {
      if (w.subscriptions === "down") return json(502, {});
      return json(200, { data: w.subscriptions ?? [], meta: { pagination: { has_more: false } } });
    }
    return json(418, { unexpected: `${req.method} ${req.url.href}` });
  });
}

async function call(w: World, auth?: string) {
  const f = world(w);
  const token = auth ?? `Bearer ${await kit.token(USER, T)}`;
  const req = new Request("https://api.example.test/v1/web/plan", { method: "POST", headers: { authorization: token }, body: "{}" });
  const response = await handleWebPlan(req, config, { fetch: f.fetch, now: () => T });
  return { response, seen: f.seen };
}

const writes = (seen: Seen[]) => seen.filter((r) => r.method !== "GET");

describe("your plan, read-only", () => {
  it("someone who never opened the app: not started, and no trial is started", async () => {
    const { response, seen } = await call({ metadata: {} });
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("no-store");
    expect(await response.json()).toEqual({ state: "not_started" });
    expect(writes(seen)).toEqual([]);
  });

  it("a trial: the days left, a part day counting as a day", async () => {
    const { response, seen } = await call({ metadata: { zaaheen_memory: { trial_started_at: T - 10 * DAY - 3600 } } });
    expect(await response.json()).toEqual({ state: "trial", days_left: 20 });
    expect(writes(seen)).toEqual([]);
  });

  it("a trial that ran out", async () => {
    const { response } = await call({ metadata: { zaaheen_memory: { trial_started_at: T - 40 * DAY } } });
    expect(await response.json()).toEqual({ state: "ended" });
  });

  it("a payer, live from Paddle, renewing", async () => {
    const { response, seen } = await call({ metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER } }, subscriptions: [sub("active")] });
    const body = (await response.json()) as Record<string, unknown>;
    expect(body["state"]).toBe("active");
    expect(body["ending"]).toBe(false);
    expect(body["paying"]).toBe(true);
    expect(typeof body["until"]).toBe("number");
    expect(writes(seen)).toEqual([]);
  });

  it("a payer who cancelled: still active, ending on the date", async () => {
    const endsAt = new Date((T + 20 * DAY) * 1000).toISOString();
    const { response } = await call({ metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER } }, subscriptions: [sub("active", { ending: true, endsAt })] });
    expect(await response.json()).toEqual({ state: "active", ending: true, until: T + 20 * DAY, paying: true });
  });

  it("one subscription ending and another renewing: not ending (review s85)", async () => {
    const two = [sub("active", { ending: true }), { ...(sub("active") as Record<string, unknown>), id: "sub_01ffffffffffffffffffffffff" }];
    const { response } = await call({ metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER } }, subscriptions: two });
    const body = (await response.json()) as Record<string, unknown>;
    expect(body["ending"]).toBe(false);
    expect(body["paying"]).toBe(true);
  });

  it("time given without a subscription: active, but nothing to manage or cancel", async () => {
    const { response, seen } = await call({ metadata: { zaaheen_memory: { comp_until: T + 30 * DAY } } });
    expect(await response.json()).toEqual({ state: "active", until: T + 30 * DAY, ending: false, paying: false });
    expect(writes(seen)).toEqual([]);
  });

  it("a payment that failed, inside the 7-day grace", async () => {
    const { response } = await call({ metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER } }, subscriptions: [sub("past_due", { startedDaysAgo: 2 })] });
    expect(((await response.json()) as Record<string, unknown>)["state"]).toBe("payment_failed");
  });

  it("a payment that failed, past the grace: ended", async () => {
    const { response } = await call({ metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER } }, subscriptions: [sub("past_due", { startedDaysAgo: 10 })] });
    expect(await response.json()).toEqual({ state: "ended" });
  });

  it("Paddle down is a 503, never a made-up plan", async () => {
    const { response } = await call({ metadata: { zaaheen_memory: { paddle_customer_id: CUSTOMER } }, subscriptions: "down" });
    expect(response.status).toBe(503);
  });
});

describe("who may ask", () => {
  it("the app's opaque token is refused here, and Clerk is never asked to verify it", async () => {
    const { response, seen } = await call({}, "Bearer at_OPAQUE_APP_TOKEN");
    expect(response.status).toBe(401);
    expect(seen).toEqual([]);
  });

  it("no token is a 401", async () => {
    const { response } = await call({}, "");
    expect(response.status).toBe(401);
  });

  it("a Worker with no website settings answers 503", async () => {
    const f = world({});
    const { web: _web, ...noWeb } = config;
    const req = new Request("https://api.example.test/v1/web/plan", { method: "POST", headers: { authorization: `Bearer ${await kit.token(USER, T)}` } });
    const response = await handleWebPlan(req, noWeb, { fetch: f.fetch, now: () => T });
    expect(response.status).toBe(503);
  });
});
