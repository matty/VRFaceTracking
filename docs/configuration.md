# Configuration and Debugging

## Configuration (`config.json`)

The daemon is configured via a `config.json` file located in the `vrft_d` directory.

### Key Parameters

| Parameter             | Type   | Description                                                |
| :-------------------- | :----- | :--------------------------------------------------------- |
| `smoothness`          | float  | Amount of smoothing applied to tracking data (0.0 to 1.0). |
| `mutator_enabled`     | bool   | Whether to enable data mutation logic.                     |
| `calibration_enabled` | bool   | Whether to enable runtime calibration.                     |
| `transport_type`      | string | The target protocol (e.g., `VRChatOSC`).                   |
| `osc_send_address`    | string | IP address to send OSC data to.                            |
| `osc_send_port`       | int    | Port to send OSC data to.                                  |
| `active_plugin`       | string | The filename of the hardware module DLL to load.           |
| `max_fps`             | float  | Target update rate for the daemon.                         |

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

### PowerShell Debug Script

A helper script `debug_expressions.ps1` is provided in the `scripts/` directory to automate testing common expressions. It requires the port number as an argument:

```powershell
./scripts/debug_expressions.ps1 -Port 8080
```

The script iterates through several facial states:

1.  **Reset**: Returns all parameters to neutral.
2.  **Jaw Open**: Tests maximum jaw opening.
3.  **Smile**: Tests combined mouth and eye squint parameters.
4.  **Surprise**: Tests brow and jaw movement.
5.  **Pog/Kiss/Cheek Puff**: Tests various mouth shapes.
