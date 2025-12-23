# Architecture Overview

The VRFT Daemon (`vrft_d`) is a modular Rust-based system designed to bridge various face tracking hardware with social VR platforms like VRChat and Resonite.

## Project Structure

The project is organized into several crates within the `vrft_d` directory:

- **`api/`**: Defines the core data structures and traits used across the project. This includes the unified tracking data format.
- **`common/`**: Contains shared logic such as calibration management, filters (e.g., Euro Filter), and data mutators.
- **`app/`**: The main executable. It handles:
  - Plugin loading and management.
  - OSC communication (sending and receiving).
  - Dispatching tracking data to various strategies.
  - SteamVR integration via OpenVR manifests.
- **`plugins/`**: A directory for compiled dynamic library modules (`.dll`) that interface with specific hardware (e.g., Virtual Desktop, Vive SRanipal).

## Data Flow

1.  **Hardware Module**: A plugin (DLL) captures raw data from tracking hardware and converts it into the unified API format.
2.  **Daemon Core**: The daemon receives this data, applies any active mutators or filters, and performs calibration if enabled.
3.  **Output Strategies**: The processed data is then dispatched via OSC to target applications (VRChat, Resonite, etc.) based on the configured transport.

## SteamVR Integration

The daemon includes a `vrft_d.vrmanifest` which allows it to be registered as a SteamVR application, enabling automatic startup alongside SteamVR.
