# Tongue model benchmark: VRFT against QFT+

QFT+ says its universal face model is faster and uses less memory. It publishes no benchmark, so this one times VRFT's models beside it, one frame at a time, as each app runs them.

## How to run it

```powershell
cargo build -p vrft-tongue --release --example benchmark
pip install onnxruntime numpy          # onnxruntime-directml for -gpu on Windows
python tools/benchmark/qftplus.py --fetch
python tools/benchmark/compare.py --pair models/quest-pro --face <universal-face-v1.safetensors> --recording <five-camera recording> [--gpu]
```

- **`examples/benchmark.rs`** runs a VRFT model through the daemon's own code: `TongueModel` for the stereo pair, `FaceModel` for the universal face model. That includes shrinking the views.
- **`tools/benchmark/qftplus.py`** runs QFT+'s `universal-face-v2` as QFT+'s `universal_face.py` does: the ONNX graph on the raw strip, then its mouth and brow heads in NumPy. It uses QFT+'s ONNX Runtime session options, which pin one thread.
  - `--fetch` downloads QFT+'s v0.4.0-rc.25.2 package, checks its SHA-256, and keeps the model's two files in `.local/qftplus/`.
  - QFT+'s weights are trained on Ava-256 (CC BY-NC 4.0). They're only for comparison and never ship with VRFT.
- **`compare.py`** runs each model in its own process, on one thread and on every core, and prints a table.
- **Weights don't change timing:** the networks do the same arithmetic whatever their weights are, so an untrained checkpoint times the same as a trained one.

## Results: CPU

Taken on 2026-10-04 on a 4-core Intel Xeon at 2.1 GHz (cloud VM), Linux, with AVX-512 and VNNI. Each figure is the mean of 100 frames after 30 warm-up frames, on rendered five-camera frames.

| Model | Runtime | Threads | Mean ms | p95 ms | Frames/s | Model files | Peak memory |
|---|---|---|---|---|---|---|---|
| VRFT stereo pair (gate + direction, 224/192 px) | Burn 0.21, fp32 | 1 | 268 | 305 | 3.7 | 30.2 MB | 78 MB |
| VRFT universal face (5 × 128 px) | Burn 0.21, fp32 | 1 | 123 | 142 | 8.1 | 19.7 MB | 78 MB |
| QFT+ universal face v2 (5 × 128 px) | ONNX Runtime 1.30, int8 | 1 | 13.2 | 17.3 | 76 | 13.7 MB | 127 MB |
| VRFT stereo pair | Burn 0.21, fp32 | 4 | 161 | 183 | 6.2 | 30.2 MB | 90 MB |
| VRFT universal face | Burn 0.21, fp32 | 4 | 76 | 87 | 13 | 19.7 MB | 91 MB |
| QFT+ universal face v2 | ONNX Runtime 1.30, int8 | 4 | 10.4 | 13.9 | 97 | 13.7 MB | 127 MB |

Peak memory is the whole process's resident set (Linux `VmHWM`).
- VRFT's figure is the benchmark binary, which is what the model adds to the daemon.
- QFT+'s includes the Python interpreter and NumPy, 34 MB before the model loads. That's what QFT+ runs too, as a separate process beside its app.

### What this says

- **Memory: QFT+'s claim doesn't hold against VRFT.** VRFT's models peak at 78–91 MB of process memory, and QFT+'s at 127 MB. Even without its 34 MB Python baseline, QFT+'s model adds about 92 MB, against VRFT's 73–86 MB. Its model files are smaller, 13.7 MB against 19.7 MB, because they're int8.
- **Speed on the CPU: QFT+'s claim holds, by a wide margin.** On one thread its model is 9 times faster than VRFT's universal face model, and 20 times faster than the stereo pair VRFT runs today. That gap has three parts:

  | Part | Factor | How it was measured |
  | --- | --- | --- |
  | ONNX Runtime against Burn's CPU backend | about 2.5× | QFT+'s graph with ONNX Runtime's optimisations off runs its int8 steps as fp32 and takes 48 ms, against VRFT's 123 ms for the same 18 convolutions |
  | int8 | about 4× | with optimisations on, ONNX Runtime fuses the int8 steps into int8 kernels: 48 ms falls to 12 ms. This CPU has AVX-512 VNNI, which int8 kernels use; CPUs without VNNI gain less |
  | the stereo pair's larger views | about 2× | two networks at 224 and 192 px, against one at 128 px |

- **Why Burn is slower.** Burn's CPU backend (Flex) runs a 3×3 convolution at about 10 GMAC/s on one thread, which is reasonable. Then it runs each batch norm, SiLU and residual add as its own pass over the tensor, each with a new allocation. Those passes take about as long as the convolutions. ONNX Runtime folds batch norm into the convolution weights and fuses the activations.
- **This compares the CPU paths only.** Both apps use the GPU first: VRFT through wgpu (DX12 or Vulkan), QFT+ through DirectML. Run `compare.py --gpu` on a Windows PC for those numbers. On the CPU, only QFT+'s model keeps up with the cameras' 24 fps.

## Ways to close the gap

From cheapest to most involved. None of these is done yet.

1. **Fold batch norm into the convolutions** when a model loads for inference. Every stage keeps its weights; nothing about training or the files changes. This removes one per-pixel pass per layer, maybe 1.3–1.5× on the CPU.
2. **Fuse the activation and residual add**, or run inference through Burn's fusion backend on the CPU if Burn supports it. This removes most of the remaining per-pixel passes.
3. **Run inference on ONNX Runtime**, through the `ort` crate.
   - It needs an ONNX export of VRFT's models, written from the safetensors weights, and ONNX Runtime's library shipped beside the daemon (about 15 MB).
   - It gives ONNX Runtime's fp32 speed (about 2.5×), and DirectML on the GPU.
4. **int8**, with ONNX Runtime's static quantization as QFT+ does, calibrated on recordings. That's another few times on CPUs with VNNI, but it needs an accuracy check on real recordings, as QFT+ kept its tongue path in fp32.
