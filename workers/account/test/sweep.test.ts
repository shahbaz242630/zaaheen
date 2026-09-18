// The daily cron (SIGNIN-DESIGN.md §5, quoted): "each run handles one page
// of subscriptions whose billing period ends within +-48 h and stays under
// 50 subrequests, PATCHing only changes." §8.30: the same binding rule as
// the Paddle webhook (only a record that already names the customer is
// touched), and one user's failure never stops the rest of the sweep.
import { describe, expect, it } from "vitest";

import type { Config } from "../src/config";
import { SUBREQUEST_BUDGET, sweepRenewals } from "../src/sweep";
import { DAY, HOUR } from "../src/time";
import { type Seen, fakeFetch, json, rfcPkcs8 } from "./support";

const T = 1_800_000_000;
const PRODUCT = "pro_01aaaaaaaaaaaaaaaaaaaaaaaa";
const MONTHLY = "pri_01bbbbbbbbbbbbbbbbbbbbbbbb";
const ANNUAL = "pri_01cccccccccccccccccccccccc";
const iso = (epoch: number) => new Date(epoch * 1000).toISOString();

const config: Config = {
  clerk: { secretKey: "sk_test_SECRET", clientId: "client_ours", webhookSecret: "whsec_placeholder" },
  paddle: { apiKey: "pdl_sdbx_apikey_SECRET", environment: "sandbox", productId: PRODUCT, prices: { monthly: MONTHLY, annual: ANNUAL }, webhookSecret: "pdl_ntfset_test" },
  lease: { kid: "primary", pkcs8: rfcPkcs8("primary") },
  killSwitch: false,
};

/** User n and customer n, with a subscription whose period ends at `endsAt`. */
function customer(n: number): string {
  return `ctm_01${String(n).padStart(24, "0")}`;
}
function user(n: number): string {
  return `user_2sweep${String(n).padStart(20, "0")}`;
}
function sub(n: number, endsAt: number, status = "active"): Record<string, unknown> {
  return {
    id: `sub_01${String(n).padStart(24, "0")}`,
    status,
    customer_id: customer(n),
    custom_data: { clerk_user_id: user(n) },
    items: [{ price: { product_id: PRODUCT } }],
    current_billing_period: { starts_at: iso(endsAt - 30 * DAY), ends_at: iso(endsAt) },
    scheduled_change: null,
  };
}

interface World {
  page: Record<string, unknown>[];
  hasMore?: boolean;
  /** Record per user number; default: bound to its customer, stale. */
  records?: Record<number, unknown>;
  failUser?: number;
}

function world(w: World) {
  const writes: Array<{ user: string; body: unknown }> = [];
  const f = fakeFetch((req: Seen) => {
    const { pathname, origin } = req.url;
    if (origin === "https://sandbox-api.paddle.com" && pathname === "/subscriptions") {
      const c = req.url.searchParams.get("customer_id");
      if (c !== null) return json(200, { data: w.page.filter((s) => s["customer_id"] === c), meta: { pagination: { has_more: false } } });
      return json(200, { data: w.page, meta: { pagination: { has_more: w.hasMore ?? false, next: w.hasMore ? "https://sandbox-api.paddle.com/subscriptions?after=x" : null } } });
    }
    const m = /^\/v1\/users\/(user_2sweep(\d+))(\/metadata)?$/.exec(pathname);
    if (origin === "https://api.clerk.com" && m) {
      const n = Number(m[2]);
      if (n === w.failUser) return json(500, {});
      if (m[3]) {
        writes.push({ user: m[1] ?? "", body: JSON.parse(req.body) });
        return json(200, { id: m[1] });
      }
      const record = w.records?.[n] ?? { zaaheen_memory: { paddle_customer_id: customer(n), active_until: T - DAY } };
      return json(200, { id: m[1], private_metadata: record });
    }
    return json(418, { unexpected: req.url.href });
  });
  return { ...f, writes };
}

