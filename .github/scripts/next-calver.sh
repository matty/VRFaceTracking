#!/usr/bin/env bash
# Prints the next version, YYYY.M.N, for a tag prefix (`v` or `apk-v`).
# With `dev`, prints YYYY.M.N-dev.C, where C is commits since the last release.
#
# Usage: next-calver.sh <tag-prefix> [dev]
set -euo pipefail
prefix=$1
month=$(date -u +%Y.%-m)
last=-1
for tag in $(git tag --list "$prefix$month.*"); do
  count=${tag#"$prefix$month."}
  if [[ $count =~ ^[0-9]+$ ]] && ((count > last)); then
    last=$count
  fi
done
next="$month.$((last + 1))"
if [ "${2:-}" != dev ]; then
  echo "$next"
  exit 0
fi
if previous=$(git describe --tags --match "$prefix[0-9]*" --abbrev=0 2>/dev/null); then
  commits=$(git rev-list --count "$previous..HEAD")
else
  commits=$(git rev-list --count HEAD)
fi
echo "$next-dev.$commits"
