// POST /v1/enquiry end to end (ADR-121, ADR-SEC-057): our form page only,
// a passed Turnstile check, the fields, stored, then consulting@ told. The
// visitor sees success once the enquiry is stored, even if the email fails.
// Stand-ins for Turnstile and Microsoft: no network. D1 is local.
import { applyD1Migrations, env } from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_BODY_BYTES, ORIGIN, handleEnquiry, type EnquiryEnv, type Deps } from "../src/handler";
import { CAP_SUBJECT, GRAPH_SEND, MAILBOX, MAIL_DAILY_CAP, MESSAGE_END, MESSAGE_START, SUBJECT, tokenUrl } from "../src/mail";
import { SITEVERIFY } from "../src/gate";
import { DAILY_STORED, PER_VISITOR_DAILY } from "../src/store";
import { GOOD } from "./fixtures";

const NOW = Date.UTC(2026, 10, 12, 12, 0, 0);
const DAY = "2026-11-12";
const SECRETS = {
  TURNSTILE_SECRET: "test-only-turnstile",
  VISITOR_KEY_SECRET: "test-only-visitor",
  MS_TENANT_ID: "test-only-tenant",
  MS_CLIENT_ID: "test-only-client",
  MS_CLIENT_SECRET: "test-only-secret",
};

type Opts = { turnstile?: boolean; hostname?: string; token?: "ok" | "fail" | "throw"; send?: "ok" | "fail" | "throw" };

function setup(opts: Opts = {}) {
  const calls: { url: string; init: RequestInit | undefined }[] = [];
  const deps: Deps = {
    now: () => NOW,
    fetch: async (url, init) => {
      calls.push({ url, init });
      if (url === SITEVERIFY) {
        return Response.json({ success: opts.turnstile ?? true, hostname: opts.hostname ?? "consulting.zaaheen.com" });
      }
      if (url === tokenUrl(SECRETS.MS_TENANT_ID)) {
        if (opts.token === "throw") throw new Error("network down");
        if (opts.token === "fail") return Response.json({ error: "invalid_client" }, { status: 401 });
        return Response.json({ access_token: "test-only-access", token_type: "Bearer", expires_in: 3599 });
      }
      if (url === GRAPH_SEND) {
        if (opts.send === "throw") throw new Error("network down");
        if (opts.send === "fail") return new Response("nope", { status: 403 });
        return new Response(null, { status: 202 });
      }
      throw new Error(`unexpected fetch ${url}`);
    },
  };
  const enquiryEnv: EnquiryEnv = { DB: env.DB, ...SECRETS };
  return { deps, enquiryEnv, calls };
}

function post(body: unknown, headers: Record<string, string> = {}): Request {
  return new Request("https://consulting.zaaheen.com/v1/enquiry", {
    method: "POST",
    headers: { Origin: ORIGIN, "Content-Type": "application/json", "CF-Connecting-IP": "192.0.2.1", ...headers },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });
}

