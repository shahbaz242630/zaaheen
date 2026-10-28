// ADR-SEC-052 §3.4a and §3.8, against a real (local) D1 database built from
// migrations/: counting under the limits, keeping a turned-away question
// scrubbed and without identity, and the daily sweep.
import { applyD1Migrations, env } from "cloudflare:test";
import { beforeEach, describe, expect, it } from "vitest";
import { DAILY_QUESTIONS, PER_VISITOR_DAILY } from "../src/budget";
import { REMOVED } from "../src/scrub";
import { claimQuestion, countOutcome, keepQuestion, sweep } from "../src/store";

const DAY = 24 * 60 * 60 * 1000;
const NOW = Date.UTC(2026, 9, 28, 12, 0, 0);

beforeEach(async () => {
  await applyD1Migrations(env.DB, env.TEST_MIGRATIONS);
  await env.DB.batch([
    env.DB.prepare("DELETE FROM daily_counts"),
    env.DB.prepare("DELETE FROM visitor_counts"),
    env.DB.prepare("DELETE FROM kept_questions"),
  ]);
});

async function count(day: string, kind: string): Promise<number> {
  const row = await env.DB.prepare("SELECT n FROM daily_counts WHERE day = ? AND kind = ?").bind(day, kind).first<{ n: number }>();
  return row?.n ?? 0;
}

describe("claimQuestion", () => {
  it("lets a question through and counts it for the visitor and the day", async () => {
    expect(await claimQuestion(env.DB, "2026-10-28", "v1")).toEqual({ ok: true });
    expect(await count("2026-10-28", "asked")).toBe(1);
  });

  it("stops a visitor after their daily limit, without using the day's budget", async () => {
    for (let i = 0; i < PER_VISITOR_DAILY; i++) expect((await claimQuestion(env.DB, "2026-10-28", "v1")).ok).toBe(true);
    expect(await claimQuestion(env.DB, "2026-10-28", "v1")).toEqual({ ok: false, reason: "visitor" });
    expect(await claimQuestion(env.DB, "2026-10-28", "v1")).toEqual({ ok: false, reason: "visitor" });
    // Their extra attempts did not eat everyone else's budget.
    expect(await count("2026-10-28", "asked")).toBe(PER_VISITOR_DAILY);
    expect(await claimQuestion(env.DB, "2026-10-28", "v2")).toEqual({ ok: true });
  });

  it("stops everyone at the daily budget, and starts again the next day", async () => {
    await env.DB.prepare("INSERT INTO daily_counts (day, kind, n) VALUES (?, 'asked', ?)")
      .bind("2026-10-28", DAILY_QUESTIONS)
      .run();
    expect(await claimQuestion(env.DB, "2026-10-28", "fresh")).toEqual({ ok: false, reason: "daily" });
    expect(await claimQuestion(env.DB, "2026-10-29", "fresh")).toEqual({ ok: true });
  });
});

describe("countOutcome", () => {
  it("adds one to the day's total for that outcome", async () => {
    await countOutcome(env.DB, "2026-10-28", "answered");
    await countOutcome(env.DB, "2026-10-28", "answered");
    await countOutcome(env.DB, "2026-10-28", "turned_away");
    expect(await count("2026-10-28", "answered")).toBe(2);
    expect(await count("2026-10-28", "turned_away")).toBe(1);
  });
});

describe("keepQuestion", () => {
  it("keeps the question scrubbed, with the time and reason and nothing else", async () => {
    await keepQuestion(env.DB, NOW, "off_topic", "  my email is sam@example.com, write me a poem  ");
    const rows = await env.DB.prepare("SELECT * FROM kept_questions").all();
    expect(rows.results).toEqual([
      { id: 1, stored_at: NOW, reason: "off_topic", question: `my email is ${REMOVED}, write me a poem` },
    ]);
  });

  it("never keeps more than the question limit", async () => {
    await keepQuestion(env.DB, NOW, "off_topic", "x".repeat(5000));
    const row = await env.DB.prepare("SELECT question FROM kept_questions").first<{ question: string }>();
    expect(row?.question.length).toBe(500);
  });
});

describe("sweep", () => {
  it("deletes kept questions over 30 days old (or from the future) and earlier days' visitor counts", async () => {
    await keepQuestion(env.DB, NOW - 30 * DAY, "off_topic", "kept: exactly 30 days");
    await keepQuestion(env.DB, NOW - 30 * DAY - 1, "off_topic", "gone: just over 30 days");
    await keepQuestion(env.DB, NOW + DAY, "off_topic", "gone: from the future");
    await claimQuestion(env.DB, "2026-10-27", "yesterday");
    await claimQuestion(env.DB, "2026-10-28", "today");

    await sweep(env.DB, NOW);

    const kept = await env.DB.prepare("SELECT question FROM kept_questions").all<{ question: string }>();
    expect(kept.results.map((r) => r.question)).toEqual(["kept: exactly 30 days"]);
    const visitors = await env.DB.prepare("SELECT visitor FROM visitor_counts").all<{ visitor: string }>();
    expect(visitors.results.map((r) => r.visitor)).toEqual(["today"]);
    // Anonymous daily totals stay.
    expect(await count("2026-10-27", "asked")).toBe(1);
  });
});
