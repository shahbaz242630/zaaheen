// ADR-SEC-052 §3.4a: our own limits come before the monthly spend limit, so
// a flood costs at most one day's share and the box comes back the next day.
import { describe, expect, it } from "vitest";
import { DAILY_QUESTIONS, PER_VISITOR_DAILY, dayOf, visitorKey } from "../src/budget";
import { RESTING } from "../src/text";

// Counting against the limits is tested in store.test.ts, on a real database.
describe("limits", () => {
  it("the limits are the agreed ones", () => {
    expect(PER_VISITOR_DAILY).toBe(30);
    expect(DAILY_QUESTIONS).toBe(500);
  });

  it("the resting sentence is the agreed wording", () => {
    expect(RESTING).toBe("The question box is resting for now. Please email customerservice@zaaheen.com and we'll help.");
  });
});

describe("dayOf", () => {
  it("is the UTC day, so the budget comes back each day at midnight UTC", () => {
    expect(dayOf(Date.UTC(2026, 9, 28, 23, 59, 59))).toBe("2026-10-28");
    expect(dayOf(Date.UTC(2026, 9, 29, 0, 0, 0))).toBe("2026-10-29");
  });
});

describe("visitorKey", () => {
  const SECRET = "test-only-secret";

  it("never contains the address", async () => {
    const key = await visitorKey("203.0.113.7", "2026-10-28", SECRET);
    expect(key).not.toContain("203.0.113.7");
    expect(key).toMatch(/^[0-9a-f]{64}$/);
  });

  it("is the same for one visitor on one day, different on another day or for another visitor", async () => {
    const a = await visitorKey("203.0.113.7", "2026-10-28", SECRET);
    expect(await visitorKey("203.0.113.7", "2026-10-28", SECRET)).toBe(a);
    expect(await visitorKey("203.0.113.7", "2026-10-29", SECRET)).not.toBe(a);
    expect(await visitorKey("203.0.113.8", "2026-10-28", SECRET)).not.toBe(a);
  });

  it("depends on the secret (a leaked table cannot be matched to addresses without it)", async () => {
    expect(await visitorKey("203.0.113.7", "2026-10-28", SECRET)).not.toBe(
      await visitorKey("203.0.113.7", "2026-10-28", "another-secret"),
    );
  });
});
