// The daily sweep (ADR-SEC-057 §4): an enquiry is kept 12 months, then deleted.
import { applyD1Migrations, env } from "cloudflare:test";
import { beforeEach, describe, expect, it } from "vitest";
import { KEEP_MS, storeEnquiry, sweep } from "../src/store";
import { validateEnquiry } from "../src/validate";
import { GOOD } from "./fixtures";

const NOW = Date.UTC(2026, 10, 12, 12, 0, 0);

beforeEach(async () => {
  await applyD1Migrations(env.DB, env.TEST_MIGRATIONS);
  await env.DB.batch([
    env.DB.prepare("DELETE FROM enquiries"),
    env.DB.prepare("DELETE FROM daily_mail"),
    env.DB.prepare("DELETE FROM visitor_counts"),
    env.DB.prepare("DELETE FROM daily_counts"),
  ]);
});

describe("counts", () => {
  it("are kept for today only", async () => {
    await env.DB.prepare("INSERT INTO visitor_counts (day, visitor, n) VALUES ('2026-11-11', 'a', 1), ('2026-11-12', 'b', 1)").run();
    await env.DB.prepare("INSERT INTO daily_counts (day, kind, n) VALUES ('2026-11-11', 'stored', 3), ('2026-11-12', 'stored', 1)").run();
    await sweep(env.DB, NOW);
    const v = await env.DB.prepare("SELECT day FROM visitor_counts").all<{ day: string }>();
    const d = await env.DB.prepare("SELECT day FROM daily_counts").all<{ day: string }>();
    expect(v.results.map((r) => r.day)).toEqual(["2026-11-12"]);
    expect(d.results.map((r) => r.day)).toEqual(["2026-11-12"]);
  });
});

describe("sweep", () => {
  it("deletes enquiries older than 12 months and keeps the rest", async () => {
    const v = validateEnquiry(GOOD);
    if (!v.ok) throw new Error("fixture invalid");
    await storeEnquiry(env.DB, NOW - KEEP_MS - 1, v.enquiry);
    await storeEnquiry(env.DB, NOW - KEEP_MS + 60_000, v.enquiry);
    await storeEnquiry(env.DB, NOW, v.enquiry);
    await env.DB.prepare("INSERT INTO daily_mail (day, n) VALUES ('2025-01-01', 3), ('2026-11-12', 1)").run();
    await sweep(env.DB, NOW);
    const left = await env.DB.prepare("SELECT received_at FROM enquiries ORDER BY received_at").all<{ received_at: number }>();
    expect(left.results.map((r) => r.received_at)).toEqual([NOW - KEEP_MS + 60_000, NOW]);
    const days = await env.DB.prepare("SELECT day FROM daily_mail").all<{ day: string }>();
    expect(days.results.map((r) => r.day)).toEqual(["2026-11-12"]);
  });

  it("is about 12 months", () => {
    expect(KEEP_MS).toBe(365 * 24 * 60 * 60 * 1000);
  });
});
