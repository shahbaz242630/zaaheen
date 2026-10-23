import type { APIRoute } from 'astro';
import { ALL_PAGES, absolute } from '../data/site';
import { lastModified } from '../lib/lastmod';
import { LOCALES, alternatesOf } from '../i18n/locales';

// Built from the page list in every language (ALL_PAGES), so a page cannot
// exist without being listed, or be listed without existing (scripts/audit.mjs
// checks both directions). lastmod is when the page's words last changed,
// never the clock (lib/lastmod.ts). priority and changefreq are omitted
// because Google ignores them. A page with translations lists every language
// version, itself included, plus x-default (ADR-113 D4), as Google's
// sitemap hreflang format asks.
export const GET: APIRoute = () => {
  const entries = ALL_PAGES.map((p) => {
    const modified = lastModified(p.path);
    const lastmod = modified ? `\n    <lastmod>${modified}</lastmod>` : '';
    const alternates = alternatesOf(p.path);
    const links = alternates.length
      ? [
          ...alternates.map((a) => `\n    <xhtml:link rel="alternate" hreflang="${LOCALES[a.lang].hreflang}" href="${absolute(a.path)}"/>`),
          `\n    <xhtml:link rel="alternate" hreflang="x-default" href="${absolute(alternates[0].path)}"/>`,
        ].join('')
      : '';
    return `  <url>\n    <loc>${absolute(p.path)}</loc>${lastmod}${links}\n  </url>`;
  });
  const xml =
    '<?xml version="1.0" encoding="UTF-8"?>\n' +
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">\n' +
    entries.join('\n') +
    '\n</urlset>\n';
  return new Response(xml, { headers: { 'Content-Type': 'application/xml; charset=utf-8' } });
};
