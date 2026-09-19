// Coarse flood protection (§8.30 residual: "Flooding /v1/lease with bogus
// tokens costs Clerk verify calls"). The rate-limit binding is permissive
// and per-location (§8.28), so it only stops floods; per-user limits live in
// the record (`live_fetch_at`).
import { describe, expect, it } from "vitest";
import { FLOOD_LIMITED_ROUTES, floodCheck, type Limiter } from "../src/flood";

function limiter(answer: boolean | "throws"): Limiter & { keys: string[] } {
  const keys: string[] = [];
  return {
    keys,
    async limit({ key }) {
      keys.push(key);
      if (answer === "throws") throw new Error("limiter down");
      return { success: answer };
    },
  };
}

const post = (path: string, ip?: string) =>
  new Request(`https://api.example.test${path}`, { method: "POST", headers: ip === undefined ? {} : { "cf-connecting-ip": ip } });

describe("floodCheck", () => {
  it("limits the two routes that cost upstream calls per request", () => {
    expect([...FLOOD_LIMITED_ROUTES].sort()).toEqual(["/v1/checkout", "/v1/lease"]);
  });

  it("lets a request through while the address is under the limit, keyed by the client address", async () => {
    const l = limiter(true);
    expect(await floodCheck(post("/v1/lease", "203.0.113.9"), "/v1/lease", l)).toBeNull();
    expect(l.keys).toEqual(["203.0.113.9"]);
  });

  it("answers 429 with Retry-After once the address is over the limit", async () => {
    const res = await floodCheck(post("/v1/checkout", "203.0.113.9"), "/v1/checkout", limiter(false));
    expect(res?.status).toBe(429);
    expect(res?.headers.get("retry-after")).toBe("60");
    expect(res?.headers.get("cache-control")).toBe("no-store");
    expect(await res?.json()).toEqual({ error: "rate_limited" });
  });

  it("never limits the webhooks: a refused notification is a lost payment update", async () => {
    for (const path of ["/paddle/webhook", "/clerk/webhook"]) {
      const l = limiter(false);
      expect(await floodCheck(post(path, "203.0.113.9"), path, l)).toBeNull();
      expect(l.keys).toEqual([]);
    }
  });

  it("uses one shared key when the client address is missing (never an empty key)", async () => {
    const l = limiter(true);
    await floodCheck(post("/v1/lease"), "/v1/lease", l);
    expect(l.keys).toEqual(["unknown"]);
  });

  it("lets the request through when the limiter itself fails: it is flood protection, not a gate", async () => {
    expect(await floodCheck(post("/v1/lease", "203.0.113.9"), "/v1/lease", limiter("throws"))).toBeNull();
  });

  it("answers 503 when the binding is missing: a deploy mistake, like any other missing configuration", async () => {
    const res = await floodCheck(post("/v1/lease", "203.0.113.9"), "/v1/lease", undefined);
    expect(res?.status).toBe(503);
    expect(await res?.json()).toEqual({ error: "unavailable" });
  });
});
