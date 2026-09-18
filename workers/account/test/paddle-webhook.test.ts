// POST /paddle/webhook (SIGNIN-DESIGN.md §5, §8.30), quoted:
//   subscribed to subscription.* only. Verify Paddle-Signature (any h1, 5 s
//   tolerance), derive synchronously (re-fetch that customer's
//   subscriptions), write, then 200. Any failure -> 5xx so Paddle retries.
// §8.30: the user is taken from the subscription's custom_data.clerk_user_id,
// which a buyer's browser can set, so a record is only updated when it
// already names this Paddle customer (bound by our own /v1/checkout).
import { describe, expect, it } from "vitest";

import type { Config } from "../src/config";
import { handlePaddleWebhook } from "../src/routes/paddle-webhook";
import { DAY } from "../src/time";
import { type Seen, fakeFetch, json, rfcPkcs8 } from "./support";

const T = 1_800_000_000;
const USER = "user_2abcdefghijklmnopqrstuvwxyz0";
const CUSTOMER = "ctm_01h8441jn5pcwrfhwh78jqt8hk";
const OTHER_CUSTOMER = "ctm_01zzzzzzzzzzzzzzzzzzzzzzzz";
const PRODUCT = "pro_01aaaaaaaaaaaaaaaaaaaaaaaa";
const SECRET = "pdl_ntfset_01test_WEBHOOKSECRETvalue";

const config: Config = {
  clerk: { secretKey: "sk_test_SECRET", clientId: "client_ours", webhookSecret: "whsec_placeholder" },
  paddle: {
    apiKey: "pdl_sdbx_apikey_SECRET",
    environment: "sandbox",
    productId: PRODUCT,
    prices: { monthly: "pri_01bbbbbbbbbbbbbbbbbbbbbbbb", annual: "pri_01cccccccccccccccccccccccc" },
    webhookSecret: SECRET,
  },
  lease: { kid: "primary", pkcs8: rfcPkcs8("primary") },
  killSwitch: false,
};

const iso = (epoch: number) => new Date(epoch * 1000).toISOString();

function activeSub(): Record<string, unknown> {
  return {
    id: "sub_01eeeeeeeeeeeeeeeeeeeeeeee",
    status: "active",
    customer_id: CUSTOMER,
    custom_data: { clerk_user_id: USER },
    items: [{ price: { product_id: PRODUCT } }],
    current_billing_period: { starts_at: iso(T), ends_at: iso(T + 30 * DAY) },
    scheduled_change: null,
  };
}

function event(type = "subscription.activated", data: Record<string, unknown> = activeSub()): string {
  return JSON.stringify({ event_id: "evt_1", event_type: type, occurred_at: iso(T), notification_id: "ntf_1", data });
}

async function hmacHex(secret: string, message: string): Promise<string> {
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(message)));
  return [...mac].map((b) => b.toString(16).padStart(2, "0")).join("");
}

async function signature(body: string, ts = T, secret = SECRET): Promise<string> {
  return `ts=${ts};h1=${await hmacHex(secret, `${ts}:${body}`)}`;
}

interface World {
  metadata?: unknown;
  userGone?: boolean;
  subscriptions?: unknown[] | "down";
  writeFails?: boolean;
}

function world(w: World) {
  const writes: unknown[] = [];
  const f = fakeFetch((req: Seen) => {
    const { pathname, origin } = req.url;
    if (origin === "https://api.clerk.com" && pathname === `/v1/users/${USER}`) {
      if (w.userGone) return json(404, {});
      return json(200, { id: USER, private_metadata: w.metadata ?? { zaaheen_memory: { trial_started_at: T - 5 * DAY, paddle_customer_id: CUSTOMER, checkout_at: T - 60 } } });
    }
    if (origin === "https://api.clerk.com" && pathname === `/v1/users/${USER}/metadata`) {
      if (w.writeFails) return json(500, {});
      writes.push(JSON.parse(req.body));
      return json(200, { id: USER });
    }
    if (origin === "https://sandbox-api.paddle.com" && pathname === "/subscriptions") {
      if (w.subscriptions === "down") return json(502, {});
      return json(200, { data: w.subscriptions ?? [activeSub()], meta: { pagination: { has_more: false } } });
    }
    return json(418, { unexpected: req.url.href });
  });
  return { fetch: f.fetch, seen: f.seen, writes };
}

async function call(w: World, body: string, sig: string | null, now = T) {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (sig !== null) headers["paddle-signature"] = sig;
  const wd = world(w);
  const response = await handlePaddleWebhook(new Request("https://api.zaaheen.com/paddle/webhook", { method: "POST", headers, body }), config, {
    fetch: wd.fetch,
    now: () => now,
  });
  return { response, ...wd };
}

