// POST /v1/lease end to end, against fake Clerk and Paddle (SIGNIN-DESIGN.md
// §5 "/v1/lease", the §8.27 contract, §8.30). The lease that comes back is
// verified with the RFC 8032 public key, exactly as the app would.
import { describe, expect, it } from "vitest";

import type { Config } from "../src/config";
import { LEASE_DOMAIN } from "../src/lease";
import { handleLease } from "../src/routes/lease";
import { DAY } from "../src/time";
import { type Seen, fakeFetch, json, rfcPkcs8 } from "./support";
import vectors from "./vectors/lease-v1.json";

const T = 1_800_000_000;
const USER = "user_2abcdefghijklmnopqrstuvwxyz0";
const CUSTOMER = "ctm_01h8441jn5pcwrfhwh78jqt8hk";
const PRODUCT = "pro_01aaaaaaaaaaaaaaaaaaaaaaaa";

const config: Config = {
  clerk: { secretKey: "sk_test_SECRET", clientId: "client_ours", webhookSecret: "whsec_placeholder" },
  paddle: { apiKey: "pdl_sdbx_apikey_SECRET", environment: "sandbox", productId: PRODUCT, prices: { monthly: "pri_01bbbbbbbbbbbbbbbbbbbbbbbb", annual: "pri_01cccccccccccccccccccccccc" }, webhookSecret: "pdl_ntfset_test" },
  lease: { kid: "primary", pkcs8: rfcPkcs8("primary") },
  killSwitch: false,
};

interface World {
  token?: "valid" | "refused" | "clerk-down";
  metadata?: unknown;
  userGone?: boolean;
  subscriptions?: unknown[] | "down";
  writeFails?: boolean;
}

function world(w: World) {
  const writes: unknown[] = [];
  const f = fakeFetch((req: Seen) => {
    const { pathname, origin } = req.url;
    if (origin === "https://api.clerk.com" && pathname === "/v1/oauth_applications/access_tokens/verify") {
      if (w.token === "clerk-down") return json(503, {});
      if (w.token === "refused") return json(404, { errors: [] });
      return json(200, {
        object: "clerk_idp_oauth_access_token",
        id: "oat_1",
        client_id: "client_ours",
        subject: USER,
        scopes: [],
        revoked: false,
        revocation_reason: null,
        expired: false,
        expiration: T + DAY,
        created_at: T,
        updated_at: T,
      });
    }
    if (origin === "https://api.clerk.com" && pathname === `/v1/users/${USER}`) {
      if (w.userGone) return json(404, { errors: [] });
      return json(200, { id: USER, primary_email_address_id: null, email_addresses: [], private_metadata: w.metadata ?? {} });
    }
    if (origin === "https://api.clerk.com" && pathname === `/v1/users/${USER}/metadata`) {
      if (w.writeFails) return json(500, {});
      writes.push(JSON.parse(req.body));
      return json(200, { id: USER });
    }
    if (origin === "https://sandbox-api.paddle.com" && pathname === "/subscriptions") {
      if (w.subscriptions === "down") return json(502, {});
      return json(200, { data: w.subscriptions ?? [], meta: { pagination: { has_more: false, next: null } } });
    }
    return json(418, { unexpected: req.url.href });
  });
  return { fetch: f.fetch, seen: f.seen, writes };
}

function request(body: unknown, headers: Record<string, string> = {}): Request {
  return new Request("https://api.zaaheen.com/v1/lease", {
    method: "POST",
    headers: { authorization: "Bearer at_TOKEN", "content-type": "application/json", ...headers },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });
}

const goodBody = { client_now: T - 7, app_version: "0.3.0" };

async function call(w: World, req: Request = request(goodBody), cfg: Config = config) {
  const wd = world(w);
  const response = await handleLease(req, cfg, { fetch: wd.fetch, now: () => T });
  return { response, ...wd };
}

