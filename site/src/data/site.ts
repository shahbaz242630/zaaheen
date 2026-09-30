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
  ogImage: '/og-v2.png',
  // What the link-preview picture says (scripts/og-card.html).
  ogImageAlt: 'Zaaheen: One memory for all your AI apps',
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

// Our own sign-in and sign-up pages, on their own origin (AUTH-PAGES-DESIGN D1,
// astro.account.config.mjs). The header's "Sign in" / "Get started" link here,
// and public/.htaccess forwards /sign-in and /sign-up here; scripts/audit.mjs
// pins both. Deployed by site.yml's account jobs, switched on by ACCOUNT_PUBLISH;
// zaaheen.com itself is never published before them (site.yml).
export const ACCOUNT = {
  signIn: 'https://account.zaaheen.com/sign-in/',
  signUp: 'https://account.zaaheen.com/sign-up/',
} as const;

export const RELEASE = {
  // false until the public installer is uploaded and live payments are set up
  // (founder, session 63: the website and coaching go live first). While false,
  // every download button reads "Coming soon for Windows and Mac", no page links to
  // either download, and the deploy leaves /pay out of the published site (site.yml;
  // scripts/audit.mjs treats a home page without the installer link as "not on
  // sale"). Set true in the same change that uploads the installer.
  available: false,
  version: '0.3.0',
  stage: 'Beta',
  windows: {
    file: 'Zaaheen_0.3.0_x64_en-US.msi',
    url: 'https://dl.zaaheen.com/Zaaheen_0.3.0_x64_en-US.msi',
    // As Windows Explorer reports it (216,092,672 bytes).
    size: '206 MB',
    arch: '64-bit',
    // Only Windows 11 has been tested. Do not claim Windows 10 until it has been.
    tested: 'Windows 11',
  },
  // The Mac launches with Windows (founder, session 78). Apple chip only
  // (founder, session 77). Signed and notarised by Apple, a disk image to drag
  // into Applications.
  mac: {
    file: 'Zaaheen_0.3.0_aarch64.dmg',
    url: 'https://dl.zaaheen.com/Zaaheen_0.3.0_aarch64.dmg',
    // A placeholder: the session 77 test copy (141,059,416 bytes, as Finder
    // counts). Set from the production Mac build before the download goes on.
    size: '141 MB',
    arch: 'Apple chip (M1 or newer)',
    // Only macOS 26 has been tested. Do not claim older versions until they have been.
    tested: 'macOS 26',
  },
} as const;

/** "Windows and Mac": the platforms the app runs on, for running text. */
export const PLATFORMS = 'Windows and Mac';

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

// The prices in US dollars (Paddle's catalogue), and the plan buttons' words as
// the app shows them ("$5 a month" / "$48 a year").
export const PRICES = { monthly: 5, yearly: 48, currency: 'USD' } as const;
export const PLANS = {
  monthly: `$${PRICES.monthly} a month`,
  yearly: `$${PRICES.yearly} a year`,
} as const;

