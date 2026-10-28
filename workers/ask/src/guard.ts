// The one door between the model's reply and the visitor (ADR-SEC-052 §3.9).
// The model answers in a fixed shape { on_topic, answer, pages }; this code,
// not the model, decides what is shown. Anything it cannot vouch for becomes
// our own sentence (src/text.ts).

/** An answer longer than this is replaced, never cut (ADR-117). */
export const MAX_ANSWER_WORDS = 250;

/** At most this many links under an answer. */
export const MAX_PAGES = 3;

const SITE = "https://zaaheen.com";

export type TurnAwayReason = "stop" | "shape" | "off_topic" | "empty" | "too_long" | "supplier" | "code";

export type Reply =
  | { kind: "answer"; text: string; pages: string[] }
  | { kind: "turned_away"; reason: TurnAwayReason };

// White label: the box never names the model behind it. "Claude" alone is not
// here: Claude Desktop and Claude Code are apps our pages tell people to connect.
const SUPPLIER = [/\bhaiku\b/i, /\bclaude-[a-z]+-\d/i];

const FENCE = "```";
const FENCED = /```[^\n]*\n?([\s\S]*?)```/g;
const INLINE = /`([^`\n]+)`/g;

/**
 * Decide what the visitor sees. `raw` is the model's structured reply as
 * text, `stopReason` the API's stop reason, `knowledge` the page text the
 * answer had to come from.
 */
export function checkReply(raw: string, stopReason: string | null, knowledge: string): Reply {
  if (stopReason !== "end_turn") return away("stop");

  const parsed = parse(raw);
  if (!parsed) return away("shape");
  if (!parsed.onTopic) return away("off_topic");

  const text = parsed.answer.trim();
  if (text === "") return away("empty");
  if (text.split(/\s+/).length > MAX_ANSWER_WORDS) return away("too_long");
  if (SUPPLIER.some((pattern) => pattern.test(text))) return away("supplier");
  if (!codeIsOurs(text, knowledge)) return away("code");

  return { kind: "answer", text, pages: ourPages(parsed.pages, knowledge) };
}

function away(reason: TurnAwayReason): Reply {
  return { kind: "turned_away", reason };
}

function parse(raw: string): { onTopic: boolean; answer: string; pages: unknown[] } | null {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const { on_topic: onTopic, answer, pages } = value as Record<string, unknown>;
  if (typeof onTopic !== "boolean" || typeof answer !== "string" || !Array.isArray(pages)) return null;
  return { onTopic, answer, pages };
}

/** Every code fence and inline code span must be copied from our pages. */
function codeIsOurs(text: string, knowledge: string): boolean {
  // An odd number of fences means one never closes: refuse rather than guess.
  if (text.split(FENCE).length % 2 === 0) return false;
  for (const [, body] of text.matchAll(FENCED)) {
    if (!knowledge.includes((body ?? "").trim())) return false;
  }
  const outsideFences = text.replace(FENCED, "");
  for (const [, body] of outsideFences.matchAll(INLINE)) {
    if (!knowledge.includes((body ?? "").trim())) return false;
  }
  return true;
}

/** Only https://zaaheen.com pages that our knowledge names, once each. */
function ourPages(pages: unknown[], knowledge: string): string[] {
  const kept: string[] = [];
  for (const page of pages) {
    if (typeof page !== "string" || kept.includes(page)) continue;
    let url: URL;
    try {
      url = new URL(page);
    } catch {
      continue;
    }
    if (url.origin !== SITE || url.href !== page || !knowledge.includes(page)) continue;
    kept.push(page);
    if (kept.length === MAX_PAGES) break;
  }
  return kept;
}
