import { SITE, RELEASE, COMPANY, absolute, type PageEntry } from '../data/site';

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
export function graphFor(page: PageEntry, modified?: string): string {
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
      operatingSystem: 'Windows',
      softwareVersion: RELEASE.version,
      // Nothing to download or buy until the app is on sale (RELEASE.available).
      ...(RELEASE.available
        ? {
            downloadUrl: RELEASE.windows.url,
            fileSize: RELEASE.windows.size,
            offers: { '@type': 'Offer', price: '0', priceCurrency: 'GBP' },
          }
        : {}),
      publisher: { '@id': org },
    });
  }

  const json = JSON.stringify({ '@context': 'https://schema.org', '@graph': graph });
  // A literal "</script" inside the JSON would end the script element early.
  return json.replace(/<\/script/gi, '<\\/script');
}