// The home page's FAQ, above the footer (founder, session 63: written during the
// SEO setup). Questions people actually search for, the first one defining the
// product (SEO-HANDOFF §3a rule 18); plain HTML, never FAQPage schema. While
// empty, the section is left out of the built site and shows a placeholder only
// in the dev preview, so it never blocks publishing.
// Session 71: questions from the choosing-a-memory-tool research
// (pricing-research-s69 §5) that the Pricing FAQ does not already answer, plus
// how-it-works questions; every answer restates a fact already on /docs/,
// /ai-and-your-data/ or this page. Pricing, trial, cancelling and competitors
// live in the Pricing FAQ (src/data/pricing.ts), so the two never duplicate.
export const FAQ: readonly { q: string; a: string }[] = [
  {
    q: 'What does Zaaheen do?',
    a: `Zaaheen gives your AI apps one shared memory about you. Tell ${verifiedAppList()} something once, and every app you connect can recall it later. It runs on your Windows PC or Mac and keeps your memories there, encrypted.`,
  },
  // Brand questions (founder, session 71: "how is zaaheen better than other
  // memory providers .. why choose zaaheen"). Stated as what Zaaheen does, never
  // as others' faults; the sourced, dated comparison stays on /pricing/.
  {
    q: 'Why choose Zaaheen over other AI memory tools?',
    a: `Zaaheen puts three things together. Your memories stay on your own computer, encrypted, and our servers never receive them. One memory works across all your connected AI apps instead of a separate one inside each. And you stay in charge: see every memory, forget any one, download them all, or delete everything in one step. It costs ${PLANS.monthly} or ${PLANS.yearly} after a ${TRIAL.days}-day free trial with no card. Our Pricing page compares it with other memory tools, checked on each company's own website.`,
  },
  {
    q: 'Where does Zaaheen keep my memories?',
    a: 'On your own computer, encrypted, and they are never uploaded. You can move them to another folder or drive from Settings with Move my memories.',
  },
  {
    q: 'Can I see and delete what my AI apps remember about me?',
    a: 'Yes. Every memory is listed in the Zaaheen app, where you can search it or forget any one. Delete everything, under Settings, destroys the encryption key and the memory files in one step.',
  },
  {
    q: 'Do I need an account to use Zaaheen?',
    a: `Yes. You create an account or sign in when you first open the app, and that first sign-in from the app starts your ${TRIAL.days}-day free trial. The account handles sign-in and your subscription. It never holds your memories.`,
  },
  {
    q: 'Do I need to know what MCP is?',
    a: `No. MCP is the standard way AI apps connect to tools, and the Zaaheen app connects ${verifiedAppList()} for you. For other AI apps that support MCP, the Documents page shows how.`,
  },
  {
    q: 'Does Zaaheen use AI itself?',
    a: 'Yes, three small AI models that run on your own computer: one understands meaning, one ranks results, and one tidies up. Once downloaded they work offline, and your memories never pass through an online AI service. Zaaheen hands your AI apps the memories that match, as stored, and the app you are using writes the reply.',
  },
  {
    q: 'What does tidying up do?',
    a: 'It merges near-duplicate memories, settles facts that contradict each other in favour of the newer one, and moves memories you never use out of everyday recall. It never deletes a memory. It is off until you turn it on in Settings, and the first time it makes a one-time download of about 2.5 GB.',
  },
  {
    q: 'Who makes Zaaheen?',
    a: `Zaaheen is made by ${COMPANY.legalName}, a company licensed in Dubai. We build software for people who use AI every day, and this private memory for your AI apps is our first product. You can reach us at ${COMPANY.email}.`,
  },
];

export const INDEXNOW_KEY = 'dc7e96914b463f8b38a2ca7309b9a25f';

// The 1-to-1 coaching booking app. It is a separate app (Next.js, with payments)
// on its own sub-address: two site engines cannot share one host without a
// router in front of the whole site (founder decision, 2026-09-12).
export const COACHING = {
  // false until coaching.zaaheen.com answers: every booking link is left out
  // and the buttons read "Booking opens soon" (founder, session 75; the live
  // site's links led to a host that does not resolve).
  bookingOpen: false,
  bookingUrl: 'https://coaching.zaaheen.com/training',
  // The Knowledge Centre mailbox: coaching bookings (founder, 2026-09-25).
  email: 'knowledgecentre@zaaheen.com',
} as const;

// The top bar on every page. Labels and order are the founder's (Pricing added
// session 67).
export const NAV = [
  { href: '/products/', label: 'Products' },
  { href: '/docs/', label: 'Documents' },
  { href: '/knowledge-centre/', label: 'Knowledge Centre' },
  { href: '/pricing/', label: 'Pricing' },
] as const;

