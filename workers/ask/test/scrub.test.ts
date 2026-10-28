// ADR-SEC-052 §3.8: an unanswered question is kept for 30 days without any
// identity. Before it is stored, anything shaped like an email address, a
// phone number or a card number is blanked.
import { describe, expect, it } from "vitest";
import { REMOVED, scrub } from "../src/scrub";

describe("scrub", () => {
  it.each([
    ["my email is sam.lee+test@example.co.uk, why?", `my email is ${REMOVED}, why?`],
    ["call +44 20 7946 0958 please", `call ${REMOVED} please`],
    ["call (020) 7946-0958 please", `call ${REMOVED} please`],
    ["card 4111 1111 1111 1111 charged twice", `card ${REMOVED} charged twice`],
    ["card 4111-1111-1111-1111 charged twice", `card ${REMOVED} charged twice`],
    ["card 4111111111111111 charged twice", `card ${REMOVED} charged twice`],
  ])("blanks %j", (input, expected) => {
    expect(scrub(input)).toBe(expected);
  });

  it("blanks every match, not just the first", () => {
    expect(scrub("a@b.com and c@d.org")).toBe(`${REMOVED} and ${REMOVED}`);
  });

  it.each([
    "Does Zaaheen work on version 0.3.8?",
    "Is it $5 a month or $48 a year?",
    "I have 30 days left in 2026",
    "What does zaaheen mcp serve do?",
  ])("leaves an ordinary question alone (%j)", (q) => {
    expect(scrub(q)).toBe(q);
  });
});
