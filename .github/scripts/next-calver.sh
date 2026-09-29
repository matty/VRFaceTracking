#!/usr/bin/env bash
# Prints the next CalVer version, YYYY.M.N, for a release tag prefix: `v` for
# VRFT, `apk-v` for the headset app. N counts that prefix's releases this
# month (UTC), from 0.
#
# With `dev`, prints a development build of that next release instead,
# YYYY.M.N-dev.C, where C counts the commits since the last release with the
# prefix. It sorts after every earlier dev build and before the release
# itself, as the updater needs. build-support/version.rs does the same for
# local builds.
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
