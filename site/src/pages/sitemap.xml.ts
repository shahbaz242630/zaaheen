import type { APIRoute } from 'astro';
import { PAGES, absolute } from '../data/site';
import { lastModified } from '../lib/lastmod';

// Built from PAGES, so a page cannot exist without being listed, or be listed
// without existing (scripts/audit.mjs checks both directions). lastmod comes
// from git, never the clock; see lib/lastmod.ts. priority and changefreq are
// omitted because Google ignores them.
export const GET: APIRoute = () => {
  const entries = PAGES.map((p) => {
    const modified = lastModified(p.sources);
    const lastmod = modified ? `\n    <lastmod>${modified}</lastmod>` : '';
    return `  <url>\n    <loc>${absolute(p.path)}</loc>${lastmod}\n  </url>`;
  });
  const xml =
    '<?xml version="1.0" encoding="UTF-8"?>\n' +
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n' +
    entries.join('\n') +
    '\n</urlset>\n';
  return new Response(xml, { headers: { 'Content-Type': 'application/xml; charset=utf-8' } });
};
