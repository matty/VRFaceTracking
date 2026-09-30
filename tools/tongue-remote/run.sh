#!/bin/bash
# On the GPU server: trains every job in a jobs file, one per GPU at a time,
# then scores each on a test recording. Paths are under /workspace/vrft/data.
# A jobs file has one job per line:
#
#   test <recording>                  the recording every job is scored on
#   base <model dir>                  the pair every job starts from
#   <name> <epochs> <rate> <layers> <recording>[,<recording>...]
#
# Each job's pair, report.json, training log and score end up in
# /workspace/vrft/results/<name>/. A job whose report already exists is
# scored again but not retrained.
set -euo pipefail
ROOT=/workspace/vrft
export PATH="$HOME/.cargo/bin:$PATH"
BIN="$ROOT/src/target/release/examples"
GPUS=$(nvidia-smi -L | wc -l)
test="" base=""
names=() lines=()
while read -r first rest; do
    case "$first" in
    "" | "#"*) ;;
    test) test="$ROOT/data/$rest" ;;
    base) base="$ROOT/data/$rest" ;;
    *) names+=("$first"); lines+=("$rest") ;;
    esac
done < "$1"
: "${test:?the jobs file needs a test line}" "${base:?the jobs file needs a base line}"

train() {
    local name=$1 gpu=$2 epochs=$3 rate=$4 layers=$5 recordings=$6
    local out="$ROOT/results/$name"
    [ -e "$out/report.json" ] && return
    rm -rf "$out"
    mkdir -p "$out"
    local list
    list=$(echo "$recordings" | tr ',' '\n' | sed "s#^#$ROOT/data/#" |
        python3 -c 'import json, sys; print(json.dumps([l.strip() for l in sys.stdin if l.strip()]))')
    echo "{\"name\": \"$name\", \"device\": \"gpu\", \"base_model_dir\": \"$base\", \"recordings\": $list}" \
        > "$out/request.json"
    echo "$name: training on GPU $gpu"
    CUDA_VISIBLE_DEVICES=$gpu "$BIN/train" --request "$out/request.json" --output "$out" \
        --epochs "$epochs" --learning-rate "$rate" --layers "$layers" > "$out/training.log" 2>&1 ||
        echo "$name: training failed, see $out/training.log"
}

for i in "${!names[@]}"; do
    gpu=$((i % GPUS))
    read -r epochs rate layers recordings <<< "${lines[$i]}"
    train "${names[$i]}" "$gpu" "$epochs" "$rate" "$layers" "$recordings" &
    # Once every GPU has a job, wait for this round before starting the next.
    if [ $((gpu + 1)) -eq "$GPUS" ]; then wait; fi
done
wait

for name in "${names[@]}"; do
    out="$ROOT/results/$name"
    echo "=== $name"
    if [ -e "$out/report.json" ]; then
        "$BIN/evaluate" "$out" "$test" | tee "$out/score.txt" | tail -3
    else
        tail -3 "$out/training.log"
    fi
done
