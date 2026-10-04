# Tongue model benchmark: VRFT against QFT+

QFT+ says its universal face model is faster and uses less memory. It publishes no benchmark, so this one times VRFT's models beside it, one frame at a time, as each app runs them.

## How to run it

```powershell
cargo build -p vrft-tongue --release --example benchmark
./tools/onnxruntime/fetch.ps1                # ONNX Runtime into .local/onnxruntime/
pip install onnxruntime numpy                # onnxruntime-directml for -gpu on Windows
python tools/benchmark/qftplus.py --fetch
python tools/benchmark/compare.py --pair models/quest-pro --face <universal-face-v1.safetensors> --recording <five-camera recording> [--gpu]
```

- **`examples/benchmark.rs`** runs a VRFT model through the daemon's own code: `TongueModel` for the stereo pair, `FaceModel` for the universal face model, including shrinking the views.
  - It uses ONNX Runtime when its library is found, as the daemon does, and times what a CPU session settles on after calibrating (int8 or float).
  - `VRFT_INFERENCE=burn` times Burn instead.
- **`tools/benchmark/qftplus.py`** runs QFT+'s `universal-face-v2` as QFT+'s `universal_face.py` does: the ONNX graph on the raw strip, then its mouth and brow heads in NumPy, with QFT+'s ONNX Runtime session options (one thread).
  - `--fetch` downloads QFT+'s v0.4.0-rc.25.2 package, checks its SHA-256, and keeps the model's two files in `.local/qftplus/`.
  - QFT+'s weights are trained on Ava-256 (CC BY-NC 4.0). They're only for comparison and never ship with VRFT.
- **`compare.py`** runs each model in its own process, on one thread and on every core, and prints a table.
- **Weights don't change timing:** the networks do the same arithmetic whatever their weights are, so an untrained checkpoint times the same as a trained one.

## Results: CPU

Taken on 2026-10-04 on a 4-core Intel Xeon at 2.1 GHz (cloud VM), Linux, with AVX-512 and VNNI. Each figure is the mean over 150 to 300 rendered five-camera frames after warm-up. Peak memory is the whole process's resident set (Linux `VmHWM`). QFT+'s includes Python and NumPy, 34 MB before its model loads.

### Universal face model, five views at 128 px

| Model | Runtime | 1 thread | 2 threads | 4 threads | Memory, running |
|---|---|---|---|---|---|
| QFT+ universal face v2 | ONNX Runtime, int8 (tongue tail float) | 13.7 ms | | 9.2 ms | 127 MB |
| VRFT universal face | ONNX Runtime, int8 (tongue tail float), the default | 12.8 ms | 9.3 ms | 8.0 ms | 104 MB |
| VRFT universal face | ONNX Runtime, all int8 (`VRFT_ONNX_INT8=all`) | 10.4 ms | 7.3 ms | 6.6 ms | 100 MB |
| VRFT universal face | Burn 0.21, float | 123 ms | | 76 ms | 78–91 MB |

### Stereo pair (gate at 224 px, direction at 192 px)

| Runtime | 1 thread | 2 threads | 4 threads | Memory, running |
|---|---|---|---|---|
| ONNX Runtime, float, the default | 64 ms | 36 ms | 28 ms | 171 MB |
| ONNX Runtime, int8 (`VRFT_ONNX_INT8=all`) | 24 ms | 15 ms | 13 ms | 102 MB |
| Burn 0.21, float | 268 ms | | 161 ms | 78–90 MB |

A model's first load on the CPU also calibrates, which holds up to 350 MB for a few seconds. Later loads start from the int8 graph saved beside the checkpoint.

### What changed, and why it's faster

- **ONNX Runtime instead of Burn's CPU backend.** Burn's convolutions run at a reasonable 10 GMAC/s on one thread, but each batch norm, SiLU and residual add is its own pass over the tensor, which costs as much again. ONNX Runtime fuses them.
- **Batch norm folded and int8.** VRFT writes each model's ONNX graph from its weights at load, with batch norm folded into the convolutions. On the CPU it quantizes the graph to int8, as ONNX Runtime's own static QDQ quantization does:
  - activations uint8, calibrated on the first 48 live frames;
  - weights int8 per channel.
  It's built in the background and swapped in, so tracking never stops. All 18 of the universal model's convolutions then run as ONNX Runtime's int8 kernels, the same as QFT+'s graph, and the graph alone times the same as ONNX Runtime quantizing it itself.
- **Faster view shrinking.** It gives the same bytes as before (OpenCV's `INTER_AREA`), but computes each source row's horizontal pass once: 0.5 ms for five views instead of 2.1 ms.
- **No Python.** QFT+ runs its heads in NumPy in a separate Python process; VRFT runs everything in one ONNX Runtime call inside the daemon.

### Accuracy of int8

Measured as the int8 model's outputs against the same model in float, on rendered frames it wasn't calibrated on.

- **Universal face model:** mean differences under 0.01 per output, at most 0.03.
- **The v8 stereo pair:**
  - Mean differences of 0.02 to 0.06 per output, up to 0.34.
  - 5% of tongue-out decisions near a 0.3 threshold flip.
  - Keeping some stages float didn't help: the error is spread through the encoder.
  - Percentile and entropy calibration were no better.

So the pair stays float by default. Like QFT+, the universal model keeps its tongue tail float; its brows and cheeks are int8.

## GPU

Both apps can use the GPU: VRFT through DirectML (`gpu` device setting) or wgpu (Burn), QFT+ through DirectML. VRFT's `auto` now means ONNX Runtime on the CPU, which leaves the GPU to the game. Run `compare.py --gpu` on a Windows PC for GPU numbers; this machine has none.
