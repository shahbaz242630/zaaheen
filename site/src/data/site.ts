// Single source of truth for facts that appear on more than one page, in the
// sitemap, in llms.txt or in structured data. Change a fact here, never inline.
// Every claim must be true of the build a visitor actually downloads.

export const SITE = {
  name: 'Zaaheen',
  origin: 'https://zaaheen.com',
  locale: 'en_GB',
  lang: 'en-GB',
  tagline: 'A private memory for your AI assistants',
  summary:
    'Zaaheen keeps what your AI assistants know about you on your own computer, ' +
    'encrypted, and entirely under your control.',
  themeColor: '#faf8f3',
  ogImage: '/og.png',
  logo: { path: '/icon-512.png', width: 512, height: 512 },
} as const;

// The company, as its Dubai trade licence names it (founder, 2026-09-25). The
// licence is home-based with no business address, so no street address ever
// appears; the founder's personal phone and email are never published.
export const COMPANY = {
  legalName: 'Zaaheen Artificial Intelligence Developing Services',
  licence: 'Dubai trade licence 1651252',
  city: 'Dubai',
  country: 'AE',
  // General support. Coaching bookings use COACHING.email.
  email: 'customerservice@zaaheen.com',
} as const;

export const RELEASE = {
  // false until the public installer is uploaded and live payments are set up
  // (founder, session 63: the website and coaching go live first). While false,
  // every download button reads "Coming soon for Windows", no page links to the
  // installer, and the deploy leaves /pay out of the published site (site.yml;
  // scripts/audit.mjs treats a home page without the installer link as "not on
  // sale"). Set true in the same change that uploads the installer.
  available: false,
  version: '0.2.2',
  stage: 'Beta',
  windows: {
    file: 'Zaaheen_0.2.2_x64_en-US.msi',
    url: 'https://dl.zaaheen.com/Zaaheen_0.2.2_x64_en-US.msi',
    // As Windows Explorer reports it (216,092,672 bytes).
    size: '206 MB',
    arch: '64-bit',
    // Only Windows 11 has been tested. Do not claim Windows 10 until it has been.
    tested: 'Windows 11',
  },
} as const;

// Apps we have verified end to end against a real install. Add one only after a
// live test; the site must never promise an app we have not seen work.
// ChatGPT: the desktop app, live-tested in session 59 (CONNECT-APPS-DESIGN.md).
export const VERIFIED_APPS = ['Claude', 'Cursor', 'ChatGPT'] as const;

// The home page's animated app window only (founder, session 61: "keep hermes
// and openclaw .. we will test them shortly"). A picture, never a written
// claim: move an app to VERIFIED_APPS after its live test.
export const DEMO_APPS = [...VERIFIED_APPS, 'Hermes', 'OpenClaw'] as const;

/** "Claude, Cursor and ChatGPT": the verified apps as one English list. */
export const verifiedAppList = (): string =>
  VERIFIED_APPS.length < 2
    ? VERIFIED_APPS.join('')
    : `${VERIFIED_APPS.slice(0, -1).join(', ')} and ${VERIFIED_APPS[VERIFIED_APPS.length - 1]}`;

// The subscription, as the app sells it (SIGNIN-DESIGN.md §8.26, ADR-104).
export const TRIAL = { days: 30, card: false } as const;

// The plan buttons' prices, as the app shows them ("$5 a month" / "$48 a year").
export const PLANS = { monthly: '$5 a month', yearly: '$48 a year' } as const;

export const INDEXNOW_KEY = 'dc7e96914b463f8b38a2ca7309b9a25f';

// The 1-to-1 coaching booking app. It is a separate app (Next.js, with payments)
// on its own sub-address: two site engines cannot share one host without a
// router in front of the whole site (founder decision, 2026-09-12). Not
// connected yet; until it is, these links lead nowhere.
export const COACHING = {
  bookingUrl: 'https://coaching.zaaheen.com/training',
  // The Knowledge Centre mailbox: coaching bookings (founder, 2026-09-25).
  email: 'knowledgecentre@zaaheen.com',
} as const;

// The top bar on every page. Labels and order are the founder's.
export const NAV = [
  { href: '/products/', label: 'Products' },
  { href: '/docs/', label: 'Documents' },
  { href: '/knowledge-centre/', label: 'Knowledge Centre' },
] as const;

export interface PageEntry {
  /** Canonical path with trailing slash, e.g. '/privacy/'. */
  path: string;
  title: string;
  description: string;
  /**
   * Files (relative to site/) whose committed content makes up this page. The
   * sitemap's lastmod is the newest git commit date across them, never the
   * build time, so an unchanged page never looks fresh.
   */
  sources: string[];
}

export const PAGES: PageEntry[] = [
  {
    path: '/',
    title: 'Zaaheen: a private memory for your AI assistants',
    description:
      'Zaaheen gives Claude, Cursor, ChatGPT and other AI apps one shared memory about you, encrypted on your own computer. For Windows, 30 days free.',
    sources: ['src/pages/index.astro', 'src/data/site.ts'],
  },
  {
    path: '/products/',
    title: 'Products · Zaaheen',
    description:
      'The products Zaaheen makes, starting with a private, encrypted memory that your AI assistants share on your own computer.',
    sources: ['src/pages/products.astro', 'src/data/site.ts'],
  },
  {
    path: '/docs/',
    title: 'Documents · Zaaheen',
    description:
      'Guides for Zaaheen products: connecting Claude, Cursor and other AI apps, keeping your data safe, and fixing common problems.',
    sources: ['src/pages/docs.astro', 'src/data/site.ts'],
  },
  {
    path: '/knowledge-centre/',
    title: 'Knowledge Centre · Zaaheen',
    description:
      'Learn to work with AI properly: private 1-to-1 AI coaching sessions from Zaaheen, built around a real task and booked online.',
    sources: ['src/pages/knowledge-centre.astro', 'src/data/site.ts'],
  },
];

export const pageFor = (path: string): PageEntry => {
  const page = PAGES.find((p) => p.path === path);
  if (!page) throw new Error(`No PAGES entry for ${path}`);
  return page;
};

export const absolute = (path: string): string => new URL(path, SITE.origin).href;
