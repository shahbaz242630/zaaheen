import { execFileSync } from 'node:child_process';

// The newest git commit date across a page's source files, as ISO 8601.
//
// Search engines only trust <lastmod> when it is "consistently and verifiably
// accurate", so it must move when a page's content changes and never otherwise.
// A build timestamp would make every page look fresh on every deploy. Git dates
// are stable across rebuilds of the same commit.
//
// Needs full history: CI must check out with fetch-depth 0, or every file
// reports the date of the single commit it has.
//
// Returns undefined when no source has been committed yet: silence beats a lie.
export function lastModified(sources: string[]): string | undefined {
  let newest: string | undefined;
  for (const rel of sources) {
    let stamp = '';
    try {
      stamp = execFileSync('git', ['log', '-1', '--format=%cI', '--', rel], {
        cwd: process.cwd(),
        encoding: 'utf8',
      }).trim();
    } catch {
      stamp = '';
    }
    if (stamp && (!newest || Date.parse(stamp) > Date.parse(newest))) newest = stamp;
  }
  return newest;
}
