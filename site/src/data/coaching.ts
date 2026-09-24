// The coaching catalogue as the Knowledge Centre shows it.
//
// The source of truth is the coaching app (repo "Training Page",
// src/config/sessions.ts, src/components/training/Progression.tsx and
// HowItWorks.tsx, src/config/site.ts DELIVERY), mirrored here at its commit
// 33cf0e9 (2026-09-24). Prices are never typed anywhere else on this site.
// scripts/coaching-sync.test.mjs compares every field below with that repo
// whenever it is checked out next to this one, and fails on any difference.
// Wording is the coaching app's, word for word, except where it used a long
// dash (this site allows none).

export interface CoachingSession {
  readonly number: number;
  readonly slug: string;
  readonly shortTitle: string;
  /** VAT-inclusive, whole dirhams, as the coaching app charges. */
  readonly priceAed: number;
  readonly hasPrerequisites: boolean;
}

export const SESSIONS: readonly CoachingSession[] = [
  { number: 1, slug: 'ai-research-prompting-foundations', shortTitle: 'Research & Foundations', priceAed: 1299, hasPrerequisites: false },
  { number: 2, slug: 'chatgpt-codex-openai', shortTitle: 'ChatGPT & Codex', priceAed: 1499, hasPrerequisites: false },
  { number: 3, slug: 'claude-claude-code', shortTitle: 'Claude & Claude Code', priceAed: 1499, hasPrerequisites: false },
  { number: 4, slug: 'ai-agents', shortTitle: 'AI Agents', priceAed: 1699, hasPrerequisites: false },
  { number: 5, slug: 'ai-builder-tech-stack', shortTitle: 'Builder Tech Stack', priceAed: 1899, hasPrerequisites: false },
  { number: 6, slug: 'production-ai-deployment', shortTitle: 'Production Deployment', priceAed: 2499, hasPrerequisites: true },
];

export const STAGES = [
  { name: 'AI User', description: 'Get reliable, repeatable results instead of guessing at prompts.', sessions: [1] },
  { name: 'AI Builder', description: 'Turn ideas into working output using coding agents and real tooling.', sessions: [2, 3] },
  { name: 'Agent Operator', description: 'Run agents that execute real tasks, with safe boundaries.', sessions: [4] },
  { name: 'AI Implementer', description: 'Understand the whole stack and deploy something real.', sessions: [5, 6] },
] as const;

export const DELIVERY = {
  durationMinutes: 90,
  platform: 'Microsoft Teams',
  availability: 'Evenings, Monday to Thursday, plus selected weekend slots',
  timezoneLabel: 'Gulf Standard Time (UTC+4)',
} as const;

export const STEPS = [
  {
    title: 'Choose the capability you want',
    body: 'Pick the session that matches where you are now. There is no consultation call to sit through first.',
  },
  {
    title: 'Tell us what you’re working on',
    body: 'A short form captures your goal and a real task, so the session is prepared around your situation.',
  },
  {
    title: 'Pick an evening slot and pay',
    body: 'Choose a time that suits you and pay securely. Your booking is confirmed once payment is verified.',
  },
  {
    title: 'Join privately and leave with next steps',
    body: `${DELIVERY.durationMinutes} minutes one to one over ${DELIVERY.platform}, followed by a written summary of what to do next.`,
  },
] as const;

export const sessionByNumber = (n: number): CoachingSession => {
  const s = SESSIONS.find((x) => x.number === n);
  if (!s) throw new Error(`No coaching session ${n}`);
  return s;
};

/** "AED 1,299", as the coaching app formats it. */
export const formatAed = (aed: number): string => `AED ${aed.toLocaleString('en-AE')}`;

export const LOWEST_PRICE_AED = Math.min(...SESSIONS.map((s) => s.priceAed));