export interface PageEntry {
  /** Canonical path with trailing slash, e.g. '/privacy/'. */
  path: string;
  title: string;
  description: string;
  /**
   * Files (relative to site/) whose content makes up this page, its own page
   * file first: a guide's datePublished is the commit that added that file
   * (lib/lastmod.ts). Its last-updated date is not taken from these but from
   * its own words (scripts/page-dates.mjs).
   */
  sources: string[];
  /**
   * The page's short name in a breadcrumb trail, e.g. 'Knowledge Centre'.
   * Needed by every nested page and every parent of one (breadcrumbsFor).
   */
  crumb?: string;
  /**
   * The page this one sits under in the breadcrumb trail when that is not what
   * its address says: the policies live at /terms/ etc. but are reached through
   * the Documents menu, so their trail is Home › Documents › the page.
   */
  parent?: string;
  /**
   * A how-to guide under /docs/ (layouts/Guide.astro): its H1, which is also
   * the headline of its Article JSON-LD (lib/seo.ts; scripts/audit.mjs
   * requires the two to be identical).
   */
  heading?: string;
}

export const PAGES: PageEntry[] = [
  {
    path: '/',
    title: 'Zaaheen: a private memory for your AI assistants',
    description:
      'Zaaheen gives Claude, Cursor, ChatGPT and other AI apps one shared memory about you, encrypted on your own computer. For Windows and Mac, 30 days free.',
    sources: ['src/pages/index.astro', 'src/data/site.ts'],
  },
  {
    path: '/products/',
    title: 'Products: the memory app and AI coaching · Zaaheen',
    description:
      'The products Zaaheen makes, starting with a private, encrypted memory that your AI assistants share on your own computer.',
    sources: ['src/pages/products.astro', 'src/data/site.ts'],
  },
  {
    path: '/docs/',
    crumb: 'Documents',
    title: 'Help and guides for the memory app · Zaaheen',
    description:
      'Guides for Zaaheen products: connecting Claude, Cursor and other AI apps, keeping your data safe, and fixing common problems.',
    sources: ['src/pages/docs.astro', 'src/data/site.ts'],
  },
  {
    path: '/knowledge-centre/',
    crumb: 'Knowledge Centre',
    title: 'Knowledge Centre: AI coaching sessions · Zaaheen',
    description:
      'Learn to work with AI properly: private 1-to-1 AI coaching sessions from Zaaheen, built around a real task and booked online.',
    sources: ['src/pages/knowledge-centre/index.astro', 'src/data/site.ts'],
  },
  {
    path: '/privacy/',
    parent: '/docs/',
    crumb: 'Privacy Policy',
    title: 'Privacy Policy · Zaaheen',
    description:
      'How Zaaheen handles your data: your memories stay encrypted on your own computer and never reach us. What your account holds, and your rights.',
    sources: ['src/pages/privacy.astro', 'src/layouts/Policy.astro', 'src/data/site.ts'],
  },
  {
    path: '/terms/',
    parent: '/docs/',
    crumb: 'Terms of Service',
    title: 'Terms of Service · Zaaheen',
    description:
      'The terms for using Zaaheen: your 30-day free trial, your subscription and how to cancel, and why your memories always stay yours, on your computer.',
    sources: ['src/pages/terms.astro', 'src/layouts/Policy.astro', 'src/data/site.ts'],
  },
  {
    path: '/refunds/',
    parent: '/docs/',
    crumb: 'Refund Policy',
    title: 'Refund Policy · Zaaheen',
    description:
      'Zaaheen refunds your latest payment in full if you ask within 14 days of being charged, on the monthly or yearly plan. How to cancel and how to ask.',
    sources: ['src/pages/refunds.astro', 'src/layouts/Policy.astro', 'src/data/site.ts'],
  },
  {
    path: '/ai-and-your-data/',
    parent: '/docs/',
    crumb: 'AI and Your Data',
    title: 'AI and Your Data · Zaaheen',
    description:
      'How Zaaheen uses AI: small models run on your own computer to find and tidy your memories. Nothing goes to an online AI, and nothing trains AI.',
    sources: ['src/pages/ai-and-your-data.astro', 'src/layouts/Policy.astro', 'src/data/site.ts'],
  },
  {
    path: '/security/',
    parent: '/docs/',
    crumb: 'Security',
    title: 'Security · Zaaheen',
    description:
      'How Zaaheen protects your memories: encrypted on your computer, a key that never leaves it, sign-in in your browser, and a server that holds none of them.',
    sources: ['src/pages/security.astro', 'src/layouts/Policy.astro', 'src/data/site.ts'],
  },
  {
    path: '/company/',
    parent: '/docs/',
    crumb: 'Company Information',
    title: 'Company Information · Zaaheen',
    description:
      'Zaaheen is the trading name of Zaaheen Artificial Intelligence Developing Services, licensed in Dubai. Our licence details and how to contact us.',
    sources: ['src/pages/company.astro', 'src/layouts/Policy.astro', 'src/data/site.ts'],
  },
  {
    path: '/licences/',
    parent: '/docs/',
    crumb: 'Open-source licences',
    title: 'Open-source licences · Zaaheen',
    description:
      'Zaaheen includes open-source software and AI models used under their own licences, such as MIT and Apache 2.0. How we honour them and where to find the notices.',
    sources: ['src/pages/licences.astro', 'src/layouts/Policy.astro', 'src/data/site.ts'],
  },
  {
    path: '/knowledge-centre/terms/',
    crumb: 'Coaching Terms',
    title: 'Coaching Terms · Zaaheen Knowledge Centre',
    description:
      'The terms for booking private 1-to-1 AI coaching with Zaaheen: what a session is, paying, moving a session, refunds, your legal rights and how to complain.',
    sources: ['src/pages/knowledge-centre/terms.astro', 'src/layouts/Policy.astro', 'src/data/site.ts', 'src/data/coaching.ts'],
  },
  {
    path: '/knowledge-centre/booking-and-refunds/',
    crumb: 'Booking and Refund Policy',
    title: 'Booking and Refund Policy · Zaaheen Knowledge Centre',
    description:
      'How coaching bookings work: move your session free up to 24 hours before, no refund for a change of mind or a missed session, and when we always refund.',
    sources: ['src/pages/knowledge-centre/booking-and-refunds.astro', 'src/layouts/Policy.astro', 'src/data/site.ts', 'src/data/coaching.ts'],
  },
  {
    path: '/knowledge-centre/privacy/',
    crumb: 'Coaching Privacy Notice',
    title: 'Coaching Privacy Notice · Zaaheen Knowledge Centre',
    description:
      'What Zaaheen collects when you book coaching, why, who helps us run it, how long we keep it, and your rights over your data.',
    sources: ['src/pages/knowledge-centre/privacy.astro', 'src/layouts/Policy.astro', 'src/data/site.ts'],
  },
  // The how-to guides (SEO-HANDOFF §1 step 6): one task per page, the answer
  // first, every step in the app's own words. Listed in GUIDES below.
  {
    path: '/docs/getting-started/',
    crumb: 'Getting started',
    heading: 'How to install and set up Zaaheen',
    title: 'Install and set up Zaaheen on Windows or Mac · Zaaheen',
    description:
      'Install Zaaheen on Windows or Mac, create your account, choose where your memories live and connect your first AI app. Setting up takes about three minutes.',
    sources: ['src/pages/docs/getting-started.astro', 'src/layouts/Guide.astro'],
  },
  {
    path: '/docs/connect-claude/',
    crumb: 'Connect Claude',
    heading: 'How to connect Claude to Zaaheen',
    title: 'Connect Claude to Zaaheen · Zaaheen',
    description:
      "Give the Claude app on your computer a private memory it shares with your other AI apps: one click in Zaaheen, then Install in Claude.",
    sources: ['src/pages/docs/connect-claude.astro', 'src/layouts/Guide.astro'],
  },
  {
    path: '/docs/connect-cursor/',
    crumb: 'Connect Cursor',
    heading: 'How to connect Cursor to Zaaheen',
    title: 'Connect Cursor to Zaaheen · Zaaheen',
    description:
      'Give Cursor a memory that your other AI apps share: one click in Zaaheen, then Install in Cursor. How to check it worked, and the setting by hand.',
    sources: ['src/pages/docs/connect-cursor.astro', 'src/layouts/Guide.astro'],
  },
  {
    path: '/docs/connect-chatgpt/',
    crumb: 'Connect ChatGPT',
    heading: 'How to connect ChatGPT to Zaaheen',
    title: 'Connect ChatGPT to Zaaheen · Zaaheen',
    description:
      'Connect the ChatGPT app for your computer to Zaaheen in Settings, Integrations, Plugins, use it in Work or Codex mode, and make ChatGPT check it.',
    sources: ['src/pages/docs/connect-chatgpt.astro', 'src/layouts/Guide.astro'],
  },
  {
    path: '/docs/connect-other-apps/',
    crumb: 'Connect other AI apps',
    heading: 'How to connect other AI apps to Zaaheen',
    title: 'Connect other AI apps to Zaaheen with MCP · Zaaheen',
    description:
      'Zaaheen works with AI apps that support MCP. The setting to give your app, where Zaaheen shows it ready to copy, and how to check it connected.',
    sources: ['src/pages/docs/connect-other-apps.astro', 'src/layouts/Guide.astro'],
  },
  {
    path: '/docs/troubleshooting/',
    crumb: 'Troubleshooting',
    heading: 'Fixing common problems with Zaaheen',
    title: 'Zaaheen troubleshooting: fix common problems · Zaaheen',
    description:
      "What to do when an AI app doesn't use your Zaaheen memory, isn't listed as connected, or ChatGPT can't see it, and how to send us a record.",
    sources: ['src/pages/docs/troubleshooting.astro', 'src/layouts/Guide.astro'],
  },
  {
    path: '/pricing/',
    title: `Pricing: ${PLANS.monthly} after ${TRIAL.days} days free · Zaaheen`,
    description:
      `Zaaheen pricing: ${TRIAL.days} days free with no card, then ${PLANS.monthly} or ${PLANS.yearly}. How Zaaheen compares with other AI memory tools and ChatGPT and Claude memory.`,
    sources: ['src/pages/pricing.astro', 'src/data/site.ts', 'src/data/pricing.ts'],
  },
];

