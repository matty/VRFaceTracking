# AGENTS.md

This file provides guidance to coding agents working in this repository.

## Workspace layout

Scripts here resolve sibling folders by relative path, so the repo must sit in a workspace next to them:

- `.local/toolchain/` (gitignored, created by `android/questpro-camera/setup-toolchain.ps1`): JDK 17, Android SDK (platform 34, build-tools 34.0.0, platform-tools, and NDK `26.1.10909125` with `-Ndk`), Gradle 8.7 via the wrapper. The build scripts fall back to a sibling `../toolchain/`. Every APK build is signed with the committed `android/questpro-camera/dev.keystore`, not the toolchain's debug keystore.
- `.local/toolchain/android-sdk/platform-tools/adb.exe` (or `../android-tools/platform-tools/adb.exe`): adb is not on PATH, so call it by this path.
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
- Preview JS: `node --check extensions/quest-pro/daemon/src/training.js`

## Running the daemon

- Run from the repo root (`cargo run -p vrft-daemon`). `config.json`, `plugins/` and `runtime/VrcftRuntime.exe` resolve against the working directory. A development build started inside `target/` (such as by `vrft_app`) switches to the repo root itself.
- A development build copies the modules under `modules/` that cargo built beside it into `plugins/` when it starts, replacing any that changed, so rebuild them (`cargo build`) and restart the daemon rather than copying DLLs by hand.
- `vrft_d --extensions-only` runs the extensions (the Quest Pro preview at http://127.0.0.1:27275/) without loading modules or sending OSC. `--camera-preview-only` is its old name and still works.
- Only one daemon runs at a time: each holds the named mutex `vrft_protocol::DAEMON_INSTANCE`, and a second exits with an error. `vrft_d train-tongue` runs don't hold it, so they are never mistaken for the daemon.
- Lifetime (`crates/daemon/src/lifetime.rs`): the daemon joins a kill-on-close job, so `VrcftRuntime.exe` and tongue training end with it even when it's force-ended. Once asked to stop it ends itself after 4 s. `--owner-pid <pid>` (the app passes its own) stops it when that process exits.
- Tongue training runs below normal priority while `VRChat.exe` runs, checked every 2 s (`extensions/quest-pro/daemon/src/priority.rs`).
- Threads are named (`output`, `local-api`, `quest-pro-tongue`, ...), so Process Explorer's Threads tab shows which one is busy.
- Logging: `RUST_LOG=info,vrft_d=debug`.
- Env overrides: `VRFT_QUEST_PRO_ADDR`, `VRFT_TONGUE_MODEL_DIR`, `VRFT_FACE_MODEL` (a universal face checkpoint), `VRFT_TONGUE_DEVICE`, `VRFT_EYE_CALIBRATION`.
- Tongue model inference runs on ONNX Runtime when its library is found (`crates/tongue/src/onnx/`), else on Burn:
  - The library is `onnxruntime.dll` beside `vrft_d.exe` (releases ship it with `DirectML.dll`), `.local/onnxruntime/` for a development build (`tools/onnxruntime/fetch.ps1`), or `VRFT_ONNXRUNTIME`.
  - The graphs are built from the safetensors weights at load. On the CPU a model calibrates on its first 48 frames, switches to int8 in the background and saves `<checkpoint>.int8.onnx` beside the checkpoint.
  - The universal face model goes int8 except its tongue tail; the stereo pair stays float, because int8 moves its directions by a few hundredths.
  - `VRFT_INFERENCE=burn` uses Burn; `VRFT_ONNX_THREADS` sets the CPU threads per model; `VRFT_ONNX_INT8=0` keeps every model float, `all` quantizes every model whole.
  - The tongue device setting `auto` runs ONNX Runtime on the CPU, so the game keeps the GPU; `gpu` uses DirectML.
- Over USB: `adb forward tcp:27274 tcp:27274`, then `VRFT_QUEST_PRO_ADDR=127.0.0.1:27274`.
- Ports:
  - 27273: relay, on the headset's loopback
  - 27274: APK stream on the LAN
  - 27275: the daemon's local API, and the Quest Pro preview
  - 9000/9001: OSC/OSCQuery

  Upstream Qpro tools also use 27274–27276, so don't run them alongside.

## Extensions (`crates/extension`, `crates/gui-core`, `crates/protocol`, `extensions/`)

- Optional support such as Quest Pro is an extension: a daemon crate implementing `vrft_extension::DaemonExtension`, an app crate implementing `vrft_gui_core::extension::GuiExtension`, and a serde-only protocol crate both use for the extension's id, routes and wire types (`extensions/quest-pro/{daemon,gui,protocol}`). Both are compiled in, each behind a `quest-pro` Cargo feature on `vrft-daemon` and `vrft-gui` (on by default), and registered in `crates/daemon/src/extensions.rs` and `crates/gui/src/extensions.rs`.
- At runtime `config.json`'s `extensions.<id>.enabled` turns one on or off (on when missing). The app's Home has an **Extensions** section that sets it through `POST /extensions/enabled` and restarts the daemon. The app shows an extension's pages only while the daemon reports it running.
- Every HTTP boundary has one typed definition both sides compile against. `vrft-protocol` (`crates/protocol`) is the daemon's own API: `/status`, `DaemonReport`, `RunMode`, the shutdown and enable requests. `vrft-quest-pro-protocol` is Quest Pro's routes and types, including the trainer's `request.json`, `progress.json` and `report.json`, which `vrft-tongue` writes and reads with the same types. Change a field there, not in either half. Recording files (`metadata.json`, `samples.jsonl`) are still untyped JSON.
- The daemon's local API (`crates/daemon/src/api.rs`) serves `/status` (`{daemon, extensions: {<id>: ...}}`), `/shutdown`, `/extensions/enabled` and `/config` (the app's Settings page: module, output, smoothing; checked against `MutationConfig` and written by `crates/daemon/src/config_file.rs`, keeping unknown keys), and each extension's routes under `/ext/<id>`. `/` redirects to the first extension page, so http://127.0.0.1:27275/ still opens the Quest Pro preview.
- An extension's frame hook runs on the output thread: `before_mutation` sees the module's values before smoothing, `after_mutation` changes what's sent. Don't block in either.
- Build the core without any extension (no Burn, no GPUI extension pages): `cargo build -p vrft-daemon -p vrft-gui --no-default-features`.