describe("a genuine subscription event", () => {
  it("re-fetches the customer's subscriptions, writes the derivation, then answers 200", async () => {
    const body = event();
    const { response, seen, writes } = await call({}, body, await signature(body));
    expect(response.status).toBe(200);
    expect(seen.map((r) => `${r.method} ${r.url.pathname}`)).toEqual([
      `GET /v1/users/${USER}`,
      "GET /subscriptions",
      `PATCH /v1/users/${USER}/metadata`,
    ]);
    expect(seen[1]?.url.searchParams.get("customer_id")).toBe(CUSTOMER);
    expect(writes).toEqual([{ private_metadata: { zaaheen_memory: { active_until: T + 33 * DAY, payment_failed: false, synced_at: T } } }]);
  });

  it("derives from Paddle's current list, not from the event's own copy", async () => {
    const body = event("subscription.updated", { ...activeSub(), status: "past_due" });
    const { writes } = await call({}, body, await signature(body));
    expect(writes).toEqual([{ private_metadata: { zaaheen_memory: { active_until: T + 33 * DAY, payment_failed: false, synced_at: T } } }]);
  });

  it("writes nothing when nothing changed", async () => {
    const metadata = { zaaheen_memory: { paddle_customer_id: CUSTOMER, active_until: T + 33 * DAY, payment_failed: false, synced_at: T } };
    const body = event();
    const { response, writes } = await call({ metadata }, body, await signature(body));
    expect(response.status).toBe(200);
    expect(writes).toEqual([]);
  });

  it("accepts any matching h1 while a secret is being rotated", async () => {
    const body = event();
    const good = await signature(body);
    const rotated = `ts=${T};h1=${"0".repeat(64)};${good.split(";")[1]}`;
    expect((await call({}, body, rotated)).response.status).toBe(200);
  });

  it("accepts a timestamp exactly 5 seconds away, either side", async () => {
    const body = event();
    expect((await call({}, body, await signature(body, T - 5))).response.status).toBe(200);
    expect((await call({}, body, await signature(body, T + 5))).response.status).toBe(200);
  });
});

describe("anything not provably from Paddle is refused (401) before any other call", () => {
  const cases: Array<[string, () => Promise<[string, string | null]>]> = [
    ["no signature header", async () => [event(), null]],
    ["a signature made with another secret", async () => [event(), await signature(event(), T, "pdl_ntfset_wrong")]],
    ["a body changed after signing", async () => [event().replace("activated", "canceled"), await signature(event())]],
    ["a timestamp 6 seconds old", async () => [event(), await signature(event(), T - 6)]],
    ["a timestamp 6 seconds ahead", async () => [event(), await signature(event(), T + 6)]],
    ["an h1 that is not 64 hex characters", async () => [event(), `ts=${T};h1=abc`]],
    ["no timestamp", async () => [event(), (await signature(event())).replace(/^ts=\d+;/, "")]],
    ["a non-numeric timestamp", async () => [event(), (await signature(event())).replace(/^ts=\d+/, "ts=soon")]],
    ["two timestamps", async () => [event(), `ts=${T};${await signature(event())}`]],
  ];
  for (const [name, make] of cases) {
    it(name, async () => {
      const [body, sig] = await make();
      const { response, seen } = await call({}, body, sig);
      expect(response.status).toBe(401);
      expect(seen).toHaveLength(0);
    });
  }
});

describe("genuine events that change nothing (200, so Paddle stops retrying)", () => {
  it("a non-subscription event", async () => {
    const body = event("transaction.completed", { id: "txn_1", customer_id: CUSTOMER });
    const { response, seen } = await call({}, body, await signature(body));
    expect(response.status).toBe(200);
    expect(seen).toHaveLength(0);
  });

  it("a subscription with no Clerk user on it", async () => {
    const body = event("subscription.created", { ...activeSub(), custom_data: null });
    const { response, seen } = await call({}, body, await signature(body));
    expect(response.status).toBe(200);
    expect(seen).toHaveLength(0);
  });

  it("a user who no longer exists", async () => {
    const body = event();
    const { response, seen } = await call({ userGone: true }, body, await signature(body));
    expect(response.status).toBe(200);
    expect(seen.map((r) => r.url.pathname)).toEqual([`/v1/users/${USER}`]);
  });

  it("a record that names another Paddle customer (or none) is never touched", async () => {
    for (const metadata of [{ zaaheen_memory: { paddle_customer_id: OTHER_CUSTOMER } }, { zaaheen_memory: { trial_started_at: T } }, {}]) {
      const body = event();
      const { response, seen, writes } = await call({ metadata }, body, await signature(body));
      expect(response.status).toBe(200);
      expect(seen.some((r) => r.url.pathname === "/subscriptions")).toBe(false);
      expect(writes).toEqual([]);
    }
  });
});

describe("failures ask Paddle to retry (5xx)", () => {
  it("Paddle down while re-fetching", async () => {
    const body = event();
    expect((await call({ subscriptions: "down" }, body, await signature(body))).response.status).toBe(503);
  });

  it("Clerk refusing the write", async () => {
    const body = event();
    expect((await call({ writeFails: true }, body, await signature(body))).response.status).toBe(503);
  });

  it("unreadable data for our product", async () => {
    const body = event();
    const broken = { ...activeSub(), current_billing_period: null };
    expect((await call({ subscriptions: [broken] }, body, await signature(body))).response.status).toBe(503);
  });
});

describe("the request itself", () => {
  it("only POST", async () => {
    const wd = world({});
    const response = await handlePaddleWebhook(new Request("https://api.zaaheen.com/paddle/webhook"), config, { fetch: wd.fetch, now: () => T });
    expect(response.status).toBe(405);
  });

  it("a signed body that is not JSON is 400", async () => {
    const body = "{not json";
    expect((await call({}, body, await signature(body))).response.status).toBe(400);
  });

  it("a body over 256 KB is 413, without checking anything else", async () => {
    const body = event("subscription.updated", { ...activeSub(), pad: "x".repeat(300_000) });
    const { response, seen } = await call({}, body, await signature(body));
    expect(response.status).toBe(413);
    expect(seen).toHaveLength(0);
  });
});