// The policies, each on its own address (founder, session 67), in the order the
// Documents menu lists them. The footer (components/Footer.astro) links each,
// and scripts/audit.mjs POLICIES pins that: each page built and linked from
// every footer. Each also has a PAGES entry (title, description, sitemap).
export const POLICIES = [
  { path: '/terms/', label: 'Terms of Service' },
  { path: '/privacy/', label: 'Privacy Policy' },
  { path: '/refunds/', label: 'Refund Policy' },
  { path: '/ai-and-your-data/', label: 'AI and Your Data' },
  { path: '/security/', label: 'Security' },
  { path: '/company/', label: 'Company Information' },
  { path: '/licences/', label: 'Open-source licences' },
] as const;

// The coaching documents (founder, session 70: the Knowledge Centre shows
// coaching's own documents, never the app's). The Knowledge Centre's footer
// (components/Footer.astro, variant "coaching") links each entry marked live;
// the rest stay out of every page until their page is written and approved.
// Going live = write the page, add its PAGES entry, set live: true, and add its
// path to COACHING_POLICIES in scripts/audit.mjs, which then requires it.
export const COACHING_POLICIES: readonly { path: string; label: string; live: boolean }[] = [
  { path: '/knowledge-centre/terms/', label: 'Coaching Terms', live: true },
  { path: '/knowledge-centre/booking-and-refunds/', label: 'Booking and Refund Policy', live: true },
  { path: '/knowledge-centre/privacy/', label: 'Coaching Privacy Notice', live: true },
];

