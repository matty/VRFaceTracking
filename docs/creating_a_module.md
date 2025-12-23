# Creating a Tracking Module

This guide walks you through the process of creating a tracking module plugin for the `vrft_d` face tracking system.

## Overview

The `vrft_d` system uses a plugin architecture where tracking modules are dynamically loaded libraries (`.dll` files on Windows) that implement the `TrackingModule` trait defined in the `api` crate.

## Module Architecture

### Key Components

1.  **API Crate** (`vrft_d/api`): Defines the shared interface and data types.
2.  **Module Implementation**: Your plugin code that implements `TrackingModule`.
3.  **Dynamic Loading**: The host application loads your module as a C-compatible dynamic library.

### Core Data Types

The API provides several key types defined in [`vrft_d/api/src/lib.rs`](../vrft_d/api/src/lib.rs):

- `UnifiedTrackingData`: Contains eye, expression shapes, and head pose data.
- `UnifiedEyeData`: Eye gaze, openness, and pupil diameter.
- `UnifiedExpressionShape`: Individual expression weight (0.0 to 1.0).
- `UnifiedExpressions`: Enum of all supported expressions.
- `ModuleLogger`: Logging interface provided by the host.

## Step-by-Step Guide

### 1. Create a New Cargo Project

Create a new library crate in the root directory:

```bash
cargo new --lib my_module
```

### 2. Configure Cargo.toml

Your `Cargo.toml` must specify `cdylib` as the crate type:

```toml
[package]
name = "my_module"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
api = { path = "vrft_d/api" }
anyhow = "1.0"
glam = "0.24"
```

### 3. Implement the TrackingModule Trait

The `TrackingModule` trait has three required methods:

```rust
pub trait TrackingModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()>;
    fn update(&mut self, data: &mut UnifiedTrackingData) -> Result<()>;
    fn unload(&mut self);
}
```

#### `initialize()`

Called once when the module is loaded. Use this to set up connections to hardware or data sources.

#### `update()`

Called repeatedly in the main tracking loop. Read data from your source and update the provided `UnifiedTrackingData` struct.

#### `unload()`

Called when the module is being unloaded. Clean up resources and close connections.

### 4. Export the Module Factory Function

Your library **must** export a C-compatible function named `create_module`:

```rust
#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(MyTrackingModule::new())
}
```

## Best Practices

### Error Handling

- Return errors from `update()` when data is temporarily unavailable; the host will skip processing for that frame.
- Use the provided `ModuleLogger` instead of `println!`.

### Performance

- Keep `update()` fast and non-blocking.
- Use `debug` or `trace` log levels for frequent update loop diagnostics.

### Deployment

Build in release mode and copy the DLL to the `vrft_d/plugins` directory:

```bash
cargo build --release
copy target\release\my_module.dll vrft_d\plugins\
```
