// Deriving the billing record from Paddle subscriptions (SIGNIN-DESIGN.md §5,
// "Derived billing record"; amendment 3 in §8.29).
import { describe, expect, it } from "vitest";

import { BillingError, deriveBilling } from "../src/billing";
import { DAY, epochSeconds } from "../src/time";

const OURS = "pro_01ourmemoryapp00000000000a";
const OTHER = "pro_01anotherproduct000000000b";
const T = 1_800_000_000;

function iso(epoch: number): string {
  return new Date(epoch * 1000).toISOString();
}

function sub(fields: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    status: "active",
    items: [{ price: { id: "pri_1", product_id: OURS } }],
    current_billing_period: { starts_at: iso(T - 10 * DAY), ends_at: iso(T + 20 * DAY) },
    scheduled_change: null,
    ...fields,
  };
}

describe("one subscription", () => {
  it("active with no scheduled change: the period end plus a 3-day renewal gap", () => {
    expect(deriveBilling([sub()], OURS)).toEqual({ active_until: T + 23 * DAY, payment_failed: false });
  });

  it("active with a scheduled cancel: until the cancel takes effect, no gap", () => {
    const s = sub({ scheduled_change: { action: "cancel", effective_at: iso(T + 20 * DAY), resume_at: null } });
    expect(deriveBilling([s], OURS)).toEqual({ active_until: T + 20 * DAY, payment_failed: false });
  });

  it("active with a scheduled pause: until the pause takes effect, like a cancel", () => {
    const s = sub({ scheduled_change: { action: "pause", effective_at: iso(T + 20 * DAY), resume_at: null } });
    expect(deriveBilling([s], OURS)).toEqual({ active_until: T + 20 * DAY, payment_failed: false });
  });

  it("a scheduled resume does not end anything", () => {
    const s = sub({ scheduled_change: { action: "resume", effective_at: iso(T + 5 * DAY), resume_at: null } });
    expect(deriveBilling([s], OURS)).toEqual({ active_until: T + 23 * DAY, payment_failed: false });
  });

  it("past due: the period start plus 7 days, flagged as a failed payment", () => {
    expect(deriveBilling([sub({ status: "past_due" })], OURS)).toEqual({
      active_until: T - 3 * DAY,
      payment_failed: true,
    });
  });

  for (const status of ["paused", "canceled", "trialing", "something_new"]) {
    it(`${status} counts for nothing`, () => {
      expect(deriveBilling([sub({ status, current_billing_period: null })], OURS)).toEqual({
        active_until: null,
        payment_failed: false,
      });
    });
  }
});

describe("which subscriptions count", () => {
  it("only those with an item of our product", () => {
    const other = sub({ items: [{ price: { id: "pri_9", product_id: OTHER } }] });
    expect(deriveBilling([other], OURS).active_until).toBeNull();
    const mixed = sub({
      items: [{ price: { id: "pri_9", product_id: OTHER } }, { price: { id: "pri_1", product_id: OURS } }],
    });
    expect(deriveBilling([mixed], OURS).active_until).toBe(T + 23 * DAY);
  });

  it("another product's malformed subscription is not our problem", () => {
    const other = sub({ items: [{ price: { product_id: OTHER } }], current_billing_period: null });
    expect(deriveBilling([other, sub()], OURS).active_until).toBe(T + 23 * DAY);
  });

  it("no subscriptions at all is nothing", () => {
    expect(deriveBilling([], OURS)).toEqual({ active_until: null, payment_failed: false });
  });
});

describe("several subscriptions", () => {
  it("the latest end wins, with its own payment state", () => {
    const failedEarlier = sub({ status: "past_due" }); // until T - 3d
    expect(deriveBilling([failedEarlier, sub()], OURS)).toEqual({
      active_until: T + 23 * DAY,
      payment_failed: false,
    });
    const failedLater = sub({
      status: "past_due",
      current_billing_period: { starts_at: iso(T + 30 * DAY), ends_at: iso(T + 60 * DAY) },
    }); // until T + 37d
    expect(deriveBilling([sub(), failedLater], OURS)).toEqual({
      active_until: T + 37 * DAY,
      payment_failed: true,
    });
  });

  it("a tie goes to the subscription that is paying", () => {
    const paying = sub({ scheduled_change: { action: "cancel", effective_at: iso(T), resume_at: null } });
    const failing = sub({
      status: "past_due",
      current_billing_period: { starts_at: iso(T - 7 * DAY), ends_at: iso(T + 23 * DAY) },
    });
    expect(deriveBilling([failing, paying], OURS)).toEqual({ active_until: T, payment_failed: false });
    expect(deriveBilling([paying, failing], OURS)).toEqual({ active_until: T, payment_failed: false });
  });
});

describe("unreadable data from Paddle is an upstream error, never a silent 'nothing'", () => {
  const cases: Array<[string, unknown]> = [
    ["an active subscription with no billing period", sub({ current_billing_period: null })],
    ["a past-due one with no period start", sub({ status: "past_due", current_billing_period: { ends_at: iso(T) } })],
    ["an impossible date", sub({ current_billing_period: { starts_at: iso(T), ends_at: "2026-13-45T00:00:00Z" } })],
    ["a date with no time zone", sub({ current_billing_period: { starts_at: iso(T), ends_at: "2026-09-18T12:00:00" } })],
    ["a numeric date", sub({ current_billing_period: { starts_at: T, ends_at: T } })],
    ["a cancel with no date", sub({ scheduled_change: { action: "cancel" } })],
    ["a subscription that is not an object", null],
    ["items that are not a list", { status: "active", items: "pri_1" }],
  ];
  for (const [name, value] of cases) {
    it(name, () => {
      expect(() => deriveBilling([value], OURS)).toThrow(BillingError);
    });
  }
});

describe("RFC 3339 timestamps", () => {
  it("parses UTC, offsets and fractions to whole epoch seconds", () => {
    expect(epochSeconds("2026-09-18T12:00:00Z")).toBe(Date.UTC(2026, 8, 18, 12) / 1000);
    expect(epochSeconds("2026-09-18T16:00:00+04:00")).toBe(Date.UTC(2026, 8, 18, 12) / 1000);
    expect(epochSeconds("2026-09-18T12:00:00.999999Z")).toBe(Date.UTC(2026, 8, 18, 12) / 1000);
  });

  it("refuses everything else", () => {
    for (const bad of ["", "2026-09-18", "2026-09-18 12:00:00Z", "2026-02-30T00:00:00Z", "tomorrow", 5, null]) {
      expect(() => epochSeconds(bad)).toThrow();
    }
  });
});
