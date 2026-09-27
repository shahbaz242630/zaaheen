import { PRICES, TRIAL, VERIFIED_APPS, verifiedAppList } from './site';

/** "Claude, Cursor, ChatGPT": for sentences that go on with "and other apps". */
const appsThen = VERIFIED_APPS.join(', ');

// The Pricing page's comparison and FAQ (founder, session 69: "add cost
// comparison between us and our competitors so users can see how we add value",
// and an FAQ with brand weaving for search and AI answers).
//
// Comparative advertising rules (UK CAP Code 3.7 and 3.33 to 3.44, EU Directive
// 2006/114/EC Art 4): compare products that meet the same need, use the price
// the buyer actually pays, state only what the other company's own page says
// (their words in quotes), never run anyone down, and date the check. Every
// competitor fact below was read on that company's own pricing page on
// CHECKED_ON, with a dated screenshot kept in
// C:\Projects\MemoryVault-artifacts\pricing-research-s69\evidence\. Re-check
// before any change to this file, and at least every three months.
// Left out on purpose: Supermemory (its paid plan says "For developers building
// with AI memory"), Mem0 (a developer API) and Notion (full Notion AI needs a
// team Business plan): not the same need.

export const CHECKED_ON = '27 September 2026';

export interface Competitor {
  name: string;
  /** The plan compared, or what kind of product it is. */
  kind: string;
  price: string;
  where: string;
  encrypted: string;
  training: string;
  apps: string;
}

// Seven rows (founder, session 69: "big names like obsedian . memai", and the
// columns "where memories are kept", "encrypted", "used for training yes or
// no"). Facts from each company's own pages (pricing-research-s69
// competitor-privacy-facts.md, prices and training policies re-checked in a
// browser with screenshots). "Not stated" where the company says nothing:
// never a guessed "No". Built-in memory is described as built into that app,
// which is what it is; no company says its memory is shut to other apps.
export const COMPARISON: readonly Competitor[] = [
  {
    name: 'Zaaheen',
    kind: 'Memory for all your AI apps',
    price: `$${PRICES.monthly} a month or $${PRICES.yearly} a year. ${TRIAL.days} days free, no card`,
    where: 'Your computer',
    encrypted: 'Yes, on your computer. Our servers never receive your memories',
    training: 'No',
    apps: `Yes: ${appsThen} and other apps that support MCP`,
  },
  {
    name: 'MemoryPlugin',
    kind: 'Core plan',
    price: '$15 a month or $89 a year',
    where: 'Their servers',
    encrypted: 'Yes, in transit and at rest, not end-to-end',
    training: 'No',
    apps: 'Yes: "21+ AI tools"',
  },
  {
    name: 'Pieces',
    kind: 'Pro plan',
    price: '$18.99 a month',
    where: 'Your computer',
    encrypted: 'Not stated',
    training: 'No',
    apps: 'Yes: "every MCP-ready AI tool"',
  },
  {
    name: 'Mem',
    kind: 'AI notes app, Plus plan',
    price: '$9 a month (free plan available)',
    where: 'Their servers',
    encrypted: 'Yes, at rest and in transit, not end-to-end',
    training: 'No',
    apps: "Yes, through Mem's own connector",
  },
  {
    name: 'Obsidian',
    kind: 'Notes app',
    price: 'App free. Sync $5 a month or $48 a year',
    where: 'Your computer. Sync copies notes to their servers',
    encrypted: 'Sync: yes, end-to-end',
    training: 'Not stated',
    apps: 'Only through community-made plugins',
  },
  {
    name: 'ChatGPT memory',
    kind: 'Built into ChatGPT',
    price: 'Included; the free plan has "limited memory"',
    where: 'Their servers',
    encrypted: 'Yes, at rest and in transit',
    training: 'Yes, unless you turn it off',
    apps: 'Built into ChatGPT',
  },
  {
    name: 'Claude memory',
    kind: 'Built into Claude',
    price: 'Included, even on the free plan',
    where: 'Their servers',
    encrypted: 'Yes, at rest and in transit',
    training: 'Only if you allow it',
    apps: 'Built into Claude',
  },
];

export const COMPARISON_SOURCES = [
  { name: 'MemoryPlugin', url: 'https://www.memoryplugin.com/pricing' },
  { name: 'Pieces', url: 'https://pieces.app/pricing' },
  { name: 'Mem', url: 'https://get.mem.ai/pricing' },
  { name: 'Obsidian', url: 'https://obsidian.md/pricing' },
  { name: 'ChatGPT memory', url: 'https://help.openai.com/en/articles/8590148-memory-in-chatgpt' },
  { name: 'Claude pricing', url: 'https://claude.com/pricing' },
  { name: 'Claude training policy', url: 'https://privacy.claude.com/en/articles/10023580-is-my-data-used-for-model-training' },
  { name: 'Supermemory', url: 'https://supermemory.ai/pricing' },
] as const;

