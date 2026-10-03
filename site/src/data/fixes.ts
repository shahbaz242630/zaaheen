// Problems and fixes keyed by the EXACT words people (and their AI agents)
// see, so a search for an error message finds its fix (session 82, founder:
// users will ask their agent to "check Zaaheen's documents for the fix").
// Shown on /docs/troubleshooting/ and in llms.txt. Add one each time a live
// test finds a new one; every entry comes from a real test.
export interface Fix {
  /** The words on screen or in the app's log, verbatim. */
  error: string;
  /** Where it appears. */
  where: string;
  /** What to do, in a sentence or two. */
  fix: string;
}

export const FIXES: Fix[] = [
  {
    error: 'Invalid result for tools/call: missing required resultType',
    where: 'Claude Code, when it calls Zaaheen',
    fix: 'Update Zaaheen to the latest version from zaaheen.com: version 0.3.1 and later answer the newest apps correctly.',
  },
  {
    error: 'The filename, directory name, or volume label syntax is incorrect. (os error 123)',
    where: "ChatGPT Desktop or Codex, in the app's log; ChatGPT itself only says Zaaheen isn't accessible",
    fix: 'The Command has a space or quote marks before C:. Edit the MCP server so the Command starts exactly with C:\\ (copy it with the Copy button in Zaaheen\'s Agents tab), save, and restart ChatGPT.',
  },
  {
    error: "Zaaheen isn't accessible in this chat",
    where: 'ChatGPT Desktop',
    fix: "Use a Work or Codex chat (ChatGPT's Chat mode can't connect to apps on your computer), check the Command has no space before it, then quit ChatGPT completely and open it again.",
  },
  {
    error: 'request timed out (zaaheen: Tools: (none))',
    where: 'Codex in the terminal, after /mcp',
    fix: 'Update Codex (npm install -g @openai/codex@latest), then run codex again and type /mcp: zaaheen shows as connected with 5 tools.',
  },
  {
    error: 'The AI app saves to its own memory, or answers "I don\'t know", instead of using Zaaheen',
    where: 'Claude Desktop, ChatGPT Desktop, Codex',
    fix: "Add Zaaheen's line to the app's instructions (Claude: Settings, Account, Instructions for Claude; ChatGPT: Custom instructions; Codex: AGENTS.md in the .codex folder). The line is on this page and in each connect guide.",
  },
  {
    error: 'Double-clicking "Zaaheen for Claude.mcpb" asks which app to open it with',
    where: 'Windows, with Claude from the Microsoft Store',
    fix: 'Cancel that box. In Claude, open Settings, then Extensions, then Advanced settings, then Install Extension, and choose "Zaaheen for Claude.mcpb".',
  },
  {
    error: "isn't commonly downloaded. Make sure you trust [the Zaaheen installer] before you open it.",
    where: 'Microsoft Edge, when downloading',
    fix: 'Choose the three dots next to the download, then Keep, then Show more, then Keep anyway. The installer is new and not code-signed yet, so Edge has not seen it often.',
  },
  {
    error: 'Windows protected your PC. Microsoft Defender SmartScreen prevented an unrecognized app from starting.',
    where: 'Windows, when opening the installer',
    fix: 'Choose More info, then Run anyway.',
  },
  // s81 friend's Mac test of 0.3.0: the delete could not remove the key, so
  // nothing was deleted (ADR-SEC-040, fixed in 0.3.1).
  {
    error: 'Your memories were NOT deleted, and they are still readable.',
    where: 'Mac, after Delete everything or Delete my account in Zaaheen 0.3.0',
    fix: 'Update Zaaheen to the latest version from zaaheen.com (0.3.1 or later), then try again. Nothing was deleted, and your account and subscription were not changed.',
  },
  // A known limit, not seen in testing (s83 review): on a Mac the delete
  // waits, with no time limit, for any keychain question macOS asks, so it
  // never claims a delete it cannot confirm.
  {
    error: 'Still working. This is taking longer than usual. Please keep Zaaheen open.',
    where: 'Mac, during Delete everything or Delete my account',
    fix: 'macOS may be asking about Zaaheen and your keychain in a box behind the Zaaheen window: look for it and choose Allow. The delete then finishes. Nothing is deleted until your key is gone, so if you close Zaaheen instead, your memories are still there and you can try again.',
  },
  // A known limit (s83 review): after a sign-in the browser moves on to
  // zaaheen.com/signed-in/, a confirmation page only.
  {
    error: "This site can't be reached (or another browser error) right after signing in",
    where: 'Your browser, after signing in from the Zaaheen app',
    fix: 'Go back to the Zaaheen app: it shows whether you are signed in. The page after sign-in only confirms it, so it does not matter if that page failed to load.',
  },
];
