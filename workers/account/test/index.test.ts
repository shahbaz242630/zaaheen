// The entry point's order of checks: route, then flood protection, then
// configuration. Nothing here reaches Clerk or Paddle: the configuration is
// left incomplete, so a request that passes the flood check stops at 503.
import { describe, expect, it } from "vitest";
import worker from "../src/index";
import type { Limiter } from "../src/flood";

function limiter(success: boolean): Limiter & { calls: number } {
  const l = {
    calls: 0,
    async limit() {
      l.calls++;
      return { success };
    },
  };
  return l;
}

const call = (path: string, env: Record<string, unknown>) =>
  worker.fetch(new Request(`https://api.example.test${path}`, { method: "POST", headers: { "cf-connecting-ip": "203.0.113.9" } }), env as unknown as Env);

describe("the Worker's entry point", () => {
  it("answers 404 for an unknown path without touching the limiter", async () => {
    const l = limiter(false);
    const res = await call("/nope", { FLOOD: l });
    expect(res.status).toBe(404);
    expect(l.calls).toBe(0);
  });

  it("refuses a flooding address with 429 before reading any configuration", async () => {
    const l = limiter(false);
    const res = await call("/v1/lease", { FLOOD: l });
    expect(res.status).toBe(429);
    expect(l.calls).toBe(1);
  });

  it("goes on to the configuration check once the address is under the limit", async () => {
    const l = limiter(true);
    const res = await call("/v1/checkout", { FLOOD: l });
    expect(res.status).toBe(503);
    expect(l.calls).toBe(1);
  });

  it("never asks the limiter about a webhook", async () => {
    const l = limiter(false);
    const res = await call("/paddle/webhook", { FLOOD: l });
    expect(res.status).toBe(503);
    expect(l.calls).toBe(0);
  });

  it("answers 503 when the rate-limit binding is missing", async () => {
    const res = await call("/v1/lease", {});
    expect(res.status).toBe(503);
  });
});