function fromBase64url(text: string): Uint8Array {
  return Uint8Array.from(atob(text.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
}

/** Verify the returned lease exactly as vault-account does, and return its payload. */
async function verified(response: Response): Promise<Record<string, unknown>> {
  expect(response.status).toBe(200);
  const body = (await response.json()) as { lease: string };
  const [p, s] = body.lease.split(".");
  const payload = fromBase64url(p ?? "");
  const message = new Uint8Array([...new TextEncoder().encode(LEASE_DOMAIN), ...payload]);
  const key = await crypto.subtle.importKey("raw", fromBase64url(vectors.public_keys.primary), { name: "Ed25519" }, false, ["verify"]);
  expect(await crypto.subtle.verify({ name: "Ed25519" }, key, fromBase64url(s ?? ""), message)).toBe(true);
  return JSON.parse(new TextDecoder().decode(payload)) as Record<string, unknown>;
}

describe("a good request", () => {
  it("a first call starts the trial and returns a lease the app accepts", async () => {
    const { response, writes } = await call({});
    expect(response.headers.get("content-type")).toContain("application/json");
    expect(response.headers.get("cache-control")).toBe("no-store");
    expect(await verified(response)).toEqual({
      v: 1,
      kid: "primary",
      sub: USER,
      state: "trial",
      trial_ends_at: T + 30 * DAY,
      issued_at: T,
      client_time: T - 7,
      offline_days: 30,
    });
    expect(writes).toEqual([{ private_metadata: { zaaheen_memory: { trial_started_at: T } } }]);
  });

  it("signs the client's clock as received, however far off (§4)", async () => {
    const { response } = await call({}, request({ client_now: 1_000, app_version: "0.3.0" }));
    expect((await verified(response))["client_time"]).toBe(1_000);
  });

  it("a paying user is active, and Paddle is not asked when nothing calls for it", async () => {
    const metadata = { zaaheen_memory: { trial_started_at: T - 40 * DAY, paddle_customer_id: CUSTOMER, active_until: T + 20 * DAY } };
    const { response, seen, writes } = await call({ metadata, subscriptions: "down" });
    expect(await verified(response)).toMatchObject({ state: "active", active_until: T + 20 * DAY });
    expect(seen.some((r) => r.url.origin.includes("paddle"))).toBe(false);
    expect(writes).toEqual([]);
  });

  it("just paid: asks Paddle and signs active", async () => {
    const metadata = { zaaheen_memory: { trial_started_at: T - 5 * DAY, paddle_customer_id: CUSTOMER, checkout_at: T - 60 } };
    const subscriptions = [
      {
        status: "active",
        items: [{ price: { product_id: PRODUCT } }],
        current_billing_period: { starts_at: new Date(T * 1000).toISOString(), ends_at: new Date((T + 30 * DAY) * 1000).toISOString() },
        scheduled_change: null,
      },
    ];
    const { response } = await call({ metadata, subscriptions });
    expect(await verified(response)).toMatchObject({ state: "active", active_until: T + 33 * DAY });
  });

  it("the kill switch signs every Paddle customer active for 3 days", async () => {
    const metadata = { zaaheen_memory: { trial_started_at: T - 400 * DAY, paddle_customer_id: CUSTOMER, active_until: T - 300 * DAY } };
    const { response, seen } = await call({ metadata, subscriptions: "down" }, request(goodBody), { ...config, killSwitch: true });
    expect(await verified(response)).toMatchObject({ state: "active", active_until: T + 3 * DAY });
    expect(seen.some((r) => r.url.origin.includes("paddle"))).toBe(false);
  });
});

describe("refusals (401) and try-later answers (503)", () => {
  it("no or malformed Authorization is 401, without calling Clerk", async () => {
    for (const headers of [{ authorization: "" }, { authorization: "Basic abc" }, { authorization: "Bearer " }, { authorization: "Bearer a b" }]) {
      const { response, seen } = await call({}, request(goodBody, headers));
      expect(response.status).toBe(401);
      expect(seen).toHaveLength(0);
    }
  });

  it("a token Clerk refuses is 401", async () => {
    expect((await call({ token: "refused" })).response.status).toBe(401);
  });

  it("a user who no longer exists is 401", async () => {
    expect((await call({ userGone: true })).response.status).toBe(401);
  });

  it("Clerk failing is 503, never 401", async () => {
    expect((await call({ token: "clerk-down" })).response.status).toBe(503);
  });

  it("Paddle down for someone who never paid is 503; for a payer it is a signed 'active'", async () => {
    const never = { zaaheen_memory: { trial_started_at: T - 40 * DAY, paddle_customer_id: CUSTOMER } };
    expect((await call({ metadata: never, subscriptions: "down" })).response.status).toBe(503);
    const payer = { zaaheen_memory: { trial_started_at: T - 40 * DAY, paddle_customer_id: CUSTOMER, active_until: T - DAY } };
    expect(await verified((await call({ metadata: payer, subscriptions: "down" })).response)).toMatchObject({ state: "active", active_until: T + 2 * DAY });
  });

  it("a new user whose trial start cannot be saved is 503", async () => {
    expect((await call({ writeFails: true })).response.status).toBe(503);
  });

  it("an unusable signing key is 503", async () => {
    const { response } = await call({}, request(goodBody), { ...config, lease: { kid: "primary", pkcs8: "bm90IGEga2V5" } });
    expect(response.status).toBe(503);
  });

  it("error bodies are fixed text that never echo a secret or the token", async () => {
    for (const w of [{ token: "clerk-down" as const }, { token: "refused" as const }]) {
      const text = await (await call(w)).response.text();
      expect(text).not.toContain("SECRET");
      expect(text).not.toContain("at_TOKEN");
    }
  });
});

describe("the request itself", () => {
  it("only POST is allowed", async () => {
    const { response, seen } = await call({}, new Request("https://api.zaaheen.com/v1/lease", { method: "GET" }));
    expect(response.status).toBe(405);
    expect(seen).toHaveLength(0);
  });

  const bad: Array<[string, unknown]> = [
    ["not JSON", "{client_now"],
    ["an array", "[]"],
    ["no client_now", { app_version: "0.3.0" }],
    ["a string client_now", { client_now: "1800000000", app_version: "0.3.0" }],
    ["a fractional client_now", { client_now: 1.5, app_version: "0.3.0" }],
    ["client_now 0", { client_now: 0, app_version: "0.3.0" }],
    ["a negative client_now", { client_now: -1, app_version: "0.3.0" }],
    ["an unsafe client_now", { client_now: 2 ** 53, app_version: "0.3.0" }],
    ["no app_version", { client_now: T }],
    ["an empty app_version", { client_now: T, app_version: "" }],
    ["an app_version with a space", { client_now: T, app_version: "0.3 beta" }],
    ["a 33-character app_version", { client_now: T, app_version: "1".repeat(33) }],
    ["a body over 1 KB", { client_now: T, app_version: "0.3.0", pad: "x".repeat(2000) }],
  ];
  for (const [name, body] of bad) {
    it(`${name} is 400, without calling Clerk`, async () => {
      const { response, seen } = await call({}, request(body));
      expect(response.status).toBe(400);
      expect(seen).toHaveLength(0);
    });
  }

  it("the app's exact request shape is accepted, and unknown fields are ignored", async () => {
    expect((await call({}, request({ client_now: T, app_version: "1.2.3-beta+4" }))).response.status).toBe(200);
    expect((await call({}, request({ client_now: T, app_version: "0.3.0", extra: true }))).response.status).toBe(200);
  });
});
