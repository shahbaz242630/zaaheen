// The shared words (menu, footer, layouts, components, and the desktop app's
// help text the connect guides quote), in each translation (ADR-113 D3, D6).
//
// Keyed by the English text itself: change an English phrase and every
// translation of it stops matching, so the build fails until all four are
// translated again. That is the "stale translation" check for shared words.
// English is the key and needs no dictionary.
import { LOCALES, type Lang, type Translation } from './locales';
import es from './ui/es';
import pt from './ui/pt';
import de from './ui/de';
import fr from './ui/fr';

const DICTS: Record<Translation, Readonly<Record<string, string>>> = { es, pt, de, fr };

/**
 * `english` in `lang`, with any {name} placeholders filled from `vars`. Throws
 * when a translation is missing, so a page can never ship half translated.
 */
export function t(lang: Lang, english: string, vars: Record<string, string | number> = {}): string {
  const text = lang === 'en' ? english : DICTS[lang][english];
  if (text === undefined) {
    throw new Error(`i18n: no ${LOCALES[lang].name} (${lang}) for "${english}": add it to src/i18n/ui/${lang}.ts`);
  }
  return text.replace(/\{(\w+)\}/g, (all, key: string) => (key in vars ? String(vars[key]) : all));
}

const ESCAPES: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
/** Text made safe to place inside a translated sentence rendered with set:html. */
export const escapeHtml = (s: string | number): string => String(s).replace(/[&<>"']/g, (c) => ESCAPES[c]);

/**
 * Like t, for a sentence that carries our own inline markup (<em>, <strong>,
 * <code>, <a href>): every value filled in is escaped first. Render with set:html.
 */
export function tHtml(lang: Lang, english: string, vars: Record<string, string | number> = {}): string {
  const safe = Object.fromEntries(Object.entries(vars).map(([k, v]) => [k, escapeHtml(v)]));
  return t(lang, english, safe);
}

/** A date as the reader's language writes it ('22 October 2026', '22 de octubre de 2026'). */
export function formatDate(iso: string, lang: Lang): string {
  return new Date(iso).toLocaleDateString(LOCALES[lang].htmlLang, { day: 'numeric', month: 'long', year: 'numeric' });
}

/** Every key of every dictionary, for scripts/translations.mjs. */
export const DICTIONARIES = DICTS;
