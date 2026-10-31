// Ask Zaaheen's only stored data (ADR-SEC-052 §3.4a, §3.8; migrations/).
// Counts are anonymous; a visitor is a daily keyed hash (budget.ts); kept
// questions are scrubbed and carry no identity.
import { DAILY_QUESTIONS, PER_VISITOR_DAILY, dayOf } from "./budget";
import { KEEP_DAYS } from "./keep";
import { MAX_QUESTION_CHARS } from "./limits";
import { scrub } from "./scrub";

const DAY_MS = 24 * 60 * 60 * 1000;

// first: this is the refusal that crossed the limit. Later refusals write
// nothing at all, so hammering past a limit cannot wear out the database.
export type Claim = { ok: true } | { ok: false; reason: "visitor" | "daily"; first: boolean };

export type Outcome = "answered" | "turned_away" | "unavailable" | "resting" | "region";

/**
 * Count a question against the visitor's and the day's limits, in that
 * order. Each count is one statement (insert or add one, returning the new
 * total), so two requests at once cannot both slip under a limit. A visitor
 * over their limit never touches the day's budget, so one flooder cannot use
 * up everyone else's share. A count stops one past its limit: once over,
 * the update matches no row and returns nothing.
 */
export async function claimQuestion(db: D1Database, day: string, visitor: string): Promise<Claim> {
  const mine = await db
    .prepare(
      "INSERT INTO visitor_counts (day, visitor, n) VALUES (?, ?, 1) " +
        "ON CONFLICT (day, visitor) DO UPDATE SET n = n + 1 WHERE n <= ? RETURNING n",
    )
    .bind(day, visitor, PER_VISITOR_DAILY)
    .first<{ n: number }>();
  if (!mine) return { ok: false, reason: "visitor", first: false };
  if (mine.n > PER_VISITOR_DAILY) return { ok: false, reason: "visitor", first: true };

  const everyone = await db
    .prepare(
      "INSERT INTO daily_counts (day, kind, n) VALUES (?, 'asked', 1) " +
        "ON CONFLICT (day, kind) DO UPDATE SET n = n + 1 WHERE n <= ? RETURNING n",
    )
    .bind(day, DAILY_QUESTIONS)
    .first<{ n: number }>();
  if (!everyone) return { ok: false, reason: "daily", first: false };
  if (everyone.n > DAILY_QUESTIONS) return { ok: false, reason: "daily", first: true };

  return { ok: true };
}

/** Add one to the day's total for an outcome. */
export async function countOutcome(db: D1Database, day: string, outcome: Outcome): Promise<void> {
  await db
    .prepare("INSERT INTO daily_counts (day, kind, n) VALUES (?, ?, 1) ON CONFLICT (day, kind) DO UPDATE SET n = n + 1")
    .bind(day, outcome)
    .run();
}

/** Keep a question the box could not answer or turned away: scrubbed, capped, no identity. */
export async function keepQuestion(db: D1Database, now: number, reason: string, question: string): Promise<void> {
  const text = scrub(question.trim()).slice(0, MAX_QUESTION_CHARS);
  await db.prepare("INSERT INTO kept_questions (stored_at, reason, question) VALUES (?, ?, ?)").bind(now, reason, text).run();
}

/** The daily clean-up: kept questions past 30 days (or dated in the future), and earlier days' visitor counts. */
export async function sweep(db: D1Database, now: number): Promise<void> {
  await db.batch([
    db.prepare("DELETE FROM kept_questions WHERE stored_at < ? OR stored_at > ?").bind(now - KEEP_DAYS * DAY_MS, now),
    db.prepare("DELETE FROM visitor_counts WHERE day < ?").bind(dayOf(now)),
  ]);
}
