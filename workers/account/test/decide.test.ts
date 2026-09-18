// What `/v1/lease` signs (SIGNIN-DESIGN.md §5 "/v1/lease", §8.28, §8.29).
// Pure decisions plus `decideLease`, which runs them against a fake Paddle
// and a fake Clerk write so every upstream failure path is exercised.
import { describe, expect, it } from "vitest";

import {
  type LeaseDeps,
  decideLease,
  liveFetchWanted,
  termsFrom,
  termsOnUpstreamError,
} from "../src/decide";
import type { BillingRecord } from "../src/record";
import { DAY, MINUTE } from "../src/time";

const T = 1_800_000_000;
const HOUR = 60 * MINUTE;
const CUSTOMER = "ctm_01h8441jn5pcwrfhwh78jqt8hk";
const OURS = "pro_01ourmemoryapp00000000000a";

function iso(epoch: number): string {
  return new Date(epoch * 1000).toISOString();
}

function activeSub(endsAt: number): unknown {
  return {
    status: "active",
    items: [{ price: { product_id: OURS } }],
    current_billing_period: { starts_at: iso(endsAt - 30 * DAY), ends_at: iso(endsAt) },
    scheduled_change: null,
  };
}

describe("termsFrom: the state a record means right now", () => {
  const start = T - 10 * DAY;

  it("a trial runs 30 days from its start, to the second", () => {
    expect(termsFrom({ trial_started_at: start }, T)).toEqual({ state: "trial", trial_ends_at: start + 30 * DAY });
    const end = start + 30 * DAY;
    expect(termsFrom({ trial_started_at: start }, end - 1).state).toBe("trial");
    expect(termsFrom({ trial_started_at: start }, end)).toEqual({ state: "ended", trial_ends_at: end });
  });

  it("a paid period is active until its end", () => {
    const r: BillingRecord = { trial_started_at: start, active_until: T + DAY };
    expect(termsFrom(r, T)).toEqual({ state: "active", active_until: T + DAY, trial_ends_at: start + 30 * DAY });
    expect(termsFrom({ ...r, payment_failed: true }, T).state).toBe("payment_failed");
  });

  it("a comp date extends access beyond the paid period, and works alone", () => {
    const r: BillingRecord = { trial_started_at: start - 100 * DAY, active_until: T - DAY, comp_until: T + 60 * DAY };
    expect(termsFrom(r, T)).toMatchObject({ state: "active", active_until: T + 60 * DAY });
    expect(termsFrom({ trial_started_at: start - 100 * DAY, comp_until: T + DAY }, T)).toMatchObject({
      state: "active",
      active_until: T + DAY,
    });
  });

  it("after paying, 'ended' carries the paid end, so the app says 'subscription ended' (§8.27)", () => {
    const r: BillingRecord = { trial_started_at: start - 100 * DAY, active_until: T - DAY };
    expect(termsFrom(r, T)).toEqual({ state: "ended", active_until: T - DAY, trial_ends_at: start - 70 * DAY });
  });

  it("a comp that ran out is not a payment: 'ended' without active_until ('trial ended')", () => {
    const r: BillingRecord = { trial_started_at: start - 100 * DAY, comp_until: T - DAY };
    expect(termsFrom(r, T)).not.toHaveProperty("active_until");
    expect(termsFrom(r, T).state).toBe("ended");
  });

  it("a subscription that ended inside the 30 days falls back to the rest of the trial", () => {
    const r: BillingRecord = { trial_started_at: start, active_until: T - DAY };
    expect(termsFrom(r, T)).toMatchObject({ state: "trial", trial_ends_at: start + 30 * DAY });
  });
});