## Desktop app (`crates/gui`)

- `cargo run -p vrft-gui` opens the app, binary `vrft_app`. It tracks nothing itself: it reads the daemon's API on 127.0.0.1:27275 (`/status`, and the Quest Pro routes under `/ext/quest-pro/`: `frame`, `eye-frame`, `settings`, `eye/recenter`, `capture/*`, `training/*`).
- Shared pieces (daemon client and polled state, launcher, navigation, readings, widgets) live in `crates/gui-core`; the Quest Pro pages in `extensions/quest-pro/gui`. The app's own pages are Home and Settings (`crates/gui/src/settings.rs`). Settings and Tracking settings save as they change but apply when VRFT restarts, so a banner at the top offers **Restart now** or **Revert** (back to what VRFT is running with).
- On startup the app starts the daemon unless one is already running (it holds `DAEMON_INSTANCE`, or answers `/status`), and whenever the daemon isn't answering it offers **Start VRFT**. Both run `vrft_d.exe --owner-pid <app pid>` from the app's own folder (so build `vrft-daemon` too), with no console window and its output in `vrft_d.log` beside it. Closing the app stops a daemon it started (`POST /shutdown`, then ends it after 5 s), and the daemon also stops by itself if the app crashes; a `vrft_d` that was already running is left alone. **Stop** sends `POST /shutdown`, which needs a JSON body so a web page can't trigger it.
- The launcher (`crates/gui-core/src/launcher.rs`) holds the daemon it uses by process handle: the one it started, or the one answering, by the `pid` in `/status`. Stop waits for that process to exit, and **Force stop** ends only that process. Never find or end the daemon by executable name: `vrft_d.exe` is also the name of tongue training runs and of daemons the app isn't using.
- The Quest Pro **Headset** page runs adb (`extensions/quest-pro/gui/src/adb.rs`), only while it shows. It uses `VRFT_ADB` if set, then an adb whose server is already running, so it doesn't restart SideQuest's or the Developer Hub's, then its own copy in `platform-tools/` beside the app, then `../android-tools/` for a development build, PATH and the usual install folders. **Download to the VRFT folder** fetches Google's Platform-Tools, unpacks them in a staging folder and keeps only `adb.exe`, its two DLLs and `NOTICE.txt` in `platform-tools/`. It installs `headset-app/vrft-questpro-camera-<version>.apk` from a release, or the newest local Gradle build in a checkout (`headset_app.rs`). Before replacing the app it stops a running stream and waits for the `stop-relay` and `stop-eye` threads in `CameraStreamService.onDestroy` to finish, so the eye model is restored before Android kills the app.
- Built on gpui-kit, pinned to `=0.6.6` because `Cargo.lock` isn't committed and GPUI's API changes between snapshots. Read the published crate in `~/.cargo/registry/src/*/gpui-component-0.6.6`, not the GitHub repo: its HEAD API differs, for example `gpui_kit::open_window` doesn't exist in 0.6.6.
- Measure CPU at least 20 s after launch; GPUI compiles its shaders at startup. gpui-component's `Progress` animates every value change, so don't feed it live values; use `widgets::Meter`.

