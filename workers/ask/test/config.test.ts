// The Worker's settings that keep the box working (ADR-119, s120).
import { describe, expect, it } from "vitest";
import config from "../wrangler.jsonc?raw";

/** The settings without their comments, which mention these words too. */
const settings = config.replace(/^\s*\/\/.*$/gm, "");

describe("wrangler.jsonc", () => {
  it("runs the Worker at a fixed place in Europe, not nearest the visitor", () => {
    // Anthropic refuses unsupported regions: Hong Kong gave every failure of 2026-10-30 and 10-31.
    expect(settings).toMatch(/"placement":\s*\{\s*"region":\s*"aws:eu-central-1"\s*\}/);
  });

  it("keeps our failure lines but not a log of every visitor's request", () => {
    expect(settings).toMatch(/"observability":\s*\{\s*"enabled":\s*true,\s*"logs":\s*\{\s*"invocation_logs":\s*false\s*\}\s*\}/);
  });
});
