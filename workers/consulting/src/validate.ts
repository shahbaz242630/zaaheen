// The enquiry form's fields (ADR-SEC-057 §2). Exact keys only; fixed
// choices from fixed lists; every text field trimmed, length-capped and free
// of control characters (only the message may hold line breaks), so nothing
// a visitor types can bend the email we send or the row we store.

export const COUNTRIES = ["AE", "SA", "QA", "KW", "BH", "OM", "other"] as const;
export const SIZES = ["1-19", "20-100", "101-500", "500+"] as const;
export const DEPARTMENTS = [
  "reception",
  "customer-service",
  "sales",
  "marketing",
  "finance",
  "hr",
  "operations",
  "procurement",
  "it",
  "risk-compliance",
  "legal",
  "management",
] as const;

export const LIMITS = { name: 100, email: 254, phone: 25, company: 150, message: 2000, turnstile: 2048 } as const;

export interface Enquiry {
  name: string;
  email: string;
  phone: string;
  company: string;
  country: (typeof COUNTRIES)[number];
  size: (typeof SIZES)[number];
  departments: (typeof DEPARTMENTS)[number][];
  message: string;
  marketingConsent: boolean;
}

export type Validated = { ok: true; enquiry: Enquiry; turnstile: string } | { ok: false };

const KEYS = [
  "name",
  "email",
  "phone",
  "company",
  "country",
  "size",
  "departments",
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
// A plain address: no spaces, no angle brackets, no quotes, one @, a dot in the domain.
const EMAIL = /^[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/;
const PHONE = /^\+?[0-9 ()./-]{6,25}$/;

const NO: Validated = { ok: false };

export function validateEnquiry(value: unknown): Validated {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return NO;
  const keys = Object.keys(value);
  if (keys.length !== KEYS.length || !KEYS.every((k) => keys.includes(k))) return NO;
  const v = value as Record<string, unknown>;

  const name = line(v.name, LIMITS.name);
  const email = line(v.email, LIMITS.email);
  const phone = line(v.phone, LIMITS.phone);
  const company = line(v.company, LIMITS.company);
  const turnstile = line(v.turnstile, LIMITS.turnstile);
  if (name === null || email === null || phone === null || company === null || turnstile === null) return NO;
  if (!EMAIL.test(email) || !PHONE.test(phone) || (phone.match(/[0-9]/g)?.length ?? 0) < 6) return NO;

  if (typeof v.message !== "string") return NO;
  const message = v.message.trim();
  if (message.length > LIMITS.message || CONTROL_BUT_NEWLINE.test(message)) return NO;

  const country = COUNTRIES.find((c) => c === v.country);
  const size = SIZES.find((s) => s === v.size);
  if (!country || !size) return NO;

  if (!Array.isArray(v.departments) || v.departments.length > DEPARTMENTS.length) return NO;
  const departments: Enquiry["departments"] = [];
  for (const d of v.departments) {
    const known = DEPARTMENTS.find((k) => k === d);
    if (!known || departments.includes(known)) return NO;
    departments.push(known);
  }

  if (v.contactConsent !== true) return NO;
  if (typeof v.marketingConsent !== "boolean") return NO;

  return {
    ok: true,
    turnstile,
    enquiry: { name, email, phone, company, country, size, departments, message, marketingConsent: v.marketingConsent },
  };
}

/** A required one-line text field: a string, trimmed, not empty, within the limit, no control characters. */
function line(value: unknown, max: number): string | null {
  if (typeof value !== "string") return null;
  const text = value.trim();
  if (text.length === 0 || text.length > max || CONTROL.test(text)) return null;
  return text;
}
