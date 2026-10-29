// ADR-SEC-052 §3.8: a kept question is deleted after 30 days.
import { describe, expect, it } from "vitest";
import { KEEP_DAYS, isExpired } from "../src/keep";

const DAY = 24 * 60 * 60 * 1000;

describe("isExpired", () => {
  it("keeps for 30 days", () => {
    expect(KEEP_DAYS).toBe(30);
  });

  it("is kept up to and including the 30th day, gone after", () => {
    const stored = Date.UTC(2026, 9, 1);
    expect(isExpired(stored, stored)).toBe(false);
    expect(isExpired(stored, stored + 30 * DAY)).toBe(false);
    expect(isExpired(stored, stored + 30 * DAY + 1)).toBe(true);
  });

  it("treats a stored time in the future as expired (a clock or data error is never kept forever)", () => {
    expect(isExpired(Date.UTC(2027, 0, 1), Date.UTC(2026, 9, 1))).toBe(true);
  });
});
