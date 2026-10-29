// The chat page (public/chat/) shows our approved sentences word for word and
// keeps the same limits as the Worker: a quiet drift between the page and
// src/text.ts or src/limits.ts would show visitors unapproved words.
import { describe, expect, it } from "vitest";
import chatJs from "../public/chat/chat.js?raw";
import chatHtml from "../public/chat/index.html?raw";
import { MAX_QUESTIONS } from "../src/limits";
import { DISCLAIMER, END_OF_CHAT, GREETING, UNAVAILABLE } from "../src/text";

const js = (name: string) => new RegExp(`const ${name} = ([^;]+);`).exec(chatJs)?.[1];

describe("the chat page", () => {
  it("allows as many questions as the Worker", () => {
    expect(js("MAX_QUESTIONS")).toBe(String(MAX_QUESTIONS));
  });

  it("uses the approved sentences", () => {
    expect(js("ERROR")).toBe(JSON.stringify(UNAVAILABLE));
    expect(js("END")).toBe(JSON.stringify(END_OF_CHAT));
    expect(chatHtml).toContain(GREETING);
    expect(chatHtml).toContain(DISCLAIMER);
  });

  it("says so when a chat reaches its last question, instead of going quiet", () => {
    expect(chatJs).toMatch(/asked >= MAX_QUESTIONS\) \{\s*add\(END, "box"\);/);
  });

  it("the end-of-chat line is the founder-approved wording", () => {
    expect(END_OF_CHAT).toBe(
      "That's the most questions for one chat. To ask more, close this box and open it again, or email customerservice@zaaheen.com.",
    );
  });
});
