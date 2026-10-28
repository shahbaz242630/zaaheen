// ADR-SEC-052 §3.3: Turnstile before the first question, then a short signed
// session so follow-ups need no new challenge. Turnstile's server is a
// stand-in here: no network.
import { describe, expect, it } from "vitest";
import { SESSION_MS, SITEVERIFY, issueSession, verifySession, verifyTurnstile } from "../src/gate";

const SECRET = "test-only-session-secret";
const NOW = Date.UTC(2026, 9, 28, 12, 0, 0);

function siteverify(reply: unknown, status = 200) {
  const calls: { url: string; body: string }[] = [];
  const fetcher = async (url: string, init?: RequestInit) => {
    calls.push({ url, body: String(init?.body) });
    return new Response(JSON.stringify(reply), { status });
  };
  return { fetcher, calls };
}

describe("verifyTurnstile", () => {
  it("passes a token Cloudflare accepts for our host, sending the secret, token and address", async () => {
    const { fetcher, calls } = siteverify({ success: true, hostname: "ask.zaaheen.com" });
    expect(await verifyTurnstile("tok", "203.0.113.7", "ts-secret", fetcher)).toBe(true);
    expect(calls[0]?.url).toBe(SITEVERIFY);
    const sent = new URLSearchParams(calls[0]?.body);
    expect(sent.get("secret")).toBe("ts-secret");
    expect(sent.get("response")).toBe("tok");
    expect(sent.get("remoteip")).toBe("203.0.113.7");
  });

  it.each([
    ["refused", { success: false, hostname: "ask.zaaheen.com" }, 200],
    ["solved on another site", { success: true, hostname: "evil.example" }, 200],
    ["not JSON-shaped", "nope", 200],
    ["server error", { success: true, hostname: "ask.zaaheen.com" }, 500],
  ])("refuses a token that is %s", async (_name, reply, status) => {
    const { fetcher } = siteverify(reply, status);
    expect(await verifyTurnstile("tok", "203.0.113.7", "ts-secret", fetcher)).toBe(false);
  });

  it("refuses when Cloudflare cannot be reached", async () => {
    const fetcher = async () => {
      throw new Error("network down");
    };
    expect(await verifyTurnstile("tok", "203.0.113.7", "ts-secret", fetcher)).toBe(false);
  });

  it("refuses an empty or over-long token without asking Cloudflare", async () => {
    const { fetcher, calls } = siteverify({ success: true, hostname: "ask.zaaheen.com" });
    expect(await verifyTurnstile("", "203.0.113.7", "ts-secret", fetcher)).toBe(false);
    expect(await verifyTurnstile("x".repeat(3000), "203.0.113.7", "ts-secret", fetcher)).toBe(false);
    expect(calls).toHaveLength(0);
  });
});

describe("sessions", () => {
  it("a session we issued is valid until it expires", async () => {
    const token = await issueSession(NOW, SECRET);
    expect(await verifySession(token, NOW, SECRET)).toBe(true);
    expect(await verifySession(token, NOW + SESSION_MS, SECRET)).toBe(true);
    expect(await verifySession(token, NOW + SESSION_MS + 1, SECRET)).toBe(false);
  });

  it("lasts 30 minutes", () => {
    expect(SESSION_MS).toBe(30 * 60 * 1000);
  });

  it("two sessions are never the same", async () => {
    expect(await issueSession(NOW, SECRET)).not.toBe(await issueSession(NOW, SECRET));
  });

  it("refuses a session signed with another secret", async () => {
    expect(await verifySession(await issueSession(NOW, "other"), NOW, SECRET)).toBe(false);
  });

  it("refuses a session whose expiry was changed", async () => {
    const token = await issueSession(NOW, SECRET);
    const [version, , nonce, mac] = token.split(".");
    const later = String(NOW + 365 * 24 * 60 * 60 * 1000);
    expect(await verifySession([version, later, nonce, mac].join("."), NOW, SECRET)).toBe(false);
  });

  it.each(["", "garbage", "v1.notanumber.abc.def", "v1.1.2", "v2.1.2.3", "v1.9999999999999.n.zz"])(
    "refuses a malformed session %j",
    async (token) => {
      expect(await verifySession(token, NOW, SECRET)).toBe(false);
    },
  );
});
