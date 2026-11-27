// The form's fields (ADR-SEC-057 §2): exact keys, fixed choices, length
// limits, no control characters, and the contact consent always required.
import { describe, expect, it } from "vitest";
import { CALL_TIMES, COUNTRIES, DIAL_CODES, LIMITS, SECTORS, SIZES, TURNOVERS, validateEnquiry } from "../src/validate";
import { GOOD } from "./fixtures";

describe("validateEnquiry", () => {
  it("accepts a complete, ordinary enquiry and trims it", () => {
    const result = validateEnquiry({ ...GOOD, name: "  Test Person  " });
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.enquiry.name).toBe("Test Person");
      expect(result.turnstile).toBe("token");
      expect(result.enquiry.phone).toBe("+971 50 000 0000");
      expect(result.enquiry).toMatchObject({ turnover: "5m-20m", sector: "trading", callTime: "morning" });
    }
  });

  it("leaves only the best time to call and the comments optional (founder s129)", () => {
    expect(validateEnquiry({ ...GOOD, message: "", callTime: "" }).ok).toBe(true);
    for (const key of ["name", "email", "phone", "company"] as const) {
      expect(validateEnquiry({ ...GOOD, [key]: "" }).ok, key).toBe(false);
    }
    for (const key of ["phoneCode", "country", "size", "turnover", "sector"] as const) {
      expect(validateEnquiry({ ...GOOD, [key]: "" }).ok, key).toBe(false);
    }
  });

  it("refuses anything that is not a plain object", () => {
    for (const value of [null, 1, "x", [], true]) expect(validateEnquiry(value).ok).toBe(false);
  });

  it("refuses unknown and missing keys", () => {
    expect(validateEnquiry({ ...GOOD, extra: 1 }).ok).toBe(false);
    const { phone: _phone, ...missing } = GOOD;
    expect(validateEnquiry(missing).ok).toBe(false);
  });

  it("requires the contact consent, ticked", () => {
    expect(validateEnquiry({ ...GOOD, contactConsent: false }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, contactConsent: "true" }).ok).toBe(false);
  });

  it("takes marketing consent only as a real true or false", () => {
    expect(validateEnquiry({ ...GOOD, marketingConsent: true }).ok).toBe(true);
    expect(validateEnquiry({ ...GOOD, marketingConsent: "yes" }).ok).toBe(false);
  });

  it("refuses an empty or over-long field", () => {
    expect(validateEnquiry({ ...GOOD, name: "   " }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, name: "a".repeat(LIMITS.name + 1) }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, company: "a".repeat(LIMITS.company + 1) }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, message: "a".repeat(LIMITS.message + 1) }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, turnstile: "a".repeat(LIMITS.turnstile + 1) }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, turnstile: "" }).ok).toBe(false);
  });

  it("refuses control characters anywhere but line breaks in the message", () => {
    expect(validateEnquiry({ ...GOOD, name: "Test\r\nBcc: x@example.com" }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, company: "A\u0000B" }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, message: "line one\nline two" }).ok).toBe(true);
    expect(validateEnquiry({ ...GOOD, message: "bad\u0007bell" }).ok).toBe(false);
    const crlf = validateEnquiry({ ...GOOD, message: "line one\r\nline two\rthree" });
    expect(crlf.ok).toBe(true);
    if (crlf.ok) expect(crlf.enquiry.message).toBe("line one\nline two\nthree");
  });

  it("takes only a plain email address", () => {
    for (const email of ["a@b.co", "first.last+tag@sub.example.ae"]) expect(validateEnquiry({ ...GOOD, email }).ok).toBe(true);
    for (const email of ["no-at", "a@b", "a b@c.com", "a@b.com\r\nBcc: x@y.com", "<a@b.com>", `${"a".repeat(250)}@b.com`]) {
      expect(validateEnquiry({ ...GOOD, email }).ok).toBe(false);
    }
  });

  it("joins the country code and the number into one international number", () => {
    const uk = validateEnquiry({ ...GOOD, phoneCode: "+44", phone: "(020) 1234-5678" });
    // The trunk 0 is dropped after a country code (review s129; fields-s129.test.ts).
    expect(uk.ok && uk.enquiry.phone).toBe("+44 (20) 1234-5678");
    for (const phone of ["12", "call me", "50 000 0000 ext 5", "+971 50 000 0000", "1".repeat(LIMITS.phone + 1)]) {
      expect(validateEnquiry({ ...GOOD, phone }).ok, phone).toBe(false);
    }
    // At most 15 digits in all, the international maximum.
    expect(validateEnquiry({ ...GOOD, phoneCode: "+971", phone: "123456789012" }).ok).toBe(true);
    expect(validateEnquiry({ ...GOOD, phoneCode: "+971", phone: "1234567890123" }).ok).toBe(false);
  });

  it("takes the full number, + code first, only for another country", () => {
    const other = validateEnquiry({ ...GOOD, phoneCode: "other", phone: "+355 69 123 4567" });
    expect(other.ok && other.enquiry.phone).toBe("+355 69 123 4567");
    for (const phone of ["355 69 123 4567", "+12", "+1234567890123456"]) {
      expect(validateEnquiry({ ...GOOD, phoneCode: "other", phone }).ok, phone).toBe(false);
    }
    for (const phoneCode of ["971", "+999", "+971; drop", 971]) {
      expect(validateEnquiry({ ...GOOD, phoneCode }).ok, String(phoneCode)).toBe(false);
    }
    for (const phoneCode of DIAL_CODES) {
      const phone = phoneCode === "other" ? "+355 69 123 4567" : "123 4567";
      expect(validateEnquiry({ ...GOOD, phoneCode, phone }).ok, phoneCode).toBe(true);
    }
  });

  it("takes the country, size, turnover, sector and call time only from the fixed lists", () => {
    for (const country of COUNTRIES) expect(validateEnquiry({ ...GOOD, country }).ok).toBe(true);
    for (const size of SIZES) expect(validateEnquiry({ ...GOOD, size }).ok).toBe(true);
    for (const turnover of TURNOVERS) expect(validateEnquiry({ ...GOOD, turnover }).ok).toBe(true);
    for (const sector of SECTORS) expect(validateEnquiry({ ...GOOD, sector }).ok).toBe(true);
    for (const callTime of CALL_TIMES) expect(validateEnquiry({ ...GOOD, callTime }).ok).toBe(true);
    expect(validateEnquiry({ ...GOOD, country: "US" }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, size: "huge" }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, turnover: "a lot" }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, sector: "hacking" }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, callTime: "3am" }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, callTime: null }).ok).toBe(false);
  });

  it("refuses format characters that can reverse or hide text, in any field", () => {
    // Built from codes so this file never holds one raw.
    const codes = [0x202e, 0x202a, 0x2066, 0x2069, 0x200b, 0x200e, 0x200f, 0x061c, 0x2060, 0xfeff, 0x00ad];
    for (const code of codes) {
      const c = String.fromCharCode(code);
      expect(validateEnquiry({ ...GOOD, name: `Test${c}Person` }).ok, code.toString(16)).toBe(false);
      expect(validateEnquiry({ ...GOOD, company: `Example${c} LLC` }).ok, code.toString(16)).toBe(false);
      expect(validateEnquiry({ ...GOOD, message: `Hello${c}\nthere` }).ok, code.toString(16)).toBe(false);
    }
  });

  it("allows the zero-width non-joiner and joiner that Persian and Urdu names need", () => {
    const zwnj = String.fromCharCode(0x200c);
    const zwj = String.fromCharCode(0x200d);
    expect(validateEnquiry({ ...GOOD, name: `می${zwnj}خواهم` }).ok).toBe(true);
    expect(validateEnquiry({ ...GOOD, message: `a${zwj}b` }).ok).toBe(true);
  });
});
