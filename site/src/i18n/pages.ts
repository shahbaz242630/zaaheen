// Each translated page's title, meta description and, where the English page
// has them, its heading (H1 and Article headline), breadcrumb label and Learn
// summary (ADR-113 D3). Keyed by the English path; src/data/site.ts merges
// them into the page list, so the sitemap, JSON-LD, breadcrumbs and page dates
// treat a translated page like any other. A missing entry fails the build.
// Two files per language: the Learn articles (pages/<lang>-learn.ts) and the rest.
import type { Translation } from './locales';
import es from './pages/es';
import esLearn from './pages/es-learn';
import pt from './pages/pt';
import ptLearn from './pages/pt-learn';
import de from './pages/de';
import deLearn from './pages/de-learn';
import fr from './pages/fr';
import frLearn from './pages/fr-learn';

export interface PageMeta {
  title: string;
  description: string;
  heading?: string;
  crumb?: string;
  /** Learn articles: the one-line summary under "People also read" and on /learn/. */
  summary?: string;
}

export const PAGE_META: Record<Translation, Readonly<Record<string, PageMeta>>> = {
  es: { ...es, ...esLearn },
  pt: { ...pt, ...ptLearn },
  de: { ...de, ...deLearn },
  fr: { ...fr, ...frLearn },
};
