// The desktop app's connect wording, read from the app itself at build time
// (session 82, founder: the guides and llms.txt must say exactly what the app
// says). The block between the CONNECT WORDING markers in
// crates/vault-tauri/dist/app.js is plain data and small functions; it is
// evaluated here once per platform. A missing marker fails the build.
import fs from 'node:fs';
import path from 'node:path';

// Found by walking up from where the build runs: site/, or a copy of it inside
// site/ (scripts/audit-test-off-sale.mjs builds in site/.offsale/).
const APP_JS = (() => {
  let dir = process.cwd();
  for (let i = 0; i < 5; i += 1) {
    const candidate = path.join(dir, 'crates', 'vault-tauri', 'dist', 'app.js');
    if (fs.existsSync(candidate)) return candidate;
    dir = path.dirname(dir);
  }
  return path.resolve(process.cwd(), '..', 'crates', 'vault-tauri', 'dist', 'app.js');
})();
const START = '// >>> CONNECT WORDING';
const END = '// <<< END CONNECT WORDING';

export interface AppAgent {
  name: string;
  desc: string;
  note?: string;
  hint: string;
  openers?: { label: string; command: string | ((command: string) => string) }[];
  pasteHint?: string;
  snippet: ((command: string) => string) | null;
  connect?: string;
}

export interface ConnectWords {
  button: string;
  saved_button?: string;
  note: string;
  waiting?: string;
  asked: string;
  saved?: string;
  app_not_found: string;
  could_not_save?: string;
  could_not_open: string;
}

export interface AppWords {
  SHORT_NAME: string;
  TIP_LINE: string;
  AGENTS: AppAgent[];
  AGENT_CHECKS: Record<string, string>;
  AGENT_TIPS: Record<string, string>;
  CONNECT_WORDS: Record<string, ConnectWords>;
  /** How many steps to follow, worded as the app words it. */
  stepsWord: (n: number) => string;
  /** What to call an app in a sentence, as the app does. */
  appCalled: (name: string) => string;
}

function block(): string {
  const src = fs.readFileSync(APP_JS, 'utf8').replace(/\r\n/g, '\n');
  const start = src.indexOf(START);
  const end = src.indexOf(END);
  if (start === -1 || end === -1 || end < start) {
    throw new Error(`app-words: the CONNECT WORDING markers are missing from ${APP_JS}`);
  }
  return src.slice(start, end);
}

const cache = new Map<boolean, AppWords>();

/** The app's wording as a Windows (false) or Mac (true) computer sees it. */
export function appWords(isMac = false): AppWords {
  const hit = cache.get(isMac);
  if (hit) return hit;
  const names = ['SHORT_NAME', 'TIP_LINE', 'AGENTS', 'AGENT_CHECKS', 'AGENT_TIPS', 'CONNECT_WORDS', 'stepsWord', 'appCalled'];
  // Our own repository's file, evaluated at build time only.
  const words = new Function('IS_MAC', `${block()}\nreturn { ${names.join(', ')} };`)(isMac) as AppWords;
  cache.set(isMac, words);
  return words;
}

/** One app's entry, by the name the app shows. */
export function appAgent(name: string, isMac = false): AppAgent {
  const agent = appWords(isMac).AGENTS.find((a) => a.name === name);
  if (!agent) throw new Error(`app-words: no app named ${name} in the app's list`);
  return agent;
}

/** A step's copy box, with the usual install path where the app fills in this computer's. */
export function openerText(opener: { command: string | ((command: string) => string) }, command: string): string {
  return typeof opener.command === 'function' ? opener.command(command) : opener.command;
}

/**
 * The step titles exactly as the app numbers them (renderAgentCards in
 * app.js; a site test pins the same strings there): 1A/1B when there is a
 * quick way, then the check, then the line that makes the app use Zaaheen.
 */
export function stepTitles(name: string, isMac = false) {
  const words = appWords(isMac);
  const agent = appAgent(name, isMac);
  const quick = Boolean(agent.connect);
  let step = 1;
  const check = words.AGENT_CHECKS[name] ? `Step ${(step += 1)}: Check it's connected` : null;
  const tip = words.AGENT_TIPS[name] ? `Step ${(step += 1)}: Tell ${name} to use Zaaheen (don't skip this)` : null;
  return {
    quick: quick ? 'Step 1A: The quick way' : null,
    byHand: quick ? 'Step 1B: Or add it yourself' : 'Step 1: Add it yourself',
    check,
    tip,
    // The app's own helpers (CONNECT WORDING block), never a copy (s83).
    note: step > 1 ? `Please follow ${words.stepsWord(step)}. If you skip one, ${words.appCalled(name)} may not use Zaaheen.` : null,
  };
}

/** "On this page" for a guide built with ConnectSteps: the same ids and titles. */
export function connectOutline(name: string, prefix = '') {
  const t = stepTitles(name);
  return [
    t.quick && { id: `${prefix}quick`, title: t.quick },
    { id: `${prefix}by-hand`, title: t.byHand },
    t.check && { id: `${prefix}check`, title: t.check },
    t.tip && { id: `${prefix}tip`, title: t.tip },
  ].filter(Boolean) as { id: string; title: string }[];
}
