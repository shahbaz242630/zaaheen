// The s129 form fields (founder: phone code + number, turnover, sector, best
// time to call) and the review's Low findings: the trunk 0, values that look
// like object keys, a line break hidden in a phone, and every choice reaching
// the email with its own words.
import { describe, expect, it } from "vitest";
import { mailText } from "../src/mail";
import { CALL_TIMES, COUNTRIES, type Enquiry, SECTORS, SIZES, TURNOVERS, validateEnquiry } from "../src/validate";
import { GOOD } from "./fixtures";

function enquiry(over: Partial<typeof GOOD> | Record<string, unknown> = {}): Enquiry {
  const result = validateEnquiry({ ...GOOD, ...over });
  if (!result.ok) throw new Error(`refused: ${JSON.stringify(over)}`);
  return result.enquiry;
}

describe("the phone number", () => {
  it("drops the one trunk 0 people dial at home, so the number can be called back", () => {
    expect(enquiry({ phone: "050 123 4567" }).phone).toBe("+971 50 123 4567");
    expect(enquiry({ phoneCode: "+44", phone: "(020) 1234 5678" }).phone).toBe("+44 (20) 1234 5678");
    expect(enquiry({ phone: "50 123 4567" }).phone).toBe("+971 50 123 4567");
    // Only one 0, and only at the start.
    expect(enquiry({ phoneCode: "+39", phone: "06 1234 5678" }).phone).toBe("+39 6 1234 5678");
    // Too short once the 0 is gone.
    expect(validateEnquiry({ ...GOOD, phone: "0123" }).ok).toBe(false);
  });

  it("never lets a line break or control character into the number", () => {
    for (const phone of ["+44 20\n1234 5678", "+44 20\r1234", "+44 20\u00851234 5678", `+44 20${String.fromCharCode(0x2028)}1234`]) {
      expect(validateEnquiry({ ...GOOD, phoneCode: "other", phone }).ok, JSON.stringify(phone)).toBe(false);
    }
    expect(validateEnquiry({ ...GOOD, phone: "50 123\n4567" }).ok).toBe(false);
  });
});

describe("the fixed lists", () => {
  it("refuse values that look like object keys", () => {
    for (const key of ["phoneCode", "country", "size", "turnover", "sector", "callTime"]) {
      for (const bad of ["__proto__", "constructor", "toString", "hasOwnProperty"]) {
        expect(validateEnquiry({ ...GOOD, [key]: bad }).ok, `${key}=${bad}`).toBe(false);
      }
    }
  });
});

describe("the email to consulting@", () => {
  it("names every choice in the page's own words, never 'undefined'", () => {
    const seen: string[] = [];
    for (const country of COUNTRIES) seen.push(mailText(1, enquiry({ country }), 0));
    for (const size of SIZES) seen.push(mailText(1, enquiry({ size }), 0));
    for (const turnover of TURNOVERS) seen.push(mailText(1, enquiry({ turnover }), 0));
    for (const sector of SECTORS) seen.push(mailText(1, enquiry({ sector }), 0));
    for (const callTime of CALL_TIMES) seen.push(mailText(1, enquiry({ callTime }), 0));
    for (const text of seen) {
      expect(text).not.toContain("undefined");
      expect(text).not.toContain("[object");
    }
  });

  it("carries the new fields, the full phone number, and 'no preference' when no time was given", () => {
    const text = mailText(7, enquiry({ phone: "050 123 4567", callTime: "" }), 0);
    expect(text).toContain("Phone: +971 50 123 4567");
    expect(text).toContain("Best time to call (Gulf time): no preference");
    expect(text).toContain("Annual turnover: AED 5 to 20 million");
    expect(text).toContain("Sector: Trading and distribution");
    expect(text).toContain("Number of staff: 20-100");
    expect(mailText(7, enquiry({ callTime: "morning" }), 0)).toContain("Best time to call (Gulf time): Morning, 9 to 12");
  });
});
