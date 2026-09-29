# Versions and Releases

VRFT and the Quest Pro headset app are released separately, each with its own version. A stream protocol number, not the version numbers, decides whether a given pair can work together.

The headset app changes much less often than VRFT, so one headset app release is expected to serve many VRFT releases. Each VRFT release names the newest headset app release that works with it, which is often one from months before.

## Versions

Both use calendar versions, `YYYY.M.N`: the year, the month without a leading zero, and a count of that component's releases in the month, starting at 0. So `2026.9.0` is the first release in September 2026, and `2026.9.1` the second.

Every other build is a dev build of the next release, `YYYY.M.N-dev.C`: `YYYY.M.N` is the version a release made today would get, and `C` counts the commits since that component's last release. So `2026.9.1-dev.14` is 14 commits after `2026.9.0`. A dev build sorts after every earlier dev build and before the release it leads to, which the updater and Android both rely on.

| Component | Tag | Where the version shows |
| --- | --- | --- |
| VRFT (`vrft_d`, `vrft_gui`) | `v2026.9.0` | `/status` → `daemon.version`, the navigation and **Settings → Updates** in the app, the first line of `vrft_d.log` |
| Headset app (`android/questpro-camera`) | `apk-v2026.9.0` | Android's `versionName`, and `apk_version` in the headset's status |

A dev build also says so in its name: the desktop app is **VRFaceTracking (Dev)** in its window, navigation and installer, and the headset app is **VRFT Quest Pro Camera (Dev)**.

A release tag is the only place a version is written down. Nothing in the repository is bumped by hand:

- `build-support/version.rs`, run by the build scripts of `vrft_d` and `vrft_gui`, stamps `VRFT_VERSION` into both. CI sets it in the environment, for a release and a dev build alike. Any other build works out its dev version from git and today's date. The `version` in `Cargo.toml` is not used. `.github/scripts/next-calver.sh <prefix> [dev]` does the same for CI.
- `android/questpro-camera/app/build.gradle` does the same with `apk-v` tags. CI passes `-PappVersion`. The `versionCode` is `YYYYMMNN00` (`2026090000`), and a dev build of `YYYY.M.N` takes one of the 100 codes just below it (`2026.9.1-dev.14` is `2026090014`; commits past 99 share the last one). So every build installs over the ones before it, and a release over the dev builds leading to it. A build without git keeps `2`.

## Stream protocol

The headset app and `vrft_d` talk over one TCP stream of messages, each starting with an 8-byte `QP…` magic. The protocol number describes that stream.

- The headset app declares it as `GazePackets.PROTOCOL`, advertises it in the mDNS `protocol` TXT record, and sends it in the `QPSTAT1` status that opens every connection.
- `vrft_d` lists the protocols it reads in `PROTOCOLS` in `extensions/quest-pro/daemon/src/camera.rs`. When the headset app's protocol is outside that range, the daemon doesn't use the stream. `/status` then carries `extensions.quest-pro.headset_mismatch`, and the app's Headset tile says whether to update VRFT or the headset app.
- Headset apps from before the `protocol` field advertised `version=3` and speak protocol 3.

**When to bump it:** only when an existing message changes in a way an older reader would misread. Adding a message type doesn't need a bump. `vrft_d` skips any `QP…` message it doesn't recognise, as long as the message carries its payload length as a little-endian `u32` at byte 12, with the payload starting at byte 16, as `QPSTAT1` does. Adding a field to the status JSON doesn't need one either.

**On a bump**, widen `PROTOCOLS` (for example to `3..=4`) and keep the daemon reading the old protocol too, so headset apps that users haven't updated keep working. Release VRFT before the headset app, so a new headset app never arrives before a VRFT that can read it. Raising the bottom of `PROTOCOLS` drops support for every headset app release that speaks the old protocol and makes all their users update the headset app. Do that on purpose and say so in the release notes.

`vrft_d` builds from before the skip rule drop the connection on an unknown message.

## Installer and updates

