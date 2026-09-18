// The per-user billing record in Clerk `private_metadata.zaaheen_memory`
// (SIGNIN-DESIGN.md §5; amendment 3 in §8.29).
import { describe, expect, it } from "vitest";

import { type BillingRecord, RECORD_KEY, applyDerived, parseRecord, recordPatch } from "../src/record";
import { DAY } from "../src/time";

const T = 1_800_000_000;
const CUSTOMER = "ctm_01h8441jn5pcwrfhwh78jqt8hk";

function omit(r: BillingRecord, field: keyof BillingRecord): BillingRecord {
  const copy = { ...r };
  delete copy[field];
  return copy;
}

function meta(record: unknown): unknown {
  return { [RECORD_KEY]: record, is_admin: false };
}

describe("reading the record", () => {
  it("a new user has an empty record and nothing to warn about", () => {
    for (const m of [undefined, null, {}, { other_app: { x: 1 } }]) {
      expect(parseRecord(m)).toEqual({ record: {}, warnings: [] });
    }
  });

  it("reads every field", () => {
    const full = {
      trial_started_at: T,
      paddle_customer_id: CUSTOMER,
      active_until: T + 30 * DAY,
      payment_failed: false,
      comp_until: T + 90 * DAY,
      checkout_at: T + DAY,
      synced_at: T + DAY,
      live_fetch_at: T + DAY,
    };
    expect(parseRecord(meta(full))).toEqual({ record: full, warnings: [] });
  });

  it("ignores fields it does not know (the founder's own notes)", () => {
    expect(parseRecord(meta({ trial_started_at: T, note: "beta tester" }))).toEqual({
      record: { trial_started_at: T },
      warnings: [],
    });
  });

  it("drops a malformed field with a warning that names the field, never the value", () => {
    const { record, warnings } = parseRecord(
      meta({
        trial_started_at: "yesterday",
        active_until: 1.5,
        payment_failed: "yes",
        paddle_customer_id: "cus_SECRET_LOOKING_VALUE",
        live_fetch_at: -1,
        synced_at: 0,
        checkout_at: 2 ** 53,
      }),
    );
    expect(record).toEqual({});
    expect(warnings).toHaveLength(7);
    const all = warnings.join(" ");
    for (const field of ["trial_started_at", "active_until", "payment_failed", "paddle_customer_id", "live_fetch_at"]) {
      expect(all).toContain(field);
    }
    expect(all).not.toContain("SECRET");
    expect(all).not.toContain("yesterday");
  });

  it("a record that is not an object is empty, with a warning", () => {
    expect(parseRecord(meta("trial"))).toEqual({ record: {}, warnings: [expect.stringContaining(RECORD_KEY)] });
  });
});

describe("comp_until, typed by the founder in the Clerk dashboard", () => {
  it("accepts epoch seconds", () => {
    expect(parseRecord(meta({ comp_until: T })).record).toEqual({ comp_until: T });
  });

  it("accepts a YYYY-MM-DD date, free through the end of that day (UTC)", () => {
    expect(parseRecord(meta({ comp_until: "2026-12-31" })).record).toEqual({
      comp_until: Date.UTC(2027, 0, 1) / 1000,
    });
  });

  it("drops anything else with a warning", () => {
    for (const bad of ["2026-02-30", "31/12/2026", "2026-12-31T00:00:00Z", "", true, 1.5]) {
      const { record, warnings } = parseRecord(meta({ comp_until: bad }));
      expect(record).toEqual({});
      expect(warnings).toEqual([expect.stringContaining("comp_until")]);
    }
  });
});

describe("applying a derivation from Paddle", () => {
  const before: BillingRecord = { trial_started_at: T - 40 * DAY, paddle_customer_id: CUSTOMER };

  it("stores the result and the sync time; a live fetch also stamps live_fetch_at", () => {
    const derived = { active_until: T + 30 * DAY, payment_failed: false };
    expect(applyDerived(before, derived, T, { live: false })).toEqual({
      ...before,
      active_until: T + 30 * DAY,
      payment_failed: false,
      synced_at: T,
    });
    expect(applyDerived(before, derived, T, { live: true })).toEqual({
      ...before,
      active_until: T + 30 * DAY,
      payment_failed: false,
      synced_at: T,
      live_fetch_at: T,
    });
  });

  it("when nothing counts any more, a past end is kept (the user did pay) and a future one ends now", () => {
    const none = { active_until: null, payment_failed: false };
    const expired = applyDerived({ ...before, active_until: T - DAY }, none, T, { live: false });
    expect(expired.active_until).toBe(T - DAY);
    const refunded = applyDerived({ ...before, active_until: T + 20 * DAY, payment_failed: true }, none, T, {
      live: false,
    });
    expect(refunded.active_until).toBe(T);
    expect(refunded.payment_failed).toBe(false);
    const neverPaid = applyDerived(before, none, T, { live: false });
    expect(neverPaid).not.toHaveProperty("active_until");
  });
});

describe("the write back to Clerk", () => {
  const stored: BillingRecord = {
    trial_started_at: T - 40 * DAY,
    paddle_customer_id: CUSTOMER,
    active_until: T + DAY,
    payment_failed: false,
    comp_until: T + 5 * DAY,
  };

  it("is skipped when nothing changed", () => {
    expect(recordPatch(stored, { ...stored })).toBeNull();
  });

  it("carries only the fields that changed", () => {
    expect(recordPatch(stored, { ...stored, active_until: T + 31 * DAY, synced_at: T })).toEqual({
      active_until: T + 31 * DAY,
      synced_at: T,
    });
  });

  it("never touches comp_until, which only the founder sets", () => {
    expect(recordPatch(stored, { ...stored, comp_until: T + 500 * DAY })).toBeNull();
    expect(recordPatch(stored, omit(stored, "comp_until"))).toBeNull();
  });

  it("never changes or clears an existing trial start, and adds a new one", () => {
    expect(recordPatch(stored, { ...stored, trial_started_at: T })).toBeNull();
    expect(recordPatch(stored, omit(stored, "trial_started_at"))).toBeNull();
    expect(recordPatch({}, { trial_started_at: T })).toEqual({ trial_started_at: T });
  });

  it("removes a field it owns by writing null", () => {
    expect(recordPatch({ ...stored, checkout_at: T }, stored)).toEqual({ checkout_at: null });
  });
});
