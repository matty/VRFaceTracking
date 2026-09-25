# AGENTS.md

This file provides guidance to coding agents working in this repository.

## Workspace layout

Scripts here resolve sibling folders by relative path, so the repo must sit in a workspace next to them:

- `../toolchain/`: JDK 17, Android SDK (platform 34, build-tools 34.0.0, NDK `26.1.10909125`), Gradle 8.7. `android-user-home/debug.keystore` signs the debug APK; if it changes, `adb install -r` fails until the app is uninstalled.
- `../android-tools/platform-tools/adb.exe`: adb is not on PATH, so call it by this path.
- `../Qpro-Enhanced-FT/`: clone of the upstream n0tmast3r project that the Quest Pro code is ported from. **Read-only reference**: never edit, build or commit there.

## Checks

A PR must pass what CI (`.github/workflows/ci.yml`) runs:

```
cargo build
cargo build -p vrft-vd-module
cargo test
cargo clippy -- -D warnings
cargo fmt -- --check
```

CI does not run these, so run them yourself when you touch the area:

- Android pure-Java classes: `android/questpro-camera/tests/run-tests.ps1`
- Tongue Python: `.local/tongue-python/Scripts/python.exe -m unittest -v test_tongue_training` (the venv comes from `./setup-quest-pro-tongue.ps1`, Python 3.12; system `python` is 3.14 and won't do)
- Preview JS: `node --check crates/daemon/src/quest_pro_training.js`

## Running the daemon

- Run from the repo root (`cargo run -p vrft-daemon`). `config.json`, `plugins/` and `runtime/VrcftRuntime.exe` resolve against the working directory.
- `vrft_d --camera-preview-only` serves the Quest Pro preview at http://127.0.0.1:27275/ without loading modules or sending OSC.
- Logging: `RUST_LOG=info,vrft_d=debug`.
- Env overrides: `VRFT_QUEST_PRO_ADDR`, `VRFT_TONGUE_PYTHON`, `VRFT_TONGUE_MODEL_DIR`, `VRFT_TONGUE_DEVICE`, `VRFT_EYE_CALIBRATION`.
- Over USB: `adb forward tcp:27274 tcp:27274`, then `VRFT_QUEST_PRO_ADDR=127.0.0.1:27274`.
- Ports:
  - 27273: relay, on the headset's loopback
  - 27274: APK stream on the LAN
  - 27275: PC preview
  - 9000/9001: OSC/OSCQuery

  Upstream Qpro tools also use 27274–27276, so don't run them alongside.

## Desktop app (`crates/gui`)

- `cargo run -p vrft-gui` opens the app, binary `vrft_gui`. It tracks nothing itself: it reads the daemon's API on 127.0.0.1:27275 (`/status`, `/frame`, `/eye-frame`, `/settings`, `/eye/recenter`).
- On startup the app starts the daemon unless a `vrft_d.exe` is already running, and whenever the daemon isn't answering it offers **Start VRFT**. Both run `vrft_d.exe` from the app's own folder (so build `vrft-daemon` too), with no console window and its output in `vrft_d.log` beside it. The daemon keeps running if the app closes. **Stop** sends `POST /shutdown`, which needs a JSON body so a web page can't trigger it.
- The **Headset** page runs adb (`adb.rs`), only while it shows. It uses `VRFT_ADB` if set, then its own copy in `platform-tools/` beside the app, then an adb whose server is already running, so it doesn't restart SideQuest's or the Developer Hub's, then `../android-tools/` for a development build, PATH and the usual install folders. **Download to the VRFT folder** fetches Google's Platform-Tools, unpacks them in a staging folder and keeps only `adb.exe`, its two DLLs and `NOTICE.txt` in `platform-tools/`. It installs `headset-app/vrft-questpro-camera-<version>.apk` from a release, or the newest local Gradle build in a checkout (`headset_app.rs`). Before replacing the app it stops a running stream and waits for the `stop-relay` and `stop-eye` threads in `CameraStreamService.onDestroy` to finish, so the eye model is restored before Android kills the app.
- Built on gpui-kit, pinned to `=0.6.6` because `Cargo.lock` isn't committed and GPUI's API changes between snapshots. Read the published crate in `~/.cargo/registry/src/*/gpui-component-0.6.6`, not the GitHub repo: its HEAD API differs, for example `gpui_kit::open_window` doesn't exist in 0.6.6.
- Measure CPU at least 20 s after launch; GPUI compiles its shaders at startup. gpui-component's `Progress` animates every value change, so don't feed it live values; use `widgets::Meter`.

## Android app (`android/questpro-camera`)

- Build with `build.ps1`, which sets JAVA_HOME, ANDROID_HOME and GRADLE_USER_HOME from `../toolchain`. Don't call gradle directly. Output: `app/build/outputs/apk/debug/app-debug.apk`. `build.ps1 -Release` builds the release APK, unsigned without the release key. A release-signed APK won't install over the debug-signed one, or the reverse, without an uninstall.
- The arm64 helpers in `app/src/main/assets/native/` are prebuilt and committed. After you edit `native/*.c`, rebuild them with `build-native.ps1 -NdkRoot ..\..\..\toolchain\android-sdk\ndk\26.1.10909125`. An injected helper stays loaded until the headset reboots.
- Read `android/questpro-camera/README.md` before you change the eye pipeline. It covers engine profiles, the bind-mount model patch, and the restore sequence, including the emergency restore.

## Headset

- **Allowed without asking:**
  - read-only adb commands
  - `adb install -r`
  - logcat (`-s VRFTCamera`)
  - `am start -n io.github.matty.vrft.questprocamera/.MainActivity` with the extras `eye_enabled`, `camera_fps`, `eye_preview_fps`, `start_probe` and `stop_probe`

  With `eye_enabled`, `start_probe` makes the app patch Meta's eye model through its own root grant, and `stop_probe` restores the stock model.
- **Ask before any `su` command.** Eye-pipeline root commands must use `su --mount-master`. Plain `su` mounts into the wrong namespace.
- Add firmware or engine-profile support only with a hardware test and an explicit build fingerprint.
- In Git Bash, prefix adb commands that take device paths with `MSYS_NO_PATHCONV=1`.

## Never commit

- Camera captures or anything under `.local/`. These are biometric data.
- Extracted Meta binaries or models (`bolt*.ptl`), and `.pt` checkpoints.
- Personal eye calibration. `models/quest-pro/qpro-independent-visual-axis-v2.json` is a cleaned copy with a `provenance` field; keep it free of personal paths.

## Git

- Branch off `main`, then open a PR with `gh`. Use conventional-commit subjects (`feat:`, `fix:`, `refactor:`, `chore:`). Release notes are built from them.
- Every push to `main` runs `release.yml`, which rebuilds and republishes the `dev` prerelease.
- `release.yml` copies named Quest Pro files (Python scripts, guides, calibration JSON, license) into the package. Update it when you rename or add any of them.

## Releases and versions

Read `docs/internals/releasing.md` before you touch versions, release workflows or the headset stream format.

- VRFT (`v2026.9.0`) and the headset app (`apk-v2026.9.0`) are released separately, with CalVer `YYYY.M.N`. Only a release makes a tag: run `release.yml` or `release-apk.yml` by hand. Never bump a version in `Cargo.toml` or `build.gradle`; builds stamp it from the tags.
- Whether a daemon and a headset app work together is decided by the stream protocol: `PROTOCOLS` in `crates/daemon/src/quest_pro_camera.rs` and `GazePackets.PROTOCOL` in the app. Bump it only when an existing message changes. A new message type must put its payload length at byte 12, as `QPSTAT1` does, so older daemons can skip it.
- One headset app release is meant to serve many VRFT releases. When the protocol changes, widen `PROTOCOLS` so the daemon still reads the old one. Never raise its lower bound without being asked: that forces every user to update the headset app.

## Gotchas

- The C# projects under `dotnet/` are built only in CI. Locally the daemon uses the committed `dotnet/publish/VrcftRuntime.exe`, and this machine has only .NET runtimes, no SDK.
- `VRCFT.sln` has stale `vrft_d\dotnet\...` paths.
- Stale docs: the README Quick Start's `run_debug.ps1` no longer exists, and `docs/internals/architecture.md` describes an old `vrft_d/` layout. When docs and code disagree, trust the code.
