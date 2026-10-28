// The one call to the model (ADR-117, ADR-SEC-052). No tools, no web, no
// actions: the request can only produce a reply, and the reply reaches the
// visitor only through checkReply (guard.ts).
import type Anthropic from "@anthropic-ai/sdk";
import { checkReply, type Reply } from "./guard";
import type { Turn } from "./limits";
import { RULES, knowledgeBlock } from "./prompt";

/** Founder s116: the Haiku model, for cost. */
export const MODEL = "claude-haiku-5-5";

/** A 250-word answer in the reply shape is well under this. */
export const MAX_OUTPUT_TOKENS = 1024;

/** The fixed reply shape (ADR-SEC-052 §3.9). */
export const REPLY_SCHEMA = {
  type: "object",
  additionalProperties: false,
  required: ["on_topic", "answer", "pages"],
  properties: {
    on_topic: { type: "boolean" },
    answer: { type: "string" },
    pages: { type: "array", items: { type: "string" } },
  },
} as const;

/** The part of the SDK client this file uses; tests pass a stand-in. */
export interface AskModel {
  create(params: Anthropic.MessageCreateParamsNonStreaming): Promise<Anthropic.Message>;
}

export type AskResult = Reply | { kind: "unavailable" };

export async function answer(model: AskModel, knowledge: string, turns: Turn[]): Promise<AskResult> {
  let message: Anthropic.Message;
  try {
    message = await model.create({
      model: MODEL,
      max_tokens: MAX_OUTPUT_TOKENS,
      // A short answer from given text needs no reasoning; low effort, thinking off.
      thinking: { type: "disabled" },
      output_config: { effort: "low", format: { type: "json_schema", schema: REPLY_SCHEMA } },
      system: [
        { type: "text", text: RULES },
        { type: "text", text: knowledgeBlock(knowledge), cache_control: { type: "ephemeral" } },
      ],
      messages: turns.map((turn) => ({ role: turn.role, content: turn.text })),
    });
  } catch {
    // Nothing from the error reaches the visitor or the reply: it can name the key.
    return { kind: "unavailable" };
  }

  const raw = message.content
    .filter((block): block is Anthropic.TextBlock => block.type === "text")
    .map((block) => block.text)
    .join("");
  return checkReply(raw, message.stop_reason, knowledge);
}
