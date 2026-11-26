// The enquiry form's fields (ADR-SEC-057 §2, amendment 2 of s129). Exact keys
// only; fixed choices from fixed lists; every text field trimmed,
// length-capped and free of control characters, so nothing a visitor types
// can bend the email we send or the row we store.
//
// Founder s129: a simple form. Required: name, email, phone (country code +
// number), company, country, number of staff, annual turnover, sector.
// Optional: best time to call, and a comments box. The teams ticks are gone.
// Only the comments may hold line breaks.

export const COUNTRIES = ["AE", "SA", "QA", "KW", "BH", "OM", "other"] as const;
export const SIZES = ["1-19", "20-100", "101-500", "500+"] as const;
/** Estimated annual turnover, in UAE dirhams or the equivalent. */
export const TURNOVERS = ["under-5m", "5m-20m", "20m-50m", "50m-200m", "200m+"] as const;
export const SECTORS = [
  "retail",
  "trading",
  "manufacturing",
  "construction-real-estate",
  "hospitality-food",
  "healthcare",
  "education",
  "logistics",
  "financial-services",
  "professional-services",
  "technology",
  "energy",
  "government",
  "other",
] as const;
/**
 * The phone's country code (founder s129: "countries drop down with
 * international call extension so we can grab full number"). The Gulf first,
 * then common business and expat countries; "other" means the visitor types
 * the full number with its own + code. The page lists the same countries
 * (public/index.html; test/page.test.ts keeps the two in step).
 */
export const DIAL_CODES = [
  "+971", "+966", "+974", "+965", "+973", "+968",
  "+20", "+962", "+961", "+964", "+963", "+970", "+967", "+249", "+212", "+213", "+216", "+90", "+98",
  "+91", "+92", "+880", "+94", "+977", "+63", "+62", "+60", "+65", "+66", "+84", "+86", "+852", "+81", "+82",
  "+44", "+353", "+33", "+49", "+31", "+32", "+41", "+39", "+34", "+351", "+46", "+47", "+45", "+48", "+7",
  "+1", "+52", "+55", "+61", "+64", "+27", "+234", "+254", "+251",
  "other",
] as const;

/** Best time to call, Gulf time. Empty means no preference (the one optional field). */
export const CALL_TIMES = ["", "morning", "midday", "afternoon", "evening"] as const;

export const LIMITS = { name: 100, email: 254, phone: 25, company: 150, message: 2000, turnstile: 2048 } as const;

export interface Enquiry {
  name: string;
  email: string;
  phone: string;
  company: string;
  country: (typeof COUNTRIES)[number];
  size: (typeof SIZES)[number];
  turnover: (typeof TURNOVERS)[number];
  sector: (typeof SECTORS)[number];
  callTime: (typeof CALL_TIMES)[number];
  /** The optional comments; empty when none. */
  message: string;
  marketingConsent: boolean;
}

export type Validated = { ok: true; enquiry: Enquiry; turnstile: string } | { ok: false };

const KEYS = [
  "name",
  "email",
  "phoneCode",
  "phone",
  "company",
  "country",
  "size",
  "turnover",
  "sector",
  "callTime",
  "message",
  "contactConsent",
  "marketingConsent",
  "turnstile",
] as const;

