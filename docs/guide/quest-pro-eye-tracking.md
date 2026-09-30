# Quest Pro independent eye gaze

The Quest Pro's own eye tracker blends both eyes into one binocular estimate before streaming apps such as Virtual Desktop or Steam Link see it. Both avatar eyes therefore point the same way, and convergence (both eyes turning inward to look at something close) never shows. With the rooted-headset camera APK, VRFT can instead send each eye's own gaze, so the avatar's eyes converge and diverge.

This is a port of the independent-gaze feature from [Qpro-Enhanced-FT](https://github.com/n0tmast3r/Qpro-Enhanced-FT), including the engine profile for build `51503870024400340` added by the [Fwooffy fork](https://github.com/Fwooffy/Qpro-Enhanced-FT-Wireless). It needs a rooted Quest Pro, the camera APK in [android/questpro-camera](../../android/questpro-camera/README.md), and Wi-Fi between the headset and PC. No USB or ADB connection is needed while tracking.

## How it works

1. When **Independent eye gaze** is on, the APK briefly replaces Meta's experimental eye model with a copy patched so its public gaze output comes from each eye's own branch instead of the blended one. The patch changes two bytes of the model graph. The original file is never modified: the patched copy is bind-mounted over it until the stream stops.
2. The APK traces the tracker's per-eye visual-axis vectors in the kernel and streams them to VRFT with the camera frames. A sample whose vectors aren't roughly unit length is not a direction, and VRFT drops it.
3. VRFT converts each vector to yaw and pitch, applies a per-eye calibration, filters each eye separately (a three-sample median, then a One Euro filter), and replaces the tracking module's gaze. Eye openness, pupils and all face expressions still come from the tracking module, if one is running. Without one, VRFT still sends the per-eye gaze.
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

1. Start the stream. **Independent eye gaze** is ticked by default in the headset app; untick it there, or use **Per-eye gaze** on the desktop app's Headset page, to turn it off. Changing it restarts a running stream. The first start takes a few seconds longer while the tracking service restarts.
2. Open the desktop app's **Eyes** page. (The [local preview](http://127.0.0.1:27275/)'s **Eyes** tab is turned off for now.) It shows:
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

## Pupil size

Meta's eye tracking reports no pupil size, so streaming apps send a fixed one. VRFT instead measures each pupil in the eye camera snapshots the APK sends (5 a second by default; set in the APK's **Eye-camera snapshots**) and sends it as pupil dilation. It works with or without the modified eye model.

Under the headset's infrared light the pupil is the darkest round area of each eye image. VRFT finds it, outlines it halfway between its own darkness and the iris around it, fills in the lights' reflections on it, and measures its width along its longest axis, so a pupil partly under the eyelid still measures right. A snapshot where it is too faint, too small, or mostly covered, as in a blink, is skipped, and the median of the last three measurements is used so one bad snapshot does not show.

Camera pixels are not millimetres, and the size in pixels depends on how far each eye sits from its camera. So each eye's width is mapped onto the range that eye has shown over the last ten minutes, leaving out its rarest 3% at either end so a few misreadings can't stretch it, and that onto 2 to 8 mm, a typical adult pupil's range. Dilation, what VRChat's `PupilDilation` parameters carry, is therefore right after your pupils have changed size once or twice, as they do when you look from something bright to something dark. The millimetre values are an estimate on that scale, not a measurement. Until an eye has changed by 45% its range is widened to that much, so small wobbles don't read as full dilation.

Pupils widen and narrow together, so VRFT checks each eye against the other. Looking aside can turn an eye away from its camera, and its lashes can then pass for a small pupil. So a width far from what that eye has shown over the last minute (below 0.6 or above 1.7 times its median) counts only when the other eye has changed the same way. A sudden change of more than 30% must also show in both eyes and hold steady for 0.6 seconds. A width held back leaves that eye's value where it was. After two seconds of that, the eye starts over from its next width, so long as it is within that band.

After a measurement VRFT keeps it for 1.5 seconds. An eye without one then takes the other eye's value, and with neither the tracking module's pupils are sent again.

The desktop app's Eyes page shows the dilation and each pupil under **Measure pupil size**, whose switch turns it on and off.

## Limitations

- Convergence quality depends on Meta's per-eye branch, which is noisier than the blended output. The filter trades a little latency for stability.
- The patched model only affects gaze. Blink and face values are unchanged.
- The stream has no authentication or encryption. Use it on a trusted network.
