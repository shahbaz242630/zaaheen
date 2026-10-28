// ADR-SEC-052 §3.9: the model's reply reaches the visitor only through
// checkReply. Anything off-topic, malformed, too long, carrying code we did
// not publish, or naming the model becomes our own fixed sentence.
import { describe, expect, it } from "vitest";
import { MAX_ANSWER_WORDS, checkReply } from "../src/guard";
import { TURNED_AWAY } from "../src/text";

const KNOWLEDGE = [
  "# Zaaheen",
  "- Free trial: 30 days, no card needed.",
  "- Price after the trial: $5 a month or $48 a year.",
  "- Guide: https://zaaheen.com/docs/connect-cursor/",
  "- Pricing: https://zaaheen.com/pricing/",
  "Run zaaheen mcp serve to start the relay.",
].join("\n");

const reply = (fields: Record<string, unknown>) =>
  JSON.stringify({ on_topic: true, known: true, answer: "The trial is 30 days.", pages: [], ...fields });

describe("checkReply", () => {
  it("passes an on-topic answer and keeps our pages", () => {
    const r = checkReply(
      reply({ answer: "The trial is 30 days, no card needed.", pages: ["https://zaaheen.com/pricing/"] }),
      "end_turn",
      KNOWLEDGE,
    );
    expect(r).toEqual({
      kind: "answer",
      text: "The trial is 30 days, no card needed.",
      pages: ["https://zaaheen.com/pricing/"],
      known: true,
    });
  });

  it("shows an on-topic 'I don't know' answer, marked as not known, so the question is kept", () => {
    const answer = "I don't know that one. Please email customerservice@zaaheen.com.";
    expect(checkReply(reply({ known: false, answer }), "end_turn", KNOWLEDGE)).toEqual({
      kind: "answer",
      text: answer,
      pages: [],
      known: false,
    });
  });

  it("turns away an off-topic reply, whatever text the model wrote", () => {
    const r = checkReply(reply({ on_topic: false, answer: "Here is a poem about the sea." }), "end_turn", KNOWLEDGE);
    expect(r).toEqual({ kind: "turned_away", reason: "off_topic" });
  });

  it.each(["max_tokens", "refusal", "tool_use", null])("turns away a reply that stopped with %s", (stop) => {
    expect(checkReply(reply({}), stop, KNOWLEDGE)).toEqual({ kind: "turned_away", reason: "stop" });
  });

  it.each([
    ["not JSON", "The trial is 30 days."],
    ["no on_topic", JSON.stringify({ known: true, answer: "x", pages: [] })],
    ["no known", JSON.stringify({ on_topic: true, answer: "x", pages: [] })],
    ["known not boolean", JSON.stringify({ on_topic: true, known: "no", answer: "x", pages: [] })],
    ["on_topic not boolean", JSON.stringify({ on_topic: "yes", known: true, answer: "x", pages: [] })],
    ["answer not text", JSON.stringify({ on_topic: true, known: true, answer: 5, pages: [] })],
    ["pages not a list", JSON.stringify({ on_topic: true, known: true, answer: "x", pages: "https://zaaheen.com/" })],
    ["a list, not an object", "[]"],
  ])("turns away a malformed reply (%s)", (_name, raw) => {
    expect(checkReply(raw, "end_turn", KNOWLEDGE)).toEqual({ kind: "turned_away", reason: "shape" });
  });

  it("turns away an empty answer", () => {
    expect(checkReply(reply({ answer: "   " }), "end_turn", KNOWLEDGE)).toEqual({ kind: "turned_away", reason: "empty" });
  });

  it("replaces an answer over the word cap instead of cutting it", () => {
    const long = Array.from({ length: MAX_ANSWER_WORDS + 1 }, () => "word").join(" ");
    expect(checkReply(reply({ answer: long }), "end_turn", KNOWLEDGE)).toEqual({
      kind: "turned_away",
      reason: "too_long",
    });
    const atCap = Array.from({ length: MAX_ANSWER_WORDS }, () => "word").join(" ");
    expect(checkReply(reply({ answer: atCap }), "end_turn", KNOWLEDGE).kind).toBe("answer");
  });

  describe("code", () => {
    it("turns away a code block we never published", () => {
      const answer = "Try this:\n```python\nprint('hello')\n```";
      expect(checkReply(reply({ answer }), "end_turn", KNOWLEDGE)).toEqual({ kind: "turned_away", reason: "code" });
    });

    it("turns away inline code we never published", () => {
      const answer = "Run `rm -rf /` first.";
      expect(checkReply(reply({ answer }), "end_turn", KNOWLEDGE)).toEqual({ kind: "turned_away", reason: "code" });
    });

    it("keeps code copied word for word from our pages", () => {
      const answer = "Run `zaaheen mcp serve` to start the relay.";
      expect(checkReply(reply({ answer }), "end_turn", KNOWLEDGE).kind).toBe("answer");
    });

    it("turns away an unclosed code fence rather than guessing where it ends", () => {
      const answer = "Here:\n```\nanything";
      expect(checkReply(reply({ answer }), "end_turn", KNOWLEDGE)).toEqual({ kind: "turned_away", reason: "code" });
    });
  });

  it.each(["I am Claude Haiku.", "I run on HAIKU 5.5", "model claude-haiku-5-5"])(
    "turns away an answer naming the model (%s)",
    (answer) => {
      expect(checkReply(reply({ answer }), "end_turn", KNOWLEDGE)).toEqual({ kind: "turned_away", reason: "supplier" });
    },
  );

  describe("pages", () => {
    it("drops pages that are not ours or not in the knowledge, and duplicates", () => {
      const r = checkReply(
        reply({
          pages: [
            "https://zaaheen.com/pricing/",
            "https://zaaheen.com/pricing/",
            "https://evil.example/zaaheen.com/pricing/",
            "https://zaaheen.com/made-up-page/",
            "javascript:alert(1)",
            "http://zaaheen.com/pricing/",
            "https://zaaheen.com.evil.example/pricing/",
            42,
          ],
        }),
        "end_turn",
        KNOWLEDGE,
      );
      expect(r).toMatchObject({ kind: "answer", pages: ["https://zaaheen.com/pricing/"] });
    });

    it("keeps at most three pages", () => {
      const knowledge = [1, 2, 3, 4].map((n) => `https://zaaheen.com/p${n}/`).join("\n");
      const pages = [1, 2, 3, 4].map((n) => `https://zaaheen.com/p${n}/`);
      const r = checkReply(reply({ pages }), "end_turn", knowledge);
      expect(r).toMatchObject({ kind: "answer", pages: pages.slice(0, 3) });
    });
  });

  it("the fixed sentence is the founder-approved wording", () => {
    expect(TURNED_AWAY).toBe(
      "I can only help with questions about Zaaheen. For anything else, please email customerservice@zaaheen.com.",
    );
  });
});
