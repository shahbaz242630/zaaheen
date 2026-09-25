import type { APIRoute } from 'astro';
import { SITE, RELEASE, TRIAL, COMPANY, COACHING, PAGES, absolute, verifiedAppList } from '../data/site';

// A plain-text summary for AI tools (llmstxt.org proposal). No search engine
// has confirmed it reads these files, and Google says it neither helps nor
// harms; it is kept because it costs nothing and is generated from the same
// facts as the pages, so it cannot drift from them.
export const GET: APIRoute = () => {
  const win = RELEASE.windows;
  const lines = [
    `# ${SITE.name}`,
    '',
    `> ${SITE.summary}`,
    '',
    `${SITE.name} is a desktop app that gives AI assistants one private, persistent memory about the person using them. ` +
      'Memories are stored and encrypted on that person\'s own computer and are never uploaded. ' +
      'The account is only for signing in and the subscription; it never holds the memories.',
    '',
    `- Current version: ${RELEASE.version} (${RELEASE.stage.toLowerCase()}), ${win.arch} Windows, tested on ${win.tested}.`,
    RELEASE.available ? `- Download: ${win.url} (${win.size}).` : '- Download: coming soon for Windows.',
    `- Free trial: ${TRIAL.days} days, no card needed.`,
    `- Tested with: ${verifiedAppList()}. Other apps that support MCP should work the same way.`,
    '- Delete everything: one button destroys the encryption key and the files, leaving what remains on disk unreadable.',
    '- macOS and Linux: not available yet.',
    '',
    '## Company',
    '',
    `- ${SITE.name} is the trading name of ${COMPANY.legalName} (${COMPANY.licence}).`,
    `- Support: ${COMPANY.email}. Coaching bookings: ${COACHING.email}.`,
    '',
    '## Pages',
    '',
    ...PAGES.map((p) => `- [${p.title}](${absolute(p.path)}): ${p.description}`),
    '',
  ];
  return new Response(lines.join('\n'), { headers: { 'Content-Type': 'text/plain; charset=utf-8' } });
};
