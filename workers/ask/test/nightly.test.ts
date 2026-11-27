// s130: the daily cron also runs the consulting form's nightly job (the free
// plan's 5 crons per account are all used). Ours runs first, theirs is
// called once, and their failure never fails ours.
import { applyD1Migrations, env } from "cloudflare:test";
import { beforeEach, describe, expect, it } from "vitest";
import worker from "../src/index";
import type { AskEnv } from "../src/handler";

const controller = { scheduledTime: Date.now(), cron: "10 0 * * *", noRetry() {} } as ScheduledController;

beforeEach(async () => {
  await applyD1Migrations(env.DB, env.TEST_MIGRATIONS);
});

function askEnv(consulting?: AskEnv["CONSULTING"]): AskEnv {
  const base: AskEnv = { DB: env.DB, ANTHROPIC_API_KEY: "", TURNSTILE_SECRET: "", SESSION_SECRET: "", VISITOR_SECRET: "" };
  return consulting ? { ...base, CONSULTING: consulting } : base;
}

describe("daily cron", () => {
  it("runs the consulting nightly job once", async () => {
    let calls = 0;
    await worker.scheduled!(controller, askEnv({ run: async () => void calls++ }));
    expect(calls).toBe(1);
  });

  it("does not fail when the consulting job fails", async () => {
    await expect(
      worker.scheduled!(controller, askEnv({ run: async () => { throw new Error("down"); } })),
    ).resolves.toBeUndefined();
  });

  it("still sweeps when the binding is absent", async () => {
    await expect(worker.scheduled!(controller, askEnv(undefined))).resolves.toBeUndefined();
  });
});
