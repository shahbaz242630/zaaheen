// The conversation the page sends with each question (ADR-117). Nothing is
// kept between requests, so the whole conversation is checked every time:
// the page runs in a visitor's browser and can be tampered with.

/** Longest question a visitor may type. */
export const MAX_QUESTION_CHARS = 500;

/** Questions per conversation. */
export const MAX_QUESTIONS = 6;

/** Longest earlier answer accepted back: well above a real one (250 words). */
export const MAX_ANSWER_CHARS = 3000;

export type Turn = { role: "user" | "assistant"; text: string };

export type ConversationCheck =
  | { ok: true; turns: Turn[] }
  | { ok: false; reason: "shape" | "order" | "too_many" | "empty" | "too_long" };

export function checkConversation(value: unknown): ConversationCheck {
  if (!Array.isArray(value) || value.length === 0) return { ok: false, reason: "shape" };

  const turns: Turn[] = [];
  for (const item of value) {
    const turn = asTurn(item);
    if (!turn) return { ok: false, reason: "shape" };
    turns.push(turn);
  }

  // user, assistant, user, ... ending with the question being asked now.
  const inOrder = turns.every((turn, i) => turn.role === (i % 2 === 0 ? "user" : "assistant"));
  if (!inOrder || turns.length % 2 === 0) return { ok: false, reason: "order" };
  if ((turns.length + 1) / 2 > MAX_QUESTIONS) return { ok: false, reason: "too_many" };

  const checked: Turn[] = [];
  for (const turn of turns) {
    if (turn.role === "assistant") {
      if (turn.text.length > MAX_ANSWER_CHARS) return { ok: false, reason: "too_long" };
      checked.push(turn);
      continue;
    }
    const text = turn.text.trim();
    if (text === "") return { ok: false, reason: "empty" };
    if (text.length > MAX_QUESTION_CHARS) return { ok: false, reason: "too_long" };
    checked.push({ role: "user", text });
  }
  return { ok: true, turns: checked };
}

function asTurn(item: unknown): Turn | null {
  if (typeof item !== "object" || item === null || Array.isArray(item)) return null;
  const keys = Object.keys(item);
  if (keys.length !== 2 || !keys.includes("role") || !keys.includes("text")) return null;
  const { role, text } = item as Record<string, unknown>;
  if ((role !== "user" && role !== "assistant") || typeof text !== "string") return null;
  return { role, text };
}
