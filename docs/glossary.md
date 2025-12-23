# Glossary

This document defines key terms and concepts used throughout the VRFT project.

## Core Concepts

### Unified Tracking Data

The standardized internal data format used by the daemon to represent facial expressions, eye tracking, and head pose. It acts as a bridge between hardware-specific data and platform-specific outputs.

### Tracking Module (Plugin)

A dynamically loaded library (`.dll`) that interfaces with specific tracking hardware (e.g., Vive Pro Eye, Virtual Desktop) and converts its raw data into the Unified Tracking Data format.

### Parameter Solver

The component responsible for translating raw unified tracking weights into specific avatar parameters (e.g., VRChat OSC addresses).

### OSC (Open Sound Control)

The network protocol used to communicate tracking data to social VR applications like VRChat and Resonite.

### OSC Query

A protocol used to discover the available parameters and configuration of a running OSC-compatible application (like a VRChat avatar).

## Technical Terms

### Blendshape / Shape Key

A single facial movement or expression (e.g., `JawOpen`, `MouthSmileLeft`) represented as a weight between 0.0 and 1.0.

### V1 / Legacy Parameters

The original set of facial parameters used by early versions.

### V2 Parameters

The modern, expanded set of facial parameters providing higher fidelity and more granular control over facial expressions.

### Euro Filter

A specific signal filtering algorithm used to reduce jitter in tracking data while maintaining low latency for fast movements.

### Mutator

A logic component that modifies tracking data in real-time (e.g., mirroring expressions, applying offsets, or procedural animations).

### Calibration

The process of mapping a user's specific facial range to the normalized 0.0 - 1.0 scale used by the tracking system.