describe("liveFetchWanted: when to ask Paddle before answering", () => {
  const payer: BillingRecord = { trial_started_at: T - 100 * DAY, paddle_customer_id: CUSTOMER };

  it("never for a plain trial user", () => {
    expect(liveFetchWanted({ trial_started_at: T - DAY }, T)).toBe(false);
  });

  it("(a) a paid period that has run out, until a sync has happened since", () => {
    const expired: BillingRecord = { ...payer, active_until: T - DAY, synced_at: T - 20 * DAY };
    expect(liveFetchWanted(expired, T)).toBe(true);
    expect(liveFetchWanted({ ...expired, synced_at: T - DAY }, T)).toBe(false);
    expect(liveFetchWanted({ ...expired, synced_at: T - HOUR }, T)).toBe(false);
  });

  it("(b) a Paddle customer about to be told 'ended', until a sync has happened since a paid period ended", () => {
    expect(liveFetchWanted(payer, T)).toBe(true);
    // Never had a paid period: a past sync proves nothing (the payment may
    // have landed since, with its webhook lost), so keep asking, inside the
    // rate limit (§8.29, found by the step-1 review).
    expect(liveFetchWanted({ ...payer, synced_at: T - 5 * DAY }, T)).toBe(true);
    expect(liveFetchWanted({ ...payer, synced_at: T - 5 * DAY, live_fetch_at: T - 5 * MINUTE }, T)).toBe(false);
    // Had one, and synced since it ended: Paddle has already said it is over.
    expect(liveFetchWanted({ ...payer, active_until: T - 3 * DAY, synced_at: T - 2 * DAY }, T)).toBe(false);
  });

  it("(c) just after checkout, even when already synced, unless already active", () => {
    // In the trial, so (a) and (b) cannot fire: only (c) is being tested.
    const paying: BillingRecord = {
      trial_started_at: T - 5 * DAY,
      paddle_customer_id: CUSTOMER,
      checkout_at: T - 2 * HOUR,
      synced_at: T - HOUR,
    };
    expect(liveFetchWanted(paying, T)).toBe(true);
    expect(liveFetchWanted({ ...paying, checkout_at: T - 25 * HOUR }, T)).toBe(false);
    expect(liveFetchWanted({ ...paying, active_until: T + 30 * DAY }, T)).toBe(false);
    // A former payer resubscribing, synced since the old period ended, so
    // (a) and (b) are skipped: (c) must still ask.
    const returning: BillingRecord = {
      trial_started_at: T - 100 * DAY,
      paddle_customer_id: CUSTOMER,
      active_until: T - 3 * DAY,
      synced_at: T - 2 * DAY,
      checkout_at: T - 2 * HOUR,
    };
    expect(liveFetchWanted(returning, T)).toBe(true);
    expect(liveFetchWanted({ ...returning, checkout_at: T - 25 * HOUR }, T)).toBe(false);
  });

  it("at most once per 10 minutes per user", () => {
    const r: BillingRecord = { ...payer, checkout_at: T - 2 * HOUR };
    expect(liveFetchWanted({ ...r, live_fetch_at: T - 9 * MINUTE }, T)).toBe(false);
    expect(liveFetchWanted({ ...r, live_fetch_at: T - 10 * MINUTE }, T)).toBe(true);
  });

  it("every 30 seconds while the checkout is under an hour old", () => {
    const r: BillingRecord = { ...payer, checkout_at: T - 20 * MINUTE };
    expect(liveFetchWanted({ ...r, live_fetch_at: T - 29 }, T)).toBe(false);
    expect(liveFetchWanted({ ...r, live_fetch_at: T - 30 }, T)).toBe(true);
  });

  it("a last-fetch time in the future counts as no fetch", () => {
    expect(liveFetchWanted({ ...payer, live_fetch_at: T + HOUR }, T)).toBe(true);
  });
});

