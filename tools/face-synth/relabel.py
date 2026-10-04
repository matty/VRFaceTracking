"""Grades a rendered (or packed) face-synth set's labels again with other
label scales, from the measurements each sample keeps, without rendering
it again.

    python tools/face-synth/relabel.py <recording>... --label-scales scales.json
    python tools/face-synth/relabel.py <recording>... --defaults

`scales.json` overrides any of gnm_head.SCALES, such as
`{"suck_full_mm": 5.0, "brow_raise_full_mm": 3.5}`. `--defaults` grades with
gnm_head.SCALES as they are now. Rewrites each recording's samples.jsonl
(tongue `targets` and `face`) and records the scales in metadata.json.
Samples from before measurements were kept are left alone, and counted.
"""

import argparse
import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import gnm_head as gh  # noqa: E402

# The tongue model's targets that face-synth labels, by column, and the face
# labels; as render_face.py writes them.
TONGUE_COLUMNS = {"visibility": 0, "extension": 1, "horizontal": 2, "vertical": 3,
                  "cheek_puff_left": 10, "cheek_puff_right": 11}


def relabel(directory, scale):
    samples = directory / "samples.jsonl"
    lines = samples.read_text().splitlines()
    changed = skipped = 0
    out = []
    for text in lines:
        if not text.strip():
            continue
        sample = json.loads(text)
        measured = (sample.get("synthetic") or {}).get("measured")
        if measured is None:
            skipped += 1
            out.append(text)
            continue
        values = gh.grade(measured, scale)
        targets = list(sample["targets"])
        for name, column in TONGUE_COLUMNS.items():
            targets[column] = round(values[name], 5)
        face = {name: round(values[name], 5) for name in sample.get("face", {})}
        if targets != sample["targets"] or face != sample.get("face"):
            changed += 1
        sample["targets"], sample["face"] = targets, face
        sample["dot"] = [targets[2], targets[3]]
        out.append(json.dumps(sample))
    partial = samples.with_name(samples.name + ".part")
    partial.write_text("\n".join(out) + "\n")
    os.replace(partial, samples)
    metadata_path = directory / "metadata.json"
    metadata = json.loads(metadata_path.read_text())
    metadata.setdefault("synthetic", {})["label_scales"] = scale
    metadata_path.write_text(json.dumps(metadata, indent=2))
    return len(out), changed, skipped


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("recordings", nargs="+", type=Path)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--label-scales", type=Path, help="a JSON file overriding gnm_head.SCALES")
    group.add_argument("--defaults", action="store_true", help="grade with gnm_head.SCALES")
    args = parser.parse_args()
    scale = gh.scales(None if args.defaults else args.label_scales)
    for directory in args.recordings:
        total, changed, skipped = relabel(directory, scale)
        note = f", {skipped} without measurements left alone" if skipped else ""
        print(f"relabel: {directory}: {changed} of {total} samples changed{note}")


if __name__ == "__main__":
    main()
