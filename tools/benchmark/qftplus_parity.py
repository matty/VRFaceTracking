"""Replays frames through QFT+'s own universal_face.py and prints what it
outputs per frame, as JSON lines, to compare with VRFT's port:

    python tools/benchmark/qftplus.py --fetch          # model and QFT+'s Python, into .local/qftplus/
    python tools/benchmark/qftplus_parity.py <replay.json> > qftplus.jsonl
    cargo run -p vrft-tongue --release --example universal_v2_replay -- <replay.json> > vrft.jsonl
    python tools/benchmark/qftplus_parity.py --compare qftplus.jsonl vrft.jsonl

`replay.json` (made by `--make <five-camera recording with a face setup>`):
the recording, its face setup's frames by slot, and per frame the time and
the Meta native sample to use (or none). QFT+'s sockets and GPU lock are
stubbed out; nothing else is changed.
"""

import argparse
import json
import math
import sys
import types
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]
APP = REPO / ".local" / "qftplus" / "app"
MODEL = REPO / ".local" / "qftplus" / "universal-face-v2.npz"
NATIVE_NAMES = [
    "JawDrop", "LipsToward", "TongueOut", "CheekPuffL", "CheekPuffR", "CheekSuckL", "CheekSuckR",
    "InnerBrowRaiserL", "InnerBrowRaiserR", "OuterBrowRaiserL", "OuterBrowRaiserR", "BrowLowererL", "BrowLowererR",
    "LipCornerPullerL", "LipCornerPullerR", "MouthLeft", "MouthRight", "ChinRaiserB", "ChinRaiserT",
    "DimplerL", "DimplerR", "UpperLipRaiserL", "UpperLipRaiserR", "LowerLipDepressorL", "LowerLipDepressorR",
]
FRAME = 400 * 2000


def strips(recording, indices):
    raw = np.memmap(Path(recording) / "frames.gray8", np.uint8, mode="r")
    return np.stack([np.array(raw[i * FRAME:(i + 1) * FRAME]).reshape(400, 2000) for i in indices])


def make(recording, out, seed):
    """A replay of a five-camera recording: its face setup by anchor slot,
    then every frame with a native sample that wanders, goes stale and comes
    back, and a stretch of speech-like jaw motion."""
    samples = [json.loads(line) for line in open(Path(recording) / "samples.jsonl")]
    rng = np.random.default_rng(seed)
    setup = {}
    for s in samples:
        slot = s.get("anchor")
        if slot:
            setup.setdefault(slot, []).append(s["index"])
    frames, now = [], 0
    values = rng.uniform(0, 0.1, len(NATIVE_NAMES))
    for k, s in enumerate(samples):
        now += 41_666_667
        values = np.clip(values + rng.normal(0, 0.03, len(values)), 0, 1)
        if 40 <= k < 70:  # speech: jaw at ~4 Hz
            values[0] = 0.3 + 0.2 * math.sin(2 * math.pi * 4 * now / 1e9)
        native = {"arrival": now - int(rng.integers(0, 30_000_000)), "values": dict(zip(NATIVE_NAMES, map(float, values)))}
        if k % 17 == 5:
            native = None  # no sample
        elif k % 23 == 7:
            native["arrival"] -= 300_000_000  # stale
        if k == 90:
            now += 3_000_000_000  # taken off and put back on
        frames.append({"index": s["index"], "now": now, "native": native})
    json.dump({"recording": str(recording), "setup": setup, "frames": frames}, open(out, "w"))
    print(f"replay: {len(frames)} frames, setup slots {sorted(setup)}", file=sys.stderr)


