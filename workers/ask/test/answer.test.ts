// ADR-117 / ADR-SEC-052: the one call to the model. Tests use a stand-in
// model: no network, no cost. They pin what is sent (model, effort, no tools,
// the fixed reply shape, our pages as cached knowledge) and that every reply
// still goes through checkReply.
import type Anthropic from "@anthropic-ai/sdk";
import { describe, expect, it } from "vitest";
import { MAX_OUTPUT_TOKENS, MODEL, REPLY_SCHEMA, answer, type AskModel } from "../src/answer";
import { RULES } from "../src/prompt";

const KNOWLEDGE = "# Zaaheen\n- Free trial: 30 days, no card needed.\n- Pricing: https://zaaheen.com/pricing/";

function message(content: Anthropic.ContentBlock[], stop: Anthropic.StopReason | null = "end_turn"): Anthropic.Message {
  return {
    id: "msg_test",
    type: "message",
    role: "assistant",
    model: MODEL,
    content,
    stop_reason: stop,
    stop_sequence: null,
    usage: {} as Anthropic.Usage,
  } as Anthropic.Message;
}

const text = (t: string) => ({ type: "text", text: t, citations: null }) as Anthropic.TextBlock;
const json = (v: unknown) => text(JSON.stringify(v));

function stand_in(reply: Anthropic.Message | Error) {
  const sent: Anthropic.MessageCreateParamsNonStreaming[] = [];
  const model: AskModel = {
    async create(params) {
      sent.push(params);
      if (reply instanceof Error) throw reply;
      return reply;
    },
  };
  return { model, sent };
}

const ON_TOPIC = { on_topic: true, answer: "The trial is 30 days.", pages: ["https://zaaheen.com/pricing/"] };

describe("answer", () => {
  it("asks the chosen model with low effort, no tools, thinking off and the fixed reply shape", async () => {
    const { model, sent } = stand_in(message([json(ON_TOPIC)]));
    await answer(model, KNOWLEDGE, [{ role: "user", text: "How long is the trial?" }]);
    const p = sent[0]!;
    expect(p.model).toBe("claude-haiku-5-5");
    expect(p.max_tokens).toBe(MAX_OUTPUT_TOKENS);
    expect(p.output_config).toEqual({ effort: "low", format: { type: "json_schema", schema: REPLY_SCHEMA } });
    expect(p.thinking).toEqual({ type: "disabled" });
    expect(p.tools).toBeUndefined();
    expect(p.tool_choice).toBeUndefined();
  });

  it("sends the rules first and our pages as one cached block, then the conversation as given", async () => {
    const { model, sent } = stand_in(message([json(ON_TOPIC)]));
    await answer(model, KNOWLEDGE, [
      { role: "user", text: "Price?" },
      { role: "assistant", text: "$5 a month." },
      { role: "user", text: "And yearly?" },
    ]);
    const p = sent[0]!;
    expect(p.system).toEqual([
      { type: "text", text: RULES },
      { type: "text", text: `<knowledge>\n${KNOWLEDGE}\n</knowledge>`, cache_control: { type: "ephemeral" } },
    ]);
    expect(p.messages).toEqual([
      { role: "user", content: "Price?" },
      { role: "assistant", content: "$5 a month." },
      { role: "user", content: "And yearly?" },
    ]);
  });

  it("the reply shape allows exactly the three fields", () => {
    expect(REPLY_SCHEMA).toMatchObject({
      type: "object",
      additionalProperties: false,
      required: ["on_topic", "answer", "pages"],
    });
    expect(Object.keys(REPLY_SCHEMA.properties).sort()).toEqual(["answer", "on_topic", "pages"]);
  });

  it("returns a checked answer", async () => {
    const { model } = stand_in(message([json(ON_TOPIC)]));
    expect(await answer(model, KNOWLEDGE, [{ role: "user", text: "Trial?" }])).toEqual({
      kind: "answer",
      text: "The trial is 30 days.",
      pages: ["https://zaaheen.com/pricing/"],
    });
  });

  it("joins the text blocks and ignores anything else", async () => {
    const raw = JSON.stringify(ON_TOPIC);
    const thinking = { type: "thinking", thinking: "", signature: "x" } as Anthropic.ThinkingBlock;
    const { model } = stand_in(message([thinking, text(raw.slice(0, 10)), text(raw.slice(10))]));
    expect((await answer(model, KNOWLEDGE, [{ role: "user", text: "Trial?" }])).kind).toBe("answer");
  });

  it("an off-topic reply comes back turned away", async () => {
    const { model } = stand_in(message([json({ on_topic: false, answer: "A poem.", pages: [] })]));
    expect(await answer(model, KNOWLEDGE, [{ role: "user", text: "Write a poem" }])).toEqual({
      kind: "turned_away",
      reason: "off_topic",
    });
  });

  it("a refusal comes back turned away, its text never shown", async () => {
    const { model } = stand_in(message([text("I won't help with that.")], "refusal"));
    expect(await answer(model, KNOWLEDGE, [{ role: "user", text: "x" }])).toEqual({
      kind: "turned_away",
      reason: "stop",
    });
  });

  it("any failure to reach the model is 'unavailable', with nothing from the error", async () => {
    const { model } = stand_in(new Error("401 invalid x-api-key sk-ant-secret"));
    expect(await answer(model, KNOWLEDGE, [{ role: "user", text: "x" }])).toEqual({ kind: "unavailable" });
  });
});

describe("RULES", () => {
  it("keeps the model to our pages and our subject", () => {
    expect(RULES).toContain("only from the text inside <knowledge>");
    expect(RULES).toContain("on_topic");
    expect(RULES).toMatch(/never (name|say) .*model/i);
    expect(RULES).toContain("customerservice@zaaheen.com");
  });

  it("holds nothing that changes per request (so the cache keeps working)", () => {
    expect(RULES).not.toMatch(/\d{4}-\d{2}-\d{2}/);
  });
});
