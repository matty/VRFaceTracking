"""Precomputes GNM expression vectors from GNM's semantic sampler, offline,
so render_face.py can mix in named expressions without TensorFlow.

    git clone https://github.com/google/gnm.git     # outside this repo
    pip install -e gnm/gnm/shape                     # GNM's package, with TensorFlow
    python tools/face-synth/precompute_expressions.py --count 200

Needs Google's GNM package and TensorFlow (the sampler is a Keras model);
the renderer needs neither. Writes `.local/gnm/semantic_expressions.npz`:
one (count, 383) float32 array per expression class. Pass it to
render_face.py with `--expressions`. Labels still come from the geometry
these expressions make, never from the class names.
"""

import argparse
from pathlib import Path

import numpy as np

# The classes that move the mouth, cheeks, tongue and brows the way the
# models' outputs care about, plus a few for variety.
CLASSES = ["BLOW", "SUCK", "PUCKER", "FUNNELER", "MOUTH_LEFT", "MOUTH_RIGHT", "LIPS_ROLL_IN",
           "TONGUE_CENTER", "SURPRISE", "HAPPY", "SMILE_WIDE", "SQUINT", "CORNERS_DOWN", "DISGUST"]


def main():
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser()
    parser.add_argument("--count", type=int, default=200, help="expressions per class")
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--classes", default=",".join(CLASSES))
    parser.add_argument("--out", type=Path, default=repo / ".local" / "gnm" / "semantic_expressions.npz")
    args = parser.parse_args()

    from gnm.shape import semantic_sampler  # GNM's package, with TensorFlow

    sampler = semantic_sampler.ExpressionSampler()
    rng = np.random.default_rng(args.seed)
    arrays = {}
    for name in args.classes.split(","):
        label = semantic_sampler.Expression[name.strip().upper()]
        vectors = sampler.sample_expression(label, num_samples=args.count, rng=rng)
        arrays[label.name.lower()] = np.asarray(vectors, np.float32)
        print(f"precompute: {label.name.lower()} {arrays[label.name.lower()].shape}")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    np.savez_compressed(args.out, **arrays)
    print(f"precompute: wrote {args.out}")


if __name__ == "__main__":
    main()
