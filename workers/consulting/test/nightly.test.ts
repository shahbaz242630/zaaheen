// The nightly job (ADR-SEC-057 §5, review s123): one alert email, counts
// only, when enquiries were not emailed or the form hit its limit; then the
// sweep, whose failure is logged. Stand-in for Microsoft: no network.
import { applyD1Migrations, env } from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EnquiryEnv } from "../src/handler";
import { nightly } from "../src/index";
import { GRAPH_SEND, NIGHTLY_SUBJECT, tokenUrl } from "../src/mail";
import { DAILY_STORED, KEEP_MS, markMailed, storeEnquiry } from "../src/store";
import { validateEnquiry } from "../src/validate";
import { GOOD } from "./fixtures";

const NOW = Date.UTC(2026, 10, 13, 0, 20, 0);
const HOUR = 60 * 60 * 1000;
const SECRETS = {
  TURNSTILE_SECRET: "test-only-turnstile",
  VISITOR_KEY_SECRET: "test-only-visitor",
  MS_TENANT_ID: "test-only-tenant",
  MS_CLIENT_ID: "test-only-client",
  MS_CLIENT_SECRET: "test-only-secret",
};

function stub() {
  const sent: { subject: string; content: string }[] = [];
  const fetcher = async (url: string, init?: RequestInit) => {
    if (url === tokenUrl(SECRETS.MS_TENANT_ID)) return Response.json({ access_token: "test-only-access" });
    if (url === GRAPH_SEND) {
      const m = (JSON.parse(String(init?.body)) as { message: { subject: string; body: { content: string } } }).message;
      sent.push({ subject: m.subject, content: m.body.content });
      return new Response(null, { status: 202 });
    }
    throw new Error(`unexpected fetch ${url}`);
  };
  return { sent, fetcher };
}

const enquiryEnv = (): EnquiryEnv => ({ DB: env.DB, ...SECRETS });

async function store(at: number, mailed: boolean) {
  const v = validateEnquiry(GOOD);
  if (!v.ok) throw new Error("fixture invalid");
  const id = await storeEnquiry(env.DB, at, v.enquiry);
  if (mailed) await markMailed(env.DB, id);
}

beforeEach(async () => {
  await applyD1Migrations(env.DB, env.TEST_MIGRATIONS);
  await env.DB.batch([
    env.DB.prepare("DELETE FROM enquiries"),
    env.DB.prepare("DELETE FROM daily_mail"),
    env.DB.prepare("DELETE FROM visitor_counts"),
    env.DB.prepare("DELETE FROM daily_counts"),
  ]);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("nightly", () => {
  it("sends one alert, counts only, when enquiries in the last day were not emailed", async () => {
    await store(NOW - 2 * HOUR, false);
    await store(NOW - 3 * HOUR, false);
    await store(NOW - 4 * HOUR, true);
    await store(NOW - 30 * HOUR, false); // older than a day: already reported
    const { sent, fetcher } = stub();
    await nightly(enquiryEnv(), fetcher, NOW);
    expect(sent).toHaveLength(1);
    expect(sent[0]?.subject).toBe(NIGHTLY_SUBJECT);
    expect(sent[0]?.content).toContain("not emailed: 2.");
    for (const value of [GOOD.name, GOOD.email, GOOD.phone, GOOD.company, GOOD.message]) {
      expect(sent[0]?.content).not.toContain(value);
    }
  });

  it("sends nothing when every enquiry was emailed", async () => {
    await store(NOW - 2 * HOUR, true);
    const { sent, fetcher } = stub();
    await nightly(enquiryEnv(), fetcher, NOW);
    expect(sent).toHaveLength(0);
  });

  it("says so when yesterday reached the form's limit", async () => {
    await env.DB.prepare("INSERT INTO daily_counts (day, kind, n) VALUES ('2026-11-12', 'stored', ?)").bind(DAILY_STORED).run();
    const { sent, fetcher } = stub();
    await nightly(enquiryEnv(), fetcher, NOW);
    expect(sent).toHaveLength(1);
    expect(sent[0]?.content).toContain(`limit of ${DAILY_STORED} enquiries`);
  });

  it("still sweeps when the alert cannot be sent, and logs why without details", async () => {
    await store(NOW - 2 * HOUR, false);
    await store(NOW - KEEP_MS - 1, true);
    const lines: string[] = [];
    vi.spyOn(console, "error").mockImplementation((...a: unknown[]) => lines.push(a.map(String).join(" ")));
    vi.spyOn(console, "warn").mockImplementation((...a: unknown[]) => lines.push(a.map(String).join(" ")));
    await nightly({ ...enquiryEnv(), MS_CLIENT_SECRET: "" }, stub().fetcher, NOW);
    expect(lines.join("\n")).toContain("consulting_alert_failed secret_missing");
    const left = await env.DB.prepare("SELECT COUNT(*) AS n FROM enquiries").first<{ n: number }>();
    expect(left?.n).toBe(1);
  });

  it("logs a reason-only line when the sweep fails", async () => {
    const lines: string[] = [];
    vi.spyOn(console, "error").mockImplementation((...a: unknown[]) => lines.push(a.map(String).join(" ")));
    await env.DB.prepare("DROP TABLE visitor_counts").run();
    try {
      await nightly(enquiryEnv(), stub().fetcher, NOW);
      expect(lines).toContain("consulting_sweep_failed");
    } finally {
      await env.DB.prepare(
        "CREATE TABLE IF NOT EXISTS visitor_counts (day TEXT NOT NULL, visitor TEXT NOT NULL, n INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (day, visitor))",
      ).run();
    }
  });
});
