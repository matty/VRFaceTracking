"""Benchmarks VRFT's tongue models beside QFT+'s universal face model and
prints a Markdown table. Each run is its own process, so each memory
figure is that model's alone.

    cargo build -p vrft-tongue --release --example benchmark
    pip install onnxruntime numpy        # onnxruntime-directml for --gpu on Windows
    python tools/benchmark/qftplus.py --fetch
    python tools/benchmark/compare.py --pair <model dir> --face <universal-face-v1.safetensors> [--gpu]

Every model runs on the CPU with one thread (as QFT+ runs ONNX Runtime) and
with all cores. `--gpu` adds the GPU: wgpu for VRFT, DirectML for QFT+.
`--recording` times real five-camera frames instead of a noise strip.
"""

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
EXE = ".exe" if sys.platform == "win32" else ""
BENCHMARK = REPO / "target" / "release" / "examples" / f"benchmark{EXE}"
COLUMNS = [("model", "Model"), ("runtime", "Runtime"), ("device", "Device"), ("threads", "Threads"),
           ("mean_ms", "Mean ms"), ("p50_ms", "p50 ms"), ("p95_ms", "p95 ms"), ("fps", "Frames/s"),
           ("model_mb", "Model MB"), ("rss_before_mb", "RSS before MB"), ("rss_after_mb", "RSS running MB"),
           ("peak_mb", "Peak MB"), ("load_ms", "Load ms")]


def run(command, threads):
    env = dict(os.environ, RAYON_NUM_THREADS=str(threads))
    done = subprocess.run(command, capture_output=True, text=True, env=env)
    if done.returncode:
        sys.stderr.write(done.stderr)
        raise SystemExit(f"failed: {' '.join(map(str, command))}")
    return json.loads(done.stdout.strip().splitlines()[-1])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--pair", type=Path, help="a stereo pair model folder")
    parser.add_argument("--face", type=Path, help="a universal-face-v1 checkpoint")
    parser.add_argument("--qftplus", type=Path, default=REPO / ".local" / "qftplus" / "universal-face-v2.npz")
    parser.add_argument("--gpu", action="store_true")
    parser.add_argument("--frames", type=int, default=300)
    parser.add_argument("--recording", type=Path, help="a five-camera recording to time")
    parser.add_argument("--threads", type=int, default=os.cpu_count())
    args = parser.parse_args()
    common = ["--frames", str(args.frames), "--json"]
    if args.recording:
        common += ["--recording", str(args.recording)]
    runs = []
    for threads in sorted({1, args.threads}):
        if args.pair:
            runs.append(([BENCHMARK, "--pair", args.pair, "--device", "cpu", *common], threads))
        if args.face:
            runs.append(([BENCHMARK, "--face", args.face, "--device", "cpu", *common], threads))
        runs.append(([sys.executable, REPO / "tools" / "benchmark" / "qftplus.py", "--model", args.qftplus,
                      "--threads", str(threads), *common], threads))
    if args.gpu:
        if args.pair:
            runs.append(([BENCHMARK, "--pair", args.pair, "--device", "gpu", *common], args.threads))
        if args.face:
            runs.append(([BENCHMARK, "--face", args.face, "--device", "gpu", *common], args.threads))
        runs.append(([sys.executable, REPO / "tools" / "benchmark" / "qftplus.py", "--model", args.qftplus,
                      "--provider", "directml", *common], 1))
    rows = []
    for command, threads in runs:
        row = run([str(part) for part in command], threads)
        print(f"compare: {row['model']} {row['device']} x{row['threads']}: {row['mean_ms']} ms", file=sys.stderr)
        rows.append(row)
    print("| " + " | ".join(title for _, title in COLUMNS) + " |")
    print("|" + "---|" * len(COLUMNS))
    for row in rows:
        print("| " + " | ".join("" if row.get(key) is None else str(row[key]) for key, _ in COLUMNS) + " |")


if __name__ == "__main__":
    main()
