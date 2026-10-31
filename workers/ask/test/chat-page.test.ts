// The chat page (public/chat/) shows our approved sentences word for word and
// keeps the same limits as the Worker: a quiet drift between the page and
// src/text.ts or src/limits.ts would show visitors unapproved words. Its
// translations and its list of translated pages follow zaaheen.com's
// (site/src/i18n/), so a visitor on /de/ gets the box in German and German links.
import { describe, expect, it } from "vitest";
import chatJs from "../public/chat/chat.js?raw";
import chatHtml from "../public/chat/index.html?raw";
import siteLocales from "../../../site/src/i18n/locales.ts?raw";
import siteDe from "../../../site/src/i18n/ui/de.ts?raw";
import siteEs from "../../../site/src/i18n/ui/es.ts?raw";
import siteFr from "../../../site/src/i18n/ui/fr.ts?raw";
import sitePt from "../../../site/src/i18n/ui/pt.ts?raw";
import { MAX_QUESTIONS } from "../src/limits";
import { DISCLAIMER, END_OF_CHAT, GREETING, REGION, RESTING, TURNED_AWAY, UNAVAILABLE } from "../src/text";

const js = (name: string) => new RegExp(`const ${name} = ([^;]+);`).exec(chatJs)?.[1];
// The two tables are written as JSON inside chat.js, so they parse as written.
const WORDS = JSON.parse(/const WORDS = (\{[\s\S]*?\n {2}\});/.exec(chatJs)?.[1] ?? "null") as { en: Record<string, string> } & Record<string, Record<string, string>>;
const TRANSLATED = JSON.parse(/const TRANSLATED = (\[[\s\S]*?\n {2}\]);/.exec(chatJs)?.[1] ?? "null") as string[];
const SITE_UI: Record<string, string> = { de: siteDe, es: siteEs, fr: siteFr, pt: sitePt };

describe("the chat page", () => {
  it("allows as many questions as the Worker", () => {
    expect(js("MAX_QUESTIONS")).toBe(String(MAX_QUESTIONS));
  });

  it("uses the approved sentences in English", () => {
    expect(WORDS.en.greeting).toBe(GREETING);
    expect(WORDS.en.note).toBe(DISCLAIMER);
    expect(WORDS.en.unavailable).toBe(UNAVAILABLE);
    expect(WORDS.en.end).toBe(END_OF_CHAT);
    expect(WORDS.en.turned_away).toBe(TURNED_AWAY);
    expect(WORDS.en.resting).toBe(RESTING);
    expect(WORDS.en.region).toBe(REGION);
    // Before the script runs, and without it, the page is the English one.
    expect(chatHtml).toContain(GREETING);
    expect(chatHtml).toContain(DISCLAIMER);
  });

  it("says so when a chat reaches its last question, instead of going quiet", () => {
    expect(chatJs).toMatch(/asked >= MAX_QUESTIONS\) \{\s*add\(t\.end, "box"\);/);
  });

  it("the end-of-chat line is the founder-approved wording", () => {
    expect(END_OF_CHAT).toBe(
      "That's the most questions for one chat. To ask more, close this box and open it again, or email customerservice@zaaheen.com.",
    );
  });
});

describe("the chat page in the site's languages", () => {
  it("speaks every language zaaheen.com does", () => {
    const langs = /export const LANGS[^=]*=\s*\[([^\]]*)\]/.exec(siteLocales)?.[1]?.match(/'([a-z]{2})'/g)?.map((q) => q.slice(1, -1));
    expect(langs?.length).toBeGreaterThan(1);
    expect(Object.keys(WORDS).sort()).toEqual([...(langs ?? [])].sort());
  });

  it("has every line in every language, each keeping the support address", () => {
    for (const [lang, words] of Object.entries(WORDS)) {
      expect(Object.keys(words).sort(), lang).toEqual(Object.keys(WORDS.en).sort());
      for (const [key, text] of Object.entries(words)) {
        expect(text.trim().length, `${lang}.${key}`).toBeGreaterThan(0);
        if (WORDS.en[key]?.includes("customerservice@zaaheen.com")) {
          expect(text, `${lang}.${key}`).toContain("customerservice@zaaheen.com");
        }
      }
    }
  });

  it("names the box as the site's bubble does", () => {
    for (const [lang, ui] of Object.entries(SITE_UI)) {
      expect(ui, lang).toContain(`'Ask Zaaheen': '${WORDS[lang]?.title}'`);
    }
  });

  it("knows the same translated pages as the site", () => {
    const site = /export const TRANSLATED[^=]*=\s*\[([\s\S]*?)\];/.exec(siteLocales)?.[1]?.match(/'([^']+)'/g)?.map((q) => q.slice(1, -1));
    expect(site?.length).toBeGreaterThan(10);
    expect(TRANSLATED).toEqual(site);
  });

  it("shows our own sentences, never the model's text, for every fixed reply", () => {
    expect(js("FIXED")).toBe('["turned_away", "resting", "unavailable", "region"]');
    expect(chatJs).toMatch(/data\.kind === "answer" && typeof data\.text === "string"/);
  });

  it("waits for the check to load, and never gives up without telling the Worker", () => {
    expect(chatJs).toMatch(/!window\.turnstile && waited < CHECK_WAIT_MS/);
    expect(chatJs).not.toMatch(/if \(!passed\) return \{ kind: "error" \}/);
    expect(chatJs).toMatch(/setTimeout\(\(\) => resolve\(null\), CHECK_ANSWER_MS\)/);
    expect(chatJs).toMatch(/passed \? \{ turns: conversation, turnstile: passed \} : \{ turns: conversation \}/);
  });

  it("takes the language only from its own list", () => {
    expect(chatJs).toMatch(/Object\.hasOwn\(WORDS, params\.get\("lang"\)\) \? params\.get\("lang"\) : "en"/);
  });
});
