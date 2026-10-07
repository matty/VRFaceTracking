# Configuration and Debugging

## Configuration (`config.json`)

The daemon is configured via a `config.json` file located alongside the executable.

### Structure

```json
{
  "module": {
    "active": "vd_module.dll"
  },
  "mutator": {
    "enabled": true,
    "smoothness": 0.0
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
| `module` | `active` | string | The tracking module to load: its path under `plugins/` with `/` separators (for a module directly in `plugins/`, its filename). Empty or missing loads no module, as on a new install until the desktop app's first-launch setup chooses one. A bare filename, as older configs have, still matches the first module with that name. The desktop app's **Use** writes this and switches modules without a restart. Its runtime (native Rust vs .NET/VRCFT) is auto-detected from the `.dll`'s PE header — no `runtime` field is needed. (A legacy `runtime` value in older configs still parses but is ignored.) |
| `module` | `registry_url` | string | Optional. Where the Modules page gets its module list. Defaults to VRCFT's registry, `https://registry.vrcft.io/modules`. |
| `mutator` | `enabled` | bool | Whether to enable the mutation pipeline. |
| `mutator` | `smoothness` | float | Smoothing amount (0.0 to 1.0). |
| `mutator` | `filter` | object | Raw smoothing settings. See [Tracking tuning](#tracking-tuning). |
| `mutator` | `correctors` | object | Fixes to shape combinations. See [Tracking tuning](#tracking-tuning). |
| `mutator` | `adjustment` | object | Per-group range remapping. See [Tracking tuning](#tracking-tuning). |
| `osc` | `output_mode` | string | Target platform: `VRChat`, `Resonite`, or `Generic`. |
| `osc` | `send_address` | string | IP address to send OSC data to. For VRChat, only the running VRChat at this address is used; `127.0.0.1` means this PC. |
| `osc` | `send_port` | int | Port to send OSC data to. For VRChat this is a fallback: once VRChat is found over OSCQuery (mDNS), the port it reports is used. |
| — | `max_fps` | float | Target update rate for the daemon. |
| — | `setup_done` | bool | Set by the desktop app once its first-launch setup is finished or skipped. While it's missing and no module is chosen, the app opens on setup. Settings' **Run setup again** opens it any time. |

## Tracking tuning

These match the tuning options in VRCFaceTracking (Data Filter, Unified
Correctors and Parameter Adjustment). VRCFaceTracking's calibration isn't
included. Changes take effect when the daemon starts. The desktop app's
**Tracking settings** page, under **Modules**, sets all of them.

```json
"mutator": {
  "enabled": true,
  "smoothness": 0.3,
  "filter": { "min_cutoff": 1.0, "beta": 0.5, "d_cutoff": 0.1, "head": true },
  "correctors": {
    "enabled": true,
    "mouth_closed_clamp": true,
    "lip_suck_limiter": true,
    "eyelid_blend": 0.0,
    "eye_look_symmetrize": false
  },
  "adjustment": {
    "enabled": true,
    "ranges": { "jaw": [0.0, 0.8], "eye_wide": [0.1, 1.0], "head_yaw": [-0.5, 0.5] }
  }
}
```

By default the pipeline runs the adjustment (when enabled), then the
correctors (when enabled), then smoothing. `mutator.enabled: false` turns all
of them off. An explicit `mutator.pipeline` list, such as
`[{"type": "adjustment"}, {"type": "correctors"}, {"type": "smoothing"}]`,
runs exactly the steps it lists, whatever their `enabled` says.

### `filter`

Smoothing uses a One Euro filter. `smoothness` picks `min_cutoff` and `beta`
for you; set either here to use your own value instead.

| Parameter | Default | Description |
| :-------- | :------ | :---------- |
| `min_cutoff` | from `smoothness` | Cutoff frequency (Hz) while a value is still. Lower is smoother but lags more. Must be above 0. |
| `beta` | from `smoothness` | How quickly the cutoff rises as a value moves. Higher lags less on fast movement. 0 or more. |
| `d_cutoff` | `0.1` | Cutoff frequency (Hz) for the speed estimate. Must be above 0. |
| `head` | `true` | Whether head rotation and position are smoothed too. |

### `correctors`

| Parameter | Default | Description |
| :-------- | :------ | :---------- |
| `enabled` | `true` | Whether the default pipeline runs the correctors. |
| `mouth_closed_clamp` | `true` | Keeps `MouthClosed` at or below `JawOpen`. |
| `lip_suck_limiter` | `true` | Reduces each lip suck as the lip on that side opens (`MouthUpperUp`/`MouthLowerDown`). |
| `eyelid_blend` | `0.0` | How much each side follows the other, from 0 (not at all) to 1 (both become their average). Applies to eye openness, pupil size, `EyeWide`, `EyeSquint` and the brows. |
| `eye_look_symmetrize` | `false` | Gives both eyes their average vertical gaze, for hardware whose eyes drift apart. |

### `adjustment`

Each group in `ranges` has its `[floor, ceil]` stretched to the full range:
values at or below `floor` become the minimum, values at or above `ceil`
become the maximum. So `"jaw": [0.0, 0.8]` opens the avatar's jaw fully at 80%,
and `"eye_wide": [0.1, 1.0]` ignores small widening. Face groups run from 0 to
1, and head groups from -1 to 1. `floor` must be below `ceil`. Unknown groups
and unusable ranges are logged and ignored.

| Group | Shapes |
| :---- | :----- |
| `brow_raiser` | BrowInnerUp, BrowOuterUp |
| `brow_lowerer` | BrowLowerer, BrowPinch |
| `eye_squint` | EyeSquint |
| `eye_wide` | EyeWide |
| `cheek` | CheekPuff, CheekSuck |
| `cheek_squint` | CheekSquint |
| `jaw` | JawOpen, JawClench, JawMandibleRaise |
| `mouth_closed` | MouthClosed |
| `jaw_sideways` | JawLeft, JawRight |
| `jaw_forward_backward` | JawForward, JawBackward |
| `lip_funnel` | LipFunnel (all four) |
| `lip_suck` | LipSuck (upper, lower, corners) |
| `lip_pucker` | LipPucker (all four) |
| `mouth_open` | MouthUpperUp, MouthUpperDeepen, MouthLowerDown |
| `mouth_smile` | MouthCornerPull, MouthCornerSlant |
| `mouth_frown` | MouthFrown |
| `mouth_stretch` | MouthStretch |
| `mouth_dimple` | MouthDimple |
| `mouth_tightener` | MouthTightener |
| `mouth_press` | MouthPress |
| `mouth_sideways` | MouthUpperLeft/Right, MouthLowerLeft/Right |
| `mouth_raiser` | MouthRaiserUpper, MouthRaiserLower |
| `nose` | NasalConstrict, NasalDilation |
| `nose_sneer` | NoseSneer |
| `neck` | NeckFlex, SoftPalateClose, ThroatSwallow |
| `tongue_out` | TongueOut |
| `tongue_directions` | TongueUp/Down/Left/Right, TongueBendDown, TongueCurlUp |
| `tongue_other` | TongueRoll, TongueFlat, TongueSquish, TongueTwist |
| `head_yaw`, `head_pitch`, `head_roll` | Head rotation |
| `head_pos_x`, `head_pos_y`, `head_pos_z` | Head position |

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
