# Architecture Overview

The VRFT Daemon (`vrft_d`) is a modular Rust-based system designed to bridge various face tracking hardware with social VR platforms like VRChat and Resonite.

## Project Structure

The project is organized into several crates within the `vrft_d` directory:

- **`api/`**: Core data structures and traits including the unified tracking data format.
- **`common/`**: Shared logic including:
  - **Mutation Pipeline**: Trait-based, pluggable processing steps.
  - **Calibration**: Per-expression min/max calibration with profile support.
  - **Filters**: Euro Filter for data smoothing.
- **`app/`**: The main executable handling plugin loading, OSC communication, and dispatch.
- **`dotnet/`**: .NET runtime host for loading VRCFT modules.

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
}
```

Default pipeline order:
1. **SmoothingMutation**: Applies Euro Filter to reduce jitter.
2. **CalibrationMutation**: Scales values based on learned min/max per expression.
3. **NormalizationMutation**: Normalizes pupil diameter to 0-1 range.
