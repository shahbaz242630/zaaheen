// The daily cron (SIGNIN-DESIGN.md §5, quoted): "each run handles one page
// of subscriptions whose billing period ends within +-48 h and stays under
// 50 subrequests, PATCHing only changes." §8.30: the same binding rule as
// the Paddle webhook (only a record that already names the customer is
// touched), and one user's failure never stops the rest of the sweep.
// ACCOUNT-DELETION-DESIGN D6 adds the backstop: a subscription whose Clerk
// user is gone (404) is cancelled at once; any other Clerk failure cancels
// nothing.
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Config } from "../src/config";
import { MAX_GONE_PER_RUN, SUBREQUEST_BUDGET, sweepRenewals } from "../src/sweep";
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
  trials: { key: "k".repeat(32) },
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
  /** Users Clerk answers 404 for (deleted). */
  goneUsers?: number[];
  /** Subscriptions whose cancel Paddle refuses. */
  failCancel?: string[];
  /** Client ids of the OAuth applications the key's instance lists (default: ours). */
  instanceApps?: string[];
  /** A status for the instance check other than 200. */
  instanceStatus?: number;
}

function world(w: World) {
  const writes: Array<{ user: string; body: unknown }> = [];
  const cancels: Array<{ id: string; body: unknown }> = [];
  let instanceChecks = 0;
  const f = fakeFetch((req: Seen) => {
    const { pathname, origin } = req.url;
    const cancel = /^\/subscriptions\/(sub_[a-z0-9]+)\/cancel$/.exec(pathname);
    if (origin === "https://sandbox-api.paddle.com" && cancel && req.method === "POST") {
      const id = cancel[1] ?? "";
      if (w.failCancel?.includes(id)) return json(500, {});
      cancels.push({ id, body: JSON.parse(req.body) });
      return json(200, { data: { id, status: "canceled" } });
    }
    if (origin === "https://sandbox-api.paddle.com" && pathname === "/subscriptions") {
      const c = req.url.searchParams.get("customer_id");
      if (c !== null) return json(200, { data: w.page.filter((s) => s["customer_id"] === c), meta: { pagination: { has_more: false } } });
      return json(200, { data: w.page, meta: { pagination: { has_more: w.hasMore ?? false, next: w.hasMore ? "https://sandbox-api.paddle.com/subscriptions?after=x" : null } } });
    }
    if (origin === "https://api.clerk.com" && pathname === "/v1/oauth_applications" && req.method === "GET") {
      instanceChecks += 1;
      if (w.instanceStatus !== undefined) return json(w.instanceStatus, {});
      const apps = (w.instanceApps ?? ["client_ours"]).map((id) => ({ object: "oauth_application", client_id: id }));
      return json(200, { data: apps, total_count: apps.length });
    }
    const m = /^\/v1\/users\/(user_2sweep(\d+))(\/metadata)?$/.exec(pathname);
    if (origin === "https://api.clerk.com" && m) {
      const n = Number(m[2]);
      if (n === w.failUser) return json(500, {});
      if (w.goneUsers?.includes(n)) return json(404, { errors: [{ code: "resource_not_found" }] });
      if (m[3]) {
        writes.push({ user: m[1] ?? "", body: JSON.parse(req.body) });
        return json(200, { id: m[1] });
      }
      const record = w.records?.[n] ?? { zaaheen_memory: { paddle_customer_id: customer(n), active_until: T - DAY } };
      return json(200, { id: m[1], private_metadata: record });
    }
    return json(418, { unexpected: req.url.href });
  });
  return { ...f, writes, cancels, instanceChecks: () => instanceChecks };
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

describe("the deleted-account backstop (ACCOUNT-DELETION-DESIGN D6)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("cancels at once a renewing subscription whose Clerk user is gone", async () => {
    const { cancels, writes, result } = await run({ page: [sub(1, T + HOUR)], goneUsers: [1] });
    expect(cancels).toEqual([{ id: sub(1, T)["id"], body: { effective_from: "immediately" } }]);
    expect(writes).toEqual([]);
    expect(result).toMatchObject({ candidates: 1, cancelled: 1, updated: 0, skipped: 0, failed: 0 });
  });

  it("leaves alone a subscription whose user exists, bound or not", async () => {
    const records = { 2: { zaaheen_memory: { paddle_customer_id: customer(9) } } };
    const { cancels, result } = await run({ page: [sub(1, T + HOUR), sub(2, T + HOUR)], records });
    expect(cancels).toEqual([]);
    expect(result).toMatchObject({ cancelled: 0, updated: 1, skipped: 1 });
  });

  it("a Clerk failure other than 404 cancels nothing and counts as a failure", async () => {
    const { cancels, result } = await run({ page: [sub(1, T + HOUR), sub(2, T + HOUR)], failUser: 1, goneUsers: [2] });
    expect(cancels.map((c) => c.id)).toEqual([sub(2, T)["id"]]);
    expect(result).toMatchObject({ cancelled: 1, failed: 1 });
  });

  it("cancels every renewing subscription the gone user has on the page", async () => {
    const second = { ...sub(1, T - HOUR), id: `sub_02${"1".padStart(24, "0")}` };
    const { cancels, result } = await run({ page: [sub(1, T + HOUR), second], goneUsers: [1] });
    expect(cancels.map((c) => c.id).sort()).toEqual([sub(1, T)["id"], second.id].sort());
    expect(result).toMatchObject({ candidates: 1, cancelled: 2 });
  });

  it("a refused cancel is a failure for that user only", async () => {
    const one = sub(1, T + HOUR);
    const { cancels, result } = await run({ page: [one, sub(2, T + HOUR)], goneUsers: [1, 2], failCancel: [one["id"] as string] });
    expect(cancels.map((c) => c.id)).toEqual([sub(2, T)["id"]]);
    expect(result).toMatchObject({ cancelled: 1, failed: 1 });
  });

  it("stays within the subrequest budget however many subscriptions the gone users have", async () => {
    const page: Record<string, unknown>[] = [];
    for (let n = 1; n <= 3; n += 1) {
      for (let k = 0; k < 8; k += 1) page.push({ ...sub(n, T + HOUR), id: `sub_0${k}${String(n).padStart(24, "0")}` });
    }
    for (let n = 4; n <= 20; n += 1) page.push(sub(n, T + HOUR));
    const { seen, result } = await run({ page, goneUsers: [1, 2, 3] });
    expect(seen.length).toBeLessThanOrEqual(SUBREQUEST_BUDGET);
    expect(result.cancelled).toBeGreaterThan(0);
    expect(result.left).toBeGreaterThan(0);
  });

  // Session 67: a Clerk key from the wrong instance makes every user look
  // gone. Real deletions arrive one or two a day; many in one run means
  // something is misconfigured, so the run cancels none of them and says so.
  it(`cancels up to ${MAX_GONE_PER_RUN} gone accounts in one run`, async () => {
    const page = Array.from({ length: MAX_GONE_PER_RUN }, (_, i) => sub(i + 1, T + HOUR));
    const { cancels, result } = await run({ page, goneUsers: page.map((_, i) => i + 1) });
    expect(cancels).toHaveLength(MAX_GONE_PER_RUN);
    expect(result).toMatchObject({ cancelled: MAX_GONE_PER_RUN, held: 0 });
  });

  it("with more gone accounts than that in one run, cancels none of them and warns", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const n = MAX_GONE_PER_RUN + 1;
    const page = [...Array.from({ length: n }, (_, i) => sub(i + 1, T + HOUR)), sub(n + 1, T + HOUR)];
    const { cancels, writes, result } = await run({ page, goneUsers: Array.from({ length: n }, (_, i) => i + 1) });
    expect(cancels).toEqual([]);
    expect(result).toMatchObject({ cancelled: 0, held: n, updated: 1 });
    // The live user is still re-derived: the hold is only for cancels.
    expect(writes.map((x) => x.user)).toEqual([user(n + 1)]);
    const events = warn.mock.calls.map((args) => (JSON.parse(String(args[0])) as Record<string, unknown>)["event"]);
    expect(events).toContain("gone_guard");
  });

  // Session 68 (review M1): at launch there are three or fewer renewals a
  // day, so the count guard alone never trips. Before any cancel, the key must
  // prove it belongs to our instance (its OAuth application list names our
  // client id); otherwise nothing is cancelled.
  for (let n = 1; n <= MAX_GONE_PER_RUN; n += 1) {
    it(`a wrong-instance key with ${n} renewing account(s) cancels nothing and warns`, async () => {
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
      const page = Array.from({ length: n }, (_, i) => sub(i + 1, T + HOUR));
      const { cancels, result } = await run({ page, goneUsers: page.map((_, i) => i + 1), instanceApps: ["client_other_instance"] });
      expect(cancels).toEqual([]);
      expect(result).toMatchObject({ cancelled: 0, held: n });
      const events = warn.mock.calls.map((args) => (JSON.parse(String(args[0])) as Record<string, unknown>)["event"]);
      expect(events).toContain("gone_guard");
    });
  }

  it("an instance check that fails (not 200) cancels nothing", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    for (const status of [401, 403, 429, 500]) {
      const { cancels, result } = await run({ page: [sub(1, T + HOUR)], goneUsers: [1], instanceStatus: status });
      expect(cancels).toEqual([]);
      expect(result).toMatchObject({ cancelled: 0, held: 1 });
    }
  });

  it("checks the instance once, before the first cancel, and only when someone is gone", async () => {
    const none = await run({ page: [sub(1, T + HOUR), sub(2, T + HOUR)] });
    expect(none.instanceChecks()).toBe(0);
    const some = await run({ page: [sub(1, T + HOUR), sub(2, T + HOUR)], goneUsers: [1, 2] });
    expect(some.instanceChecks()).toBe(1);
    const order = some.seen.map((r) => r.url.pathname);
    const check = order.indexOf("/v1/oauth_applications");
    const firstCancel = order.findIndex((p) => p.endsWith("/cancel"));
    expect(check).toBeGreaterThanOrEqual(0);
    expect(check).toBeLessThan(firstCancel);
  });

  it("the instance check fits in the budget when the page is full of gone users", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const page = Array.from({ length: 40 }, (_, i) => sub(i + 1, T + HOUR));
    const { seen } = await run({ page, goneUsers: [1, 2, 3] });
    expect(seen.length).toBeLessThanOrEqual(SUBREQUEST_BUDGET);
  });

  it("the instance check is reserved at the exact edge of the budget", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    // 1 list + gone user A (1 read, 1 cancel reserved) + 13 live users (3 calls
    // each) = 42 used or reserved; gone user B (1 read, 2 cancels) would take
    // it to exactly 45, leaving no room for the check.
    const page: Record<string, unknown>[] = [sub(1, T + HOUR)];
    for (let n = 2; n <= 14; n += 1) page.push(sub(n, T + HOUR));
    page.push(sub(15, T + HOUR), { ...sub(15, T - HOUR), id: `sub_02${"15".padStart(24, "0")}` });
    const { seen, result } = await run({ page, goneUsers: [1, 15] });
    expect(seen.length).toBeLessThanOrEqual(SUBREQUEST_BUDGET);
    expect(result.left).toBe(1);
  });

  it("a gone account with no usable subscription id counts as a failure", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const odd = { ...sub(1, T + HOUR), id: "not-a-subscription-id" };
    const { cancels, result } = await run({ page: [odd], goneUsers: [1] });
    expect(cancels).toEqual([]);
    expect(result).toMatchObject({ cancelled: 0, failed: 1 });
  });

  it("the guard is small and fixed", () => {
    expect(MAX_GONE_PER_RUN).toBe(3);
  });

  it("logs a count, never a user or subscription id", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await run({ page: [sub(1, T + HOUR), sub(2, T + HOUR)], goneUsers: [1], failUser: 2 });
    const lines = warn.mock.calls.map((args) => args.map(String).join(" "));
    const done = lines.map((l) => JSON.parse(l) as Record<string, unknown>).find((l) => l["event"] === "done");
    expect(done).toMatchObject({ route: "sweep", cancelled: 1 });
    for (const line of lines) {
      expect(line).not.toContain("user_2sweep");
      expect(line).not.toContain("sub_0");
      expect(line).not.toContain("ctm_0");
    }
  });
});
