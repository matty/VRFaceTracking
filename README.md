# VRFT (VR Face Tracking)

A modular Rust-based system for bridging face tracking hardware with social VR platforms.

## Documentation

Detailed documentation can be found in the [`docs/`](docs/) directory:

- **[Architecture Overview](docs/architecture.md)**: High-level system design and project structure.
- **[Glossary](docs/glossary.md)**: Definitions of key terms and concepts.
- **[Creating a Module](docs/creating_a_module.md)**: Guide for developing hardware tracking plugins.
- **[VRChat Parameter Pipeline](docs/vrc_parameter_pipeline.md)**: How tracking data is translated for VRChat.
- **[Configuration and Debugging](docs/debug_and_config.md)**: Guide to `config.json` and the debug API.
- **[Eye Tracking Analysis](docs/eye_tracking_analysis.md)**: Technical deep-dive into eye tracking data formats.

## Project Structure

- `vrft_d/`: The core daemon and its associated crates (`api`, `common`, `app`).
- `scripts/`: Helper scripts for debugging and development.
- `experimental/`: Experimental modules.
- `docs/`: Project documentation.
