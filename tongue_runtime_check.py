"""Check the tongue Python runtime on its accelerator (setup-quest-pro-tongue.ps1).

Runs one training step and one inference step of the installed model pair on
the selected device, so a broken GPU stack fails during setup rather than
inside VRFT. Mirrors the GPU smoke tests of Qpro-Enhanced-FT's
Install-QproRocm.ps1 (MIT license).
"""

import argparse

import numpy as np
import torch

import train_vrft_tongue  # noqa: F401  applies the ROCm BatchNorm workaround as training does
from tongue_inference import describe_device, inputs, load_checkpoint


def synchronize(device: torch.device) -> None:
    if device.type == "cuda":
        torch.cuda.synchronize(device)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--accelerator", choices=("cpu", "cuda", "rocm"), required=True)
    parser.add_argument("--gate", required=True)
    parser.add_argument("--direction", required=True)
    args = parser.parse_args()
    print(f"PyTorch {torch.__version__}; CUDA {torch.version.cuda}; HIP {torch.version.hip}", flush=True)
    device = torch.device("cpu")
    if args.accelerator != "cpu":
        if args.accelerator == "cuda" and (not torch.version.cuda or torch.version.hip):
            raise SystemExit("This PyTorch build has no NVIDIA CUDA support")
        if args.accelerator == "rocm":
            if not torch.version.hip:
                raise SystemExit("This PyTorch build has no AMD ROCm (HIP) support")
            if torch.backends.cudnn.enabled:
                raise SystemExit("The ROCm MIOpen BatchNorm workaround is not active")
        if not torch.cuda.is_available():
            vendor = "AMD" if args.accelerator == "rocm" else "NVIDIA"
            raise SystemExit(f"PyTorch cannot access an {vendor} GPU; install or update the {vendor} "
                             "graphics driver, or rerun setup with -Accelerator cpu")
        device = torch.device("cuda:0")
        print(f"GPU: {torch.cuda.get_device_name(device)}", flush=True)

    model, size, _ = load_checkpoint(args.gate, device)
    model.train()
    batch = torch.rand(2, 2, size, size, device=device)
    model(batch).float().square().mean().backward()
    synchronize(device)
    print(f"Training step passed on {describe_device(device)}", flush=True)

    model.eval()
    direction, direction_size, _ = load_checkpoint(args.direction, device)
    frame = np.zeros((400, 800), dtype=np.uint8)
    with torch.inference_mode():
        visibility = model(inputs(frame, size, device))[0, 0]
        values = direction(inputs(frame, direction_size, device))[0]
    synchronize(device)
    if not (torch.isfinite(visibility) and torch.isfinite(values).all()):
        raise SystemExit("The tongue model returned non-finite values")
    print(f"Inference step passed on {describe_device(device)}", flush=True)


if __name__ == "__main__":
    main()
