# Resonite Integration

> **Status: Work in Progress.** The Resonite output strategy is functional but incomplete. This document captures current implementation state and known gaps — it is not a user guide.

## What the Resonite Strategy Does

The `ResoniteOsc` strategy (`vrft_d/app/src/strategies/resonite.rs`) sends face tracking data to Resonite via OSC over UDP.

Resonite uses two OSC address namespaces for face tracking:

- `/avatar/parameters/` — same as VRChat; used for eye tracking parameters (gaze, openness)
- `/sl/xrfb/facew/` — Steam Link face weight parameters; used for expression blendshapes

Unlike the VRChat strategy, the Resonite strategy does **not** use OSC Query to discover avatar parameters. It sends a fixed set of addresses unconditionally on every frame.

## Known Gaps

- No delta checking — all parameters are sent every frame regardless of whether values changed.
- No avatar-change detection or parameter filtering.
- The `/sl/xrfb/facew/` parameter set is based on the Meta XR Face Tracking SDK shape names, which may not map cleanly to all hardware modules.
- Binary parameters and the `FT/` prefix convention are not used — Resonite does not support them.

## Configuration

To use the Resonite strategy, set `output_mode` in `config.json`:

```json
{
  "osc": {
    "output_mode": "Resonite",
    "send_address": "127.0.0.1",
    "send_port": 9000
  }
}
```

Resonite listens on port 9000 or 9015 depending on its settings (Settings > Devices > OSC Face Tracking Port).
