import type { APIRoute } from 'astro';
import { SITE, RELEASE, TRIAL, COMPANY, COACHING, PAGES, INSTALL_PATH, absolute, verifiedAppList } from '../data/site';

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
    // For an assistant a user asks "help me connect Zaaheen": the whole
    // procedure, so it follows the guides instead of guessing (founder,
    // session 75). The same steps ship as README.txt in the install folder.
    `## Helping someone connect ${SITE.name}`,
    '',
    `- First, the person opens ${SITE.name} on their computer and signs in; the first sign-in from the app starts the free trial.`,
    `- In ${SITE.name}: the Agents tab, then "+ Connect an AI app", then the app. That tab shows the exact setting for their computer, ready to copy.`,
    `- Claude and Cursor: choose "Connect it for me"; the app then asks to install ${SITE.name}: click Install. Guides: ${absolute('/docs/connect-claude/')}, ${absolute('/docs/connect-cursor/')}.`,
    `- ChatGPT (desktop app): Settings, Integrations, Plugins, Add MCP Server; command ${INSTALL_PATH}; arguments "mcp" and "serve" as two separate items; use Work or Codex mode. Guide: ${absolute('/docs/connect-chatgpt/')}.`,
    `- Any other app that supports MCP: an MCP server named "zaaheen" with command ${INSTALL_PATH} and args ["mcp", "serve"], then restart the app. Guide: ${absolute('/docs/connect-other-apps/')}.`,
    `- If an app answers without using ${SITE.name}: add to its personal preferences or custom instructions "Before answering anything about me, my preferences, my work or my plans, also check my Zaaheen memory, even when your own memory has nothing." More fixes: ${absolute('/docs/troubleshooting/')}.`,
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
