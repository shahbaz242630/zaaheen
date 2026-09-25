#!/usr/bin/env bash
# Prints "app=true" or "app=false" for $GITHUB_OUTPUT: whether a change can
# touch the app, so its long checks (cargo clippy, cargo build + test, CodeQL
# for Rust) must run.
#
#   app-changed.sh <event name> <base sha> <head sha>
#
# Only a pull request whose every changed file is website (site/) or a notes
# file at the repository root (*.md; no crate reads them, the one repo-walking
# test walks crates/ only) prints app=false. Everything else prints app=true:
# a push to main, a manual or scheduled run, a missing sha, an empty diff, and
# any file anywhere else, .github/ included. When unsure, run everything.
set -euo pipefail

event="${1:-}"
base="${2:-}"
head="${3:-}"

if [ "$event" != "pull_request" ] || [ -z "$base" ] || [ -z "$head" ]; then
  echo "app=true"
  exit 0
fi

files="$(git diff --name-only "$base...$head")"
printf 'Changed files:\n%s\n' "$files" >&2

if [ -z "$files" ]; then
  echo "app=true"
elif printf '%s\n' "$files" | grep -Evq '^site/|^[^/]+\.md$'; then
  echo "app=true"
else
  echo "Website-only change: the app's long checks stand down." >&2
  echo "app=false"
fi
