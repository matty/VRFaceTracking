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

Download the latest release from [GitHub Releases](https://github.com/matty/VRFaceTracking/releases/latest). Releases are named by date, `YYYY.M.N`; the [dev build](https://github.com/matty/VRFaceTracking/releases/tag/dev) is the latest untested build from `main`, named `YYYY.M.N-dev.C` after the release it leads to.

The easiest way in is the installer, `VRFaceTracking-stable-Setup.exe` (or `VRFaceTracking-dev-Setup.exe` from the dev build, which shows as **VRFaceTracking (Dev)**). An installed copy checks for updates when it opens and every few hours, downloads them by itself, and installs them when it restarts; **Settings → Updates** shows where it stands. A release follows releases and a dev build follows dev builds. Installing one over the other switches between them. Updates replace the app's folder, so an installed copy keeps `config.json`, `plugins/`, models, recordings and `vrft_d.log` in a `data/` folder beside it (`%LocalAppData%\VRFaceTracking\data`), which updates leave alone.

You can still extract a zip to a location of your choice instead. That copy doesn't update itself.

The Quest Pro headset app is released separately, in releases tagged `apk-v…`, and much less often. Each VRFT release links the headset app release to install and carries a copy of it in `headset-app/`, which the desktop app's **Headset** page installs for you. It is often an older one, and you don't need to reinstall the headset app when you update VRFT unless VRFT tells you to. If the two can't work together, the Headset tile in `vrft_app.exe` says whether to update VRFT or the headset app.

## Installation

The extracted folder contains:

```
vrft_app.exe       ← the desktop app; starts vrft_d.exe
vrft_d.exe
headset-app/       ← the Quest Pro headset app this VRFT works with
platform-tools/    ← adb, once the Headset page has downloaded it
config.json
plugins/           ← tracking modules, native or VRCFT (.NET), anywhere in here
  registry/        ← modules installed from the app's Modules page
runtime/
  VrcftRuntime.exe ← runs VRCFT (.NET) modules
```

On first launch the desktop app opens on a short setup. It asks which tracking module to use, one already in `plugins/` (such as `vd_module.dll`) or one from VRCFT's module registry, then whether you have a Quest Pro, to turn on the Quest Pro add-on. No module is chosen until you pick one, there or later. Settings' **Run setup again** opens it again.

The easiest way to get a tracking module is the desktop app's **Modules** page. It lists VRCFT's module registry (the same modules the VRCFT app offers) and installs, updates and removes them. **Use** switches VRFT to a module, native or .NET, straight away: the running module is unloaded and the new one loaded without restarting VRFT or its extensions. If it doesn't load, the page says why and offers **Try again**. VRFT runs these .NET modules itself through `runtime/VrcftRuntime.exe`, so VRCFT doesn't need to be installed. Modules are made by their authors, not VRFT, so install only ones you trust.

Each module installs into `plugins/registry/<ModuleId>/` with the registry entry saved as `module.json`, the layout VRCFT uses. A folder with a `module.json` counts as one module, the `.dll` its `DllFileName` names, so the dependencies beside it aren't listed as modules. You can copy module folders from a VRCFT install into `plugins/` and they're found the same way. An update to the module in use downloads straight away and replaces it the next time VRFT starts.

To add a module by hand, place its `.dll` (or its folder) anywhere under `plugins/`.

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

`active` is the module's path under `plugins/`, such as `registry/<ModuleId>/net7.0/Module.dll`; for a module directly in `plugins/` that's its filename. A bare filename also works, and picks the first module with that name. For VRCFT .NET modules, just set `active` to the module — the daemon auto-detects that it's a .NET module and launches it via the runtime host.

For a full reference of all config options, see [Configuration](configuration.md).

## Running

1. Start VRChat (or your target platform).
2. Run `vrft_app.exe`. It starts `vrft_d.exe`, stops it again when you close the app, and shows whether the module, output and headset are working. You can also run `vrft_d.exe` on its own; the app then connects to it and leaves it running when it closes.
3. Watch the app, or `vrft_d.exe`'s console. You should see the module initialize and parameters begin to send.

`vrft_d.exe` must be run from the same directory as `config.json` and the `plugins/` folder.

### Quest Pro enhanced tongue tracking and independent eye gaze

Quest Pro support is an extension, on by default. If you don't use a Quest Pro, turn it off on the desktop app's **Modules** page, under **Add-ons**, or set `"extensions": { "quest-pro": { "enabled": false } }` in `config.json` and restart VRFT.

The rooted Quest Pro APK in [android/questpro-camera](../../android/questpro-camera/README.md) streams its two lower-face cameras to VRFT. It can also stream each eye's own gaze so avatar eyes converge; see [Quest Pro independent eye gaze](quest-pro-eye-tracking.md). Install and start that APK, then return to your streaming app (Virtual Desktop, Steam Link or another) on the headset. The cameras, eye gaze and tongue don't need a tracking module. The desktop app's **Headset** page can do this over adb: it connects over USB or Wi-Fi, installs or updates the headset app, and starts and stops its stream. It uses an adb that's already on the PC, or downloads the adb files from Google's Platform-Tools into a `platform-tools` folder beside `vrft_app.exe` and uses that copy from then on. The first time, press **Download built-in model** on the app's **Training** page (or the preview's **Tongue** tab). VRFT downloads the Qpro-Enhanced-FT v0.1.10 release once (about 140 MB), checks it and both model files against fixed SHA-256 hashes, and saves the demo pair in `models/quest-pro/`. The same download then fetches the rendered training examples that personal training mixes in (about 123 MB; see [tongue training](tongue-training.md#rendered-training-examples)). To use archives you already have, put `QproFaceTracking-0.1.10-poc.zip` or `tongue-synthetic-v4.zip` in `.local/` first. Nothing else needs installing: VRFT runs the model itself, with no Python.

Run `vrft_d.exe` from its folder with `models/quest-pro/` beside it. The model runs on ONNX Runtime (`onnxruntime.dll`, beside `vrft_d.exe`) on the CPU, in int8 where it keeps the model's accuracy, which takes a few milliseconds a frame and leaves the GPU to the game. Set the tongue device to `gpu` to run it through DirectML instead. Without ONNX Runtime it runs on Burn: on the GPU (NVIDIA, AMD or Intel, through DX12 or Vulkan) when one works, and on the CPU otherwise, which is much slower. When camera frames arrive, VRFT starts the model automatically and sends its twelve detailed tongue expressions through its normal VRChat OSC output. If camera frames or model output are older than 250 ms, it uses the active tracking module's tongue values, if there is one, without interrupting other tracking. With no tracking module loaded or sending, VRFT still sends the camera tongue and eye gaze at the `max_fps` rate. The console logs discovery, connection, model readiness, inference time, skipped frames, and transitions between enhanced and module tracking. No Quest Pro device check or manual mode switch is required.

The [Qpro-Enhanced-FT v8 release](https://github.com/n0tmast3r/Qpro-Enhanced-FT/releases/tag/v0.1.10) describes these checkpoints as a one-person demo, so they may need personal training for accurate tongue direction. VRFT's preview can train and select personal pairs without replacing the built-in checkpoints. The release and personal models stay local and are excluded from Git.

The desktop app's **Mouth** page (or the preview's **Settings** tab) controls motion smoothing (0 is most responsive, 100 smoothest; default 55) and how camera visibility combines with the headset's own TongueOut: weighted (default), camera only, native only, or only when both see it out, which is more conservative. These modes only decide whether the tongue is out or in; direction always comes from the cameras. Changes apply immediately and are saved in `.local/quest-pro-settings.json`. The Mouth page's **What VRChat receives** section shows the twelve values sent, beside what the cameras and the headset each see. While the tongue is shown, TongueOut is the larger of the fused visibility and the model's extension, so a clearly visible tongue is never sent as barely out. The camera frame rate is set in the APK (24 FPS by default).

> The browser preview below is turned off for now; use the desktop app's Quest Pro pages.

The local [camera and tongue preview](http://127.0.0.1:27275/) shows both grayscale cameras and a live view of the tongue your avatar receives, with the status of the mouth cameras, the tracking module (optional), the tongue model and the eyes. Its **Settings** tab lists the model's ten predictions after smoothing, native TongueOut, the twelve final values VRFT sends through its VRChat pipeline, and frame and model age, so a stopped feed is obvious. Run `vrft_d.exe --extensions-only` to inspect the feed and raw model predictions without loading a tracking module or sending OSC; final values require normal mode. If discovery is blocked, set `VRFT_QUEST_PRO_ADDR` to the headset's `IP:27274`. Set `VRFT_TONGUE_MODEL_DIR` to use a model pair stored elsewhere. Set `VRFT_TONGUE_DEVICE=cpu` or `gpu` to force where the model runs.

To personalise tongue tracking, open the desktop app's **Training** page (or the preview's **Tongue** tab), press **Start recording** and follow the prompts for about two and a half minutes, then press **Train my model**. The new model switches on when training finishes. See the [personal tongue training guide](tongue-training.md). Recording needs only the mouth cameras, so it also works with `--extensions-only`. Training existing recordings works in either mode.

## What to Expect

On startup, vrft_d will:

1. Load the configured tracking module.
2. Find VRChat over OSCQuery (mDNS), using the port it reports, and listen for its replies on `send_port` + 1 (9001 by default). VRChat's OSC must be on.
3. Read which parameters your avatar supports, and read them again on each avatar change.
4. Begin sending OSC messages at the configured `max_fps` rate.

Log output goes to the console. To increase verbosity, set the `RUST_LOG` environment variable:

```powershell
$env:RUST_LOG = "info,vrft_d=debug"; .\vrft_d.exe
```

### Logs in the desktop app

When the desktop app starts tracking, `vrft_d`'s output goes to `vrft_d.log` instead of a console, and the app writes its own `vrft_app.log`, both in the data folder. Each keeps the run before it as `vrft_d.previous.log` and `vrft_app.previous.log`, so restarting after a crash doesn't lose what happened.

The app's **Logs** page shows them as they're written: this run's tracking log, the last run's, or the app's own. It can show only warnings and errors, or lines containing some text; clicking a line shows its whole record, such as an error's causes, to copy. Times are in your PC's time zone.

- **Detailed logging** records debug messages from both programs until the app closes. Turning it on or off restarts tracking, as `vrft_d` only reads its log level when it starts. A `RUST_LOG` you set yourself still decides the app's own level.
- **Save report** writes one text file with the app and tracking versions, the latest status, `config.json`, and the end of every log to `reports/` in the data folder, and shows it in Explorer, ready to attach to an issue.

A `vrft_d` started from a console, rather than by the app, still logs to that console.

### Seeing what each step does

The app's **Debug** page draws the chain every value passes through: the tracking module, any test values set through the debug API, expression ranges, corrections, smoothing, each add-on that changes values, and the output. Steps that are turned off show dashed, in their place.

- Click a **step** to list only the values it changed in the last moment, with how much.
- Click a **value** in the list to follow it: each box in the chain then shows it after that step, and what the step did to it.
- **Changed** lists only values some step altered; the columns between *From module* and *Sent* show each step's change.
- **Pause** freezes the frame on screen; **Copy values** copies every value after each step as tab-separated text.

The page reads `GET /debug/pipeline` from `vrft_d`'s local API (port 27275). `vrft_d` only records frames for a couple of seconds after each request, so it costs nothing while the page isn't open.
