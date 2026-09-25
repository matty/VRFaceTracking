"""Convert explicit VRFT guided captures into Qpro-compatible training arrays.

Keep one entire capture out of the input list for independent evaluation.
Raw inward-facing camera frames and converted caches remain local.
"""

import argparse
import json
from pathlib import Path

import cv2
import numpy as np


TARGET_NAMES = [
    "visibility", "extension", "horizontal", "vertical", "curl_up",
    "bend_down", "roll", "flat", "squish", "twist",
]
FRAME_BYTES = 800 * 400


def read_session(directory: Path):
    metadata = json.loads((directory / "metadata.json").read_text(encoding="utf-8"))
    if (metadata.get("format") != "vrft-tongue-capture-v1" or
            metadata.get("bytesPerFrame") != FRAME_BYTES or
            metadata.get("targets") != TARGET_NAMES):
        raise ValueError(f"Unsupported capture format: {directory}")
    with (directory / "samples.jsonl").open(encoding="utf-8") as labels:
        samples = [json.loads(line) for line in labels if line.strip()]
    raw = directory / "frames.gray8"
    if raw.stat().st_size != len(samples) * FRAME_BYTES:
        raise ValueError(f"Frame and label counts differ in {directory}")
    for index, sample in enumerate(samples):
        if sample.get("index") != index or len(sample.get("targets", [])) != len(TARGET_NAMES):
            raise ValueError(f"Invalid sample {index} in {directory}")
        target = np.asarray(sample["targets"], dtype=np.float32)
        native = sample.get("native_tongue_out")
        if not np.all(np.isfinite(target)) or np.any(np.abs(target) > 1):
            raise ValueError(f"Invalid targets for sample {index} in {directory}")
        if any(target[i] < 0 for i in (0, 1, 4, 5, 6, 7, 8)):
            raise ValueError(f"Negative unsigned target for sample {index} in {directory}")
        if native is not None and (not np.isfinite(native) or not 0 <= native <= 1):
            raise ValueError(f"Invalid native TongueOut for sample {index} in {directory}")
    return samples, raw


def usable_samples(directory: Path):
    """Keep original raw indices; skipped/rejected poses never become training data."""
    samples, raw = read_session(directory)
    excluded = set()
    for filename in ("excluded_steps.json", "review.json"):
        path = directory / filename
        if path.exists():
            excluded.update(json.loads(path.read_text(encoding="utf-8"))["excluded_steps"])
    return [sample for sample in samples if sample["step"] not in excluded
            and sample.get("native_tongue_out") is not None], raw


def main():
    parser = argparse.ArgumentParser(description="Prepare local VRFT tongue training captures")
    parser.add_argument("captures", nargs="+", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--size", type=int, default=224)
    args = parser.parse_args()
    if not 128 <= args.size <= 320:
        parser.error("--size must be between 128 and 320")
    sessions = [usable_samples(path.resolve()) for path in args.captures]
    count = sum(len(samples) for samples, _ in sessions)
    if count < 100:
        raise ValueError("At least 100 labeled frames are required")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    images = np.lib.format.open_memmap(
        output / "images.npy", mode="w+", dtype=np.uint8,
        shape=(count, 2, args.size, args.size),
    )
    targets = np.lib.format.open_memmap(
        output / "targets.npy", mode="w+", dtype=np.float32, shape=(count, len(TARGET_NAMES)),
    )
    native = np.lib.format.open_memmap(
        output / "native_tongue_out.npy", mode="w+", dtype=np.float32, shape=(count,),
    )
    step_ids = np.lib.format.open_memmap(
        output / "step_ids.npy", mode="w+", dtype=np.int32, shape=(count,),
    )
    trainable = np.lib.format.open_memmap(
        output / "trainable.npy", mode="w+", dtype=np.bool_, shape=(count,),
    )
    session_ids = np.lib.format.open_memmap(
        output / "session_ids.npy", mode="w+", dtype=np.int32, shape=(count,),
    )
    pose_ids = {}
    position = 0
    for session_id, (samples, raw) in enumerate(sessions):
        with raw.open("rb") as frames:
            for sample in samples:
                frames.seek(sample["index"] * FRAME_BYTES)
                payload = frames.read(FRAME_BYTES)
                if len(payload) != FRAME_BYTES:
                    raise ValueError(f"Truncated frame in {raw}")
                strip = np.frombuffer(payload, dtype=np.uint8).reshape(400, 800)
                for view in range(2):
                    images[position, view] = cv2.resize(
                        strip[:, view * 400:(view + 1) * 400],
                        (args.size, args.size), interpolation=cv2.INTER_AREA,
                    )
                targets[position] = np.asarray(sample["targets"], dtype=np.float32)
                native[position] = float(sample["native_tongue_out"])
                step_ids[position] = pose_ids.setdefault(sample["pose"], len(pose_ids))
                trainable[position] = True
                session_ids[position] = session_id
                position += 1
                if position % 500 == 0:
                    print(f"Prepared {position}/{count} frames", flush=True)
    for array in (images, targets, native, step_ids, trainable, session_ids):
        array.flush()
    (output / "metadata.json").write_text(json.dumps({
        "format": "vrft-tongue-cache-v1", "datasetType": "prompted-video",
        "imageSize": args.size, "targetNames": TARGET_NAMES, "frames": count,
        "captures": [str(path.resolve()) for path in args.captures],
        "poseNames": pose_ids,
    }, indent=2), encoding="utf-8")
    print(f"Ready: {count} frames in {output}")


if __name__ == "__main__":
    main()
