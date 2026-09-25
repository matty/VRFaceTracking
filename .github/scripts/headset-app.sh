#!/usr/bin/env bash
# Answers release questions about the headset app from its source and tags.
#
#   headset-app.sh protocol [rev]        the stream protocol the app speaks at rev (default HEAD)
#   headset-app.sh newest <low> <high>   the newest apk-v tag that speaks a protocol in low..high
set -euo pipefail
source_file=android/questpro-camera/app/src/main/java/io/github/matty/vrft/questprocamera/GazePackets.java

protocol_at() {
  git show "$1:$source_file" 2>/dev/null |
    sed -n 's/.*int PROTOCOL = \([0-9]*\);.*/\1/p' || true
}

case $1 in
  protocol)
    protocol_at "${2:-HEAD}"
    ;;
  newest)
    low=$2 high=$3
    for tag in $(git tag --list 'apk-v[0-9]*' --sort=-v:refname); do
      protocol=$(protocol_at "$tag")
      if [ -n "$protocol" ] && ((protocol >= low && protocol <= high)); then
        echo "$tag"
        exit 0
      fi
    done
    ;;
  *)
    echo "usage: headset-app.sh protocol [rev] | newest <low> <high>" >&2
    exit 2
    ;;
esac
