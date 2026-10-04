# Quest Pro wireless camera stream

This Android 14 APK relays the Quest Pro's two lower-face cameras to a PC on the local network over Wi-Fi. It serves the existing `QPLIVE3` protocol as two 400 × 400 grayscale views side by side. Android NSD advertises `_vrftcam._tcp.local.`; the Rust daemon discovers it, runs the enhanced tongue model when set up, sends tongue expressions through its VRChat output, and serves a local browser preview. If the feed or inference stops, VRFT uses its tracking module's tongue values.

Version 0.2 adds, on the same TCP stream, optional independent per-eye gaze (`QPGAZE1` raw vectors), a status object (`QPSTAT1`), low-rate eye-camera preview snapshots, and selectable camera frame rate. Per-eye gaze is on by default; all of it is configured in the app; see [Stream settings](#stream-settings) and [Independent eye gaze](#independent-eye-gaze-experimental).

## Status and plan

The connected development headset reports Android 14, API 34, arm64-v8a, build `51503870024400340`. The APK builds and installs on it. Camera capture requires a separate Magisk Superuser grant for this APK; the ADB Shell grant is independent.

On the connected Quest Pro, the foreground service obtained Magisk root, injected the native helper, started its loopback relay, and advertised the network stream through Android NSD. With Virtual Desktop in front, this PC discovered the service through mDNS and received live 800 × 400 frames. The browser preview reported advancing frame sequences. Long-duration and network-roaming behavior have not yet been tested.

When the relay connects but no frame arrives, the locally rebuilt helper writes `CAMERA_MAP_STATE` to `/data/local/tmp/questpro-live-v9.log` about every five seconds. Each `slot:counter/face` entry reports the provider's hardware frame counter and whether lower-face pixels pass the source check. The same log records `CAMERA_FRAMES_STALLED` / `CAMERA_FRAMES_RESUMED` when the provider's frames stop for over 2 s during capture, and `CAMERA_MAPS_CHANGED` when the provider rebuilt its camera buffers (see [Connection and recovery](#connection-and-recovery)). An already injected helper keeps running until a headset reboot, so updated native diagnostics need a reboot before they appear. Read the log with:

```powershell
..\..\..\android-tools\platform-tools\adb.exe shell su -c 'tail -n 20 /data/local/tmp/questpro-live-v9.log'
```

1. **Camera access:** the rooted native helper and loopback relay produce the live lower-face strip. Verified on this headset.
2. **Wireless service:** the Android foreground service exposes a TCP port on the local network and advertises it through NSD/mDNS. The Rust daemon also accepts a manual IP fallback. Verified on this LAN.
3. **PC preview:** the Rust daemon validates and receives `QPLIVE3` frames and serves a loopback-only browser preview. Verified with advancing live frames.
4. **Tongue tracking:** run the stereo gate and direction models on the PC and map the 12 tongue expressions to VRChat. This requires a licensed checkpoint or a model trained for the user.

## Build

This project targets Android API 34 with Android Gradle Plugin 8.5.2, Gradle 8.7, and **JDK 17**. Android's [AGP 8.5 compatibility table](https://developer.android.com/build/releases/agp-8-5-0-release-notes) specifies these versions. `setup-toolchain.ps1` downloads the JDK and SDK into the repo's gitignored `.local/toolchain` folder (add `-Ndk` for the NDK that `build-native.ps1` needs), and `build.ps1` uses them from there, falling back to a sibling `toolchain` folder beside the repo. Standard Android Studio/Gradle builds can use the supplied Gradle wrapper.

```powershell
.\setup-toolchain.ps1   # once
.\build.ps1
..\..\..\android-tools\platform-tools\adb.exe install -r .\app\build\outputs\apk\debug\app-debug.apk
..\..\..\android-tools\platform-tools\adb.exe shell am start -n io.github.matty.vrft.questprocamera/.MainActivity
```

The native helpers in `app/src/main/assets/native/` are committed prebuilt, so building the APK needs no NDK. After changing the C code in `native/`, rebuild them with `.\setup-toolchain.ps1 -Ndk` (once) and `.\build-native.ps1 -NdkRoot ..\..\.local\toolchain\android-sdk\ndk\26.1.10909125`, then build the APK.

`.\build.ps1 -Release` builds the release APK instead. Both are signed with the repository's dev key, so either installs over the other and over the published builds; see [Versions and Releases](../../docs/internals/releasing.md#signing-the-headset-app).

In the APK, press **Start streaming**. Grant **VRFT Quest Pro Camera** Superuser access in Magisk when prompted. Then open your streaming app (Virtual Desktop, Steam Link or another) and connect to the PC; an app using face tracking activates the face cameras while the foreground service runs behind it. VRFT doesn't need a tracking module for the cameras, eye gaze or tongue. Press **Stop streaming** in the APK to stop capture. The injected library stays loaded until the headset is rebooted, as with the reference implementation.

On the PC, launch the Rust daemon from its normal working directory:

```powershell
.\vrft_d.exe
```

Open the desktop app's Quest Pro pages to see cameras 2 and 3 and the latest frame sequence. (The daemon's browser preview is turned off for now: `BROWSER_PAGES` in `extensions/quest-pro/daemon/src/camera.rs`.) Use `.\vrft_d.exe --extensions-only` to test the feed without loading tracking modules or sending OSC. The browser endpoint binds only to `127.0.0.1`. If mDNS is unavailable on your Wi-Fi, set `$env:VRFT_QUEST_PRO_ADDR = '<headset-ip>:27274'` before starting the daemon. The camera stream has no authentication or encryption, so use a trusted local network.

For development over ADB, the activity accepts the same settings as its controls (`eye_enabled`, `camera_fps`, `eye_preview_fps`, `five_cameras`) and can press Start or Stop (`start_probe`, `stop_probe`). Quest reuses an open panel for a new `am start`, so the extras also work while the app is already open. A Quest screencap comes back empty, so debug builds also take `--ei capture_width 1280`, which draws the panel at that width to `/sdcard/Android/data/io.github.matty.vrft.questprocamera/files/panel.png`:

```powershell
$adb = '..\..\..\android-tools\platform-tools\adb.exe'
& $adb shell am start -n io.github.matty.vrft.questprocamera/.MainActivity --ez eye_enabled true --ei camera_fps 24 --ei eye_preview_fps 5 --ez start_probe true
& $adb shell am start -n io.github.matty.vrft.questprocamera/.MainActivity --ez stop_probe true
& $adb logcat -d -s VRFTCamera:I AndroidRuntime:E '*:S'
```

Over USB, `adb forward tcp:27274 tcp:27274` and `$env:VRFT_QUEST_PRO_ADDR = '127.0.0.1:27274'` connect the daemon without Wi-Fi discovery.

Meta's eye tracker only runs while the headset is worn and an app is consuming eye tracking (for example Virtual Desktop or Steam Link streaming with face tracking on). With the headset on a desk, the trace instance stays empty even though the pipeline is set up correctly: `per_cpu/cpu*/stats` under `/sys/kernel/tracing/instances/vrft_eye` shows `read events: 0`.

## Versions and stream protocol

The app is released on its own, as `apk-vYYYY.M.N`. Its `versionName` and `versionCode` come from those tags, so don't edit them in `build.gradle`. What VRFT checks is the stream protocol, `GazePackets.PROTOCOL`. The app advertises it in the mDNS `protocol` TXT record, next to `apk_version`, and sends it in the `QPSTAT1` status that opens every connection.

Bump the protocol only when an existing message changes. A new message type keeps it, provided its payload length is a little-endian `u32` at byte 12 with the payload from byte 16, as in `QPSTAT1`, because VRFT skips messages it doesn't know. [Versions and Releases](../../docs/internals/releasing.md) has the rules.

## Stream settings

The app's main screen has controls that are read from `SharedPreferences` and **applied at the next stream start** (never live). Changing **Independent eye gaze** during a stream restarts the stream to apply it; for the others, stop and start the stream:

- **Camera FPS** — 12, 15, 20, 24 (default), 30, 36. Passed to the relay as `--max-fps`.
- **Eye-camera snapshots** — Off, 1, 2, 5 (default) fps. Passed to the relay as `--eye-fps`. When on, the relay interleaves a low-rate eye-camera frame (cameras 0 + 1, `QPLIVE3` mask `0x03`, 800 × 400) into the stream, cut from the same stabilized sensor frame as the mouth frame it follows and carrying that frame's sequence and timestamp. The PC measures pupil size from them and shows them in the eye preview; it never feeds mask `0x03` frames to tongue inference or capture. At 5 fps they add about 1.6 MB/s to the stream. Before 5 became the default, the rate was saved under another key with a default of 2; a saved 2 moves to 5, any other saved rate carries over.
- **Independent eye gaze** — a checkbox, default **on**. On firmware it doesn't support, the stream carries on without it. See below.
- **All five cameras** — a switch, default **off**. Streams the whole sensor strip, including the brow camera, to a VRFT that reads it. See [Five-camera stream](#five-camera-stream).

The relay is launched as:

```
questpro-camera-relay-v9 --mode mouth --max-fps <camera-fps> --eye-fps <eye-preview-fps> --injector <injector> --streamer <streamer>
```

or, with **All five cameras** on:

```
questpro-camera-relay-v9 --mode all --eye-fps 0 --max-fps <camera-fps> --injector <injector> --streamer <streamer>
```

`--eye-fps` accepts `0`–`10` (0 disables snapshots) and is only valid alongside `--mode mouth` or `--mode face` (a mode that does not already carry cameras 0 + 1). `--injector` and `--streamer` go together; with them the relay can load the streamer again (see below).

## Five-camera stream

The headset's sensor strip is five 400 × 400 views side by side, 2000 × 400 in all: the eyes (cameras 0 and 1), the mouth (2 and 3, `cam07_left_mouth` and `cam08_right_mouth` in the factory calibration) and the brow (4, between the eyes). The mouth stream sends cameras 2 and 3 only (`QPLIVE3` mask `0x0c`), plus eye snapshots. With **All five cameras** on (or `--ez five_cameras true` over ADB, from the next stream start), the relay runs in its existing `all` mode and sends whole strips (`QPLIVE3` mask `0x1f`, width 2000); the relay itself is unchanged, so the native helpers need no rebuild.

| Stream | Bytes per frame | At 24 fps | At 36 fps |
| --- | --- | --- | --- |
| Mouth (default), with 5 fps eye snapshots | 320 KB | 7.7 MB/s + 1.6 MB/s | 11.5 MB/s + 1.6 MB/s |
| All five cameras | 800 KB | 19.2 MB/s (about 154 Mbit/s) | 28.8 MB/s (about 230 Mbit/s) |

The five-camera stream needs a good 5 GHz or 6 GHz link alongside Virtual Desktop or Steam Link, or USB (`adb forward tcp:27274 tcp:27274`). Lower **Camera FPS** if frames are skipped.

**Who gets which frames.** A VRFT that reads five-camera frames sends a `QPHELO1` message as it connects: the magic `QPHELO1\0`, a `u32` version (1) at byte 8, the payload's length as a `u32` at byte 12, then JSON such as `{"camera_masks":[12,3,31]}`. It is the only thing the PC ever sends. Only a connection whose hello lists `31` (`0x1f`) gets the strips as they are. Every other connection gets what the app sends without the five-camera stream: the mouth pair cut from each strip (mask `0x0c`, 800 × 400) and, at the **Eye-camera snapshots** rate, the eye pair cut from the same strip (mask `0x03`), with the strip's sequence and timestamp. The `QPSTAT1` status says `five_cameras` (the setting) and `camera_mask` (`31` or `12`, what this connection gets), and is sent again when a hello switches the connection to five cameras.

**Compatibility.** No existing message changed, so the stream protocol stays 3 (`GazePackets.PROTOCOL`, and `PROTOCOLS` in VRFT):

- A VRFT from before the five-camera stream never sends a hello, so with the setting on it still gets the mouth stream and eye snapshots, exactly as before. Were it sent a 2000 × 400 frame, it would reject the header (`Invalid QPLIVE3 frame header`), drop the connection and keep reconnecting with backoff without ever tracking, which is why the strips go only to a PC that asks and the setting stays off by default.
- A headset app from before the five-camera stream never reads from the PC; the 24-byte hello of a newer VRFT sits unread in its socket, and VRFT gets the mouth stream as before.
- A VRFT that gets strips cuts the mouth pair from each for the tongue model, the preview and recordings, cuts the eye pair at the headset's snapshot rate for the pupils, and shows the brow camera on the desktop app's **Mouth** page. Its recordings keep the whole strip and list the cameras in `metadata.json` (see the tongue training guide).

## Connection and recovery

- **The PC only says hello.** Apart from the `QPHELO1` above, the PC sends nothing, so end-of-stream on its socket means it has gone.
- **One PC at a time, newest wins.** A new connection on port 27274 replaces the current one, so a PC that vanished without closing its socket (sleep, Wi-Fi drop, daemon crash) never blocks the next session. The app also ends a connection when the PC closes it, when TCP keepalive gets no answer (probes after 10 s idle, every 3 s, 3 tries), when any write to it (frame, gaze or status) fails, and when a write takes longer than 3 s.
- **Capture lease.** The relay renews the streamer's capture lease only while the app is connected to it. While no frames flow it checks the connection every 200 ms, so capture stops as soon as the app lets go instead of when the next frame fails to send.
- **Relay supervision.** The app checks the relay process every 5 s and restarts it after two checks in a row find it stopped. After five restarts in one stream it gives up and shows the error; press Start again.
- **Streamer re-injection.** When frames have stopped for 3 s, the relay checks every 5 s whether the streamer is still loaded in `vendor.oculus.hardware.sensors@1.0-service` (for example after the provider restarted) and runs the injector again if not (`STREAMER_MISSING`, `STREAMER_REINJECTED` in logcat under `Relay:`). An injection briefly pauses the provider, so after a failed attempt it waits 60 s before the next. A hung injector is killed after 30 s.
- **Camera buffers.** The streamer identifies the provider's nine camera buffers by address and dmabuf inode. It checks them when capture starts and about once a second during capture, and finds them again if they changed, rather than reading addresses the provider may have unmapped, which would crash the provider and every tracking sensor with it. This narrows that window; it cannot close it.
- **mDNS.** The service holds a Wi-Fi multicast lock while it runs, so the PC's mDNS queries reach the headset while Wi-Fi is idle, and retries a failed NSD registration every 30 s.

## Independent eye gaze (experimental)

When the **Independent eye gaze** checkbox is on, the service — before it injects the camera helper — runs a headset-local eye pipeline that exposes raw per-eye visual-axis vectors and streams them to the PC as `QPGAZE1` messages on the same TCP connection. The app does **not** convert to angles, calibrate, filter, or swap eyes; it only pairs the tag 0 / tag 1 detector events. The PC daemon does the angle/calibration/filtering work.

The UI warns that this **temporarily replaces Meta's eye model while streaming and restores the stock model on Stop**.

### What it changes on the headset, and how it is restored

While active, the pipeline:

1. Reads the stock eye model `/odm/etc/eyetracking/runtime/models/Seacliff_V1_5/fbnet/int8/experimental/bolt/bolt.ptl`, patches it **in memory** (a byte-length-preserving edit that redirects the public gaze reshape from the binocular blend node 50 to the local per-eye node 18, with the member CRCs fixed up), and writes the patch to `/data/local/tmp/vrft-camera/bolt-independent-axes.ptl` (`root:root`, `0644`, SELinux `u:object_r:vendor_configs_file:s0`).
2. Records the stock value of `persist.device_config.oculus_shared_vision.oculus_eyetracking_enable_experimental_model` and a `restore_pending` flag in `SharedPreferences` **before** mounting, then bind-mounts the patched file over the stock path, sets the property to `true`, and restarts `trackingservice`.
3. Adds a uprobe on `/odm/lib64/libtrackingengines.so` in its own tracefs instance `vrft_eye` (event group `vrft_eye`, event `detector_output`) and reads `trace_pipe` for the per-eye vectors. It never touches any other tool's tracefs instance.

**Restore** reverses exactly that: it stops the trace (disable, remove the uprobe, kill our `cat`, free and remove the `vrft_eye` instance), stops `trackingservice`, sets the property back to its recorded value, unmounts the bind mount, restarts `trackingservice`, removes the patched file, and clears the flag. Restore runs on the **Stop** button, on service destroy (background thread), and automatically at the next stream start if the `restore_pending` flag is still set (crash recovery). Nothing about the model change survives a completed restore or a reboot (a bind mount does not persist across reboot).

### mount-master requirement

Every eye-pipeline root command runs under Magisk `su --mount-master -c '…'` so the bind mount lives in the **global** mount namespace where `trackingservice` can see it. If `su --mount-master -c id` does not report `uid=0`, the eye pipeline disables itself with a clear error — it never falls back to plain `su` for mount operations. Camera streaming continues regardless. After the service restart the pipeline checks that the mount is visible to `trackingservice` (via `init.svc_debug_pid.trackingservice`, falling back to `pidof trackingservice`). If the service cannot see it, gaze still streams but without the patched-model flag, and the status says convergence will not show; the mount is still restored on Stop. If the pid can't be found, the status carries a warning that visibility is unverified.

Start, stop and crash recovery are serialized: pressing **Stop** while the pipeline is still starting makes it abort at its next step and restore, rather than racing it. Root commands keep waiting for completion even if the service thread is interrupted, so the restore steps always run in order.

### Supported engine builds

The uprobe offset is firmware-specific, so the engine is matched by `stat -c %s` of `libtrackingengines.so`:

| Profile | Build | Engine size | Notes |
| --- | --- | --- | --- |
| 1 | `51483620027600340` | 47,724,232 | offset `0xB63FE4` |
| 2 | `51503870024400340` | 47,418,280 | offset `0xB1F3E8`; **also requires** sha256 `0fb6f54a3e190bec791d757ea18d32a8ecc1af4a861992d04b1703c93293cd03` |
| 3 | `51503870024400340` | 47,418,280 | **opt-in** alternative to profile 2, same sha256; offset `0x9AF054` |

Any other size sets the eye state to `error` with the message `Unsupported tracking-engine build (<size>)`, and a hash mismatch with `Tracking-engine hash mismatch for build <size>`; camera streaming carries on untouched.

Profile 3 probes the same engine where it publishes the eye data, and reads both eyes' visual axes (`x23+0x300` and `x23+0x870`) from one trace line. It comes from the Qpro-Enhanced-FT-GNimrodG fork, which took it from QFTPlus; that fork says the detector outputs don't map the same way on this build. The event keeps the name `detector_output`, so the restore and the manual restore below are the same for every profile. The parser treats elements 0 and 1 as tags 0 and 1, and drops a line that repeats the previous one exactly.

The APK uses profile 2 on this build unless the alternative probe is chosen over ADB. It has no on-screen control, and it applies from the next stream start:

```powershell
& $adb shell am start -n io.github.matty.vrft.questprocamera/.MainActivity --ez eye_alt_probe true
# back to profile 2:
& $adb shell am start -n io.github.matty.vrft.questprocamera/.MainActivity --ez eye_alt_probe false
```

On a build without an alternative (profile 1) the setting is ignored. The running profile shows as **Eye model setup** on the desktop app's Eyes page, and in `QPSTAT1`.

For every profile the app marks a vector invalid (`QPGAZE1` flag bits 0 and 1) unless it is finite with a squared length between 0.25 and 2.25, and VRFT drops such samples.

**Validation note:** profile 1 (`51483620027600340`) is the build the fork established convergence on. **Neither profile 2 nor profile 3 has been confirmed on hardware.** For profile 2, the upstream Qpro-Enhanced-FT README says eye convergence was only tested on `51483620027600340` and may not work on newer firmware, and the Fwooffy fork's release notes say the newer build was probed but a complete convergence session was not established. Profile 3 comes without any recorded validation. To compare them, run a stream with each and check in the Eyes page that samples arrive, that closing one eye doesn't move the other, and that looking near makes the eyes converge. Trying a profile costs the usual tracking-service restarts at stream start and stop. This headset is not currently connected, so none of the on-device eye path in this release has been exercised on hardware.

### Reading logs

All service and eye-pipeline messages are tagged `VRFTCamera`. Relay output (including the `RELAY_LISTENING … eye_fps=<n>` line) is logged with a `Relay:` prefix. The eye state also appears in the app's on-screen status and in each `QPSTAT1` message.

```powershell
..\..\..\android-tools\platform-tools\adb.exe logcat -d -s VRFTCamera:I AndroidRuntime:E '*:S'
..\..\..\android-tools\platform-tools\adb.exe shell su -c 'tail -n 20 /data/local/tmp/questpro-live-v9.log'
```

### Manual restore (emergency)

If the app is force-stopped or uninstalled while the patched model is mounted (so its automatic restore never ran), restore the stock eye model by hand with a **mount-master** root shell. Read back the original property value first if you know it; otherwise `false` is the stock default:

```sh
adb shell su --mount-master -c 'stop trackingservice'
adb shell su --mount-master -c 'setprop persist.device_config.oculus_shared_vision.oculus_eyetracking_enable_experimental_model false'
adb shell su --mount-master -c "umount '/odm/etc/eyetracking/runtime/models/Seacliff_V1_5/fbnet/int8/experimental/bolt/bolt.ptl'"
adb shell su --mount-master -c 'start trackingservice'
adb shell su --mount-master -c 'rm -f /data/local/tmp/vrft-camera/bolt-independent-axes.ptl'
# Remove the trace instance if it was left behind:
adb shell su --mount-master -c "echo 0 > /sys/kernel/tracing/instances/vrft_eye/tracing_on; echo '-:vrft_eye/detector_output' >> /sys/kernel/tracing/uprobe_events; echo 1 > /sys/kernel/tracing/instances/vrft_eye/free_buffer; rmdir /sys/kernel/tracing/instances/vrft_eye"
```

Rebooting the headset also clears the bind mount and any tracefs instance.

## Native code provenance

The native arm64 assets originate from [Qpro-Enhanced-FT v0.1.10](https://github.com/n0tmast3r/Qpro-Enhanced-FT/releases/tag/v0.1.10). Release ZIP SHA-256: `db40f4b8331a50ca6c2ec37372f1ab4b44cfbe6d21e09ca04244aaaa339cb18f`. Their editable C sources are copied into `native/` from upstream commit `df52b87d282324b84ba172ae3c6354a6bda2aef6`; `build-native.ps1` rebuilds the assets with an Android NDK. The streamer asset was rebuilt locally with the camera-map diagnostic above. `native/relay.c` additionally carries the local `--eye-fps` option (interleaved mask `0x03` eye snapshots); rebuilding left the injector and streamer assets byte-identical to the previous local assets. The upstream MIT license is copied to `REFERENCE_LICENSE.txt`.

Version 9 of the streamer and relay ports robustness fixes from the MIT-licensed [GNimrodG fork](https://github.com/GNimrodG/Qpro-Enhanced-FT) of Qpro-Enhanced-FT: camera-buffer revalidation and stall logging in the streamer, and the idle disconnect check and streamer re-injection in the relay (see [Connection and recovery](#connection-and-recovery)). The streamer was renamed from v8 because an injected library stays loaded until the headset reboots and the injector skips a name that is already loaded: a v8 streamer injected before the update stays in the provider, idle, until the next reboot, while the v9 one is injected beside it. The relay's shared-memory file (`questpro-live-v9-shared.bin`), PID file and the streamer log (`questpro-live-v9.log`) moved to v9 with it, and `relay --stop` still stops a v8 relay. The frame format and the relay protocol are unchanged. The injector is unchanged.

The app copies those assets into its private storage, then asks Magisk to install and execute them from `/data/local/tmp/vrft-camera`. The injector loads the camera helper into the Quest Pro sensor provider. The native relay stays on headset loopback port 27273; the APK forwards complete frames on LAN port 27274. This is a compatibility path for the observed headset build, not a firmware-independent camera API.
