# Getting Started

This guide covers downloading, installing, and running `vrft_d` for the first time.

## Prerequisites

- Windows 10/11
- A supported face tracking device (see Hardware Support below)

## Hardware Support

| Source | Module | Runtime |
|--------|--------|---------|
| Virtual Desktop (face tracking passthrough) | `vd_module.dll` | Native |
| VRCFaceTracking-compatible devices | VRCFT module `.dll` | .NET |

For Virtual Desktop, face tracking must be enabled in Virtual Desktop Streamer settings.

## Download

Download the latest release from [GitHub Releases](https://github.com/matty/VRFaceTracking/releases/latest). Extract the archive to a location of your choice. Releases are named by date, `YYYY.M.N`; the [dev build](https://github.com/matty/VRFaceTracking/releases/tag/dev) is the latest untested build from `main`.

The Quest Pro headset app is released separately, in releases tagged `apk-v…`, and much less often. Each VRFT release links the headset app release to install and carries a copy of it in `headset-app/`, which the desktop app's **Headset** page installs for you. It is often an older one, and you don't need to reinstall the headset app when you update VRFT unless VRFT tells you to. If the two can't work together, the Headset tile in `vrft_gui.exe` says whether to update VRFT or the headset app.

## Installation

The extracted folder contains:

```
vrft_gui.exe       ← the desktop app; starts vrft_d.exe
vrft_d.exe
headset-app/       ← the Quest Pro headset app this VRFT works with
platform-tools/    ← adb, once the Headset page has downloaded it
config.json
plugins/
  native/          ← place native (.dll) tracking modules here
  dotnet/
    modules/       ← place VRCFT .dll modules here
```

Place your tracking module `.dll` in the appropriate plugins subfolder.

## Configuration

Open `config.json` and set the `active` module filename:

```json
{
  "module": { "active": "vd_module.dll" },
  "mutator": { "enabled": true, "smoothness": 0.0 },
  "osc": { "output_mode": "VRChat", "send_address": "127.0.0.1", "send_port": 9000 },
  "max_fps": 60.0
}
```

For VRCFT .NET modules, just set `active` to your module filename — the daemon auto-detects that it's a .NET module and launches it via the runtime host.

For a full reference of all config options, see [Configuration](configuration.md).

## Running

1. Start VRChat (or your target platform).
2. Run `vrft_gui.exe`. It starts `vrft_d.exe` and shows whether the module, output and headset are working. You can also run `vrft_d.exe` on its own.
3. Watch the app, or `vrft_d.exe`'s console. You should see the module initialize and parameters begin to send.

`vrft_d.exe` must be run from the same directory as `config.json` and the `plugins/` folder.

### Quest Pro enhanced tongue tracking and independent eye gaze

The rooted Quest Pro APK in [android/questpro-camera](../../android/questpro-camera/README.md) streams its two lower-face cameras to VRFT. It can also stream each eye's own gaze so avatar eyes converge; see [Quest Pro independent eye gaze](quest-pro-eye-tracking.md). Install and start that APK, then return to Virtual Desktop on the headset. The desktop app's **Headset** page can do this over adb: it connects over USB or Wi-Fi, installs or updates the headset app, and starts and stops its stream. It uses an adb that's already on the PC, or downloads the adb files from Google's Platform-Tools into a `platform-tools` folder beside `vrft_gui.exe` and uses that copy from then on. In this repository, run `./setup-quest-pro-tongue.ps1` once to download and verify the v0.1.10 demo model pair and install a private Python 3.12 inference runtime. You can instead pass `-ReleaseZip C:\path\to\QproFaceTracking-0.1.10-poc.zip` to use a previously downloaded release archive. The script verifies the archive and both model files against fixed SHA-256 hashes.

Run `vrft_d.exe` from its folder with `tongue_inference.py`, `qpro_model.py`, `models/quest-pro/`, and `.local/tongue-python/` beside it. When camera frames arrive, VRFT starts the model automatically and sends its twelve detailed tongue expressions through its normal VRChat OSC output. If camera frames or model output are older than 250 ms, it uses the active tracking module's tongue values without interrupting other tracking. The console logs discovery, connection, model readiness, inference time, skipped frames, and transitions between enhanced and module tracking. No Quest Pro device check or manual mode switch is required.

The [Qpro-Enhanced-FT v8 release](https://github.com/n0tmast3r/Qpro-Enhanced-FT/releases/tag/v0.1.10) describes these checkpoints as a one-person demo, so they may need personal training for accurate tongue direction. VRFT's preview can train and select personal pairs without replacing the built-in checkpoints. Keep `train_vrft_tongue.py` and `prepare_vrft_tongue.py` beside the daemon for this workflow. The release and personal models stay local and are excluded from Git.

The preview's **Settings** tab controls motion smoothing (0 is most responsive, 100 smoothest; default 55) and how camera visibility combines with the headset's own TongueOut: weighted (default), camera only, native only, or conservative agreement, which needs both. Changes apply immediately and are saved in `.local/quest-pro-settings.json`. While the tongue is shown, TongueOut is the larger of the fused visibility and the model's extension, so a clearly visible tongue is never sent as barely out. The camera frame rate is set in the APK (24 FPS by default).

The local [camera and tongue preview](http://127.0.0.1:27275/) shows both grayscale cameras and a live view of the tongue your avatar receives, with the status of the mouth cameras, the headset's face tracking, the tongue model and the eyes. Its **Settings** tab lists the model's ten predictions after smoothing, native TongueOut, the twelve final values VRFT sends through its VRChat pipeline, and frame and model age, so a stopped feed is obvious. Run `vrft_d.exe --camera-preview-only` to inspect the feed and raw model predictions without loading a tracking module or sending OSC; final values require normal mode. If discovery is blocked, set `VRFT_QUEST_PRO_ADDR` to the headset's `IP:27274`. Set `VRFT_TONGUE_PYTHON` or `VRFT_TONGUE_MODEL_DIR` to use a runtime or model pair stored elsewhere. Set `VRFT_TONGUE_DEVICE=cpu` to force CPU inference.

To personalise tongue tracking, open the preview's **Tongue** tab, press **Start recording** and follow the prompts for about two minutes, then press **Train my model**. The new model switches on when training finishes. See the [personal tongue training guide](tongue-training.md). Recording requires normal VRFT mode with a live tracking module. Training existing recordings also works in preview-only mode.

## What to Expect

On startup, vrft_d will:

1. Load the configured tracking module.
2. Connect to VRChat via OSC Query on port 9001 (default).
3. Wait for an avatar change to discover which parameters your avatar supports.
4. Begin sending OSC messages at the configured `max_fps` rate.

Log output goes to the console. To increase verbosity, set the `RUST_LOG` environment variable:

```powershell
$env:RUST_LOG = "info,vrft_d=debug"; .\vrft_d.exe
```
