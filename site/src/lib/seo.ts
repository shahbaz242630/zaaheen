import { SITE, RELEASE, COMPANY, PRICES, absolute, breadcrumbsFor, type PageEntry } from '../data/site';

// A subscription plan as a schema.org Offer: the price per billing period.
const plan = (name: string, price: number, billingDuration: 'P1M' | 'P1Y') => ({
  '@type': 'Offer',
  name,
  price: String(price),
  priceCurrency: PRICES.currency,
  priceSpecification: { '@type': 'UnitPriceSpecification', price: String(price), priceCurrency: PRICES.currency, billingDuration },
});

// JSON-LD for a page. Everything stated here must also be visible on the page:
// structured data describes content, it never adds claims of its own.
//
// Deliberately absent:
// - aggregateRating / review: Google's software-app rich result needs one, and a
//   beta has no genuine reviews. Faking them is a spam-policy violation, so we
//   accept "no rich result" (Search Console will flag the missing field; that is
//   expected, not a bug).
// - FAQPage / HowTo: those rich results no longer exist.
// - sameAs: added once the founder names the official profiles.
export function graphFor(page: PageEntry, modified?: string, published?: string): string {
  const org = `${SITE.origin}/#organization`;
  const website = `${SITE.origin}/#website`;
  const app = `${SITE.origin}/#app`;
  const url = absolute(page.path);

  const graph: Record<string, unknown>[] = [
    {
      '@type': 'WebSite',
      '@id': website,
      url: absolute('/'),
      name: SITE.name,
      inLanguage: SITE.lang,
      publisher: { '@id': org },
    },
    {
      '@type': 'Organization',
      '@id': org,
      name: SITE.name,
      legalName: COMPANY.legalName,
      email: COMPANY.email,
      address: {
        '@type': 'PostalAddress',
        addressLocality: COMPANY.city,
        addressCountry: COMPANY.country,
      },
      url: absolute('/'),
      logo: {
        '@type': 'ImageObject',
        url: absolute(SITE.logo.path),
        width: SITE.logo.width,
        height: SITE.logo.height,
      },
    },
    {
      '@type': 'WebPage',
      '@id': `${url}#webpage`,
      url,
      name: page.title,
      description: page.description,
      isPartOf: { '@id': website },
      inLanguage: SITE.lang,
      ...(page.path === '/' ? { about: { '@id': app } } : {}),
      ...(modified ? { dateModified: modified } : {}),
    },
  ];

  if (page.path === '/') {
    graph.push({
      '@type': 'SoftwareApplication',
      '@id': app,
      name: SITE.name,
      description: SITE.summary,
      applicationCategory: 'UtilitiesApplication',
      // Both systems, as the pages say (the Mac: Apple chip only).
      operatingSystem: 'Windows 11, macOS',
      softwareVersion: RELEASE.version,
      // Nothing to download or buy until the app is on sale (RELEASE.available).
      // schema.org's downloadUrl takes several URLs: one per system. No
      // fileSize: the two downloads differ, and one value would be wrong for one.
      ...(RELEASE.available
        ? {
            downloadUrl: [RELEASE.windows.url, RELEASE.mac.url],
            // One Offer per plan, from PRICES; the free trial is stated in the
            // page text. scripts/audit.mjs requires exactly these.
            offers: [plan('Monthly', PRICES.monthly, 'P1M'), plan('Yearly', PRICES.yearly, 'P1Y')],
          }
        : {}),
      publisher: { '@id': org },
    });
  }

  // A how-to guide: an Article whose headline is the page's H1 (Guide.astro
  // renders page.heading), written and published by the company.
  if (page.heading) {
    graph.push({
      '@type': 'Article',
      '@id': `${url}#article`,
      headline: page.heading,
      description: page.description,
      inLanguage: SITE.lang,
      mainEntityOfPage: { '@id': `${url}#webpage` },
      image: absolute(SITE.ogImage),
      author: { '@id': org },
      publisher: { '@id': org },
      ...(published ? { datePublished: published } : {}),
      ...(modified ? { dateModified: modified } : {}),
    });
  }

  // Nested pages only, the same trail components/Breadcrumbs.astro shows.
  const trail = breadcrumbsFor(page);
  if (trail.length) {
    graph.push({
      '@type': 'BreadcrumbList',
      itemListElement: trail.map((c, i) => ({ '@type': 'ListItem', position: i + 1, name: c.name, item: absolute(c.path) })),
    });
  }

  const json = JSON.stringify({ '@context': 'https://schema.org', '@graph': graph });
  // A literal "</script" inside the JSON would end the script element early.
  return json.replace(/<\/script/gi, '<\\/script');
}
