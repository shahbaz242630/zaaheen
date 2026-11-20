// The form page (public/) sends exactly the fields the Worker accepts, with
// the same fixed choices, and both consent boxes start unticked.
import { describe, expect, it } from "vitest";
import formHtml from "../public/index.html?raw";
import formJs from "../public/form.js?raw";
import headers from "../public/_headers?raw";
import { COUNTRIES, DEPARTMENTS, SIZES } from "../src/validate";

describe("the form page", () => {
  it("offers exactly the Worker's countries, sizes and departments", () => {
    const values = (name: string) => [...formHtml.matchAll(new RegExp(`name="${name}"[^>]*value="([^"]+)"|<option value="([^"]+)"`, "g"))];
    for (const c of COUNTRIES) expect(formHtml).toContain(`<option value="${c}"`);
    for (const s of SIZES) expect(formHtml).toContain(`<option value="${s}"`);
    for (const d of DEPARTMENTS) expect(formHtml).toContain(`value="${d}"`);
    expect(values("departments").length).toBeGreaterThan(0);
  });

  it("starts with both consent boxes unticked, and requires only the contact one", () => {
    const boxes = [...formHtml.matchAll(/<input type="checkbox" id="(contactConsent|marketingConsent)"[^>]*>/g)].map((m) => m[0]);
    expect(boxes).toHaveLength(2);
    for (const box of boxes) expect(box).not.toMatch(/\bchecked\b/);
    expect(boxes.find((b) => b.includes("contactConsent"))).toMatch(/\brequired\b/);
    expect(boxes.find((b) => b.includes("marketingConsent"))).not.toMatch(/\brequired\b/);
  });

  it("links the privacy notice next to the consent", () => {
    expect(formHtml).toContain("https://zaaheen.com/privacy/#consulting");
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
