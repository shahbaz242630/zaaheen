#!/usr/bin/env bash
# Proves app-changed.sh stands the app's checks down only for website-only
# pull requests. Runs in a throwaway repository; exits 1 on any wrong answer.
# CI runs it before trusting the script, so a broken rule blocks the pull
# request (no check reports) instead of skipping checks it should not.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/app-changed.sh"
repo="$(mktemp -d)"
trap 'rm -rf "$repo"' EXIT
cd "$repo"
git init -q
git config user.email test@example.invalid
git config user.name test
echo base > README.md
git add -A
git commit -qm base
base="$(git rev-parse HEAD)"

failed=0
n=0
expect() { # <name> <want> <got>
  if [ "$3" = "app=$2" ]; then echo "ok    $1"; else echo "FAIL  $1 (want app=$2, got $3)"; failed=1; fi
}
change() { # <name> <want> <files...>
  local name="$1" want="$2"
  shift 2
  n=$((n + 1))
  git checkout -q "$base"
  git checkout -q -b "case$n"
  for f in "$@"; do
    mkdir -p "$(dirname "$f")"
    echo "$n" > "$f"
  done
  git add -A
  git commit -qm "case $n" --allow-empty
  expect "$name" "$want" "$(bash "$script" pull_request "$base" "$(git rev-parse HEAD)" 2>/dev/null)"
}

change "website only" false site/src/pages/x.astro
change "website and a root notes file" false site/a.css HANDOFF.md
change "website and a crate" true site/a.css crates/x/src/lib.rs
change "a nested .md" true docs/guide.md
change "the site workflow" true .github/workflows/site.yml
change "a root file named site.txt" true site.txt
change "a folder named sitefoo" true sitefoo/x
change "no files at all" true
expect "a push to main" true "$(bash "$script" push "$base" "$base" 2>/dev/null)"
expect "missing shas" true "$(bash "$script" pull_request "" "" 2>/dev/null)"
expect "no arguments" true "$(bash "$script" 2>/dev/null)"

exit "$failed"
