# Versions and Releases

VRFT and the Quest Pro headset app are released separately, each with its own version. A stream protocol number, not the version numbers, decides whether a given pair can work together.

The headset app changes much less often than VRFT, so one headset app release is expected to serve many VRFT releases. Each VRFT release names the newest headset app release that works with it, which is often one from months before.

## Versions

Both use calendar versions, `YYYY.M.N`: the year, the month without a leading zero, and a count of that component's releases in the month, starting at 0. So `2026.9.0` is the first release in September 2026, and `2026.9.1` the second.

| Component | Tag | Where the version shows |
| --- | --- | --- |
| VRFT (`vrft_d`, `vrft_gui`) | `v2026.9.0` | `/status` → `daemon.version`, the app's Home page, the first line of `vrft_d.log` |
| Headset app (`android/questpro-camera`) | `apk-v2026.9.0` | Android's `versionName`, and `apk_version` in the headset's status |

A release tag is the only place a version is written down. Nothing in the repository is bumped by hand:

- `crates/daemon/build.rs` stamps `VRFT_VERSION` into `vrft_d`. A release build takes it from the environment. Any other build runs `git describe` against the last `v` tag, giving `2026.9.0-14-g3f2a1c9` for 14 commits after a release, or `dev-3f2a1c9` before the first. The `version` in `Cargo.toml` is not used.
- `android/questpro-camera/app/build.gradle` does the same with `apk-v` tags. A release passes `-PappVersion=2026.9.0`. The `versionCode` is the version packed into `YYYYMMNN` (`20260900`), so every release installs over the one before; builds before the first release keep `2`.

## Stream protocol

The headset app and `vrft_d` talk over one TCP stream of messages, each starting with an 8-byte `QP…` magic. The protocol number describes that stream.

- The headset app declares it as `GazePackets.PROTOCOL`, advertises it in the mDNS `protocol` TXT record, and sends it in the `QPSTAT1` status that opens every connection.
- `vrft_d` lists the protocols it reads in `PROTOCOLS` in `crates/daemon/src/quest_pro_camera.rs`. When the headset app's protocol is outside that range, the daemon doesn't use the stream. `/status` then carries `headset_mismatch`, and the app's Headset tile says whether to update VRFT or the headset app.
- Headset apps from before the `protocol` field advertised `version=3` and speak protocol 3.

**When to bump it:** only when an existing message changes in a way an older reader would misread. Adding a message type doesn't need a bump. `vrft_d` skips any `QP…` message it doesn't recognise, as long as the message carries its payload length as a little-endian `u32` at byte 12, with the payload starting at byte 16, as `QPSTAT1` does. Adding a field to the status JSON doesn't need one either.

**On a bump**, widen `PROTOCOLS` (for example to `3..=4`) and keep the daemon reading the old protocol too, so headset apps that users haven't updated keep working. Release VRFT before the headset app, so a new headset app never arrives before a VRFT that can read it. Raising the bottom of `PROTOCOLS` drops support for every headset app release that speaks the old protocol and makes all their users update the headset app. Do that on purpose and say so in the release notes.

`vrft_d` builds from before the skip rule drop the connection on an unknown message.

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
- Every push to `main` still republishes the rolling `dev` prerelease, which reports its `git describe` version.

## Signing the headset app

Android only installs an update signed with the same key as the installed app. The debug key in `../toolchain/android-user-home/debug.keystore` stays for local builds. Releases use a separate release key, so moving between a local debug build and a release build means uninstalling the app first.

Create the release key once and keep a backup outside the repository: a lost key means every user has to uninstall to take the next update.

```powershell
keytool -genkeypair -keystore vrft-release.jks -alias vrft -keyalg RSA -keysize 4096 -validity 36500 -dname "CN=VRFT"
```

Then add four repository secrets, which `release-apk.yml` reads. `keytool` makes a PKCS12 keystore, where the key password is the keystore password, so enter the same value for both:

```powershell
gh secret set VRFT_APK_KEYSTORE_BASE64 --body ([Convert]::ToBase64String([IO.File]::ReadAllBytes("vrft-release.jks")))
gh secret set VRFT_APK_KEYSTORE_PASSWORD
gh secret set VRFT_APK_KEY_ALIAS --body vrft
gh secret set VRFT_APK_KEY_PASSWORD
```

To build a signed release APK locally, set `VRFT_APK_KEYSTORE` (the `.jks` path), `VRFT_APK_KEYSTORE_PASSWORD`, `VRFT_APK_KEY_ALIAS` and `VRFT_APK_KEY_PASSWORD`, then run `android/questpro-camera/build.ps1 -Release`. Without them, `-Release` produces an unsigned APK.
