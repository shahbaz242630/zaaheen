// The one door between the model's reply and the visitor (ADR-SEC-052 §3.9).
// The model answers in a fixed shape { on_topic, known, answer, pages }; this code,
// not the model, decides what is shown. Anything it cannot vouch for becomes
// our own sentence (src/text.ts).

/** An answer longer than this is replaced, never cut (ADR-117). */
export const MAX_ANSWER_WORDS = 250;

/** At most this many links under an answer. */
export const MAX_PAGES = 3;

const SITE = "https://zaaheen.com";

export type TurnAwayReason = "stop" | "shape" | "off_topic" | "empty" | "too_long" | "supplier" | "code";

export type Reply =
  // known: false = on topic, but our pages did not answer it (the question is kept, §3.8).
  | { kind: "answer"; text: string; pages: string[]; known: boolean }
  | { kind: "turned_away"; reason: TurnAwayReason };

/** Also a cap in characters, for languages that do not put spaces between words. */
export const MAX_ANSWER_CHARS = 2500;

// White label: the box never names the model behind it or its maker. "Claude"
// alone is not here: Claude Desktop and Claude Code are apps our pages tell
// people to connect. "Anthropic" alone is not here either: our comparison
// pages name Anthropic's reference memory server; only the ways an assistant
// names its own maker are.
const SUPPLIER = [
  /\bhaiku\b/i,
  /\bclaude-[a-z]+-\d/i,
  /\b(?:by|from)\s+anthropic\b/i,
  /\banthropic(?:'s|’s)?\s+(?:ai|model|assistant|llm|claude)\b/i,
];

// Characters that hide inside a word without showing: zero-width spaces and
// joiners, word joiner, byte-order mark, soft hyphen.
const INVISIBLE = /[­​-‍⁠﻿]/g;

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
  // The checks read the text as it looks: full-width letters folded, hidden characters gone.
  const plain = text.normalize("NFKC").replace(INVISIBLE, "");
  if (plain.split(/\s+/).length > MAX_ANSWER_WORDS || plain.length > MAX_ANSWER_CHARS) return away("too_long");
  if (SUPPLIER.some((pattern) => pattern.test(plain))) return away("supplier");
  if (!codeIsOurs(text, knowledge)) return away("code");

  return { kind: "answer", text, pages: ourPages(parsed.pages, knowledge), known: parsed.known };
}

function away(reason: TurnAwayReason): Reply {
  return { kind: "turned_away", reason };
}

function parse(raw: string): { onTopic: boolean; known: boolean; answer: string; pages: unknown[] } | null {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const { on_topic: onTopic, known, answer, pages } = value as Record<string, unknown>;
  if (typeof onTopic !== "boolean" || typeof known !== "boolean") return null;
  if (typeof answer !== "string" || !Array.isArray(pages)) return null;
  return { onTopic, known, answer, pages };
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

/** Only https://zaaheen.com pages that our knowledge names in full, once each. */
function ourPages(pages: unknown[], knowledge: string): string[] {
  const named = pagesIn(knowledge);
  const kept: string[] = [];
  for (const page of pages) {
    if (typeof page !== "string" || kept.includes(page)) continue;
    let url: URL;
    try {
      url = new URL(page);
    } catch {
      continue;
    }
    if (url.origin !== SITE || url.href !== page || !named.has(page)) continue;
    kept.push(page);
    if (kept.length === MAX_PAGES) break;
  }
  return kept;
}

/** Every whole https://zaaheen.com address in the knowledge, without a sentence's closing punctuation. */
function pagesIn(knowledge: string): Set<string> {
  const found = knowledge.match(/https:\/\/zaaheen\.com\/[^\s<>"'()[\]]*/g) ?? [];
  return new Set(found.map((page) => page.replace(/[.,;:!?]+$/, "")));
}