describe("termsOnUpstreamError: never a signed 'ended' because Paddle or Clerk failed", () => {
  it("someone who has paid is signed active until the stored end plus 3 days", () => {
    const r: BillingRecord = { trial_started_at: T - 100 * DAY, active_until: T - DAY, payment_failed: true };
    expect(termsOnUpstreamError(r)).toEqual({ state: "active", active_until: T + 2 * DAY, trial_ends_at: T - 70 * DAY });
  });

  it("a later comp date is kept", () => {
    const r: BillingRecord = { active_until: T - DAY, comp_until: T + 50 * DAY };
    expect(termsOnUpstreamError(r)).toMatchObject({ state: "active", active_until: T + 50 * DAY });
  });

  it("anyone else gets no lease (the Worker answers 503)", () => {
    expect(termsOnUpstreamError({ trial_started_at: T - DAY, paddle_customer_id: CUSTOMER })).toBeNull();
    expect(termsOnUpstreamError({})).toBeNull();
  });
});

// ---- decideLease -------------------------------------------------------------------

interface Fake {
  deps: LeaseDeps;
  fetches: string[];
  writes: Array<Record<string, unknown>>;
}

function fake(
  opts: {
    now?: number;
    killSwitch?: boolean;
    subs?: unknown[] | "fail";
    write?: "fail";
  } = {},
): Fake {
  const fetches: string[] = [];
  const writes: Array<Record<string, unknown>> = [];
  const deps: LeaseDeps = {
    now: opts.now ?? T,
    killSwitch: opts.killSwitch ?? false,
    productId: OURS,
    fetchSubscriptions: async (customerId) => {
      fetches.push(customerId);
      if (opts.subs === "fail") throw new Error("paddle 502");
      return opts.subs ?? [];
    },
    writeRecord: async (patch) => {
      if (opts.write === "fail") throw new Error("clerk 500");
      writes.push(patch);
    },
  };
  return { deps, fetches, writes };
}

