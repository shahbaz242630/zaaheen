// The site's languages (ADR-113, notes/I18N-DESIGN.md): English at the root,
// each translation under its own folder with the same page names after it
// (/es/pricing/ is /pricing/ in Spanish). No redirect by country or browser
// language: the address is the choice (D1).

export const LANGS = ['en', 'es', 'pt', 'de', 'fr'] as const;
export type Lang = (typeof LANGS)[number];
/** The translations, in the language menu's order after English. */
export const TRANSLATIONS = ['es', 'pt', 'de', 'fr'] as const satisfies readonly Lang[];
export type Translation = (typeof TRANSLATIONS)[number];

export const LOCALES: Record<Lang, {
  /** Folder before the page path: '' for English. */
  prefix: string;
  /** <html lang>, JSON-LD inLanguage. */
  htmlLang: string;
  /** hreflang in the alternates and the sitemap. */
  hreflang: string;
  /** og:locale. */
  ogLocale: string;
  /** The language menu's button. */
  short: string;
  /** The language's own name for itself, in the menu. */
  name: string;
}> = {
  en: { prefix: '', htmlLang: 'en-GB', hreflang: 'en', ogLocale: 'en_GB', short: 'EN', name: 'English' },
  es: { prefix: '/es', htmlLang: 'es', hreflang: 'es', ogLocale: 'es_LA', short: 'ES', name: 'Español' },
  pt: { prefix: '/pt', htmlLang: 'pt-BR', hreflang: 'pt-BR', ogLocale: 'pt_BR', short: 'PT', name: 'Português' },
  de: { prefix: '/de', htmlLang: 'de', hreflang: 'de', ogLocale: 'de_DE', short: 'DE', name: 'Deutsch' },
  fr: { prefix: '/fr', htmlLang: 'fr', hreflang: 'fr', ogLocale: 'fr_FR', short: 'FR', name: 'Français' },
};

/**
 * The English pages that exist in every translation (D2). Legal pages, the
 * Knowledge Centre, release notes and the unlisted pages stay English.
 * scripts/translations.mjs fails the build when one of these has no page in a
 * language, or its English words changed since that page was translated.
 */
export const TRANSLATED: readonly string[] = [
  '/', '/products/', '/pricing/',
  '/docs/', '/docs/getting-started/', '/docs/subscribe/', '/docs/connect-claude/', '/docs/connect-claude-code/', '/docs/connect-cursor/', '/docs/connect-chatgpt/', '/docs/connect-codex/', '/docs/connect-antigravity/', '/docs/connect-other-apps/', '/docs/troubleshooting/', '/docs/contact/',
  '/learn/', '/learn/ai-memory-apps-compared/', '/learn/ai-memory-tool-or-notes-file/', '/learn/best-memory-mcp-server-for-claude-code/', '/learn/chatgpt-memory-vs-claude-memory/', '/learn/does-claude-code-remember-between-sessions/', '/learn/mem0-alternative/', '/learn/memoryplugin-alternative/', '/learn/obsidian-vs-ai-memory-app/', '/learn/one-memory-across-claude-code-cursor-and-codex/', '/learn/share-memory-between-chatgpt-and-claude/', '/learn/supermemory-alternative/', '/learn/switch-from-chatgpt-to-claude/',
];

const PREFIXED = new RegExp(`^/(${TRANSLATIONS.join('|')})(/|$)`);

/** The language of a site path ('/es/pricing/' → 'es'). */
export function langOf(path: string): Lang {
  const m = path.match(PREFIXED);
  return m ? (m[1] as Lang) : 'en';
}

/** The English path behind a site path ('/es/pricing/' → '/pricing/'). */
export function englishPath(path: string): string {
  return path.replace(PREFIXED, '/');
}

/**
 * Where a link to an English path should go from a page in `lang`: the same
 * page in that language when it is translated, else the English page (never a
 * page that does not exist). Keeps any #fragment or ?query.
 */
export function localize(href: string, lang: Lang): string {
  if (lang === 'en' || !href.startsWith('/') || href.startsWith('//')) return href;
  const cut = href.search(/[?#]/);
  const base = cut === -1 ? href : href.slice(0, cut);
  const rest = cut === -1 ? '' : href.slice(cut);
  return TRANSLATED.includes(base) ? `${LOCALES[lang].prefix}${base}${rest}` : href;
}

/** Every language version of a page, or none when the page is English-only. */
export function alternatesOf(path: string): { lang: Lang; path: string }[] {
  const en = englishPath(path);
  if (!TRANSLATED.includes(en)) return [];
  return LANGS.map((lang) => ({ lang, path: lang === 'en' ? en : `${LOCALES[lang].prefix}${en}` }));
}

/** Where the language menu takes a reader of `path` who picks `lang`. */
export function switchTo(path: string, lang: Lang): string {
  const en = englishPath(path);
  if (lang === 'en') return en;
  return TRANSLATED.includes(en) ? `${LOCALES[lang].prefix}${en}` : `${LOCALES[lang].prefix}/`;
}