async function run(w: World) {
  const wd = world(w);
  const result = await sweepRenewals(config, { fetch: wd.fetch, now: () => T });
  return { result, ...wd };
}

describe("the daily sweep", () => {
  it("lists one page of our active and past-due subscriptions", async () => {
    const { seen } = await run({ page: [] });
    expect(seen).toHaveLength(1);
    expect(seen[0]?.url.searchParams.get("price_id")).toBe(`${MONTHLY},${ANNUAL}`);
    expect(seen[0]?.url.searchParams.get("status")).toBe("active,past_due");
    expect(seen[0]?.url.searchParams.get("per_page")).toBe("200");
  });

  it("re-derives only subscriptions whose period ends within 48 hours either side", async () => {
    const page = [sub(1, T + 47 * HOUR), sub(2, T - 47 * HOUR), sub(3, T + 49 * HOUR), sub(4, T - 49 * HOUR), sub(5, T + 48 * HOUR)];
    const { result, writes } = await run({ page });
    expect(writes.map((x) => x.user).sort()).toEqual([user(1), user(2), user(5)].sort());
    expect(result).toMatchObject({ candidates: 3, updated: 3 });
    const one = writes.find((x) => x.user === user(1));
    expect(one?.body).toEqual({ private_metadata: { zaaheen_memory: { active_until: T + 47 * HOUR + 3 * DAY, payment_failed: false, synced_at: T } } });
  });

  it("writes nothing for a record that is already right", async () => {
    const records = { 1: { zaaheen_memory: { paddle_customer_id: customer(1), active_until: T + 47 * HOUR + 3 * DAY, payment_failed: false, synced_at: T } } };
    const { writes, result } = await run({ page: [sub(1, T + 47 * HOUR)], records });
    expect(writes).toEqual([]);
    expect(result).toMatchObject({ candidates: 1, updated: 0 });
  });

  it("never touches a record that does not name the subscription's customer", async () => {
    const records = { 1: { zaaheen_memory: { paddle_customer_id: customer(9) } }, 2: {} };
    const { writes, seen } = await run({ page: [sub(1, T + HOUR), sub(2, T + HOUR)], records });
    expect(writes).toEqual([]);
    expect(seen.filter((r) => r.url.searchParams.get("customer_id") !== null)).toEqual([]);
  });

  it("one user's failure does not stop the others", async () => {
    const { writes, result } = await run({ page: [sub(1, T + HOUR), sub(2, T + HOUR), sub(3, T + HOUR)], failUser: 2 });
    expect(writes.map((x) => x.user).sort()).toEqual([user(1), user(3)].sort());
    expect(result).toMatchObject({ candidates: 3, updated: 2, failed: 1 });
  });

  it("stays within the free plan's subrequest budget, and says how many it left", async () => {
    const page = Array.from({ length: 40 }, (_, i) => sub(i + 1, T + HOUR));
    const { seen, result } = await run({ page });
    expect(seen.length).toBeLessThanOrEqual(SUBREQUEST_BUDGET);
    expect(SUBREQUEST_BUDGET).toBeLessThan(50);
    expect(result.candidates).toBe(40);
    expect(result.updated + result.left).toBe(40);
    expect(result.left).toBeGreaterThan(0);
  });

  it("a second page is not read (one page per run)", async () => {
    const { seen } = await run({ page: [sub(1, T + HOUR)], hasMore: true });
    expect(seen.filter((r) => r.url.searchParams.get("after") !== null)).toEqual([]);
  });

  it("skips entries it cannot read instead of failing the run", async () => {
    const odd = { ...sub(1, T + HOUR), current_billing_period: null };
    const unmapped = { ...sub(2, T + HOUR), custom_data: null };
    const { result, writes } = await run({ page: [odd, unmapped, sub(3, T + HOUR)] });
    expect(writes.map((x) => x.user)).toEqual([user(3)]);
    expect(result.candidates).toBe(1);
  });
});
