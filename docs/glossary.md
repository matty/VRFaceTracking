# Glossary

This document defines key terms and concepts used throughout the VRFT project.

## Core Concepts

### Unified Tracking Data

The standardized internal data format used by the daemon to represent facial expressions, eye tracking, and head pose. It acts as a bridge between hardware-specific data and platform-specific outputs.

### Tracking Module (Plugin)

A dynamically loaded library (`.dll`) that interfaces with specific tracking hardware (e.g., Vive Pro Eye, Virtual Desktop) and converts its raw data into the Unified Tracking Data format.

### Parameter Registry

The component (`ParameterRegistry`) that holds all supported output parameters and their compute functions. It generates OSC messages from `UnifiedTrackingData`, filtering to only parameters relevant to the current avatar.

### EParam

A container parameter that emits three sub-parameters for a single expression: a float (0–1 or -1–1), a bool (threshold-based), and optionally binary integer encoding (split across `Name1`, `Name2`, `Name4`... bool parameters). Most v2 parameters are EParms.

### OSC (Open Sound Control)

The network protocol used to communicate tracking data to social VR applications like VRChat and Resonite.

### OSC Query

A protocol used to discover the available parameters and configuration of a running OSC-compatible application (like a VRChat avatar).

## Technical Terms

### Blendshape / Shape Key

A single facial movement or expression (e.g., `JawOpen`, `MouthSmileLeft`) represented as a weight between 0.0 and 1.0.

### V1 / Legacy Parameters

SRanipal-era parameter names (e.g., `EyeLeftX`, `JawOpen`, `MouthSmile_L`) supported for backwards compatibility with existing avatars. Not recommended for new avatars.

### V2 Parameters

The unified expression parameter set using the `v2/` OSC prefix (e.g., `v2/JawOpen`, `v2/EyeLeftX`). Recommended for all new avatars. See [docs/avatars/v2-parameters.md](avatars/v2-parameters.md) for the full list.

### SRanipal

HTC's legacy lip and eye tracking SDK. vrft_d translates unified expressions into SRanipal-compatible shapes (legacy parameters) for backwards compatibility with avatars built against that format.

### OSCmooth

A VRChat community tool for smoothing OSC parameters client-side. vrft_d's binary parameter discovery supports OSCmooth's address naming convention — parameters like `v2/JawOpen1`, `v2/JawOpen2`, `v2/JawOpen4` are automatically matched to the `v2/JawOpen` EParam.

### Euro Filter

A specific signal filtering algorithm used to reduce jitter in tracking data while maintaining low latency for fast movements.

### Mutator

A logic component that modifies tracking data in real-time (e.g., mirroring expressions, applying offsets, or procedural animations).