describe("decideLease", () => {
  it("a first call starts the trial and saves its start", async () => {
    const f = fake();
    expect(await decideLease({}, f.deps)).toEqual({
      kind: "lease",
      terms: { state: "trial", trial_ends_at: T + 30 * DAY },
    });
    expect(f.writes).toEqual([{ trial_started_at: T }]);
    expect(f.fetches).toEqual([]);
  });

  it("a trial whose start could not be saved is never signed", async () => {
    const f = fake({ write: "fail" });
    expect(await decideLease({}, f.deps)).toEqual({ kind: "unavailable" });
  });

  it("an unchanged record is not written", async () => {
    const f = fake();
    await decideLease({ trial_started_at: T - DAY }, f.deps);
    expect(f.writes).toEqual([]);
  });

  it("just paid: asks Paddle, signs active, saves what it learned", async () => {
    const f = fake({ subs: [activeSub(T + 30 * DAY)] });
    const r: BillingRecord = { trial_started_at: T - 5 * DAY, paddle_customer_id: CUSTOMER, checkout_at: T - MINUTE };
    expect(await decideLease(r, f.deps)).toEqual({
      kind: "lease",
      terms: { state: "active", active_until: T + 33 * DAY, trial_ends_at: T + 25 * DAY },
    });
    expect(f.fetches).toEqual([CUSTOMER]);
    expect(f.writes).toEqual([{ active_until: T + 33 * DAY, payment_failed: false, synced_at: T, live_fetch_at: T }]);
  });

  it("does not ask Paddle again inside the rate limit", async () => {
    const f = fake({ subs: [activeSub(T + 30 * DAY)] });
    const r: BillingRecord = {
      trial_started_at: T - 5 * DAY,
      paddle_customer_id: CUSTOMER,
      checkout_at: T - 2 * HOUR,
      live_fetch_at: T - 5 * MINUTE,
    };
    expect(await decideLease(r, f.deps)).toMatchObject({ kind: "lease", terms: { state: "trial" } });
    expect(f.fetches).toEqual([]);
  });

  it("Paddle down, someone who has paid: signed active until the stored end plus 3 days", async () => {
    const f = fake({ subs: "fail" });
    const r: BillingRecord = { trial_started_at: T - 100 * DAY, paddle_customer_id: CUSTOMER, active_until: T - HOUR };
    expect(await decideLease(r, f.deps)).toMatchObject({
      kind: "lease",
      terms: { state: "active", active_until: T - HOUR + 3 * DAY },
    });
    // The failed attempt still counts toward the rate limit.
    expect(f.writes).toEqual([{ live_fetch_at: T }]);
  });

  it("Paddle down, never paid (an abandoned checkout): 503, and the attempt is recorded", async () => {
    const f = fake({ subs: "fail" });
    const r: BillingRecord = { trial_started_at: T - 40 * DAY, paddle_customer_id: CUSTOMER, checkout_at: T - HOUR };
    expect(await decideLease(r, f.deps)).toEqual({ kind: "unavailable" });
    expect(f.writes).toEqual([{ live_fetch_at: T }]);
  });

  it("unreadable data for our product counts as Paddle being down", async () => {
    const f = fake({ subs: [{ status: "active", items: [{ price: { product_id: OURS } }] }] });
    const r: BillingRecord = { trial_started_at: T - 40 * DAY, paddle_customer_id: CUSTOMER, active_until: T - DAY };
    expect(await decideLease(r, f.deps)).toMatchObject({ kind: "lease", terms: { state: "active", active_until: T + 2 * DAY } });
  });

  it("Clerk refusing the write, someone who has paid: signed active until the stored end plus 3 days", async () => {
    const f = fake({ subs: [], write: "fail" });
    const r: BillingRecord = { trial_started_at: T - 100 * DAY, paddle_customer_id: CUSTOMER, active_until: T - HOUR };
    expect(await decideLease(r, f.deps)).toMatchObject({
      kind: "lease",
      terms: { state: "active", active_until: T - HOUR + 3 * DAY },
    });
  });

  it("the kill switch: every Paddle customer is active for 3 days, and Paddle is not asked", async () => {
    const f = fake({ killSwitch: true, subs: "fail" });
    const payer: BillingRecord = { trial_started_at: T - 100 * DAY, paddle_customer_id: CUSTOMER, active_until: T - 50 * DAY };
    expect(await decideLease(payer, f.deps)).toEqual({
      kind: "lease",
      terms: { state: "active", active_until: T + 3 * DAY, trial_ends_at: T - 70 * DAY },
    });
    expect(f.fetches).toEqual([]);
    // Someone who never reached checkout is decided as usual.
    expect(await decideLease({ trial_started_at: T - DAY }, f.deps)).toMatchObject({ terms: { state: "trial" } });
  });

  it("no upstream failure ever produces a signed 'ended'", async () => {
    const records: BillingRecord[] = [
      {},
      { trial_started_at: T - 40 * DAY },
      { trial_started_at: T - 40 * DAY, paddle_customer_id: CUSTOMER },
      { trial_started_at: T - 40 * DAY, paddle_customer_id: CUSTOMER, checkout_at: T - HOUR },
      { trial_started_at: T - 40 * DAY, paddle_customer_id: CUSTOMER, synced_at: T - 5 * DAY },
      { trial_started_at: T - 400 * DAY, paddle_customer_id: CUSTOMER, active_until: T - 300 * DAY },
      { trial_started_at: T - 40 * DAY, paddle_customer_id: CUSTOMER, active_until: T + DAY, payment_failed: true },
    ];
    for (const record of records) {
      for (const failure of [{ subs: "fail" as const }, { write: "fail" as const }, { subs: "fail" as const, write: "fail" as const }]) {
        const decision = await decideLease(record, fake(failure).deps);
        const ended = decision.kind === "lease" && decision.terms.state === "ended";
        // Only a record that needs no upstream call at all may say "ended"
        // (a lapsed trial with nothing to write).
        if (ended) {
          expect(liveFetchWanted(record, T)).toBe(false);
          expect(record.trial_started_at).toBeDefined();
        }
      }
    }
  });
});
