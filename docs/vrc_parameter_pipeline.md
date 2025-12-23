# VRChat Parameter Pipeline

This document outlines how the daemon calculates facial expressions and determines which parameters to send to VRChat.

## High-Level Overview

The system follows a **"Calculate All, Filter Later"** approach. This ensures robustness and instant compatibility when switching avatars, with negligible CPU overhead.

1.  **Input**: `UnifiedTrackingData` from hardware modules.
2.  **Calculation**: `ParameterSolver` generates a master list of all supported parameters.
3.  **Filtering**: `VRChatOscStrategy` filters the master list against the current avatar's allowed parameters.
4.  **Output**: Serialized OSC packets sent to VRChat.

## 1. Master List Creation (`ParameterSolver`)

**Location**: `vrft_d/app/src/parameter_solver.rs`

The `ParameterSolver::solve` function is the core engine. It calculates every possible parameter supported by the application, regardless of the current avatar's configuration.

- **Modern V2 Expressions**: Iterates through all standard `v2` expressions (e.g., `v2/JawOpen`).
- **Combined Parameters**: Calculates derived parameters (e.g., `v2/BrowUp` as an average of inner and outer brow shapes).
- **Legacy Support**: Calls `shape_legacy` to calculate all V1 parameters.

## 2. Math Helpers (`shape_legacy`)

**Location**: `vrft_d/app/src/shape_legacy.rs`

This module acts as a pure math library for the solver. It translates modern tracking data into complex legacy shapes. It does not perform any filtering; it only handles the mathematical translation of weights.

## 3. Filtering and Dispatch (`VRChatOscStrategy`)

**Location**: `vrft_d/app/src/strategies/vrchat.rs`

Optimization occurs at the dispatch stage. The daemon maintains a set of parameters the current avatar actually listens for, discovered via the **OSC Query** protocol.

1.  **Discovery**: The application learns the avatar's parameters via OSC Query and updates the `allowed_parameters` set.
2.  **Filtering**: During the update loop, the strategy checks every calculated parameter against this set.
3.  **Serialization**: Only allowed parameters are serialized into OSC messages, ensuring efficient network bandwidth usage.
