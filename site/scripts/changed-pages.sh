#!/usr/bin/env bash
# Prints the files that really changed between the last two commits of the
# site-deploy branch, one per line. IndexNow is told about these pages.
#
# Every page links shared assets whose names carry a content hash, so one styling
# edit renames them on every page. A page whose only difference is such a name
# did not change for a reader and is left out; telling the search engines that
# the whole site changed after each styling edit risks a spam flag.
#
# Bash, git and sed only: the publish job never installs or runs npm packages.
set -euo pipefail

if ! git rev-parse -q --verify 'HEAD~1' > /dev/null; then
  git ls-files
  exit 0
fi

normalise() {
  sed -E 's#/_astro/[A-Za-z0-9._-]+\.(css|js)#/_astro/asset.\1#g'
}

git -c core.quotePath=false diff --name-only 'HEAD~1' HEAD | while IFS= read -r file; do
  case "$file" in
    *.html)
      if git cat-file -e "HEAD~1:$file" 2> /dev/null && git cat-file -e "HEAD:$file" 2> /dev/null; then
        if cmp -s <(git show "HEAD~1:$file" | normalise) <(git show "HEAD:$file" | normalise); then
          continue
        fi
      fi
      ;;
  esac
  printf '%s\n' "$file"
done
