# Configuration and Debugging

## Configuration (`config.json`)

The daemon is configured via a `config.json` file located alongside the executable.

### Structure

```json
{
  "module": {
    "runtime": "Native",
    "active": "vd_module.dll"
  },
  "mutator": {
    "enabled": true,
    "smoothness": 0.0
  },
  "calibration": {
    "enabled": false,
    "continuous": false,
    "blend": 1.0
  },
  "osc": {
    "output_mode": "VRChat",
    "send_address": "127.0.0.1",
    "send_port": 9000
  },
  "max_fps": 60.0
}
```

### Key Parameters

| Section | Parameter | Type | Description |
| :------ | :-------- | :--- | :---------- |
| `module` | `runtime` | string | Module runtime: `Native` (Rust `.dll`) or `Vrcft` (.NET via VrcftRuntime). |
| `module` | `active` | string | Filename of the tracking module to load. |
| `mutator` | `enabled` | bool | Whether to enable the mutation pipeline. |
| `mutator` | `smoothness` | float | Smoothing amount (0.0 to 1.0). |
| `calibration` | `enabled` | bool | Whether to enable runtime calibration. |
| `calibration` | `continuous` | bool | Whether calibration continuously updates min/max ranges. |
| `calibration` | `blend` | float | Blend factor between raw and calibrated data (0.0 to 1.0). |
| `osc` | `output_mode` | string | Target platform: `VRChat`, `Resonite`, or `Generic`. |
| `osc` | `send_address` | string | IP address to send OSC data to. |
| `osc` | `send_port` | int | Port to send OSC data to. |
| — | `max_fps` | float | Target update rate for the daemon. |

## Debugging API

The daemon exposes a local HTTP API for debugging and testing tracking parameters.

### Debug Endpoint: `POST /debug/params`

Allows manual injection of tracking parameters to test avatar reactions without hardware.

**Payload Example:**

```json
{
  "JawOpen": 1.0,
  "MouthSmileLeft": 0.5,
  "MouthSmileRight": 0.5
}
```
