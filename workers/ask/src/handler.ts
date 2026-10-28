// POST /v1/ask: one question, through every check in order (ADR-117,
// ADR-SEC-052). Our chat page only; a passed Turnstile check or a valid
// session; the conversation rules; the daily limits; our pages; the model;
// the guard; then the counts and the kept question. Every way out is one of
// our own answers; nothing from an error reaches the visitor.
import { answer, type AskModel } from "./answer";
import { dayOf, visitorKey } from "./budget";
import { issueSession, verifySession, verifyTurnstile, type Fetcher } from "./gate";
import { checkConversation, type Turn } from "./limits";
import { claimQuestion, countOutcome, keepQuestion } from "./store";
import { RESTING, TURNED_AWAY, UNAVAILABLE } from "./text";

/** The chat page's own address: the only Origin allowed to ask. */
export const ORIGIN = "https://ask.zaaheen.com";

/** Our published pages for AI readers: the only knowledge. */
export const KNOWLEDGE_URL = "https://zaaheen.com/llms.txt";

/** A real request (6 questions, 6 answers, a token) is far below this. */
export const MAX_BODY_BYTES = 32 * 1024;

const MAX_KNOWLEDGE_CHARS = 200_000;

export interface AskEnv {
  DB: D1Database;
  ANTHROPIC_API_KEY: string;
  TURNSTILE_SECRET: string;
  SESSION_SECRET: string;
  VISITOR_SECRET: string;
}

export interface Deps {
  model: AskModel;
  fetch: Fetcher;
  now: () => number;
}

type Body = { turns: unknown; session?: string; turnstile?: string };

export async function handleAsk(request: Request, env: AskEnv, deps: Deps): Promise<Response> {
  if (request.method !== "POST") return reply(405, { kind: "invalid" });
  if (request.headers.get("Origin") !== ORIGIN) return reply(403, { kind: "forbidden" });
  if (!env.ANTHROPIC_API_KEY || !env.TURNSTILE_SECRET || !env.SESSION_SECRET || !env.VISITOR_SECRET) {
    return reply(503, { kind: "unavailable", text: UNAVAILABLE });
  }

  const raw = await request.text();
  if (new TextEncoder().encode(raw).length > MAX_BODY_BYTES) return reply(413, { kind: "invalid" });
  const body = parseBody(raw);
  if (!body) return reply(400, { kind: "invalid" });

  const now = deps.now();
  const address = request.headers.get("CF-Connecting-IP") ?? "unknown";

  // The check: a session we signed, or a Turnstile token Cloudflare accepts.
  let session: string | undefined;
  const inSession = body.session !== undefined && (await verifySession(body.session, now, env.SESSION_SECRET));
  if (!inSession) {
    const passed =
      body.turnstile !== undefined && (await verifyTurnstile(body.turnstile, address, env.TURNSTILE_SECRET, deps.fetch));
    if (!passed) return reply(401, { kind: "challenge" });
    session = await issueSession(now, env.SESSION_SECRET);
  }
  const withSession = (fields: Record<string, unknown>) => (session ? { ...fields, session } : fields);

  const conversation = checkConversation(body.turns);
  if (!conversation.ok) return reply(400, { kind: "invalid" });
  const question = lastQuestion(conversation.turns);

  const day = dayOf(now);
  const claim = await claimQuestion(env.DB, day, await visitorKey(address, day, env.VISITOR_SECRET));
  if (!claim.ok) {
    await countOutcome(env.DB, day, "resting");
    return reply(200, withSession({ kind: "resting", text: RESTING }));
  }

  const knowledge = await loadKnowledge(deps.fetch);
  if (knowledge === null) {
    await countOutcome(env.DB, day, "unavailable");
    return reply(200, withSession({ kind: "unavailable", text: UNAVAILABLE }));
  }

  const result = await answer(deps.model, knowledge, conversation.turns);
  switch (result.kind) {
    case "answer":
      await countOutcome(env.DB, day, "answered");
      if (!result.known) await keepQuestion(env.DB, now, "unknown", question);
      return reply(200, withSession({ kind: "answer", text: result.text, pages: result.pages }));
    case "turned_away":
      await countOutcome(env.DB, day, "turned_away");
      await keepQuestion(env.DB, now, result.reason, question);
      return reply(200, withSession({ kind: "turned_away", text: TURNED_AWAY }));
    case "unavailable":
      await countOutcome(env.DB, day, "unavailable");
      return reply(200, withSession({ kind: "unavailable", text: UNAVAILABLE }));
  }
}

function parseBody(raw: string): Body | null {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const allowed = new Set(["turns", "session", "turnstile"]);
  if (!Object.keys(value).every((key) => allowed.has(key)) || !("turns" in value)) return null;
  const { turns, session, turnstile } = value as Record<string, unknown>;
  if (session !== undefined && typeof session !== "string") return null;
  if (turnstile !== undefined && typeof turnstile !== "string") return null;
  return {
    turns,
    ...(session === undefined ? {} : { session }),
    ...(turnstile === undefined ? {} : { turnstile }),
  };
}

function lastQuestion(turns: Turn[]): string {
  return turns[turns.length - 1]?.text ?? "";
}

/** Our pages, cached at Cloudflare for an hour; null unless it is really our file. */
async function loadKnowledge(fetcher: Fetcher): Promise<string | null> {
  try {
    const response = await fetcher(KNOWLEDGE_URL, { cf: { cacheTtl: 3600, cacheEverything: true } });
    if (!response.ok) return null;
    const text = await response.text();
    if (text.length > MAX_KNOWLEDGE_CHARS || !text.startsWith("# Zaaheen")) return null;
    return text;
  } catch {
    return null;
  }
}

function reply(status: number, body: Record<string, unknown>): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    },
  });
}