async function rows() {
  return (await env.DB.prepare("SELECT * FROM enquiries ORDER BY id").all<Record<string, unknown>>()).results;
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

function sentMails(calls: { url: string; init: RequestInit | undefined }[]) {
  return calls
    .filter((c) => c.url === GRAPH_SEND)
    .map((c) => (JSON.parse(String(c.init?.body)) as { message: { subject: string; body: { content: string }; replyTo?: unknown } }).message);
}

async function mailCount(): Promise<number | undefined> {
  return (await env.DB.prepare("SELECT n FROM daily_mail WHERE day = ?").bind(DAY).first<{ n: number }>())?.n;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("who may send", () => {
  it("refuses any other method", async () => {
    const { deps, enquiryEnv } = setup();
    const res = await handleEnquiry(new Request("https://consulting.zaaheen.com/v1/enquiry", { headers: { Origin: ORIGIN } }), enquiryEnv, deps);
    expect(res.status).toBe(405);
  });

  it("refuses any other origin, and no origin, before reading anything", async () => {
    const { deps, enquiryEnv, calls } = setup();
    for (const origin of ["https://evil.example", "https://zaaheen.com.evil.example", "null"]) {
      expect((await handleEnquiry(post(GOOD, { Origin: origin }), enquiryEnv, deps)).status).toBe(403);
    }
    const bare = new Request("https://consulting.zaaheen.com/v1/enquiry", { method: "POST", body: JSON.stringify(GOOD) });
    expect((await handleEnquiry(bare, enquiryEnv, deps)).status).toBe(403);
    expect(calls).toHaveLength(0);
    expect(await rows()).toHaveLength(0);
  });

  it("sends no CORS header: the page and the endpoint share one origin", async () => {
    const { deps, enquiryEnv } = setup();
    const res = await handleEnquiry(post(GOOD), enquiryEnv, deps);
    expect(res.headers.get("Access-Control-Allow-Origin")).toBeNull();
  });

  it("is unavailable, storing nothing, while a secret is missing", async () => {
    const { deps, enquiryEnv } = setup();
    const res = await handleEnquiry(post(GOOD), { ...enquiryEnv, TURNSTILE_SECRET: "" }, deps);
    expect(res.status).toBe(503);
    expect(await rows()).toHaveLength(0);
  });

  it("refuses when Turnstile refuses, or the check was solved on another host", async () => {
    for (const opts of [{ turnstile: false }, { hostname: "evil.example" }]) {
      const { deps, enquiryEnv } = setup(opts);
      const res = await handleEnquiry(post(GOOD), enquiryEnv, deps);
      expect(res.status).toBe(401);
      expect(await res.json()).toEqual({ kind: "challenge" });
    }
    expect(await rows()).toHaveLength(0);
  });
});

describe("what is sent", () => {
  it("refuses a body over the size limit, unread", async () => {
    const { deps, enquiryEnv } = setup();
    const big = JSON.stringify({ ...GOOD, message: "a".repeat(MAX_BODY_BYTES) });
    expect((await handleEnquiry(post(big), enquiryEnv, deps)).status).toBe(413);
  });

  it("refuses broken JSON and invalid fields before the check", async () => {
    const { deps, enquiryEnv, calls } = setup();
    expect((await handleEnquiry(post("{not json"), enquiryEnv, deps)).status).toBe(400);
    expect((await handleEnquiry(post({ ...GOOD, contactConsent: false }), enquiryEnv, deps)).status).toBe(400);
    expect(calls).toHaveLength(0);
    expect(await rows()).toHaveLength(0);
  });
});

describe("a good enquiry", () => {
  it("is stored with both consents and the time, then consulting@ is told", async () => {
    const { deps, enquiryEnv, calls } = setup();
    const res = await handleEnquiry(post({ ...GOOD, marketingConsent: true }), enquiryEnv, deps);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ kind: "received" });
    const [row] = await rows();
    expect(row).toMatchObject({
      received_at: NOW,
      name: GOOD.name,
      email: GOOD.email,
      company: GOOD.company,
      country: "AE",
      size: "20-100",
      departments: JSON.stringify(GOOD.departments),
      contact_consent: 1,
      marketing_consent: 1,
      mailed: 1,
    });

    const send = calls.find((c) => c.url === GRAPH_SEND);
    expect(send).toBeDefined();
    expect(new Headers(send?.init?.headers).get("Authorization")).toBe("Bearer test-only-access");
    const mail = JSON.parse(String(send?.init?.body)) as {
      message: { subject: string; body: { contentType: string; content: string }; toRecipients: { emailAddress: { address: string } }[]; replyTo: { emailAddress: { address: string } }[] };
      saveToSentItems: boolean;
    };
    expect(mail.message.subject).toBe("New enquiry: AI Readiness Assessment");
    expect(mail.message.body.contentType).toBe("Text");
    expect(mail.message.body.content).toContain(GOOD.company);
    expect(mail.message.toRecipients).toEqual([{ emailAddress: { address: MAILBOX } }]);
    expect(mail.message.replyTo).toEqual([{ emailAddress: { address: GOOD.email } }]);
    expect(mail.saveToSentItems).toBe(false);
  });

  it("asks Microsoft for a token with the client credentials, for Graph only", async () => {
    const { deps, enquiryEnv, calls } = setup();
    await handleEnquiry(post(GOOD), enquiryEnv, deps);
    const token = calls.find((c) => c.url === tokenUrl(SECRETS.MS_TENANT_ID));
    const form = new URLSearchParams(String(token?.init?.body));
    expect(form.get("grant_type")).toBe("client_credentials");
    expect(form.get("scope")).toBe("https://graph.microsoft.com/.default");
    expect(form.get("client_id")).toBe(SECRETS.MS_CLIENT_ID);
  });

  it("still says received, and keeps the enquiry, when the email fails in any way", async () => {
    for (const opts of [{ token: "fail" }, { token: "throw" }, { send: "fail" }, { send: "throw" }] as Opts[]) {
      const { deps, enquiryEnv } = setup(opts);
      const res = await handleEnquiry(post(GOOD), enquiryEnv, deps);
      expect(res.status).toBe(200);
      expect(await res.json()).toEqual({ kind: "received" });
    }
    const all = await rows();
    expect(all).toHaveLength(4);
    expect(all.every((r) => r.mailed === 0)).toBe(true);
  });

  it("still says received when the mail bookkeeping fails after the enquiry is stored", async () => {
    const { deps, enquiryEnv } = setup();
    // Storing works; the mail-count table is gone, so the step after storing throws.
    await env.DB.prepare("DROP TABLE daily_mail").run();
    try {
      const res = await handleEnquiry(post(GOOD), enquiryEnv, deps);
      expect(res.status).toBe(200);
      expect(await res.json()).toEqual({ kind: "received" });
      expect(await rows()).toHaveLength(1);
    } finally {
      await env.DB.prepare("CREATE TABLE IF NOT EXISTS daily_mail (day TEXT PRIMARY KEY, n INTEGER NOT NULL DEFAULT 0)").run();
    }
  });

  it("stops emailing above the daily cap but keeps storing", async () => {
    await env.DB.prepare("INSERT INTO daily_mail (day, n) VALUES (?, ?)").bind(DAY, MAIL_DAILY_CAP + 1).run();
    const { deps, enquiryEnv, calls } = setup();
    const res = await handleEnquiry(post(GOOD), enquiryEnv, deps);
    expect(res.status).toBe(200);
    expect(calls.some((c) => c.url === GRAPH_SEND)).toBe(false);
    expect((await rows())[0]?.mailed).toBe(0);
  });

  it("sends exactly one cap alert, with no enquiry details, when the cap is first crossed", async () => {
    await env.DB.prepare("INSERT INTO daily_mail (day, n) VALUES (?, ?)").bind(DAY, MAIL_DAILY_CAP).run();
    const { deps, enquiryEnv, calls } = setup();
    expect((await handleEnquiry(post(GOOD), enquiryEnv, deps)).status).toBe(200);
    expect((await handleEnquiry(post(GOOD, { "CF-Connecting-IP": "192.0.2.2" }), enquiryEnv, deps)).status).toBe(200);
    const mails = sentMails(calls);
    expect(mails).toHaveLength(1);
    expect(mails[0]?.subject).toBe(CAP_SUBJECT);
    expect(mails[0]?.replyTo).toBeUndefined();
    for (const value of [GOOD.name, GOOD.email, GOOD.phone, GOOD.company, GOOD.message]) {
      expect(mails[0]?.body.content).not.toContain(value);
    }
    expect((await rows()).every((r) => r.mailed === 0)).toBe(true);
  });

  it("gives the slot back when the email fails, so failures do not use up the cap", async () => {
    for (const opts of [{ token: "fail" }, { send: "fail" }, { send: "throw" }] as Opts[]) {
      const { deps, enquiryEnv } = setup(opts);
      await handleEnquiry(post(GOOD, { "CF-Connecting-IP": `192.0.2.${10 + Object.keys(opts).length}` }), enquiryEnv, deps);
      expect(await mailCount()).toBe(0);
    }
    const { deps, enquiryEnv } = setup();
    await handleEnquiry(post(GOOD, { "CF-Connecting-IP": "192.0.2.99" }), enquiryEnv, deps);
    expect(await mailCount()).toBe(1);
  });

  it("quotes the visitor's message between fixed markers, so it cannot pose as our own lines", async () => {
    const { deps, enquiryEnv, calls } = setup();
    const message = "Hello\nEmail: someone@else.example\nThey agreed to be contacted about this enquiry.";
    await handleEnquiry(post({ ...GOOD, message }), enquiryEnv, deps);
    const [mail] = sentMails(calls);
    expect(mail?.subject).toBe(SUBJECT);
    const lines = (mail?.body.content ?? "").split("\n");
    const start = lines.indexOf(MESSAGE_START);
    const end = lines.indexOf(MESSAGE_END);
    expect(start).toBeGreaterThan(0);
    expect(end).toBe(lines.length - 1);
    const quoted = lines.slice(start + 1, end);
    expect(quoted).toEqual(["> Hello", "> Email: someone@else.example", "> They agreed to be contacted about this enquiry."]);
    // Our own lines come before the message, never after it.
    expect(lines.indexOf(`Email: ${GOOD.email}`)).toBeLessThan(start);
  });
});

