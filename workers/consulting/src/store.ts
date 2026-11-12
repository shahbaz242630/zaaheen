// The only stored data (ADR-SEC-057 §4; migrations/): the enquiry as typed,
// both consent answers and when it arrived; and anonymous daily counts for
// the limits. Nothing about the visitor's network or browser. Enquiries are
// kept 12 months, then deleted; counts the next day.
import type { Enquiry } from "./validate";

export const KEEP_MS = 365 * 24 * 60 * 60 * 1000;
const DAY_MS = 24 * 60 * 60 * 1000;

/** Enquiries one visitor may send in a UTC day. A real business sends one. */
export const PER_VISITOR_DAILY = 5;
/** Enquiries everyone together may store in a UTC day: far above real traffic. */
export const DAILY_STORED = 200;

export function dayOf(now: number): string {
  return new Date(now).toISOString().slice(0, 10);
}

/**
 * Count one enquiry against the visitor's and then the day's limit. Each
 * count is one statement (insert or add one, returning the new total), so
 * two requests at once cannot both slip under a limit. A visitor over their
 * limit never touches the day's count. Once over, the update matches no row
 * and writes nothing, so hammering past a limit cannot wear out the database.
 */
export async function claimEnquiry(db: D1Database, day: string, visitor: string): Promise<boolean> {
  const mine = await db
    .prepare(
      "INSERT INTO visitor_counts (day, visitor, n) VALUES (?, ?, 1) " +
        "ON CONFLICT (day, visitor) DO UPDATE SET n = n + 1 WHERE n < ? RETURNING n",
    )
    .bind(day, visitor, PER_VISITOR_DAILY)
    .first<{ n: number }>();
  if (!mine || mine.n > PER_VISITOR_DAILY) return false;
  const everyone = await db
    .prepare(
      "INSERT INTO daily_counts (day, kind, n) VALUES (?, 'stored', 1) " +
        "ON CONFLICT (day, kind) DO UPDATE SET n = n + 1 WHERE n < ? RETURNING n",
    )
    .bind(day, DAILY_STORED)
    .first<{ n: number }>();
  return everyone !== null && everyone.n <= DAILY_STORED;
}

/** Store an enquiry; returns its row id. The contact consent is always true here (validate.ts requires it). */
export async function storeEnquiry(db: D1Database, now: number, e: Enquiry): Promise<number> {
  const row = await db
    .prepare(
      "INSERT INTO enquiries (received_at, name, email, phone, company, country, size, departments, message, " +
        "contact_consent, marketing_consent) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?) RETURNING id",
    )
    .bind(now, e.name, e.email, e.phone, e.company, e.country, e.size, JSON.stringify(e.departments), e.message, e.marketingConsent ? 1 : 0)
    .first<{ id: number }>();
  if (!row) throw new Error("enquiry_not_stored");
  return row.id;
}

export async function markMailed(db: D1Database, id: number): Promise<void> {
  await db.prepare("UPDATE enquiries SET mailed = 1 WHERE id = ?").bind(id).run();
}

/**
 * One email slot against the day's cap, in one statement, so two requests at
 * once cannot both slip under it. The count stops one past the cap:
 * "first-over" is the single request that crossed it (it sends the one
 * cap alert); after that the update matches no row and writes nothing.
 */
export async function claimMail(db: D1Database, day: string, cap: number): Promise<"ok" | "first-over" | "over"> {
  const row = await db
    .prepare("INSERT INTO daily_mail (day, n) VALUES (?, 1) ON CONFLICT (day) DO UPDATE SET n = n + 1 WHERE n <= ? RETURNING n")
    .bind(day, cap)
    .first<{ n: number }>();
  if (!row) return "over";
  return row.n <= cap ? "ok" : "first-over";
}

/**
 * Give back a slot whose email failed, so a failure does not use up the
 * day's cap. Not once the cap has been crossed: the count then stays one
 * past it, so the cap alert can never be sent twice in a day.
 */
export async function releaseMail(db: D1Database, day: string, cap: number): Promise<void> {
  await db.prepare("UPDATE daily_mail SET n = n - 1 WHERE day = ? AND n > 0 AND n <= ?").bind(day, cap).run();
}

/** For the nightly alert: counts only, never the enquiries themselves. */
export async function alertCounts(db: D1Database, now: number): Promise<{ unmailed: number; storedYesterday: number }> {
  const unmailed = await db
    .prepare("SELECT COUNT(*) AS n FROM enquiries WHERE mailed = 0 AND received_at >= ? AND received_at < ?")
    .bind(now - DAY_MS, now)
    .first<{ n: number }>();
  const stored = await db
    .prepare("SELECT n FROM daily_counts WHERE day = ? AND kind = 'stored'")
    .bind(dayOf(now - DAY_MS))
    .first<{ n: number }>();
  return { unmailed: unmailed?.n ?? 0, storedYesterday: stored?.n ?? 0 };
}

/** The daily sweep: enquiries past 12 months, and earlier days' counts. */
export async function sweep(db: D1Database, now: number): Promise<void> {
  const today = dayOf(now);
  await db.batch([
    db.prepare("DELETE FROM enquiries WHERE received_at < ?").bind(now - KEEP_MS),
    db.prepare("DELETE FROM daily_mail WHERE day < ?").bind(today),
    db.prepare("DELETE FROM visitor_counts WHERE day < ?").bind(today),
    db.prepare("DELETE FROM daily_counts WHERE day < ?").bind(today),
  ]);
}
