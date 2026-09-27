// One free trial per email, even after deleting the account
// (ACCOUNT-DELETION-DESIGN D8, quoted):
//
//   The fingerprint: HMAC-SHA256(TRIAL_KEY, normalise(email)), hex. [...]
//   normalise: trim, lower-case; drop a +tag from the local part for every
//   domain; for gmail.com and googlemail.com also drop dots and map to
//   gmail.com.
//   The store: a new Workers KV namespace TRIALS [...] Key = the
//   fingerprint; value = the original trial start (ISO time); expires two
//   years after that start (expirationTtl), after which a returning person
//   gets a new trial.
//   At the first lease [...] Found -> write that earlier start into the
//   record. Not found -> start now and put the fingerprint. A KV failure
//   fails closed for the lease call (retryable 503), never granting a fresh
//   trial on an error, and never logging the email or fingerprint.
//
// TRIAL_KEY is never rotated: a new key forgets every earlier trial. A user
// with no primary email cannot be checked, so is refused like a KV failure.
// TrialError messages are fixed text: never the email, the fingerprint or a
// KV error's own words.

import { DAY, epochSeconds } from "./time";

/** How long a trial's fingerprint is kept: two years after the start. */
export const TRIAL_MEMORY_SECONDS = 730 * DAY;

const GMAIL_DOMAINS: ReadonlySet<string> = new Set(["gmail.com", "googlemail.com"]);

/** The TRIALS namespace's shape (Cloudflare's `KVNamespace`, as used here). */
export interface TrialStore {
  get(key: string): Promise<string | null>;
  put(key: string, value: string, options: { expirationTtl: number }): Promise<void>;
}

export class TrialError extends Error {
  override name = "TrialError";
}

/** The address one person may hold under many spellings, as D8 defines it. */
export function normaliseEmail(email: string): string {
  const lower = email.trim().toLowerCase();
  const at = lower.lastIndexOf("@");
  if (at < 0) return lower;
  let local = lower.slice(0, at);
  let domain = lower.slice(at + 1);
  const plus = local.indexOf("+", 1);
  // A + that starts the local part is the name itself, not a tag.
  if (plus > 0) local = local.slice(0, plus);
  if (GMAIL_DOMAINS.has(domain)) {
    local = local.replaceAll(".", "");
    domain = "gmail.com";
  }
  return `${local}@${domain}`;
}

/** HMAC-SHA256 of the normalised email under TRIAL_KEY, lower-case hex. */
export async function trialFingerprint(key: string, email: string): Promise<string> {
  const encoder = new TextEncoder();
  const hmac = await crypto.subtle.importKey("raw", encoder.encode(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", hmac, encoder.encode(normaliseEmail(email))));
  return Array.from(mac, (b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * The trial start for a record that has none: an earlier trial of the same
 * email if TRIALS remembers one, else `now` (stored first). Throws
 * TrialError on anything else, and then nothing has been written.
 */
export async function firstTrialStart(store: TrialStore | undefined, key: string, email: string | null, now: number): Promise<number> {
  if (store === undefined) throw new TrialError("trials: no TRIALS binding");
  if (email === null) throw new TrialError("trials: no primary email");
  const fingerprint = await trialFingerprint(key, email);
  let earlier: string | null;
  try {
    earlier = await store.get(fingerprint);
  } catch {
    throw new TrialError("trials: read failed");
  }
  if (earlier !== null) {
    let start: number;
    try {
      start = epochSeconds(earlier);
    } catch {
      throw new TrialError("trials: stored start unreadable");
    }
    // A start after now would lengthen the trial; it can only be a clock fault.
    return Math.min(start, now);
  }
  try {
    await store.put(fingerprint, new Date(now * 1000).toISOString(), { expirationTtl: TRIAL_MEMORY_SECONDS });
  } catch {
    throw new TrialError("trials: write failed");
  }
  return now;
}
