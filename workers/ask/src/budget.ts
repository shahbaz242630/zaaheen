// Our own limits, before the monthly spend limit (ADR-SEC-052 §3.4a, founder
// s116). A flood costs at most one day's share; the box comes back the next
// UTC day by itself. The counts are kept by the caller (D1); this file only
// decides.

/** Questions one visitor may ask in a UTC day. Real visitors ask 2 to 5. */
export const PER_VISITOR_DAILY = 30;

/** Questions everyone together may ask in a UTC day: about a thirtieth of the monthly limit. */
export const DAILY_QUESTIONS = 500;

export type Allowance = { ok: true } | { ok: false; reason: "visitor" | "daily" };

export function allowQuestion(counts: { visitorToday: number; everyoneToday: number }): Allowance {
  if (counts.everyoneToday >= DAILY_QUESTIONS) return { ok: false, reason: "daily" };
  if (counts.visitorToday >= PER_VISITOR_DAILY) return { ok: false, reason: "visitor" };
  return { ok: true };
}

/** The UTC day a time (ms) falls in, as YYYY-MM-DD. */
export function dayOf(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

/**
 * Who is asking, without keeping who: HMAC-SHA256 of the address and the day
 * under a Worker secret, as hex. Changes every day, so nothing links a
 * visitor across days, and a leaked count table cannot be matched to
 * addresses without the secret.
 */
export async function visitorKey(address: string, day: string, secret: string): Promise<string> {
  const encoder = new TextEncoder();
  const key = await crypto.subtle.importKey("raw", encoder.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
  ]);
  const mac = await crypto.subtle.sign("HMAC", key, encoder.encode(`${day}\n${address}`));
  return [...new Uint8Array(mac)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
