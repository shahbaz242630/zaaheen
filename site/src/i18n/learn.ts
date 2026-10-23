// A Learn article's one-line summary in a language: English from LEARN in
// src/data/site.ts, a translation from that article's entry in
// src/i18n/pages/<lang>.ts (ADR-113 D3). Throws when a translation has none.
import type { Lang } from './locales';
import { PAGE_META } from './pages';

export function learnSummary(article: { path: string; summary: string }, lang: Lang): string {
  if (lang === 'en') return article.summary;
  const summary = PAGE_META[lang][article.path]?.summary;
  if (!summary) throw new Error(`i18n: ${article.path} needs a ${lang} summary in src/i18n/pages/${lang}.ts`);
  return summary;
}
