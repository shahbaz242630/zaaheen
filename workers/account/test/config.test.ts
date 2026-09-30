// Reading the Worker's configuration (§8.30). A missing or mismatched value
// makes the Worker answer 503 ("try later"), never a refusal, and nothing
// account-specific lives in the public repo: every value comes from
// Cloudflare secrets (or .dev.vars locally).
import { describe, expect, it } from "vitest";

import { readConfig } from "../src/config";

const env: Record<string, string> = {
  CLERK_SECRET_KEY: "sk_test_abc",
  CLERK_OAUTH_CLIENT_ID: "mZOxuTESTclient1",
  CLERK_WEBHOOK_SECRET: "whsec_placeholder",
  PADDLE_API_KEY: "pdl_sdbx_apikey_abc",
  PADDLE_ENVIRONMENT: "sandbox",
  PADDLE_PRODUCT_ID: "pro_01aaaaaaaaaaaaaaaaaaaaaaaa",
  PADDLE_PRICE_MONTHLY: "pri_01bbbbbbbbbbbbbbbbbbbbbbbb",
  PADDLE_PRICE_ANNUAL: "pri_01cccccccccccccccccccccccc",
  PADDLE_WEBHOOK_SECRET: "pdl_ntfset_test",
  LEASE_PRIMARY_KID: "primary",
  LEASE_PRIMARY_KEY: "placeholder-not-a-key",
  TRIAL_KEY: "k".repeat(32),
};

describe("readConfig", () => {
  it("reads a complete configuration", () => {
    expect(readConfig(env)).toEqual({
      clerk: { secretKey: "sk_test_abc", clientId: "mZOxuTESTclient1", webhookSecret: "whsec_placeholder" },
      paddle: {
        apiKey: "pdl_sdbx_apikey_abc",
        environment: "sandbox",
        productId: "pro_01aaaaaaaaaaaaaaaaaaaaaaaa",
        prices: { monthly: "pri_01bbbbbbbbbbbbbbbbbbbbbbbb", annual: "pri_01cccccccccccccccccccccccc" },
        webhookSecret: "pdl_ntfset_test",
      },
      lease: { kid: "primary", pkcs8: env["LEASE_PRIMARY_KEY"] },
      trials: { key: "k".repeat(32) },
      killSwitch: false,
    });
  });

  // Launch checklist B6: the newest app version, for the app's "a new version
  // is available" bar. Optional: missing or malformed never makes the Worker
  // 503, it only leaves the bar off.
  it("reads the latest app version only when it is a plain x.y.z", () => {
    expect(readConfig(env)?.latestAppVersion).toBeUndefined();
    expect(readConfig({ ...env, LATEST_APP_VERSION: "0.3.1" })?.latestAppVersion).toBe("0.3.1");
    expect(readConfig({ ...env, LATEST_APP_VERSION: "10.20.300" })?.latestAppVersion).toBe("10.20.300");
    for (const bad of ["", "v0.3.1", "0.3", "0.3.1.2", "0.3.1-beta", "0.3.1 ", "<b>1.0.0</b>", "01.2.3"]) {
      const cfg = readConfig({ ...env, LATEST_APP_VERSION: bad });
      expect(cfg).not.toBeNull();
      expect(cfg?.latestAppVersion).toBeUndefined();
    }
  });

  it("the kill switch is on only for exactly '1'", () => {
    expect(readConfig({ ...env, NEVER_END_PAYERS: "1" })?.killSwitch).toBe(true);
    for (const v of ["0", "true", "yes", ""]) expect(readConfig({ ...env, NEVER_END_PAYERS: v })?.killSwitch).toBe(false);
  });

  for (const name of Object.keys(env)) {
    it(`is null without ${name}`, () => {
      const partial = { ...env };
      delete partial[name];
      expect(readConfig(partial)).toBeNull();
      expect(readConfig({ ...env, [name]: "" })).toBeNull();
    });
  }

  it("is null when the Paddle key does not match the Paddle environment", () => {
    expect(readConfig({ ...env, PADDLE_API_KEY: "pdl_live_apikey_abc" })).toBeNull();
    expect(readConfig({ ...env, PADDLE_ENVIRONMENT: "live" })).toBeNull();
    expect(readConfig({ ...env, PADDLE_ENVIRONMENT: "live", PADDLE_API_KEY: "pdl_live_apikey_abc", CLERK_SECRET_KEY: "sk_live_abc" })?.paddle.environment).toBe("live");
    expect(readConfig({ ...env, PADDLE_ENVIRONMENT: "production" })).toBeNull();
  });

  // Session 68 (review M1): live payments with a test sign-in instance, or the
  // other way round, would make the sweep see every payer's account as gone.
  it("is null when Clerk's instance does not match Paddle's environment", () => {
    const live = { ...env, PADDLE_ENVIRONMENT: "live", PADDLE_API_KEY: "pdl_live_apikey_abc" };
    expect(readConfig(live)).toBeNull();
    expect(readConfig({ ...live, CLERK_SECRET_KEY: "sk_live_abc" })).not.toBeNull();
    expect(readConfig({ ...env, CLERK_SECRET_KEY: "sk_live_abc" })).toBeNull();
    expect(readConfig(env)).not.toBeNull();
  });

  it("is null for malformed identifiers", () => {
    expect(readConfig({ ...env, CLERK_SECRET_KEY: "pk_test_abc" })).toBeNull();
    expect(readConfig({ ...env, PADDLE_PRODUCT_ID: "pri_01aaaaaaaaaaaaaaaaaaaaaaaa" })).toBeNull();
    expect(readConfig({ ...env, LEASE_PRIMARY_KID: "Primary" })).toBeNull();
    expect(readConfig({ ...env, CLERK_WEBHOOK_SECRET: "missing-the-whsec-prefix" })).toBeNull();
    expect(readConfig({ ...env, PADDLE_PRICE_MONTHLY: "pro_01aaaaaaaaaaaaaaaaaaaaaaaa" })).toBeNull();
    expect(readConfig({ ...env, PADDLE_PRICE_ANNUAL: env["PADDLE_PRICE_MONTHLY"] as string })).toBeNull();
  });

  it("is null for a TRIAL_KEY shorter than 32 characters (ACCOUNT-DELETION-DESIGN D8)", () => {
    expect(readConfig({ ...env, TRIAL_KEY: "k".repeat(31) })).toBeNull();
    expect(readConfig({ ...env, TRIAL_KEY: "k".repeat(64) })?.trials.key).toBe("k".repeat(64));
  });
});
