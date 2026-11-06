# Archived model files

Copies of the third-party models the QFTPlus Model download installs, so they stay available if an upstream release is ever removed. VRFaceTracking downloads from these folders only when you ask for the model and the upstream release can't be used.

They aren't part of VRFT's releases, and they aren't under the licence of any other file in this repository. Every file is byte-identical to its upstream release, with no changes made. Their SHA-256 hashes are pinned in `extensions/quest-pro/daemon/src/builtin.rs`, and a test checks these copies against them.

## `qftplus/`: non-commercial use only

- **Files:** `universal-face-v2.area.onnx` and `universal-face-v2.npz`
- **Source:** the package `QproFaceTracking.App-0.4.0-rc.25.2-full.nupkg` from [QFT+ v0.4.0-rc.25.2](https://github.com/Yeusepe/QFTPlus/releases/tag/v0.4.0-rc.25.2)
- **Creator:** QproFaceTracking contributors, [QFT+](https://github.com/Yeusepe/QFTPlus)
- **Licence:** [CC BY-NC 4.0](https://creativecommons.org/licenses/by-nc/4.0/). QFT+'s MIT licence covers its code and doesn't cover these weights.

QFT+'s notice says these files include a mouth-camera encoder QFT+ trained on the [Ava-256 dataset](https://github.com/facebookresearch/ava-256) by Meta Platforms. They are new weights learned from that dataset, and no Ava-256 images are included.

## `qpro-enhanced-ft/`

- **Files:** `qpro-stereo-tongue-v8-gate.pt` and `qpro-stereo-tongue-v8-direction.pt`
- **Source:** the package `QproFaceTracking-0.1.10-poc.zip` from [Qpro-Enhanced-FT v0.1.10](https://github.com/n0tmast3r/Qpro-Enhanced-FT/releases/tag/v0.1.10)
- **Creator:** n0tmast3r, [Qpro-Enhanced-FT](https://github.com/n0tmast3r/Qpro-Enhanced-FT)
- **Licence:** MIT, see [`third_party/Qpro-Enhanced-FT-LICENSE`](../Qpro-Enhanced-FT-LICENSE)

Upstream describes these checkpoints as demonstrators holding learned weights from consensually recorded developer data, with no camera frames included. They may not generalise to another person.
