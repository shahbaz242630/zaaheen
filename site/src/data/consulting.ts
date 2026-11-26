// The AI Readiness Assessment's words, in one place (notes ai-assessment/
// AI-ASSESSMENT-DESIGN.md, ADR-121). The page (pages/consulting.astro) and
// llms.txt (pages/llms.txt.ts, which Ask Zaaheen answers from) both read them,
// so the chat can never tell a visitor something the page does not say.
//
// Founder s129: we recommend, we never build. "Our team spent twenty years",
// not "our founder"; no "an engineer who builds it"; after the report we help
// choose and brief a provider, we do not set systems up ourselves.

export const CONSULTING_HERO = {
  name: 'AI Readiness Assessment',
  title: "Find where AI helps your business, and where it doesn't.",
  lede: 'Not an online quiz or a generic readiness score. Someone who has spent twenty years running operations comes to your business and goes through it department by department, on site, mapping how the work really gets done.',
  then: 'Then we show you where AI saves time and money, where simpler fixes do better, and where to leave things alone.',
  where: 'For businesses across the Gulf',
  promises: ['On site, every role', 'A verdict for every team', 'Independent: no tools to sell you'],
} as const;

export const CONSULTING_PROBLEM = {
  title: 'The problem is rarely the technology. It is the way the work is done.',
  bcg: 'Boston Consulting Group found that 74% of companies had yet to see real value from AI, and that about 70% of the challenge lies in people and processes, not in the technology.',
  research: 'Other research points the same way: projects fail most often when the problem was never properly understood, and the companies that gain most are the ones that redesign their workflows.',
  demo: 'Tools chosen from a demonstration, without seeing the real work, tend to end up unused.',
} as const;

export const CONSULTING_STEPS = [
  { art: 'call', title: 'A free 30-minute call', body: 'You tell us about your business and what is not working. We tell you honestly whether an assessment is worth it for you, and what it would cover.' },
  { art: 'walk', title: 'We walk every role', body: 'On site, from reception to the managing director. We sit with the people doing the work, watch how it really gets done, and time each step.' },
  { art: 'map', title: 'We map and score it', body: 'Every process becomes a map with timed steps, hand-offs and waiting points. Each step is scored on volume, repetition, rules, data and the cost of a mistake.' },
  { art: 'report', title: 'Report and read-out', body: 'A written report and a meeting with your team: where AI helps, where it does not, what to do first, what it costs and when it pays back.' },
] as const;

export const CONSULTING_GIVES =
  'Each team gets one of four verdicts, with the reasons behind it. For every recommendation we give the effort, a cost range, the hours it saves each month, when it pays back, and where a person must stay in charge.';

export const CONSULTING_VERDICTS = [
  { id: 'ai', name: 'AI now', body: 'Work that reads, writes, sorts or answers, at volume, where the data already exists. These are your first projects.' },
  { id: 'auto', name: 'Automate without AI', body: 'Fixed rules and clean inputs. A form, a workflow or a link between two systems does it cheaper and safer than AI.' },
  { id: 'fix', name: 'Fix the process first', body: 'Too many exceptions, unclear ownership or missing data. AI on a broken process only makes it break faster.' },
  { id: 'leave', name: 'Leave as it is', body: 'Low volume, or expert judgement where a mistake is costly. Some work should stay with your people, and we say so.' },
] as const;

export const CONSULTING_TEAMS = ['Reception and front office', 'Customer service', 'Sales', 'Finance', 'HR', 'Operations', 'Procurement', 'IT', 'Risk and compliance'] as const;

export const CONSULTING_WHO = {
  title: 'Businesses with real teams and real workloads.',
  intro: 'From firms of twenty people to large companies with many departments:',
  line: 'If your people spend their days copying, chasing, checking and answering the same questions, there is usually something to find.',
} as const;

export const CONSULTING_WHY_US = {
  title: 'Twenty years inside the work, and advice that fits it.',
  paras: [
    'Our team spent twenty years in corporate operations: customer service, back office, sales, team management and business performance. We know what a busy desk looks like at month end, where the workarounds hide and why the last system was never used.',
    "We turn what we see on site into maps, scores and recommendations that fit how your people actually work. We don't build or sell the tools. We tell you which ones fit your work, and why.",
  ],
  sign: 'From the Zaaheen team',
} as const;

export const CONSULTING_INDEPENDENT = {
  title: 'We work for you, not for any AI company.',
  not: [
    'Sell or resell any AI product, software or hardware.',
    'Run an affiliate programme, or take referral fees, commissions or kickbacks from any provider.',
    'Act as a partner or reseller of any provider.',
  ],
  instead: [
    'Our only income is the fee you pay us, so our advice is based only on what fits your business.',
    'If we name a product, it is because it fits your work, and you buy it directly from its maker.',
    'If you ask us to help afterwards, that is a separate, optional piece of work, and it never depends on which tools you choose.',
  ],
} as const;

export const CONSULTING_FAQ = [
  { q: 'Is this an audit of our accounts or a certification?', a: 'No. It is an operational assessment of how work gets done and where AI can help. It is not a financial audit, an assurance report or a certification of any AI system.' },
  { q: 'What happens to what we show you?', a: "We sign a confidentiality agreement before the first visit. We record roles and processes, not people's names or personal details. Notes are deleted 90 days after the report, unless you ask us to keep them for a follow-on project." },
  { q: 'Do you sell or build the tools you recommend?', a: 'No. We do not sell, resell or build any AI product, software or hardware, and we recommend what fits your work, whoever makes it. You buy it directly from its maker.' },
  { q: 'Do you earn anything from the providers you recommend?', a: 'No. We have no affiliate programme, referral fees, commissions or kickbacks from any provider. Our only income is the fee you pay us, so we work for you.' },
  { q: 'Which countries do you cover?', a: 'We are based in Dubai and work with businesses across the Gulf: the UAE, Saudi Arabia, Qatar, Kuwait, Bahrain and Oman.' },
  { q: 'How long does it take?', a: 'It depends on the size of the business and how many teams we look at. A small firm usually takes two to three weeks from the first visit to the read-out. We agree the timeline in writing before we start.' },
  { q: 'What does it cost?', a: 'Every business is different, so we quote after the free call, once we know how many teams and sites are involved. The quote is fixed and in writing.' },
  { q: 'Can you help after the report?', a: 'Yes, if you want us to. We can be your point of contact for AI on a monthly basis, and help you choose and brief the right provider to put the recommendations in place. You buy any software or hardware directly from its maker.' },
] as const;

export const CONSULTING_CLOSE = {
  title: 'Start with a free 30-minute call.',
  body: 'Tell us about your business. We call you within one working day.',
} as const;

// The consulting documents (founder s129: its own Terms, Privacy and Refund
// policies, written after research). Until then each page is a placeholder,
// which scripts/audit.mjs --release refuses to publish.
export const CONSULTING_POLICIES = [
  { path: '/consulting/terms/', label: 'Terms of Service' },
  { path: '/consulting/privacy/', label: 'Privacy Policy' },
  { path: '/consulting/refunds/', label: 'Cancellation and Refund Policy' },
] as const;
