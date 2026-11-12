// The form's fields (ADR-SEC-057 §2): exact keys, fixed choices, length
// limits, no control characters, and the contact consent always required.
import { describe, expect, it } from "vitest";
import { COUNTRIES, DEPARTMENTS, LIMITS, SIZES, validateEnquiry } from "../src/validate";
import { GOOD } from "./fixtures";

describe("validateEnquiry", () => {
  it("accepts a complete, ordinary enquiry and trims it", () => {
    const result = validateEnquiry({ ...GOOD, name: "  Test Person  " });
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.enquiry.name).toBe("Test Person");
      expect(result.turnstile).toBe("token");
      expect(result.enquiry.departments).toEqual(["customer-service", "finance"]);
    }
  });

  it("accepts an empty message and no departments", () => {
    expect(validateEnquiry({ ...GOOD, message: "", departments: [] }).ok).toBe(true);
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

  it("takes only a phone number made of digits and spacing", () => {
    expect(validateEnquiry({ ...GOOD, phone: "(04) 123-4567" }).ok).toBe(true);
    for (const phone of ["12", "call me", "+971 50 000 0000 ext 5", "1".repeat(LIMITS.phone + 1)]) {
      expect(validateEnquiry({ ...GOOD, phone }).ok).toBe(false);
    }
  });

  it("takes the country, size and departments only from the fixed lists", () => {
    for (const country of COUNTRIES) expect(validateEnquiry({ ...GOOD, country }).ok).toBe(true);
    for (const size of SIZES) expect(validateEnquiry({ ...GOOD, size }).ok).toBe(true);
    expect(validateEnquiry({ ...GOOD, country: "US" }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, size: "huge" }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, departments: ["hacking"] }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, departments: "finance" }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, departments: ["finance", "finance"] }).ok).toBe(false);
    expect(validateEnquiry({ ...GOOD, departments: [...DEPARTMENTS] }).ok).toBe(true);
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
