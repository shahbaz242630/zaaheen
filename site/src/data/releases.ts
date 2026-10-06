// What changed in each public release (founder, session 87: "a page where we
// publish bugs, errors, issues and fixed on which release"). Shown on
// /docs/release-notes/ and, for the latest release, in llms.txt. Newest first.
// The first entry must be the version the download buttons offer
// (RELEASE.version in site.ts): the page refuses to build otherwise.
// Plain words, the app's own labels, no internals and no vendor names; a
// security fix is described only once the fixed version is out.
export interface ReleaseNote {
  version: string;
  /** The day it went on zaaheen.com, YYYY-MM-DD. */
  date: string;
  /** One sentence. */
  summary: string;
  added: string[];
  fixed: string[];
}

export const RELEASE_NOTES: ReleaseNote[] = [
  {
    version: '0.3.5',
    date: '2026-10-06',
    summary: 'Security fixes from our own audit.',
    added: [],
    fixed: [
      "An AI app could change a memory that was kept away from it. It can now change only the memories it's allowed to see.",
      "Zaaheen's log file could record words from your memories or your searches. It no longer does.",
      'The name an AI app gives itself when it saves a memory is now cleaned and shortened, and shown as not verified.',
      'Zaaheen now checks your plan with our server each time it starts and once a day, instead of relying on the record kept on your computer.',
    ],
  },
  {
    version: '0.3.4',
    date: '2026-10-06',
    summary: 'A fix to setting up again after deleting an account.',
    added: [],
    fixed: [
      'After Delete my account, setting up again without closing Zaaheen left your AI apps unable to reach it until the app was reopened. They now connect straight away.',
    ],
  },
  {
    version: '0.3.3',
    date: '2026-10-05',
    summary:
      'Cancel your subscription from the app, tidy-up switched on from the start, and fixes to closing Zaaheen and to setting up again after deleting an account. (0.3.2 was a test version and was never offered for download.)',
    added: [
      'Cancel subscription in Settings, then Account, for paid plans. It also names the other ways to cancel: the Account page on zaaheen.com, and the cancel link in any email about your subscription.',
      'Setup now has "Keep my vault tidy automatically" switched on, so Zaaheen tidies your memories every day at 3 am. "Not now" is still one click away.',
      'After paying, Zaaheen checks again as soon as you come back to its window, and "Not now" ends the wait if you closed the checkout without paying.',
      'A subscription you start or cancel on the website, or an account you delete there, shows in the app when you open Settings or come back to its window.',
      'Settings shows the version plainly, for example "Zaaheen 0.3.3".',
    ],
    fixed: [
      'Close Zaaheen did nothing on the lock screen, after Delete everything, and after Delete my account. It now closes the app.',
      'After Delete my account, signing in again skipped setup, so the nightly tidy-up was not set up again. Zaaheen now starts like a new install.',
      'The last screen of Delete my account now says plainly that all the memories on this computer are deleted, and Done takes you back to the welcome screen.',
    ],
  },
  {
    version: '0.3.1',
    date: '2026-10-03',
    summary: 'Fixes for deleting on a Mac and for the newest Claude Code.',
    added: [
      'Signing in ends on a zaaheen.com page that tells you to go back to the app.',
      'When something goes wrong, Zaaheen points your AI app to its help and fixes at zaaheen.com/llms.txt.',
    ],
    fixed: [
      'On a Mac, Delete everything and Delete my account stopped before deleting anything. They now delete your memories and the key that protects them.',
      'The newest Claude Code refused every answer from Zaaheen ("missing required resultType"). Claude Code now saves and recalls normally.',
    ],
  },
  {
    version: '0.3.0',
    date: '2026-10-01',
    summary: 'The first public release, for Windows and Mac.',
    added: [
      'One private memory for your AI apps, encrypted and kept on your own computer.',
      'Connects Claude Desktop, Claude Code, Cursor, ChatGPT, Codex, Antigravity and other apps that support MCP.',
      'A 30-day free trial with no card, then a monthly or yearly plan.',
      'Tidying up, which merges near-duplicate memories, and Delete everything in one step.',
    ],
    fixed: [],
  },
];
