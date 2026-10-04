"""Converts GNM's semantic expression sampler to NumPy weights, so
render_face.py can sample named expressions (BLOW, SUCK, PUCKER,
MOUTH_LEFT, TONGUE_CENTER and the rest) inside Blender, with no TensorFlow.

    pip install h5py numpy
    python tools/face-synth/semantic_decoder.py

Downloads the sampler's decoder (`expression_decoder_model.h5`, 1.5 MB,
Apache-2.0) from GNM's repository at a pinned commit, checks its SHA-256,
and writes its Dense layers to `.local/gnm/semantic_decoder.npz`.
render_face.py runs this itself when that file is missing and a Python with
h5py is on PATH (or in VRFT_PYTHON).
"""

import argparse
import hashlib
import json
import os
import sys
import urllib.request
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from gnm_head import SEMANTIC_CLASSES  # noqa: E402

COMMIT = "940c36b850d951f14203e751463bc9b422900fad"
URL = (f"https://raw.githubusercontent.com/google/gnm/{COMMIT}/"
       "gnm/shape/data/semantic_sampler/expression_decoder_model.h5")
SHA256 = "5eba165f8a414f73b24be96963d0a17e708c0856739ed85a19031f318dfb51e6"


def download(url=URL):
    with urllib.request.urlopen(url) as response:
        data = response.read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != SHA256:
        raise SystemExit(f"semantic: {url} has SHA-256 {digest}, expected {SHA256}")
    return data


def dense_layers(h5):
    """The model's Dense layers, in order: (kernel, bias, activation), after
    checking the inputs are the latent then the class one-hot, concatenated."""
    config = json.loads(h5.attrs["model_config"])["config"]
    layers = config["layers"]
    inputs = [layer["config"]["name"] for layer in layers if layer["class_name"] == "InputLayer"]
    if inputs != ["latent_input", "decoder_label_input"]:
        raise SystemExit(f"semantic: unexpected decoder inputs {inputs}")
    out = []
    for layer in layers:
        if layer["class_name"] == "Dense":
            name = layer["config"]["name"]
            group = h5["model_weights"][name][name]
            out.append((group["kernel:0"][()], group["bias:0"][()], layer["config"]["activation"]))
        elif layer["class_name"] not in ("InputLayer", "Concatenate"):
            raise SystemExit(f"semantic: unexpected layer {layer['class_name']}")
    activations = [activation for *_, activation in out]
    if activations != ["relu"] * (len(out) - 1) + ["linear"]:
        raise SystemExit(f"semantic: unexpected activations {activations}")
    if out[0][0].shape[0] != 64 + len(SEMANTIC_CLASSES) or out[-1][0].shape[1] != 383:
        raise SystemExit(f"semantic: unexpected shapes {out[0][0].shape} -> {out[-1][0].shape}")
    return out


def convert(source, out):
    import io

    import h5py

    with h5py.File(io.BytesIO(source), "r") as h5:
        layers = dense_layers(h5)
    arrays = {"layers": np.int32(len(layers)), "classes": np.array(SEMANTIC_CLASSES),
              "source": np.array(URL), "sha256": np.array(SHA256)}
    for i, (kernel, bias, _) in enumerate(layers):
        arrays[f"kernel_{i}"] = kernel.astype(np.float32)
        arrays[f"bias_{i}"] = bias.astype(np.float32)
    out.parent.mkdir(parents=True, exist_ok=True)
    partial = out.with_name(out.name + ".part")
    with open(partial, "wb") as file:
        np.savez(file, **arrays)
    os.replace(partial, out)


def main():
    repo = HERE.parents[1]
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, default=repo / ".local" / "gnm" / "semantic_decoder.npz")
    parser.add_argument("--h5", type=Path, help="a local copy of expression_decoder_model.h5, instead of downloading")
    args = parser.parse_args()
    if args.h5:
        source = args.h5.read_bytes()
        if hashlib.sha256(source).hexdigest() != SHA256:
            raise SystemExit(f"semantic: {args.h5} isn't GNM's decoder at {COMMIT[:12]}")
    else:
        print(f"semantic: downloading {URL}", flush=True)
        source = download()
    convert(source, args.out)
    print(f"semantic: wrote {args.out}", flush=True)


if __name__ == "__main__":
    main()