def run(replay_path, model):
    sys.path.insert(0, str(APP))
    # Stubs for what doesn't matter here: the GPU lock and the VRCFT socket.
    sys.modules["gpu_lock"] = types.SimpleNamespace(GPU_LOCK=__import__("threading").Lock())
    import universal_face as uf

    replay = json.load(open(replay_path))
    enrollment = Path(replay_path).with_suffix(".enrollment.npz")
    arrays = {f"slot_{slot}": strips(replay["recording"], idx) for slot, idx in replay["setup"].items()}
    np.savez(enrollment, meta=np.array(json.dumps({"schema": "face-enrollment-v1"})), **arrays)

    class Tongue:
        enabled = True
        last = None

        def send(self, extension, horizontal, vertical, visible):
            Tongue.last = (extension, horizontal, vertical, visible)

        def close(self):
            pass

    captured = {}
    step = uf.FaceEvents.step

    def recording_step(self, *args, **kwargs):
        captured["out"] = step(self, *args, **kwargs)
        return captured["out"]

    uf.FaceEvents.step = recording_step
    face = uf.UniversalFace(model, enrollment, enabled=False, tongue=Tongue())
    raw = np.memmap(Path(replay["recording"]) / "frames.gray8", np.uint8, mode="r")
    for frame in replay["frames"]:
        i = frame["index"]
        strip = np.array(raw[i * FRAME:(i + 1) * FRAME]).reshape(400, 2000)
        native = frame["native"]
        sample = None if native is None else {"arrivalMonotonicNs": native["arrival"], "values": list(native["values"].values())}
        names = [] if native is None else list(native["values"].keys())
        face.update(strip, sample, names, frame["now"])
        out = captured["out"]
        raises = None
        if native is not None and abs(native["arrival"] - frame["now"]) <= 100_000_000:
            nv = native["values"]
            raises = []
            for base, (left, right) in uf.NATIVE_RAISE.items():
                amp = (nv[left] + nv[right]) / 2
                share = face.share[base]
                raises += [min(1., amp * 2 * share), min(1., amp * 2 * (1 - share))]
        extension, horizontal, vertical, visible = Tongue.last
        print(json.dumps({
            "cheeks": [float(out[n]) for n in uf.CHEEKS],
            "brows": [float(out[n]) for n in ("BrowLowererLeft", "BrowLowererRight", "BrowPinchLeft", "BrowPinchRight")],
            "raises": None if raises is None else [float(r) for r in raises], "tongue_visible": bool(visible),
            "tongue_extension": float(extension), "tongue_horizontal": float(horizontal), "tongue_vertical": float(vertical),
        }), flush=True)


def compare(a, b):
    worst, frames, mismatch = {}, 0, 0
    # QFT+ prints status lines too; only the JSON lines are frames.
    lines = lambda path: [line for line in open(path) if line.startswith("{")]
    for la, lb in zip(lines(a), lines(b)):
        x, y = json.loads(la), json.loads(lb)
        frames += 1
        if x["tongue_visible"] != y["tongue_visible"] or (x["raises"] is None) != (y["raises"] is None):
            mismatch += 1
        for key in ("cheeks", "brows", "raises", "tongue_extension", "tongue_horizontal", "tongue_vertical"):
            u, v = np.atleast_1d(np.asarray(x[key] if x[key] is not None else [], float)), np.atleast_1d(np.asarray(y[key] if y[key] is not None else [], float))
            if u.shape == v.shape and u.size:
                worst[key] = max(worst.get(key, 0.0), float(np.abs(u - v).max()))
    print(f"parity: {frames} frames, {mismatch} with a different tongue state or raises; largest differences:")
    for key, value in worst.items():
        print(f"  {key:18s} {value:.2e}")
    return mismatch == 0 and all(v < 1e-3 for v in worst.values())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("replay", nargs="?")
    parser.add_argument("--model", type=Path, default=MODEL)
    parser.add_argument("--make", type=Path, help="a five-camera recording with a face setup")
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--compare", nargs=2)
    args = parser.parse_args()
    if args.compare:
        sys.exit(0 if compare(*args.compare) else 1)
    if args.make:
        make(args.make, args.replay, args.seed)
        return
    run(args.replay, args.model)


if __name__ == "__main__":
    main()
