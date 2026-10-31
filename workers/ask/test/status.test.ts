// The watch's read (ADR-120, ADR-SEC-055): one count and the day, nothing else,
// from the real route against a local database.
import { applyD1Migrations, env } from "cloudflare:test";
import { beforeEach, describe, expect, it } from "vitest";
import { dayOf } from "../src/budget";
import worker from "../src/index";
import { countOutcome } from "../src/store";

const call = (init?: RequestInit, path = "/v1/status") =>
  worker.fetch!(new Request(`https://ask.zaaheen.com${path}`, init) as never, env as never);

beforeEach(async () => {
  await applyD1Migrations(env.DB, env.TEST_MIGRATIONS);
  await env.DB.prepare("DELETE FROM daily_counts").run();
});

describe("/v1/status", () => {
  it("says zero on a quiet day", async () => {
    const res = await call();
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ day: dayOf(Date.now()), unavailable: 0 });
  });

  it("gives today's failures and no other count", async () => {
    const today = dayOf(Date.now());
    await countOutcome(env.DB, today, "unavailable");
    await countOutcome(env.DB, today, "unavailable");
    await countOutcome(env.DB, today, "answered");
    await countOutcome(env.DB, "2020-01-01", "unavailable");
    const res = await call();
    expect(await res.json()).toEqual({ day: today, unavailable: 2 });
    expect(res.headers.get("cache-control")).toBe("no-store");
  });

  it("is only for reading", async () => {
    expect((await call({ method: "POST" })).status).toBe(405);
  });

  it("leaves other addresses not found", async () => {
    expect((await call(undefined, "/v1/other")).status).toBe(404);
  });
});
