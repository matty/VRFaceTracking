#!/bin/bash
# Trains tongue models on a rented Linux GPU server, one run per GPU.
# Run from the repo root in Git Bash. VRFT_REMOTE is the server's SSH target
# (user@host) and VRFT_REMOTE_PORT its SSH port (default 22); keep both out
# of the repo.
#
#   tools/tongue-remote/remote.sh setup
#       copies the source and builds the tools there (Rust is installed if missing)
#   tools/tongue-remote/remote.sh data <name> <local recording or model dir>...
#       uploads folders to data/<name>/ on the server
#   tools/tongue-remote/remote.sh run <jobs file>
#       trains each job, one per GPU at a time, and scores it (see run.sh)
#   tools/tongue-remote/remote.sh fetch <local dir>
#       downloads every job's report, scores and model pair
set -euo pipefail
: "${VRFT_REMOTE:?set VRFT_REMOTE to user@host of the server}"
PORT="${VRFT_REMOTE_PORT:-22}"
ROOT=/workspace/vrft
ssh_() { ssh -p "$PORT" -o ServerAliveInterval=30 "$VRFT_REMOTE" "$@"; }

case "${1:-}" in
setup)
    # Tracked files plus new ones git doesn't ignore, minus local-only notes.
    git ls-files -co --exclude-standard |
        grep -v -e '^VRCFT_Reference/' -e '^docs/superpowers/' -e '^CLAUDE.md$' |
        tar cf - -T - | ssh_ "mkdir -p $ROOT/src && tar xf - -C $ROOT/src"
    ssh_ "bash $ROOT/src/tools/tongue-remote/build.sh"
    ;;
data)
    name="$2"
    shift 2
    for dir in "$@"; do
        echo "uploading $dir"
        tar cf - -C "$(dirname "$dir")" "$(basename "$dir")" |
            ssh_ "mkdir -p $ROOT/data/$name && tar xf - -C $ROOT/data/$name"
    done
    ;;
run)
    ssh_ "cat > $ROOT/jobs.txt" < "$2"
    ssh_ "bash $ROOT/src/tools/tongue-remote/run.sh $ROOT/jobs.txt"
    ;;
fetch)
    mkdir -p "$2"
    ssh_ "tar cf - -C $ROOT results" | tar xf - -C "$2"
    echo "results in $2/results"
    ;;
*)
    sed -n '2,14p' "$0"
    exit 1
    ;;
esac
