# Quest Pro independent eye gaze

The Quest Pro's own eye tracker blends both eyes into one binocular estimate before Virtual Desktop sees it. Both avatar eyes therefore point the same way, and convergence (both eyes turning inward to look at something close) never shows. With the rooted-headset camera APK, VRFT can instead send each eye's own gaze, so the avatar's eyes converge and diverge.

This is a port of the independent-gaze feature from [Qpro-Enhanced-FT](https://github.com/n0tmast3r/Qpro-Enhanced-FT), including the engine profile for build `51503870024400340` added by the [Fwooffy fork](https://github.com/Fwooffy/Qpro-Enhanced-FT-Wireless). It needs a rooted Quest Pro, the camera APK in [android/questpro-camera](../../android/questpro-camera/README.md), and Wi-Fi between the headset and PC. No USB or ADB connection is needed while tracking.

## How it works

1. When **Independent eye gaze** is on, the APK briefly replaces Meta's experimental eye model with a copy patched so its public gaze output comes from each eye's own branch instead of the blended one. The patch changes two bytes of the model graph. The original file is never modified: the patched copy is bind-mounted over it until the stream stops.
2. The APK traces the tracker's per-eye visual-axis vectors in the kernel and streams them to VRFT with the camera frames.
3. VRFT converts each vector to yaw and pitch, applies a per-eye calibration, filters each eye separately (a three-sample median, then a One Euro filter), and replaces the tracking module's gaze. Eye openness, pupils and all face expressions still come from the tracking module.
4. If eye samples stop for 250 ms, VRFT falls back to the tracking module's gaze without interrupting anything else.

Stopping the stream in the APK unmounts the patched model, restores Meta's setting and restarts the tracking service. If the APK or headset crashes first, the APK restores everything the next time it starts; a headset reboot also removes the mount. See the APK README for the manual restore commands.

## Firmware support

The trace hook is specific to the tracking-engine binary, so the APK checks it before doing anything:

| Headset build | Engine check | Status |
|---|---|---|
| `51483620027600340` | file size | Tested with convergence by the Qpro-Enhanced-FT author |
| `51503870024400340` | file size and SHA-256 | Probed by the Fwooffy fork; a full convergence session was never confirmed |

On any other build the APK reports "Unsupported tracking-engine build" in the preview and streams the cameras without eye gaze. Treat every firmware update as unsupported until the offsets are revalidated.

## Using it

1. In the headset app, tick **Independent eye gaze** and start the stream. The first start takes a few seconds longer while the tracking service restarts.
2. Open [the local preview](http://127.0.0.1:27275/). The **Eyes** tab shows:
   - whether gaze is active and its sample rate;
   - whether the headset reports the patched model as active (**Per-eye model**);
   - the left and right angles VRFT sends, and under **Eyes meet** the estimated focus distance, with the difference between the two angles (positive means converging);
   - a top-down view of both gaze rays and, if enabled in the APK, low-rate eye-camera snapshots.
3. Press **Recenter**, then look straight ahead at something far away during the three-second countdown. This stores a yaw and pitch offset for each eye, so your eyes read as parallel at a distance. It corrects fit differences without changing how the eyes move relative to each other. **Undo recenter** removes it.
4. Check in VRChat: looking left should turn both avatar eyes left, and focusing on your hand close to your face should turn them inward. If not, open **Avatar's eyes move the wrong way?** in the preview:
   - eyes move the right way but diverge when they should converge: toggle **Swap left and right eyes**;
   - everything is mirrored: toggle **Mirror left and right**.

Untick **Track each eye separately** to send the tracking module's gaze without stopping the stream. Settings are saved in `.local/quest-pro-settings.json`.

## Calibration

VRFT maps each eye's detector angles to gaze with an affine calibration in the reference project's `qpro-independent-personalized-visual-axis-v1/v2` format. It loads the first of:

1. the file named by `VRFT_EYE_CALIBRATION`;
2. `.local/eye-calibration.json`;
3. `models/quest-pro/qpro-independent-visual-axis-v2.json`, the reference author's demonstration profile.

The demonstration profile was fitted to one person on the older firmware. Its absolute alignment and depth may be off for you; **Recenter** fixes the offset but not the scale. Its own quality gate records that depth was never validated, and the preview says so. A profile made with the reference project's calibration tools (which use Project Babble's VR calibration routine) can be copied to `.local/eye-calibration.json`.

## Limitations

- Convergence quality depends on Meta's per-eye branch, which is noisier than the blended output. The filter trades a little latency for stability.
- The patched model only affects gaze. Blink and face values are unchanged.
- The stream has no authentication or encryption. Use it on a trusted network.
