#!/bin/bash
# On the GPU server: installs Rust if needed and builds the training tools
# for CUDA. Needs an NVIDIA driver and CUDA 12.8 or later (a CUDA "devel"
# image has NVRTC, which compiles the GPU kernels at run time).
set -euo pipefail
ROOT=/workspace/vrft
if ! command -v cargo >/dev/null && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
    if command -v apt-get >/dev/null; then
        apt-get update -qq && apt-get install -y -qq build-essential pkg-config curl >/dev/null
    fi
    curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
export PATH="$HOME/.cargo/bin:$PATH"
nvidia-smi -L
cd "$ROOT/src"
cargo build --release -p vrft-tongue --features cuda --example train --example evaluate --example evaluate_face --example probe
echo "built"
