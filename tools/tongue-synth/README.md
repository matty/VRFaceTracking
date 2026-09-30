# Synthetic tongue recordings

Renders labelled Quest Pro mouth-camera frames with Blender, so the tongue
model can train on more faces, mouths and tongue directions than one person
can record.

Needs Blender 4.2 or later with the **MPFB** extension (MakeHuman for
Blender: in Blender, Edit > Preferences > Get Extensions, search MPFB).

```powershell
./tools/tongue-synth/render.ps1 -Count 500            # newest Blender Launcher stable build
# or directly:
blender -b --factory-startup -P tools/tongue-synth/render_tongue.py -- --count 500 --seed 1
```

Options: `--count` frames, `--seed`, `--identities` (synthetic people; by
default one per 50 frames), `--out` (default `.local/tongue-captures`) and
`--calibration` (see below). It renders about 2 frames a second on a
desktop GPU (EEVEE, two views each).

## Output

A recording in the same `vrft-tongue-capture-v1` format the daemon saves:
`.local/tongue-captures/<unixms>-synthetic-<pid>/` with `frames.gray8`
(800x400 stereo strips), `samples.jsonl` and `metadata.json` (plus a
`synthetic` block with the seed, Blender version and where the camera
calibration came from). The Train tab lists it beside real recordings, and
training takes it with no changes.

Each frame is one of:

- **Tongue hidden** (visibility 0): relaxed, open, smiling, puckered or mid-speech mouth.
- **Tongue out**: extension 0.15 to 1, direction spread evenly over the
  horizontal/vertical disc. Horizontal +1 is the person's right, as in the
  capture poses. Extension 1 puts about 2 cm of tongue past the lips, 0.5
  about 0.8 cm and 0.25 just the tip, matching the capture poses. Mostly
  the jaw opens only as far as the tongue needs, so the lips close round
  it and hide the teeth, as they do in real recordings.
- **Cheeks puffed**: left, right or both, 0.45 to 1.

Every line carries a `dot`, so training keeps every frame as it does for
follow-the-dot, and its report counts tongue-out frames under the
"Follow the dot" direction groups. A `synthetic` field records the jaw and
expression behind each frame.

## The scene

- **Faces**: MPFB (MakeHuman) people with random gender, age, build and
  ancestry, and random face shape (MPFB's mouth, chin, cheek and nose
  targets). MakeHuman's expression units and the rig's jaw move the mouth;
  cheek puffs swell the cheeks along their normals.
- **Tongue and teeth**: procedural. The tongue is swept along a bending
  centreline, rests on the lower lip when out and turns with the jaw as
  the lower lip does. Straight out it droops over the lower lip, up it
  lies against the upper lip, down it hangs and narrows. Its surface has
  papillae-like bumps under patches of saliva that break highlights into
  glints. The teeth are two arcs behind the lips, only a little brighter
  than the lips, as enamel is in near infrared. A tongue pose whose part
  past the lips sinks more than 4 mm under the skin, through a cheek, a lip
  or the chin, is drawn again; `metadata.json` counts those by pose
  (`rejected_poses`).
- **Cameras**: the headset's own mouth cameras. Their lens model (Meta's
  Fisheye62) and pose in the headset come from its factory calibration.
  Each view is rendered as a 106 degree pinhole image and resampled
  through the lens model, then flipped as the headset sends it, so the
  left view comes out mirrored and the right one upright.
- **Headset on the face**: the eyes sit where the eye-tracking calibration
  puts them, with random head tilt and a few millimetres of play; a face
  that would reach a camera is moved back until it clears it by 2 cm.
- **Light and sensor**: an IR spot 1.2 cm below each camera, fitted to the
  brightness pattern of a real recording, plus dim stray light that keeps
  the chin and shirt out of black; auto-exposure, a shadow-lifting
  response curve, lens falloff, glare, blur and noise vary per person,
  over ranges that bracket a real recording's brightness percentiles and
  fine contrast.

Checked against a real recording: the lip line lands where it does in the
real frames (on average within a few pixels, spread about 30 px across
people), and the average frame's brightness pattern matches the real one's
around the mouth.

### Camera calibration

`--calibration` reads a headset's `ft_calib.scio.json`, by default
`.local/headset-calibration/ft_calib.scio.json`. Without it, nominal values
rounded from one Quest Pro are used, and each synthetic person gets small
random camera differences either way. To copy your headset's calibration
(needs root; the file holds its serial number, so keep it out of git):

```powershell
adb exec-out su -c "cat /persist/calibration/ft_calib.scio.json" > .local/headset-calibration/ft_calib.scio.json
```

## Releasing training examples

Personal training mixes in a synthetic set that the app downloads from a
VRFaceTracking GitHub release (see `EXAMPLES_RELEASE` in
`extensions/quest-pro/daemon/src/builtin.rs`). To make a new one:

1. Render it, for example 2,000 frames: `./tools/tongue-synth/render.ps1 -Count 2000 -Jobs 3 -Fast -Out <folder>`.
2. Pack the recordings at the model's input size, shrunk exactly as training
   shrinks real frames:
   `cargo run -p vrft-tongue --release --example pack_synthetic -- .local/tongue-synthetic-release/tongue-synthetic-v<N> <folder>/*`
3. Zip the folder (`7z a -tzip -mx=9 tongue-synthetic-v<N>.zip tongue-synthetic-v<N>`)
   and publish it as the only asset of a `tongue-synthetic-v<N>` prerelease: a
   full release would become the repository's latest while VRFT has none.
4. Update `EXAMPLES_RELEASE` (URL, name, SHA-256, size) and `EXAMPLES_DIR`.

Judge a new set by personal training on part of a real recording plus the
set, scored on the recording's held-out poses, rather than by training on
the set alone: that is what users get.

`preview.py` tiles frames of any recording, real or synthetic, into one PNG
to compare them by eye:

```powershell
python tools/tongue-synth/preview.py .local/tongue-captures/<recording> preview.png --frames 8
```

## Limits

The faces are still clean compared with real ones (no fine wrinkles or
hair beyond stubble, and lips that pass through the tongue rather than
wrapping it), so treat this as a share of a training set, not a
replacement for real recordings. Always
judge a model on a real recording it didn't train on. Only visibility,
extension, direction and cheek puffs are labelled; curl, roll and the
other shapes stay 0.
