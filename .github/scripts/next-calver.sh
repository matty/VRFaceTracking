#!/usr/bin/env bash
# Prints the next CalVer version, YYYY.M.N, for a release tag prefix: `v` for
# VRFT, `apk-v` for the headset app. N counts that prefix's releases this
# month (UTC), from 0.
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
echo "$month.$((last + 1))"
