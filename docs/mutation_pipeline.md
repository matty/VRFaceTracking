# Mutation Pipeline

The mutation pipeline is the post-processing system that transforms raw tracking data before it's sent to VR platforms.

## Overview

Instead of hardcoded processing logic, VRFT uses a **pluggable pipeline** of mutations. Each mutation implements the `Mutation` trait and processes data in sequence.

## The `Mutation` Trait

Defined in `common/src/mutation_trait.rs`:

```rust
pub trait Mutation: Send + Sync {
    fn initialize(&mut self, config: &MutationConfig) -> Result<()>;
    fn mutate(&mut self, data: &mut UnifiedTrackingData, dt: f32);
    fn name(&self) -> &str;
    fn priority(&self) -> i32 { 0 }
}
```

## Built-in Mutations

| Mutation | File | Purpose |
|----------|------|---------|
| **SmoothingMutation** | `mutations/smoothing.rs` | Applies Euro Filter to reduce jitter in gaze, shapes, and openness |
| **CalibrationMutation** | `mutations/calibration.rs` | Scales values using learned min/max per expression |
| **NormalizationMutation** | `mutations/normalization.rs` | Normalizes pupil diameter to 0-1 range |

## Pipeline Execution

The pipeline is stored in `UnifiedTrackingMutator`:

```rust
pub struct UnifiedTrackingMutator {
    pub config: MutationConfig,
    pipeline: Vec<Box<dyn Mutation>>,
}
```

On each frame, `mutate()` iterates through the pipeline:

```rust
for mutation in &mut self.pipeline {
    mutation.mutate(data, dt);
}
```

## Adding Custom Mutations

1. Create a struct implementing `Mutation`.
2. Add it to the pipeline in `UnifiedTrackingMutator::new()`.
3. (Future) Configure via `config.json` for dynamic loading.

## Configuration

Currently, the pipeline is hardcoded. Future enhancements will allow JSON configuration:

```json
{
  "mutator": {
    "enabled": true,
    "pipeline": [
      { "type": "smoothing", "smoothness": 0.5 },
      { "type": "calibration", "enabled": true },
      { "type": "normalization" }
    ]
  }
}
```
