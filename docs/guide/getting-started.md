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

Download the latest release from [GitHub Releases](https://github.com/dfgHiatus/VRCFT-VRFT/releases). Extract the archive to a location of your choice.

## Installation

The extracted folder contains:

```
vrft_d.exe
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
2. Run `vrft_d.exe`.
3. Watch the console — you should see the module initialize and parameters begin to send.

`vrft_d.exe` must be run from the same directory as `config.json` and the `plugins/` folder.

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
