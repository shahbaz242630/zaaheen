import type { APIRoute } from 'astro';
import { SITE, ACCOUNT, RELEASE, TRIAL, COMPANY, COACHING, PAGES, INSTALL_PATH, MAC_INSTALL_PATH, absolute, verifiedAppList } from '../data/site';
import { appWords, openerText, stepTitles } from '../data/app-words';
import { FIXES } from '../data/fixes';
import { RELEASE_NOTES } from '../data/releases';

const latest = RELEASE_NOTES[0];

// Each app's steps in the desktop app's own words (data/app-words.ts reads
// crates/vault-tauri/dist/app.js), so an assistant reading this file gives
// the same steps as the app's Agents tab (session 82).
const GUIDES: Record<string, string> = {
  'Claude Desktop': '/docs/connect-claude/',
  'Cursor Desktop': '/docs/connect-cursor/',
  'ChatGPT Desktop': '/docs/connect-chatgpt/',
  'Claude Code': '/docs/connect-claude-code/',
  Codex: '/docs/connect-codex/',
  Antigravity: '/docs/connect-antigravity/',
  'Another app': '/docs/connect-other-apps/',
};
const flat = (text: string) => text.replace(/\s*\n\s*/g, ' ').trim();
function appSteps(): string[] {
  const words = appWords();
  const mac = appWords(true);
  const out: string[] = [];
  for (const agent of words.AGENTS) {
    const t = stepTitles(agent.name);
    const macAgent = mac.AGENTS.find((a) => a.name === agent.name)!;
    out.push(`### ${agent.name}`, '');
    if (t.note) out.push(t.note);
    if (agent.note) out.push(agent.note);
    const quick = agent.connect ? words.CONNECT_WORDS[agent.connect] : null;
    if (quick && t.quick) {
      out.push(`- ${t.quick}: in ${SITE.name}, Agents, "+ Connect an AI app", "${agent.name}", then "${quick.button}". ${quick.note} ${quick.saved ?? quick.asked}`);
    }
    const parts = [agent.hint];
    for (const o of agent.openers ?? []) parts.push(`${o.label}: ${openerText(o, INSTALL_PATH)}`);
    if (agent.pasteHint) parts.push(agent.pasteHint);
    if (agent.snippet) parts.push(flat(agent.snippet(INSTALL_PATH)));
    out.push(`- ${t.byHand} (Windows): ${parts.join(' ')}`);
    if (macAgent.hint !== agent.hint || agent.snippet || (agent.openers ?? []).some((o) => typeof o.command === 'function')) {
      const m = [macAgent.hint];
      for (const o of macAgent.openers ?? []) m.push(`${o.label}: ${openerText(o, MAC_INSTALL_PATH)}`);
      if (macAgent.pasteHint) m.push(macAgent.pasteHint);
      if (macAgent.snippet) m.push(flat(macAgent.snippet(MAC_INSTALL_PATH)));
      out.push(`- ${t.byHand} (Mac): ${m.join(' ')}`);
    }
    if (t.check) out.push(`- ${t.check}: ${words.AGENT_CHECKS[agent.name]}`);
    if (t.tip) out.push(`- ${t.tip}: ${words.AGENT_TIPS[agent.name]} "${words.TIP_LINE}"`);
    if (GUIDES[agent.name]) out.push(`- Guide: ${absolute(GUIDES[agent.name])}`);
    out.push('');
  }
  return out;
}

// A plain-text summary for AI tools (llmstxt.org proposal). No search engine
// has confirmed it reads these files, and Google says it neither helps nor
// harms; it is kept because it costs nothing and is generated from the same
// facts as the pages, so it cannot drift from them.
export const GET: APIRoute = () => {
  const win = RELEASE.windows;
  const mac = RELEASE.mac;
  const lines = [
    `# ${SITE.name}`,
    '',
    `> ${SITE.summary}`,
    '',
    `${SITE.name} is a desktop app that gives AI assistants one private, persistent memory about the person using them. ` +
      'Memories are stored and encrypted on that person\'s own computer and are never uploaded. ' +
      'The account is only for signing in and the subscription; it never holds the memories.',
    '',
    `- Current version: ${RELEASE.version} (${RELEASE.stage.toLowerCase()}), for ${win.arch} Windows (tested on ${win.tested}) and for Macs with an ${mac.arch} (tested on ${mac.tested}).`,
    RELEASE.available
      ? `- Download: Windows ${win.url} (${win.size}, SHA-256 ${win.sha256}); Mac ${mac.url} (${mac.size}, SHA-256 ${mac.sha256}), a disk image: drag ${SITE.name} into Applications and open it from there.`
      : '- Download: coming soon for Windows and Mac.',
    `- Free trial: ${TRIAL.days} days, no card needed.`,
    `- Tested with: ${verifiedAppList()}. Other apps that support MCP should work the same way.`,
    '- Delete everything: one button destroys the encryption key and the files, leaving what remains on disk unreadable.',
    '- Linux and phones: not available yet.',
    '',
    // For an assistant a user asks "help me connect Zaaheen": the whole
    // procedure, so it follows the guides instead of guessing (founder,
    // session 75). The same steps ship as README.txt in the install folder.
    `## Helping someone connect ${SITE.name}`,
    '',
    `- First, the person opens ${SITE.name} on their computer and signs in; the first sign-in from the app starts the free trial.`,
    `- In ${SITE.name}: the Agents tab, then "+ Connect an AI app", then the app. That tab shows the exact setting for their computer (its own install path), ready to copy: prefer it to the usual paths below (Windows ${INSTALL_PATH}, Mac ${MAC_INSTALL_PATH}). Use the Copy buttons; a space typed before the command stops it working.`,
    `- Codex (terminal) and ChatGPT Desktop share one settings file (.codex/config.toml); the Cursor IDE uses Cursor Desktop's settings. Connect once.`,
    `- Other apps: ${absolute('/docs/connect-other-apps/')}. More fixes: ${absolute('/docs/troubleshooting/')}.`,
    '',
    ...appSteps(),
    // The exact words people and agents see, so a search for an error finds
    // its fix (data/fixes.ts; the troubleshooting page shows the same list).
    '## Problems and fixes',
    '',
    ...FIXES.map((f) => `- "${f.error}" (${f.where}): ${f.fix}`),
    '',
    // The latest release in full, the rest on the page (data/releases.ts).
    '## Release notes',
    '',
    `- Latest: ${latest.version}, ${latest.date}. ${latest.summary}`,
    ...latest.added.map((line) => `- New in ${latest.version}: ${line}`),
    ...latest.fixed.map((line) => `- Fixed in ${latest.version}: ${line}`),
    `- Every release, newest first: ${absolute('/docs/release-notes/')}.`,
    '',
    // How to cancel (founder s85: automatic routes only, no "email us").
    '## Cancelling',
    '',
    `- In the app: Settings, then Account, then "Cancel subscription".`,
    `- On the website: ${ACCOUNT.page} ("Account" at the top of zaaheen.com), then "Cancel subscription".`,
    '- From any device: the cancel link in any email from Paddle, the reseller.',
    '- Cancelling stops the next renewal; access continues until the end of the time paid for.',
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
