# VRChat Parameter Pipeline

This document outlines how the daemon calculates facial expressions and determines which parameters to send to VRChat.

## High-Level Overview

The system follows a **"Calculate All, Filter Later"** approach. This ensures robustness and instant compatibility when switching avatars, with negligible CPU overhead.

1.  **Input**: `UnifiedTrackingData` from hardware modules.
2.  **Calculation**: `ParameterRegistry` holds all supported parameters and their compute functions.
3.  **Filtering**: `VRChatOsc` discovers the current avatar's parameters via OSC Query and calls `registry.reset()` to mark which are relevant.
4.  **Output**: Only relevant parameters are evaluated and serialized into OSC packets.

## 1. Parameter Registry

**Location**: `vrft_d/app/src/osc/parameters/registry.rs`

`ParameterRegistry::new()` builds the full set of parameters the system can produce. Each parameter is a boxed `Parameter` trait object with a compute closure.

- **V2 Expressions**: Modern parameters like `v2/JawOpen`, `v2/EyeLeftX`, etc. Each registers as an EParam (bool + float + optional binary sub-params).
- **Combined Parameters**: Derived parameters (e.g., `v2/EyeX` as the average of left and right gaze).
- **Native Parameters**: Registered via `create_native_parameters()` — status booleans like `EyeTrackingActive`.
- **Legacy Eye Parameters**: Registered via `create_legacy_eye_parameters()` in `osc/parameters/legacy_eye.rs` — translates unified tracking data into V1/SRanipal-compatible eye shapes.
- **Legacy Lip Parameters**: Registered via `create_legacy_lip_parameters()` in `osc/parameters/legacy_lip.rs` — translates unified tracking data into SRanipal-compatible lip shapes.

## 2. Filtering and Dispatch (`VRChatOsc`)

**Location**: `vrft_d/app/src/osc/vrchat.rs`

Optimization occurs at the dispatch stage:

1.  **Discovery**: `OscQueryService` discovers the current avatar's parameters and their types (Bool, Float, Int).
2.  **Reset**: On avatar change, `VRChatOsc::send()` receives the parameter list and calls `registry.reset()`, which marks each parameter as relevant or not based on the avatar's parameter set.
3.  **Processing**: `registry.process()` evaluates only relevant parameters against the current `UnifiedTrackingData` and returns `OscMessage` values.
4.  **Serialization**: Messages are bundled and sent as an OSC bundle via UDP.
