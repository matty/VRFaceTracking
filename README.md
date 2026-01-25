# VRFT (VR Face Tracking)

A modular Rust-based system for bridging face tracking hardware with social VR platforms.

## Quick Start

1. Place tracking modules in `plugins/native/` (native) or `plugins/dotnet/modules/` (.NET).
2. Configure `config.json` with your preferred module and settings.
3. Run `vrft_d.exe`.

## Documentation

Detailed documentation in the [`docs/`](docs/) directory:

- **[Architecture Overview](docs/architecture.md)**: System design, crates, and data flow.
- **[Mutation Pipeline](docs/mutation_pipeline.md)**: How tracking data is post-processed.
- **[Glossary](docs/glossary.md)**: Key terms and concepts.
- **[Creating a Module](docs/creating_a_module.md)**: Guide for developing hardware plugins.
- **[VRChat Parameter Pipeline](docs/vrc_parameter_pipeline.md)**: Tracking data translation for VRChat.
- **[Configuration and Debugging](docs/debug_and_config.md)**: Guide to `config.json` and the debug API.
- **[Eye Tracking Analysis](docs/eye_tracking_analysis.md)**: Technical deep-dive into eye data formats.

## Project Structure

```
vrft_d/
├── api/        # Core data structures and traits
├── common/     # Shared logic: mutations, calibration, filters
│   └── src/
│       ├── mutations/       # Pluggable mutation implementations
│       │   ├── smoothing.rs
│       │   ├── calibration.rs
│       │   └── normalization.rs
│       └── mutation_trait.rs # The Mutation trait interface
├── app/        # Main executable
└── dotnet/     # .NET runtime host
```