// The how-to guides, each on its own page under /docs/, in the order the
// Documents menu lists them. Each has a PAGES entry with a heading.
export const GUIDES = [
  { path: '/docs/getting-started/', label: 'Getting started' },
  { path: '/docs/connect-claude/', label: 'Connect Claude' },
  { path: '/docs/connect-cursor/', label: 'Connect Cursor' },
  { path: '/docs/connect-chatgpt/', label: 'Connect ChatGPT' },
  { path: '/docs/connect-other-apps/', label: 'Connect other AI apps' },
  { path: '/docs/troubleshooting/', label: 'Troubleshooting' },
] as const;

// Where Zaaheen installs, as the connection settings show it. The app's Agents
// tab shows the path on each computer exactly (ADR-111), so the guides say
// "usually" and point there.
export const INSTALL_PATH = 'C:\\Program Files\\Zaaheen\\zaaheen.exe';
// The same on a Mac, with Zaaheen in the Applications folder (the app refuses
// to run from anywhere else).
export const MAC_INSTALL_PATH = '/Applications/Zaaheen.app/Contents/MacOS/zaaheen';

// The line that makes Claude and ChatGPT check Zaaheen as well as their own
// memory, word for word as the app gives it (TIP_LINE in the desktop app).
export const TIP_LINE =
  'Before answering anything about me, my preferences, my work or my plans, also check my Zaaheen memory, even when your own memory has nothing.';

