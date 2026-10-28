// The sentences the question box shows in our own words (founder-approved,
// session 116; notes ASK-ZAAHEEN-DESIGN.md §6.4). The model's text never
// replaces these.

/** Shown instead of any reply that is off-topic, malformed or unsafe. */
export const TURNED_AWAY =
  "I can only help with questions about Zaaheen. For anything else, please email customerservice@zaaheen.com.";

/** Shown at a daily limit (budget.ts); no call to the model is made. */
export const RESTING = "The question box is resting for now. Please email customerservice@zaaheen.com and we'll help.";

/** The first message when the bubble opens. */
export const GREETING = "Hi! Ask me anything about Zaaheen: how it works, connecting your AI apps, pricing or your trial.";

/** Under the box, always. */
export const DISCLAIMER =
  "Answers come from our website and can be wrong. For anything important, check the linked page or email customerservice@zaaheen.com.";
