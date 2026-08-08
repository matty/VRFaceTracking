# Architecture Overview

The VRFT Daemon (`vrft_d`) is a modular Rust-based system designed to bridge various face tracking hardware with social VR platforms like VRChat and Resonite.

## Project Structure

The project is organized into a workspace with two top-level directories:

### `vrft_d/` — Core Daemon

- **`api/`**: Core data structures and traits including the unified tracking data format.
- **`common/`**: Shared logic including:
  - **Mutation Pipeline**: Trait-based, pluggable processing steps.
  - **Filters**: Euro Filter for data smoothing.
- **`app/`**: The main executable handling plugin loading, OSC communication, and dispatch.
  - **`strategies/`**: Output strategies for VRChat, Resonite, and Generic UDP.
  - **`osc/`**: OSC protocol implementation, parameter definitions, and OSC Query support.
- **`dotnet/`**: .NET runtime host for loading VRCFT modules via shared memory proxy.

### `modules/` — Tracking Module Plugins

- **`vd_module/`**: Virtual Desktop face tracking module (native, shared memory).
- **`test_logger/`**: Example module demonstrating the plugin API and logging.
- **`vrft_udp_rcv/`**: UDP receiver test utility for the generic_udp strategy.

## Data Flow


1. **Hardware Module**: A plugin captures raw data and converts it to the unified API format.
2. **Mutation Pipeline**: A `Vec<Box<dyn Mutation>>` processes data through configurable steps.
3. **Output Strategy**: Processed data is dispatched via OSC to VRChat, Resonite, etc.

## Mutation Pipeline

The pipeline is defined in `common/src/mutator.rs`. Each step implements the `Mutation` trait:

```rust
pub trait Mutation: Send + Sync {
    fn initialize(&mut self, config: &MutationConfig) -> Result<()>;
    fn mutate(&mut self, data: &mut UnifiedTrackingData, dt: f32);
    fn name(&self) -> &str;
    fn priority(&self) -> i32 { 0 }
}
```

Default pipeline order:
1. **SmoothingMutation**: Applies Euro Filter to reduce jitter.
2. **NormalizationMutation**: Normalizes pupil diameter to 0-1 range.
