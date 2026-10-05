// The two deployments in wrangler.jsonc (SIGNIN-DESIGN.md §8.31, S6): production
// on api.zaaheen.com, the sandbox on api-sandbox.zaaheen.com, never sharing a
// store or a rate-limit namespace, and neither reachable on workers.dev. Read
// from the real file, so a deploy mistake fails here before it reaches Cloudflare.
import { describe, expect, it } from "vitest";
import raw from "../wrangler.jsonc?raw";

/** JSONC to JSON: drops `//` comments outside strings (the file has no block comments). */
function parseJsonc(text: string): Record<string, any> {
  let out = "";
  let inString = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (inString) {
      out += c;
      if (c === "\\") out += text[++i];
      else if (c === '"') inString = false;
    } else if (c === '"') {
      inString = true;
      out += c;
    } else if (c === "/" && text[i + 1] === "/") {
      while (i < text.length && text[i] !== "\n") i++;
      out += "\n";
    } else {
      out += c;
    }
  }
  return JSON.parse(out);
}

const top = parseJsonc(raw);
const sandbox = top.env.sandbox;
const PLACEHOLDER_KV = "00000000000000000000000000000001";

describe("wrangler.jsonc deployments", () => {
  it("production answers on api.zaaheen.com only, as a custom domain", () => {
    expect(top.routes).toEqual([{ pattern: "api.zaaheen.com", custom_domain: true }]);
  });

  it("the sandbox answers on api-sandbox.zaaheen.com only", () => {
    expect(sandbox.routes).toEqual([{ pattern: "api-sandbox.zaaheen.com", custom_domain: true }]);
  });

  it("production has a real trial store, not the placeholder", () => {
    expect(top.kv_namespaces).toHaveLength(1);
    expect(top.kv_namespaces[0].binding).toBe("TRIALS");
    expect(top.kv_namespaces[0].id).toMatch(/^[0-9a-f]{32}$/);
    expect(top.kv_namespaces[0].id).not.toBe(PLACEHOLDER_KV);
  });

  it("the two deployments never share a trial store or a rate-limit namespace", () => {
    expect(top.kv_namespaces[0].id).not.toBe(sandbox.kv_namespaces[0].id);
    expect(top.ratelimits[0].namespace_id).not.toBe(sandbox.ratelimits[0].namespace_id);
    expect(top.name).not.toBe(sandbox.name);
  });

  it("neither deployment is reachable on workers.dev or preview URLs", () => {
    for (const d of [top, sandbox]) {
      expect(d.workers_dev).toBe(false);
      expect(d.preview_urls).toBe(false);
    }
  });

  it("both run the daily renewal sweep", () => {
    expect(top.triggers.crons).toEqual(["17 3 * * *"]);
    expect(sandbox.triggers.crons).toEqual(["17 3 * * *"]);
  });
});

// AUTH-PAGES-DESIGN S85-1, ADR-SEC-043: production's three website settings
// are the real public ones and read as a website config; the sandbox has none.
describe("the website account page's settings", () => {
  it("production names our issuer and origin and a public RSA key that readConfig accepts", async () => {
    const { readConfig } = await import("../src/config");
    expect(top.vars.CLERK_ISSUER).toBe("https://clerk.zaaheen.com");
    expect(top.vars.ACCOUNT_ORIGIN).toBe("https://account.zaaheen.com");
    const key = JSON.parse(top.vars.CLERK_JWT_KEY);
    expect(Object.keys(key).sort()).toEqual(["e", "kty", "n"]);
    const env = {
      CLERK_SECRET_KEY: "sk_live_x", CLERK_OAUTH_CLIENT_ID: "c", CLERK_WEBHOOK_SECRET: "whsec_x",
      PADDLE_API_KEY: "pdl_live_apikey_x", PADDLE_ENVIRONMENT: "live", PADDLE_PRODUCT_ID: "pro_01aaaaaaaaaaaaaaaaaaaaaaaa",
      PADDLE_PRICE_MONTHLY: "pri_01bbbbbbbbbbbbbbbbbbbbbbbb", PADDLE_PRICE_ANNUAL: "pri_01cccccccccccccccccccccccc",
      PADDLE_WEBHOOK_SECRET: "x", LEASE_PRIMARY_KID: "prod-p1", LEASE_PRIMARY_KEY: "x", TRIAL_KEY: "k".repeat(32),
      ...top.vars,
    };
    expect(readConfig(env)?.web?.origin).toBe("https://account.zaaheen.com");
  });

  it("the sandbox has none, so its website parts stay off", () => {
    for (const k of ["CLERK_JWT_KEY", "CLERK_ISSUER", "ACCOUNT_ORIGIN"]) expect(sandbox.vars[k]).toBeUndefined();
  });

  it("both announce the same newest app version", () => {
    expect(top.vars.LATEST_APP_VERSION).toBe(sandbox.vars.LATEST_APP_VERSION);
  });
});