// The FAQ: questions people actually ask when choosing an AI memory tool
// (competitor FAQs and Hacker News threads, pricing-research-s69 section 5),
// the first one defining the product (SEO-HANDOFF section 3a rule 18). Plain
// HTML, no FAQPage schema (lib/seo.ts). Every answer is true of the app today.
export const PRICING_FAQ: readonly { q: string; a: string }[] = [
  {
    q: 'What is Zaaheen?',
    a: `Zaaheen is a private memory for your AI apps. It runs on your Windows computer and gives ${appsThen} and other AI apps one shared memory about you, so you stop repeating yourself. Your memories stay on your own computer, encrypted.`,
  },
  {
    q: 'How much does Zaaheen cost?',
    a: `Zaaheen costs $${PRICES.monthly} a month or $${PRICES.yearly} a year, plus VAT or sales tax where it applies. Both plans include everything. You start with a ${TRIAL.days}-day free trial.`,
  },
  {
    q: 'Do I need a card for the Zaaheen free trial?',
    a: `No. The ${TRIAL.days}-day Zaaheen trial takes no card, so nothing is charged when it ends. Zaaheen simply asks you to subscribe.`,
  },
  {
    q: "ChatGPT and Claude already have memory. Why would I need Zaaheen?",
    a: "Each app's built-in memory belongs to that app. What ChatGPT learns about you stays with ChatGPT, and what Claude learns stays with Claude. Zaaheen is one memory that all your connected AI apps share: save something in Claude, and Cursor or ChatGPT can use it too. You can keep using each app's own memory alongside it.",
  },
  {
    q: 'Can ChatGPT and Claude share the same memory?',
    a: 'Yes, with Zaaheen. Connect both apps to Zaaheen and they read from and save to the same memory on your computer.',
  },
  {
    q: 'How is Zaaheen different from MemoryPlugin, Pieces, Mem or Supermemory?',
    a: `Zaaheen keeps your memories on your own computer, encrypted, and our servers never receive them. MemoryPlugin and Mem keep yours on their servers. Pieces also keeps memories on your computer; its Pro plan is $18.99 a month and its trial needs a card. Zaaheen costs less than MemoryPlugin Core or Pieces Pro, and its trial needs no card. Supermemory's paid plan is aimed at developers building with AI memory. The table above has the details, checked on each company's own website on ${CHECKED_ON}.`,
  },
  {
    q: 'Is Zaaheen like Obsidian?',
    a: 'Obsidian is a notes app: you write and organise notes yourself, and AI apps can reach them only through community-made plugins. Zaaheen is built for your AI apps: they save what they learn about you and find it again by themselves, across every app you connect. Both keep your data on your own computer.',
  },
  {
    q: 'Does Zaaheen use my memories to train AI?',
    a: 'No. Zaaheen never receives your memories, so it cannot use them to train anything. The AI apps you connect have their own settings for training, which you control in each app.',
  },
  {
    q: 'Can the Zaaheen team read my memories?',
    a: 'No. Your memories are stored and encrypted on your own computer, and the key stays there too. Zaaheen never uploads them, so we have no copy to read. The AI apps you connect can send memories to their makers as part of your conversations, under those apps\' own privacy policies.',
  },
  {
    q: 'Which AI apps work with Zaaheen?',
    a: `We have tested Zaaheen with ${verifiedAppList()}. It also works with other AI apps that support MCP, the standard way AI apps connect to tools. The Zaaheen app shows you how to connect each one.`,
  },
  {
    q: 'Will my AI apps actually use Zaaheen?',
    a: 'Some apps, such as Cursor, check Zaaheen by themselves when you ask about yourself. Others, such as Claude and ChatGPT, often look in their own memory first, so the Zaaheen app gives you a one-line tip for them that makes them check Zaaheen too.',
  },
  {
    q: 'Can I bring my existing ChatGPT or Claude memories into Zaaheen?',
    a: 'There is no one-click import yet. Once an app is connected, you can ask it to save what matters to Zaaheen, and every other connected app can then use it.',
  },
  {
    q: 'Does Zaaheen work on Mac or on my phone?',
    a: 'Not yet. Zaaheen is a Windows app, tested on Windows 11.',
  },
  {
    q: 'Can I cancel Zaaheen at any time?',
    a: 'Yes. Cancel in the app under Settings, Account, Manage subscription, and you keep access until the end of the time you have paid for. If you ask within 14 days of a payment, we refund it in full, once per person.',
  },
  {
    q: 'What happens to my memories if I stop paying for Zaaheen?',
    a: 'Nothing is deleted. Your memories stay on your computer, and Download my memories keeps working, so you can always take them with you. Zaaheen stops serving your AI apps until you subscribe again.',
  },
  {
    q: 'Why not just keep a text file of notes for my AI?',
    a: 'A notes file only helps when you paste it in. Zaaheen works by itself: your AI apps save what they learn about you as they go, find the right memory when it matters, and share it across every app you connect.',
  },
];
