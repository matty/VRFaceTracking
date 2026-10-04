"""Times QFT+'s universal face model (universal-face-v2) the way QFT+ runs
it, one frame at a time, and reports its latency and the process's memory,
in the same JSON as `cargo run --example benchmark`. For comparison only:
QFT+'s weights are trained on Ava-256 (CC BY-NC 4.0) and never ship with
VRFT.

    pip install onnxruntime numpy        # onnxruntime-directml for --provider directml
    python tools/benchmark/qftplus.py --fetch
    python tools/benchmark/qftplus.py [--threads 1] [--provider cpu|directml] [--frames 300]

Per frame, as QFT+'s universal_face.py does (v0.4.0-rc.25.2): the ONNX
graph on the raw 400 x 2000 strip (it shrinks the views itself), then the
mouth head and the brow head in NumPy. The session options are QFT+'s: one
intra-op and one inter-op thread, no spinning, sequential, no memory
pattern; `--threads` changes the thread count. `--fetch` downloads QFT+'s
release package (194 MB), checks its SHA-256 and keeps only the model's two
files, in `.local/qftplus/`.
"""

import argparse
import hashlib
import io
import json
import os
import sys
import time
import urllib.request
import zipfile
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]
MODEL = REPO / ".local" / "qftplus" / "universal-face-v2.npz"
RELEASE = ("https://github.com/Yeusepe/QFTPlus/releases/download/v0.4.0-rc.25.2/"
           "QproFaceTracking.App-0.4.0-rc.25.2-full.nupkg")
RELEASE_SHA256 = "fffa879943724c5621ecdef42085bcb73aae19a5ccc12b5255d74220ba9bc61d"
FILES = ("lib/app/models/universal-face-v2.area.onnx", "lib/app/models/universal-face-v2.npz")


def fetch(out_dir):
    print(f"qftplus: downloading {RELEASE}", file=sys.stderr, flush=True)
    with urllib.request.urlopen(RELEASE) as response:
        data = response.read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != RELEASE_SHA256:
        raise SystemExit(f"qftplus: the package's SHA-256 is {digest}, expected {RELEASE_SHA256}")
    out_dir.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(io.BytesIO(data)) as package:
        for name in FILES:
            (out_dir / Path(name).name).write_bytes(package.read(name))
    print(f"qftplus: wrote {out_dir}", file=sys.stderr, flush=True)


def memory():
    """The process's resident memory and its peak so far, in bytes."""
    if sys.platform.startswith("linux"):
        fields = {}
        for line in Path("/proc/self/status").read_text().splitlines():
            name, _, rest = line.partition(":")
            if name in ("VmRSS", "VmHWM"):
                fields[name] = int(rest.split()[0]) * 1024
        return fields.get("VmRSS"), fields.get("VmHWM")
    if sys.platform == "win32":
        import ctypes
        from ctypes import wintypes

        class Counters(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD),
                        ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
                        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t),
                        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t), ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                        ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t)]
        counters = Counters(cb=ctypes.sizeof(Counters))
        process = ctypes.windll.kernel32.GetCurrentProcess()
        if ctypes.windll.psapi.GetProcessMemoryInfo(process, ctypes.byref(counters), counters.cb):
            return counters.WorkingSetSize, counters.PeakWorkingSetSize
    return None, None


def megabytes(value):
    return None if value is None else round(value / 1048576, 1)


# QFT+'s heads (src/tracking/universal_face.py, MIT), unchanged.
def silu(x):
    return x / (1.0 + np.exp(-np.clip(x, -60.0, 60.0)))


def sigmoid(x):
    return 1.0 / (1.0 + np.exp(-np.clip(x, -60.0, 60.0)))


def head_forward(h, q, anchors, present):
    a = np.where(present[:, None] > 0, anchors, h["missing"])
    z = np.concatenate([q, a.ravel(), q - a[0], present])
    z = silu(h["w1"] @ z + h["b1"]); z = silu(h["w2"] @ z + h["b2"])
    return sigmoid(h["w3"] @ z + h["b3"])


def brow_forward(h, w, neutral, present):
    n = neutral if present else h["missing"]
    return sigmoid(h["w2"] @ silu(h["w1"] @ np.concatenate([w, w - n, np.float32([present])]) + h["b1"]) + h["b2"])


