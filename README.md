# VRFT (VR Face Tracking)

vrft_d is a modular Rust daemon that reads face tracking data from hardware and sends it to social VR platforms via OSC. It supports Virtual Desktop (native) and any VRCFaceTracking-compatible module via a .NET runtime host.

With the companion Quest Pro camera APK, VRFT can discover a live stereo feed and add enhanced tongue expressions to its VRChat output. The APK can also stream each eye's own gaze, so avatar eyes converge on close objects. VRFT automatically uses the active tracking module's tongue and gaze values if the headset stream becomes stale. See [Quest Pro enhanced tongue tracking and independent eye gaze](docs/guide/getting-started.md#quest-pro-enhanced-tongue-tracking-and-independent-eye-gaze) for setup.

**Platform: Windows only.**

---

## For End Users

Download the latest release from [GitHub Releases](https://github.com/matty/VRFaceTracking/releases/latest), place your tracking module in `plugins/`, configure `config.json`, and run `vrft_gui.exe`, which starts `vrft_d.exe` beside it. The Quest Pro headset app has its own, less frequent releases, tagged `apk-v…`. Each VRFT release links the headset app release to use, and VRFT tells you if either needs updating.

**[Getting Started →](docs/guide/getting-started.md)**
**[Configuration Reference →](docs/guide/configuration.md)**

---

## For Avatar Creators

vrft_d sends unified expression parameters with the `v2/` prefix. Each expression emits a float, bool, and optional binary sub-params. Legacy SRanipal parameter names are also sent for backwards compatibility.

**[V2 Parameter Reference →](docs/avatars/v2-parameters.md)**

---

## For Developers

vrft_d is a Cargo workspace. Library and binary crates live under `crates/` (the core executable is `crates/daemon/`, package `vrft-daemon`, binary `vrft_d`). Tracking modules are `cdylib` crates in `modules/` implementing the `TrackingModule` trait.

**[Architecture Overview →](docs/internals/architecture.md)**
**[Versions and Releases →](docs/internals/releasing.md)**
**[Creating a Module →](docs/internals/creating-a-module.md)**
**[Mutation Pipeline →](docs/internals/mutation-pipeline.md)**
**[VRChat Parameter Pipeline →](docs/internals/vrc-parameter-pipeline.md)**

---

## Hardware Support

| Hardware | Module | Runtime |
|----------|--------|---------|
| Virtual Desktop (face tracking) | `vd_module.dll` | Native |
| VRCFaceTracking modules | any VRCFT `.dll` | .NET |

---

## Quick Start

```powershell
# Build and stage to run/
./run_debug.ps1

# Or just build
cargo build
```

**[Glossary →](docs/glossary.md)**