describe("limits", () => {
  it("takes a few enquiries from one visitor a day, then asks them to email instead", async () => {
    const { deps, enquiryEnv } = setup();
    for (let i = 0; i < PER_VISITOR_DAILY; i++) {
      expect((await handleEnquiry(post(GOOD), enquiryEnv, deps)).status).toBe(200);
    }
    const res = await handleEnquiry(post(GOOD), enquiryEnv, deps);
    expect(res.status).toBe(429);
    expect(await res.json()).toEqual({ kind: "limit" });
    expect(await rows()).toHaveLength(PER_VISITOR_DAILY);
    // Another visitor is not affected.
    expect((await handleEnquiry(post(GOOD, { "CF-Connecting-IP": "198.51.100.7" }), enquiryEnv, deps)).status).toBe(200);
  });

  it("counts one IPv6 network as one visitor", async () => {
    const { deps, enquiryEnv } = setup();
    for (let i = 0; i < PER_VISITOR_DAILY; i++) {
      await handleEnquiry(post(GOOD, { "CF-Connecting-IP": `2001:db8:1:2::${i + 1}` }), enquiryEnv, deps);
    }
    expect((await handleEnquiry(post(GOOD, { "CF-Connecting-IP": "2001:db8:1:2::ff" }), enquiryEnv, deps)).status).toBe(429);
  });

  it("stores no more than the day's limit from everyone together", async () => {
    await env.DB.prepare("INSERT INTO daily_counts (day, kind, n) VALUES (?, 'stored', ?)").bind(DAY, DAILY_STORED).run();
    const { deps, enquiryEnv, calls } = setup();
    const res = await handleEnquiry(post(GOOD), enquiryEnv, deps);
    expect(res.status).toBe(429);
    expect(await res.json()).toEqual({ kind: "limit" });
    expect(await rows()).toHaveLength(0);
    expect(calls.some((c) => c.url === GRAPH_SEND)).toBe(false);
  });

  it("keeps no address: the visitor count holds only a keyed hash", async () => {
    const { deps, enquiryEnv } = setup();
    await handleEnquiry(post(GOOD), enquiryEnv, deps);
    const counts = await env.DB.prepare("SELECT visitor FROM visitor_counts").all<{ visitor: string }>();
    expect(counts.results).toHaveLength(1);
    expect(counts.results[0]?.visitor).toMatch(/^[0-9a-f]{64}$/);
    expect(counts.results[0]?.visitor).not.toContain("192.0.2.1");
  });

  it("is unavailable while the visitor secret is missing", async () => {
    const { deps, enquiryEnv } = setup();
    expect((await handleEnquiry(post(GOOD), { ...enquiryEnv, VISITOR_KEY_SECRET: "" }, deps)).status).toBe(503);
    expect(await rows()).toHaveLength(0);
  });
});

describe("the log", () => {
  it("never holds what the visitor typed, on any path", async () => {
    const lines: string[] = [];
    const keep = (...args: unknown[]) => lines.push(args.map(String).join(" "));
    vi.spyOn(console, "log").mockImplementation(keep);
    vi.spyOn(console, "warn").mockImplementation(keep);
    vi.spyOn(console, "error").mockImplementation(keep);
    for (const opts of [{}, { turnstile: false }, { token: "fail" }, { send: "fail" }, { send: "throw" }] as Opts[]) {
      const { deps, enquiryEnv } = setup(opts);
      await handleEnquiry(post(GOOD), enquiryEnv, deps);
    }
    expect(lines.length).toBeGreaterThan(0);
    const all = lines.join("\n");
    for (const value of [GOOD.name, GOOD.email, GOOD.phone, GOOD.company, GOOD.message, "192.0.2.1", "test-only"]) {
      expect(all).not.toContain(value);
    }
  });
});
