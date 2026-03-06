# VRFT (VR Face Tracking)

vrft_d is a modular Rust daemon that reads face tracking data from hardware and sends it to social VR platforms via OSC. It supports Virtual Desktop (native) and any VRCFaceTracking-compatible module via a .NET runtime host.

**Platform: Windows only.**

---

## For End Users

Download from [GitHub Releases](https://github.com/dfgHiatus/VRCFT-VRFT/releases), place your tracking module in `plugins/`, configure `config.json`, and run `vrft_d.exe`.

**[Getting Started →](docs/guide/getting-started.md)**
**[Configuration Reference →](docs/guide/configuration.md)**

---

## For Avatar Creators

vrft_d sends unified expression parameters with the `v2/` prefix. Each expression emits a float, bool, and optional binary sub-params. Legacy SRanipal parameter names are also sent for backwards compatibility.

**[V2 Parameter Reference →](docs/avatars/v2-parameters.md)**

---

## For Developers

vrft_d is a Cargo workspace. The core executable is `vrft_d/app/`. Tracking modules are `cdylib` crates in `modules/` implementing the `TrackingModule` trait.

**[Architecture Overview →](docs/internals/architecture.md)**
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
