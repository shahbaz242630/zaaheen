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
