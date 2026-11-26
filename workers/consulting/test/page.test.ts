// The form page (public/) sends exactly the fields the Worker accepts, with
// the same fixed choices, and both consent boxes start unticked.
import { describe, expect, it } from "vitest";
import formHtml from "../public/index.html?raw";
import formJs from "../public/form.js?raw";
import headers from "../public/_headers?raw";
import { CALL_TIMES, COUNTRIES, DIAL_CODES, SECTORS, SIZES, TURNOVERS } from "../src/validate";

/** The option values inside one <select>, by its id. */
function options(id: string): string[] {
  const select = formHtml.match(new RegExp(`<select id="${id}"[\\s\\S]*?</select>`));
  expect(select, id).not.toBeNull();
  return [...(select?.[0] ?? "").matchAll(/<option value="([^"]*)"/g)].map((m) => m[1] ?? "");
}

describe("the form page", () => {
  it("offers exactly the Worker's choices in every list (founder s129 fields)", () => {
    expect(new Set(options("country"))).toEqual(new Set(["", ...COUNTRIES]));
    expect(new Set(options("size"))).toEqual(new Set(["", ...SIZES]));
    expect(new Set(options("turnover"))).toEqual(new Set(["", ...TURNOVERS]));
    expect(new Set(options("sector"))).toEqual(new Set(["", ...SECTORS]));
    expect(new Set(options("callTime"))).toEqual(new Set(CALL_TIMES));
    expect(new Set(options("phoneCode"))).toEqual(new Set(DIAL_CODES));
  });

  it("requires every field but the best time to call and the comments", () => {
    for (const id of ["name", "email", "phoneCode", "phone", "company", "country", "size", "turnover", "sector"]) {
      expect(formHtml, id).toMatch(new RegExp(`<(input|select) id="${id}"[^>]*\\brequired\\b`));
    }
    expect(formHtml).not.toMatch(/<select id="callTime"[^>]*\brequired\b/);
    expect(formHtml).not.toMatch(/<textarea id="message"[^>]*\brequired\b/);
    // Each optional field says so on its label (s129 restyle: an "Optional" tag).
    expect(formHtml).toContain('Best time to call, Gulf time <span class="opt">Optional</span>');
    expect(formHtml).toContain('Comments <span class="opt">Optional</span>');
    expect(formHtml.match(/class="opt">Optional</g)).toHaveLength(3);
  });

  it("sends every field the Worker expects, and no other", () => {
    for (const key of ["name", "email", "phoneCode", "phone", "company", "country", "size", "turnover", "sector", "callTime", "message", "contactConsent", "marketingConsent", "turnstile"]) {
      expect(formJs, key).toMatch(new RegExp(`\\b${key}:`));
    }
    expect(formJs).not.toContain("departments");
  });

  it("starts with both consent boxes unticked, and requires only the contact one", () => {
    const boxes = [...formHtml.matchAll(/<input type="checkbox" id="(contactConsent|marketingConsent)"[^>]*>/g)].map((m) => m[0]);
    expect(boxes).toHaveLength(2);
    for (const box of boxes) expect(box).not.toMatch(/\bchecked\b/);
    expect(boxes.find((b) => b.includes("contactConsent"))).toMatch(/\brequired\b/);
    expect(boxes.find((b) => b.includes("marketingConsent"))).not.toMatch(/\brequired\b/);
  });

  it("links the consulting privacy policy next to the consent (s129: its own documents)", () => {
    expect(formHtml).toContain("https://zaaheen.com/consulting/privacy/");
  });

  it("posts only to this host's endpoint", () => {
    expect(formJs).toContain('fetch("/v1/enquiry"');
    expect(formJs).not.toMatch(/fetch\("https?:/);
  });

  it("is served with a policy that allows only our files and Turnstile", () => {
    expect(headers).toContain("script-src 'self' https://challenges.cloudflare.com");
    expect(headers).toContain("connect-src 'self'");
    expect(headers).toContain("frame-ancestors 'none'");
  });
});