// Any C0 control character, DEL, or the C1 range; and the Unicode line and
// paragraph separators. The separators are built from their codes so this
// file never holds one raw (a parser reads a raw one as a line break).
const SEPARATORS = String.fromCharCode(0x2028, 0x2029);
const CONTROL = new RegExp(`[\\x00-\\x1f\\x7f-\\x9f${SEPARATORS}]`);
const CONTROL_BUT_NEWLINE = new RegExp(`[\\x00-\\x09\\x0b-\\x1f\\x7f-\\x9f${SEPARATORS}]`);
// Unicode format characters (category Cf): direction overrides and isolates,
// zero-width spaces, word joiners, the byte-order mark. They can make a name
// display reversed or hide text in the email we read (review s123). Two are
// allowed: the zero-width non-joiner and joiner (U+200C, U+200D), which
// Persian and Urdu names and some emoji need to be written correctly, and
// which cannot reorder or hide text.
const FORMAT = /(?![‌‍])\p{Cf}/u;
// A plain address: no spaces, no angle brackets, no quotes, one @, a dot in the domain.
const EMAIL = /^[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/;
// The number after the chosen code: digits and the usual separators.
const LOCAL = /^[0-9 ()./-]{4,20}$/;
// The full number when the visitor chose "other": their own + code first.
const FULL = /^\+[0-9][0-9 ()./-]{5,24}$/;
const digits = (s: string) => s.replace(/[^0-9]/g, "").length;

/**
 * The phone as one international number: the chosen code and the number
 * typed after it ("+971 50 123 4567"), or, for "other", the full number the
 * visitor typed. At most 15 digits in all (the international maximum), and
 * at least 6.
 */
function fullPhone(code: (typeof DIAL_CODES)[number], typed: string): string | null {
  if (code === "other") {
    if (!FULL.test(typed)) return null;
    const n = digits(typed);
    return n >= 6 && n <= 15 ? typed : null;
  }
  // A leading + in the number box means they typed the code again: refuse it
  // (the form says to leave it out) rather than guess.
  if (!LOCAL.test(typed)) return null;
  // The trunk 0 people dial at home ("050 123 4567" in the UAE, "020..." in
  // the UK) is not dialled after a country code: drop that one 0 (review s129),
  // so the number we call back is "+971 50 123 4567", not "+971 050...".
  const first = typed.search(/[0-9]/);
  const local = typed[first] === "0" ? `${typed.slice(0, first)}${typed.slice(first + 1)}`.trim() : typed;
  const n = digits(code) + digits(local);
  return digits(local) >= 4 && n <= 15 ? `${code} ${local}` : null;
}

const NO: Validated = { ok: false };

/** The entry of `list` equal to `value`, or undefined: never the visitor's own string. */
const pick = <T extends string>(list: readonly T[], value: unknown): T | undefined => list.find((x) => x === value);

export function validateEnquiry(value: unknown): Validated {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return NO;
  const keys = Object.keys(value);
  if (keys.length !== KEYS.length || !KEYS.every((k) => keys.includes(k))) return NO;
  const v = value as Record<string, unknown>;

  const name = line(v.name, LIMITS.name);
  const email = line(v.email, LIMITS.email);
  const typedPhone = line(v.phone, LIMITS.phone);
  const company = line(v.company, LIMITS.company);
  const turnstile = line(v.turnstile, LIMITS.turnstile);
  if (name === null || email === null || typedPhone === null || company === null || turnstile === null) return NO;
  if (!EMAIL.test(email)) return NO;
  const phoneCode = pick(DIAL_CODES, v.phoneCode);
  if (!phoneCode) return NO;
  const phone = fullPhone(phoneCode, typedPhone);
  if (phone === null) return NO;

  const country = pick(COUNTRIES, v.country);
  const size = pick(SIZES, v.size);
  const turnover = pick(TURNOVERS, v.turnover);
  const sector = pick(SECTORS, v.sector);
  const callTime = pick(CALL_TIMES, v.callTime);
  if (!country || !size || !turnover || !sector || callTime === undefined) return NO;

  if (typeof v.message !== "string") return NO;
  // Windows and old Mac line breaks become \n before the control-character check.
  const message = v.message.replace(/\r\n?/g, "\n").trim();
  if (message.length > LIMITS.message || CONTROL_BUT_NEWLINE.test(message) || FORMAT.test(message)) return NO;

  if (v.contactConsent !== true) return NO;
  if (typeof v.marketingConsent !== "boolean") return NO;

  return {
    ok: true,
    turnstile,
    enquiry: { name, email, phone, company, country, size, turnover, sector, callTime, message, marketingConsent: v.marketingConsent },
  };
}

/** A required one-line text field: a string, trimmed, not empty, within the limit, no control characters. */
function line(value: unknown, max: number): string | null {
  if (typeof value !== "string") return null;
  const text = value.trim();
  if (text.length === 0 || text.length > max || CONTROL.test(text) || FORMAT.test(text)) return null;
  return text;
}
