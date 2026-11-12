// The only stored data (ADR-SEC-057 §4; migrations/0001_enquiries.sql):
// the enquiry as typed, both consent answers and when it arrived. Nothing
// about the visitor's network or browser. Kept 12 months, then deleted.
import type { Enquiry } from "./validate";

export const KEEP_MS = 365 * 24 * 60 * 60 * 1000;

export function dayOf(now: number): string {
  return new Date(now).toISOString().slice(0, 10);
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
 * Count one email against the day's cap, in one statement (insert or add
 * one, returning the new total), so two requests at once cannot both slip
 * under it. Over the cap the update matches no row and returns nothing.
 */
export async function claimMail(db: D1Database, day: string, cap: number): Promise<boolean> {
  const row = await db
    .prepare("INSERT INTO daily_mail (day, n) VALUES (?, 1) ON CONFLICT (day) DO UPDATE SET n = n + 1 WHERE n < ? RETURNING n")
    .bind(day, cap)
    .first<{ n: number }>();
  return row !== null && row.n <= cap;
}

/** The daily sweep: enquiries past 12 months, and earlier days' mail counts. */
export async function sweep(db: D1Database, now: number): Promise<void> {
  await db.batch([
    db.prepare("DELETE FROM enquiries WHERE received_at < ?").bind(now - KEEP_MS),
    db.prepare("DELETE FROM daily_mail WHERE day < ?").bind(dayOf(now)),
  ]);
}