The desktop app is installed and updated with [Velopack](https://velopack.io). `release.yml` packs the std package plus the .NET host (`runtime/`) with `vpk pack` under the id `VRFaceTracking`, and uploads the setup, a portable zip, the full `.nupkg` and the channel's `releases.<channel>.json` with the release. `vpk` in the workflow and the `velopack` crate in `crates/gui/Cargo.toml` are pinned to the same version; change them together.

| Build | Channel | Release it lives in | Installer |
| --- | --- | --- | --- |
| Release | `stable` | `vYYYY.M.N` | `VRFaceTracking-stable-Setup.exe` |
| Dev build | `dev` | the rolling `dev` prerelease | `VRFaceTracking-dev-Setup.exe` |

- `crates/gui/src/updates.rs` picks the channel from the version (`-dev` means dev) and reads the repository's releases through Velopack's GitHub source, with prereleases only on the dev channel. It checks 10 seconds after opening and then every 6 hours, downloads what it finds, and offers **Restart to update** on **Settings → Updates**, with **Update ready** in the navigation. An update it downloaded is also installed the next time the app opens. A copy not installed by Velopack, such as a zip or a development build, shows that it can't update itself.
- Updates carry the full package, with no deltas: the `dev` release is recreated on every push, so it only ever holds its newest build.
- Velopack installs into `%LocalAppData%\VRFaceTracking\current\` and replaces that folder on every update. So an installed `vrft_d` runs in `data\` beside it (`vrft_protocol::layout`). As it starts, it copies what the package ships under `plugins/`, `runtime/` and `models/` into `data\` where it changed, and `config.json` only when there isn't one yet (`crates/daemon/src/installed.rs`). Modules and models added since, recordings, settings and `vrft_d.log` stay. The app starts the daemon there and downloads adb into `data\platform-tools\`; the bundled headset app is still read from `current\headset-app\`.
- Release and dev builds share the id, so an install switches channel by running the other channel's setup.

## Making a release

Both are GitHub Actions workflows run by hand on `main`, from the Actions tab or with `gh`:

```powershell
gh workflow run release.yml --ref main        # VRFT: vYYYY.M.N
gh workflow run release-apk.yml --ref main    # headset app: apk-vYYYY.M.N
```

Each one picks the next version for its tag prefix (`.github/scripts/next-calver.sh`), builds with that version, creates the tag at the commit it built, and publishes a GitHub release. The release notes list the conventional-commit subjects since that component's previous release (`.github/scripts/release-notes.sh`). VRFT's notes leave out `android/`, and the headset app's cover only `android/questpro-camera/`.

A VRFT release's notes, and the `dev` prerelease's description, also link the newest headset app release that works with it: the newest `apk-v` tag whose `GazePackets.PROTOCOL` falls inside that build's `PROTOCOLS` (`.github/scripts/headset-app.sh newest`). A headset app release's notes name its protocol.

- The VRFT package also carries that headset app release's APK as `headset-app/vrft-questpro-camera-<version>.apk`, which the desktop app's Headset page installs. Without a compatible release, the package has no `headset-app/` folder.
- The VRFT release is marked as the repository's latest, so `releases/latest/download/vrft_d-x86_64-windows-std.zip` always points to it. The headset app release is not marked latest.
- Every push to `main` republishes the rolling `dev` prerelease with a dev build, `YYYY.M.N-dev.C`, and its installer. Every push to `main` that changes `android/questpro-camera/` does the same for the headset app's rolling `apk-dev` prerelease. The VRFT dev build carries the newest compatible headset app **release**, not the `apk-dev` build.

## Signing the headset app

The headset app is always sideloaded, never published to a store, so it has no private release key. Every build, whether local or from CI, debug or release, is signed with the public dev key committed at `android/questpro-camera/dev.keystore` (alias `vrft-dev`, password `android`). Android only installs an update signed with the same key as the installed app, so one key means any build installs over any other, and the workflow needs no secrets.

Anyone can sign an APK with this key, so it proves nothing about who built one. Install the app from this repository's releases or from your own build.
