#!/usr/bin/env bash
# Prints release notes: commits since the last tag with this prefix, grouped by type.
#
# Usage: release-notes.sh <tag-prefix> [pathspec...]
set -euo pipefail
prefix=$1
shift
previous=$(git tag --list "$prefix[0-9]*" --sort=-v:refname | sed -n 1p)
if [ -z "$previous" ]; then
  echo "First release."
  exit 0
fi
subjects=$(git log --no-merges --format=%s "$previous..HEAD" -- "$@")
if [ -z "$subjects" ]; then
  echo "No changes since ${previous#"$prefix"}."
  exit 0
fi

# Prints one section of subjects matching (or with -v, not matching) a type.
section() {
  local title=$1 lines
  shift
  lines=$(grep -E "$@" <<<"$subjects" |
    sed -E 's/^[a-z]+(\([^)]*\))?!?: //; s/^(.)/\U\1/; s/^/- /' || true)
  if [ -n "$lines" ]; then
    printf '### %s\n\n%s\n\n' "$title" "$lines"
  fi
}

printf 'Changes since %s.\n\n' "${previous#"$prefix"}"
section "New" '^feat(\(|!|:)'
section "Fixes" '^fix(\(|!|:)'
section "Other changes" -v '^(feat|fix)(\(|!|:)'
