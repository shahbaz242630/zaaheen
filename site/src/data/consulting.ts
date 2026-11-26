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

// The landing FAQ, rewritten for search and answer engines (s129, lead with
// the founder's authority): questions follow the query bank and section 10 of
// notes ai-assessment/RESEARCH-CONSULTING-SEO-S129.md. Each answer leads with
// the direct answer, 2 to 4 sentences, and says nothing the consulting Terms,
// Privacy or Cancellation and Refund policies contradict. llms.txt reads this
// list, so Ask Zaaheen answers from the same words. Founder s129: "we dont want
// people thinking its a free service": the call is free, the assessment is
// paid, quoted in full with no hidden costs, plus VAT where it applies.
export const CONSULTING_FAQ = [
  { q: 'Do you carry out AI readiness assessments in Dubai, Abu Dhabi and across the UAE?', a: 'Yes. We are based in Dubai and carry out the AI Readiness Assessment on site for businesses in Dubai, Abu Dhabi and the rest of the UAE. We visit your offices, sit with the people doing the work in every role and time each step. Travel and accommodation outside Dubai are a fixed line in your quote.' },
  { q: 'Do you work in Saudi Arabia, Qatar, Kuwait, Bahrain and Oman?', a: 'Yes. We work with businesses across the Gulf, in Saudi Arabia, Qatar, Kuwait, Bahrain and Oman as well as the UAE, and the visits are on site there too, whether in Riyadh, Jeddah, Doha, Kuwait City, Manama or Muscat. Travel and accommodation are a fixed line in your quote, so the price does not change later.' },
  { q: 'What is an AI readiness assessment? Is it an AI audit or a certification?', a: 'It is an on-site review of how your work really gets done, team by team, to find where AI saves time and money, where simpler fixes do better and where to leave things alone. Some people call it an AI audit, but it is not a financial audit, an assurance report or a certification of any AI system. Unlike an online quiz, which scores your own answers, we watch and time the real work.' },
  { q: 'Is the AI readiness assessment free, and what does it cost?', a: 'The first 30-minute call is free and commits you to nothing; the assessment itself is a paid service. Every business is different, so after the call we send a fixed written quote in UAE dirhams, plus VAT where it applies, based on how many teams and sites are involved. It is set out in full with no hidden costs, and nothing is added later without your written agreement.' },
  { q: 'How do we pay, and what if we need to cancel?', a: 'You pay half when you sign the engagement letter and half after we deliver the report, each within 14 days of the invoice. If you cancel 14 days or more before the original first visit date in your engagement letter, the first payment is refunded in full. Once visits have started, you pay the greater of the work done and the first payment. Our Cancellation and Refund Policy sets out every case.' },
  { q: 'How long does an AI readiness assessment take?', a: 'It depends on the size of the business and how many teams we look at. A small firm usually takes two to three weeks from the first visit to the read-out meeting. We agree the timeline in writing before we start.' },
  { q: 'What do we get at the end of the assessment?', a: 'A written report and a read-out meeting with your team. Each team gets one of four verdicts, AI now, Automate without AI, Fix the process first or Leave as it is, with the reasons behind it. For every recommendation we give the effort, a cost range, the hours it saves each month, when it pays back, and where a person must stay in charge.' },
  { q: 'Is it right for small businesses in Dubai and the UAE?', a: 'It suits businesses from about twenty staff up to large companies with many departments. If your people spend their days copying, chasing, checking and answering the same questions, there is usually something to find. If you are smaller or unsure, the free call tells you honestly whether an assessment is worth it for you.' },
  { q: 'How do I know if my business is ready for AI?', a: 'Look at where your people spend their time. Work that reads, writes, sorts or answers at volume, where the data already exists, is usually a good candidate; work with too many exceptions, unclear ownership or missing data needs fixing first. The assessment answers this team by team, and the free call gives you a first honest view.' },
  { q: 'Why choose Zaaheen, and how are you different from a large consultancy?', a: 'We come on site and sit with every role, from reception to the managing director, and our team brings twenty years in corporate operations. We are independent: we do not build or sell tools and take no commissions, so our only income is your fee. Our four verdicts include Fix the process first and Leave as it is, because some work should stay with your people. You get one fixed written quote before we start.' },
  { q: 'What happens to what we show you?', a: 'We sign a confidentiality agreement before the first visit. We record roles and processes, not names, and delete any name we come across. Interview notes are deleted 90 days after the report, unless you ask us in writing to keep them for follow-on work. For information about your staff, your company stays the controller and we act on its behalf under a data processing agreement.' },
  { q: 'Do you sell or build the tools you recommend?', a: 'No. We do not sell, resell or build any AI product, software or hardware, and we are not a partner or reseller of any provider. We recommend what fits your work, whoever makes it, and you buy it directly from its maker.' },
  { q: 'Do you earn anything from the providers you recommend?', a: 'No. We have no affiliate programme and take no referral fees, commissions or kickbacks from any provider. Our only income is the fee you pay us, so our advice is based only on what fits your business.' },
  // Dubai AI Seal (s129 research, notes ai-assessment/RESEARCH-DUBAI-AI-SEAL-S129.md):
  // honest about our role; the Learn explainer covers the scheme itself.
  { q: 'Do you hold the Dubai AI Seal, and will the assessment help us get it?', a: 'No to both. The Dubai AI Seal is a classification for AI providers, run by the Dubai Centre for Artificial Intelligence, and we have no role in it. Our assessment is for businesses that want to use AI: it shows where AI fits your work before you choose a supplier, and you can check any supplier\'s seal on the scheme\'s own site.' },
  { q: 'Can you help after the report?', a: 'Yes, if you want us to. We can be your point of contact for AI on a monthly basis, and help you choose and brief a provider to put the recommendations in place. Each is a separate, optional piece of work, and you buy any software or hardware directly from its maker.' },
] as const;

export const CONSULTING_CLOSE = {
  title: 'Start with a free 30-minute call.',
  body: 'Tell us about your business. We call you within one working day.',
} as const;

// The consulting documents (founder s129: its own Terms, Privacy and Refund
// policies, written after research; pages/consulting/*.astro).
export const CONSULTING_POLICIES = [
  { path: '/consulting/terms/', label: 'Terms of Service' },
  { path: '/consulting/privacy/', label: 'Privacy Policy' },
  { path: '/consulting/refunds/', label: 'Cancellation and Refund Policy' },
] as const;