## Android app (`android/questpro-camera`)

- Build with `build.ps1`, which sets JAVA_HOME, ANDROID_HOME and GRADLE_USER_HOME from `.local/toolchain` (else `../toolchain`). Don't call gradle directly. Output: `app/build/outputs/apk/debug/app-debug.apk`. `build.ps1 -Release` builds the release APK. Both, and the CI builds, share the dev key, so they install over each other; an app still signed with an older key needs one uninstall.
- The arm64 helpers in `app/src/main/assets/native/` are prebuilt and committed. After you edit `native/*.c`, rebuild them with `build-native.ps1 -NdkRoot ..\..\.local\toolchain\android-sdk\ndk\26.1.10909125` (install the NDK with `setup-toolchain.ps1 -Ndk`). An injected helper stays loaded until the headset reboots.
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
- `release.yml` copies named Quest Pro files (guides, calibration JSON, license) into the package. Update it when you rename or add any of them.

## Releases and versions

Read `docs/internals/releasing.md` before you touch versions, release workflows or the headset stream format.

- VRFT (`v2026.9.0`) and the headset app (`apk-v2026.9.0`) are released separately, with CalVer `YYYY.M.N`. Only a release makes a tag: run `release.yml` or `release-apk.yml` by hand. Every other build is a dev build, `YYYY.M.N-dev.C`, named "(Dev)". Never bump a version in `Cargo.toml` or `build.gradle`; builds stamp it from the tags (`build-support/version.rs`, `build.gradle`, `.github/scripts/next-calver.sh`).
- The desktop app installs and updates itself with Velopack from GitHub releases: the `stable` channel from releases, `dev` from the rolling `dev` prerelease. An installed copy's `current/` is replaced on every update, so the daemon runs in `data/` beside it and anything it or the app writes must go there (`vrft_protocol::layout`, `vrft_gui_core::paths::data_dir`). Keep the `velopack` crate and `vpk` in `release.yml` on the same version.
- Whether a daemon and a headset app work together is decided by the stream protocol: `PROTOCOLS` in `extensions/quest-pro/daemon/src/camera.rs` and `GazePackets.PROTOCOL` in the app. Bump it only when an existing message changes. A new message type must put its payload length at byte 12, as `QPSTAT1` does, so older daemons can skip it.
- The PC's only message to the headset app is the `QPHELO1` it sends on connecting, listing the camera masks it reads. The app sends five-camera frames (mask `0x1f`, 2000 x 400) only to a daemon that lists them; a daemon from before them would drop the connection on one.
- One headset app release is meant to serve many VRFT releases. When the protocol changes, widen `PROTOCOLS` so the daemon still reads the old one. Never raise its lower bound without being asked: that forces every user to update the headset app.

## Gotchas

- .NET (VRCFT) modules: releases ship the host as `runtime/VrcftRuntime.exe`, which CI publishes. A development build has no `runtime/`, so it uses `dotnet/publish/VrcftRuntime.exe` (`dev_build::dotnet_host`), which is gitignored: publish it there yourself, and again after changing anything under `dotnet/`: `dotnet publish dotnet/VrcftRuntime/VrcftRuntime/VrcftRuntime.csproj -c Release -r win-x64 --self-contained true -p:PublishSingleFile=true -o dotnet/publish`.
- The host stands in for VRCFaceTracking's SDK: `VRCFaceTracking.SDK` (the module base class, which modules built for VRCFT 5.2 and later load from there) and `VRCFaceTracking.Core` (everything else, forwarding the base class for older modules). Modules bind to these by name and version, so keep both assembly versions, and `Microsoft.Extensions.Logging`, at least as new as any registry module asks for, and add whatever a new module uses that they lack. Some modules also expect libraries VRCFaceTracking ships without bringing their own, so the host carries `Newtonsoft.Json`. `proxy::tests::dotnet_host_runs_a_module` (ignored; its doc comment says how to run it) drives a real module through the host.
- `VRCFT.sln` has stale `vrft_d\dotnet\...` paths.
- Stale docs: the README Quick Start's `run_debug.ps1` no longer exists, and `docs/internals/architecture.md` describes an old `vrft_d/` layout. When docs and code disagree, trust the code.
