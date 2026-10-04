// CORS for the website account page (AUTH-PAGES-DESIGN S85-1, ADR-SEC-043):
// only the exact account origin, POST, two request headers, no credentials.
import { describe, expect, it } from "vitest";

import { WEB_ROUTES, corsPreflight, withCors } from "../src/cors";

const ORIGIN = "https://account.example.test";

function req(method: string, origin: string | null, path = "/v1/web/plan"): Request {
  const headers: Record<string, string> = {};
  if (origin !== null) headers["origin"] = origin;
  if (method === "OPTIONS") {
    headers["access-control-request-method"] = "POST";
    headers["access-control-request-headers"] = "authorization, content-type";
  }
  return new Request(`https://api.example.test${path}`, { method, headers });
}

describe("the routes a website may call", () => {
  it("are exactly the plan, checkout and cancel", () => {
    expect([...WEB_ROUTES].sort()).toEqual(["/v1/cancel", "/v1/checkout", "/v1/web/plan"]);
  });
});

describe("a preflight", () => {
  it("from our account origin is allowed for POST with the two headers, no credentials", () => {
    const res = corsPreflight(req("OPTIONS", ORIGIN), ORIGIN);
    expect(res.status).toBe(204);
    expect(res.headers.get("access-control-allow-origin")).toBe(ORIGIN);
    expect(res.headers.get("access-control-allow-methods")).toBe("POST");
    expect(res.headers.get("access-control-allow-headers")).toBe("authorization, content-type");
    expect(res.headers.get("access-control-allow-credentials")).toBeNull();
    expect(res.headers.get("vary")).toBe("Origin");
  });

  it("from any other origin gets no CORS headers", () => {
    for (const other of ["https://evil.test", `${ORIGIN}.evil.test`, "http://account.example.test", "null"]) {
      const res = corsPreflight(req("OPTIONS", other), ORIGIN);
      expect(res.status).toBe(403);
      expect(res.headers.get("access-control-allow-origin")).toBeNull();
    }
  });

  it("with no account origin configured gets none either", () => {
    const res = corsPreflight(req("OPTIONS", ORIGIN), undefined);
    expect(res.headers.get("access-control-allow-origin")).toBeNull();
  });
});

describe("an answer", () => {
  it("to our origin carries the allow-origin header", () => {
    const res = withCors(new Response("{}", { status: 200 }), req("POST", ORIGIN), ORIGIN);
    expect(res.headers.get("access-control-allow-origin")).toBe(ORIGIN);
    expect(res.headers.get("vary")).toBe("Origin");
  });

  it("to another origin, or to the app (no Origin), is left unchanged", () => {
    for (const r of [req("POST", "https://evil.test"), req("POST", null)]) {
      const res = withCors(new Response("{}", { status: 200 }), r, ORIGIN);
      expect(res.headers.get("access-control-allow-origin")).toBeNull();
    }
  });
});