def session(graph, threads, provider):
    import onnxruntime as ort
    options = ort.SessionOptions()
    options.enable_mem_pattern = False
    options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    options.intra_op_num_threads = options.inter_op_num_threads = threads
    options.add_session_config_entry("session.intra_op.allow_spinning", "0")
    providers = ["CPUExecutionProvider"]
    if provider == "directml":
        providers.insert(0, ("DmlExecutionProvider", {"device_id": 0, "disable_metacommands": "true"}))
    return ort.InferenceSession(graph, sess_options=options, providers=providers), ort.__version__


def frames_from(recording):
    if recording is None:
        # A fixed noise strip, as the Rust benchmark uses.
        state, out = 0x2545F4914F6CDD1D, bytearray(400 * 2000)
        mask = (1 << 64) - 1
        for i in range(len(out)):
            state ^= (state << 13) & mask
            state ^= state >> 7
            state ^= (state << 17) & mask
            out[i] = state >> 56
        return [np.frombuffer(bytes(out), np.uint8).reshape(400, 2000)]
    metadata = json.loads((recording / "metadata.json").read_text())
    if metadata.get("width") != 2000:
        raise SystemExit(f"{recording} doesn't hold five-camera strips")
    raw = np.fromfile(recording / "frames.gray8", np.uint8, count=64 * 400 * 2000)
    return list(raw.reshape(-1, 400, 2000))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", type=Path, default=MODEL)
    parser.add_argument("--fetch", action="store_true", help="download QFT+'s model to .local/qftplus/ and stop")
    parser.add_argument("--threads", type=int, default=1, help="ONNX Runtime threads (QFT+ uses 1)")
    parser.add_argument("--provider", choices=["cpu", "directml"], default="cpu")
    parser.add_argument("--frames", type=int, default=300)
    parser.add_argument("--warmup", type=int, default=30)
    parser.add_argument("--recording", type=Path)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if args.fetch:
        fetch(args.model.parent)
        return
    before = memory()
    started = time.perf_counter()
    graph_path = args.model.with_suffix(".area.onnx")
    with np.load(args.model, allow_pickle=False) as z:
        head = {k[5:]: z[k].astype(np.float32) for k in z.files if k.startswith("head_")}
        brow = {k[5:]: z[k].astype(np.float32) for k in z.files if k.startswith("brow_")}
    model, version = session(graph_path.read_bytes(), args.threads, args.provider)
    anchors, present = np.zeros((6, 512), np.float32), np.zeros(6, np.float32)
    neutral = np.zeros(480, np.float32)
    model.run(None, {"cameras": np.zeros((400, 2000), np.uint8)})
    load_ms = (time.perf_counter() - started) * 1000
    loaded = memory()

    def frame(strip):
        q, t, w = model.run(None, {"cameras": np.ascontiguousarray(strip)})
        head_forward(head, q[0], anchors, present)
        brow_forward(brow, w[0], neutral, 0.0)

    frames = frames_from(args.recording)
    for i in range(args.warmup):
        frame(frames[i % len(frames)])
    times = []
    for i in range(args.frames):
        at = time.perf_counter()
        frame(frames[i % len(frames)])
        times.append((time.perf_counter() - at) * 1000)
    after = memory()
    times.sort()
    mean = sum(times) / len(times)
    pick = lambda share: round(times[round((len(times) - 1) * share)], 3)
    report = {
        "model": "QFT+ universal face v2", "runtime": f"ONNX Runtime {version}",
        "device": model.get_providers()[0], "threads": args.threads, "frames": len(times),
        "input": "recording" if args.recording else "noise", "load_ms": round(load_ms, 3),
        "mean_ms": round(mean, 3), "p50_ms": pick(0.5), "p95_ms": pick(0.95), "p99_ms": pick(0.99),
        "fps": round(1000 / mean, 3),
        "model_mb": megabytes(graph_path.stat().st_size + args.model.stat().st_size),
        "rss_before_mb": megabytes(before[0]), "rss_loaded_mb": megabytes(loaded[0]),
        "rss_after_mb": megabytes(after[0]), "peak_mb": megabytes(after[1]),
    }
    print(json.dumps(report) if args.json else json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
