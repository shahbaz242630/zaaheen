import { execFileSync } from 'node:child_process';
import dates from '../data/page-dates.json';

// When the words on a listed page last changed, as ISO 8601: its entry in
// src/data/page-dates.json, which scripts/page-dates.mjs moves only when the
// page's text changes and scripts/audit.mjs holds to the build (the comment at
// the top of page-dates.mjs says why not a git date).
//
// Search engines only trust <lastmod> when it is "consistently and verifiably
// accurate"; a build timestamp would make every page look fresh on every deploy.
//
// Undefined for a page not yet recorded (a new page before `npm run stamp`):
// silence beats a lie, and the audit refuses to pass it that way.
export function lastModified(path: string): string | undefined {
  return (dates as Record<string, { date: string } | undefined>)[path]?.date;
}

// When a page was first published: the commit that added its own page file
// (the first source), as ISO 8601. Undefined before that file is committed.
// Needs full history: CI checks out with fetch-depth 0.
export function firstPublished(sources: string[]): string | undefined {
  const rel = sources[0];
  if (!rel) return undefined;
  try {
    const stamps = execFileSync('git', ['log', '--diff-filter=A', '--format=%cI', '--', rel], {
      cwd: process.cwd(),
      encoding: 'utf8',
    }).trim().split('\n').filter(Boolean);
    return stamps[stamps.length - 1];
  } catch {
    return undefined;
  }
}