// The Documents menu: how-to sections (anchors on /docs/) and the policies.
export const DOCS_SECTIONS = [
  { id: 'getting-started', title: 'Getting started' },
  { id: 'connecting', title: 'Connecting your AI apps' },
  { id: 'memories', title: 'Your memories' },
  { id: 'account', title: 'Account and subscription' },
  { id: 'support', title: 'Help' },
] as const;

export const pageFor = (path: string): PageEntry => {
  const page = PAGES.find((p) => p.path === path);
  if (!page) throw new Error(`No PAGES entry for ${path}`);
  return page;
};

export const absolute = (path: string): string => new URL(path, SITE.origin).href;

/**
 * The breadcrumb trail: Home, each parent, then the page itself. A page's
 * parent is its `parent` if set (the policies: Documents), else the page one
 * level up its address (/knowledge-centre/terms/ → /knowledge-centre/). Empty
 * for the home page and top-level pages without a `parent`, where a trail adds
 * nothing. Rendered by components/Breadcrumbs.astro and, from the same data,
 * as BreadcrumbList JSON-LD (lib/seo.ts); scripts/audit.mjs requires the two to
 * match, and a trail on every nested and policy page.
 */
export const breadcrumbsFor = (page: PageEntry): { name: string; path: string }[] => {
  const up = (p: PageEntry): string | undefined => {
    if (p.parent) return p.parent;
    const segments = p.path.split('/').filter(Boolean);
    return segments.length >= 2 ? `/${segments.slice(0, -1).join('/')}/` : undefined;
  };
  if (!up(page)) return [];
  const chain: PageEntry[] = [];
  for (let p: PageEntry | undefined = page; p; p = up(p) ? pageFor(up(p) as string) : undefined) {
    if (chain.includes(p)) throw new Error(`breadcrumb loop at ${p.path}`);
    chain.unshift(p);
  }
  return [
    { name: 'Home', path: '/' },
    ...chain.map((p) => {
      if (!p.crumb) throw new Error(`PAGES entry ${p.path} needs a crumb: it is in a breadcrumb trail`);
      return { name: p.crumb, path: p.path };
    }),
  ];
};
