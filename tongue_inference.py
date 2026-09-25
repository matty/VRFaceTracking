"""VRFT's latest-frame stereo tongue inference worker.

Reads little-endian u64 sequence + 800x400 gray8 frame from stdin. Writes one
JSON model handshake, then little-endian u64 sequence + ten f32 model outputs.
Outputs are raw per-frame ensemble values (gate visibility + direction heads);
the daemon applies time-based smoothing. Heads a personal checkpoint marks as
disabled are always 0.0.
The model architecture and preprocessing follow Qpro-Enhanced-FT (MIT license).
"""

import argparse
import json
import os
import struct
import sys

import cv2
import numpy as np
import torch

if torch.version.hip:
    # Windows MIOpen HIPRTC cannot compile these tongue-model BatchNorm kernels.
    torch.backends.cudnn.enabled = False

from qpro_model import create_model


TARGETS = [
    "visibility", "extension", "horizontal", "vertical", "curl_up",
    "bend_down", "roll", "flat", "squish", "twist",
]
INPUT_SIZE = 8 + 800 * 400
OUTPUT = struct.Struct("<Q10f")


def read_exact(size: int) -> bytes | None:
    chunks = []
    remaining = size
    while remaining:
        part = sys.stdin.buffer.read(remaining)
        if not part:
            if remaining == size:
                return None
            raise EOFError("truncated camera frame")
        chunks.append(part)
        remaining -= len(part)
    return b"".join(chunks)


def describe_device(device: torch.device) -> str:
    """ROCm PyTorch reuses the CUDA device API; say which backend is in use."""
    if device.type == "cuda" and torch.version.hip:
        return f"{device} (ROCm)"
    return str(device)


def synchronize_rocm(device: torch.device) -> None:
    """Windows ROCm can keep a finished Python process alive unless
    outstanding GPU work is synchronized before interpreter shutdown."""
    if device.type == "cuda" and torch.version.hip:
        try:
            torch.cuda.synchronize(device)
        except Exception as error:  # never mask the error being reported
            print(f"ROCm synchronize failed: {error}", file=sys.stderr, flush=True)


def load_checkpoint(path: str, device: torch.device):
    checkpoint = torch.load(path, map_location="cpu", weights_only=True)
    if list(checkpoint["targetNames"]) != TARGETS:
        raise ValueError(f"Unsupported model target schema in {path}")
    model = create_model(checkpoint["architecture"], TARGETS)
    model.load_state_dict(checkpoint["modelState"])
    model.to(device).eval()
    return model, int(checkpoint["imageSize"]), checkpoint


def disabled_targets(checkpoint) -> list[str]:
    """Heads personal training could not train (masked in its loss).

    ``disabledTargets`` is written by train_vrft_tongue.py. Earlier personal
    checkpoints only carry the ``supportedTargets`` mask; starting checkpoints
    carry neither and keep every head.
    """
    names = checkpoint.get("disabledTargets")
    if names is not None:
        names = [str(name) for name in names]
        unknown = sorted(set(names) - set(TARGETS))
        if unknown:
            raise ValueError(f"Unknown disabled tongue targets: {', '.join(unknown)}")
    else:
        supported = checkpoint.get("supportedTargets")
        if supported is None:
            return []
        supported = [bool(value) for value in supported]
        if len(supported) != len(TARGETS):
            raise ValueError("Invalid supported target mask")
        names = [name for name, yes in zip(TARGETS, supported) if not yes]
    if "visibility" in names:
        raise ValueError("A tongue checkpoint cannot disable visibility")
    return [name for name in TARGETS if name in names]


def inputs(strip: np.ndarray, size: int, device: torch.device) -> torch.Tensor:
    cameras = np.empty((1, 2, size, size), dtype=np.float32)
    for view in range(2):
        panel = strip[:, view * 400:(view + 1) * 400]
        cameras[0, view] = cv2.resize(
            panel, (size, size), interpolation=cv2.INTER_AREA
        ).astype(np.float32) / 255.0
    return torch.from_numpy(cameras).to(device)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gate", required=True)
    parser.add_argument("--direction", required=True)
    args = parser.parse_args()
    torch.set_num_threads(min(4, os.cpu_count() or 1))
    preference = os.environ.get("VRFT_TONGUE_DEVICE", "auto")
    device = torch.device(
        "cuda:0" if preference == "auto" and torch.cuda.is_available()
        else "cpu" if preference == "auto" else preference
    )
    gate, gate_size, gate_checkpoint = load_checkpoint(args.gate, device)
    direction, direction_size, direction_checkpoint = load_checkpoint(args.direction, device)
    # Visibility comes from the gate; every other head from the direction model.
    disabled_targets(gate_checkpoint)
    disabled = disabled_targets(direction_checkpoint)
    disabled_mask = np.array([name in disabled for name in TARGETS], dtype=bool)
    gate_config = gate_checkpoint.get("visibilityGate", {})
    metadata = {
        "version": 1,
        "targets": TARGETS,
        "camera_weight": float(gate_config.get("cameraWeight", 0.95)),
        "threshold": float(gate_config.get("threshold", 0.85)),
        "device": describe_device(device),
        "gate_size": gate_size,
        "direction_size": direction_size,
        "smoothing": "none",
        "disabled_targets": disabled,
    }
    print(json.dumps(metadata), flush=True)
    try:
        while True:
            packet = read_exact(INPUT_SIZE)
            if packet is None:
                return
            sequence = struct.unpack_from("<Q", packet)[0]
            strip = np.frombuffer(packet, dtype=np.uint8, offset=8).reshape(400, 800)
            with torch.inference_mode():
                gate_values = gate(inputs(strip, gate_size, device))[0].float().cpu().numpy()
                values = direction(inputs(strip, direction_size, device))[0].float().cpu().numpy()
            values[0] = gate_values[0]
            values[disabled_mask] = 0.0
            if not np.all(np.isfinite(values)):
                raise ValueError("Model returned non-finite tongue values")
            sys.stdout.buffer.write(OUTPUT.pack(sequence, *map(float, values)))
            sys.stdout.buffer.flush()
    finally:
        synchronize_rocm(device)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"Tongue inference failed: {error}", file=sys.stderr, flush=True)
        raise
