# Synthetic tongue recordings

Renders labelled Quest Pro mouth-camera frames with Blender, so the tongue
model can train on more faces, mouths and tongue directions than one person
can record.

```powershell
./tools/tongue-synth/render.ps1 -Count 500            # newest Blender Launcher stable build
# or directly:
blender -b --factory-startup -P tools/tongue-synth/render_tongue.py -- --count 500 --seed 1
```

Options: `--count` frames, `--seed`, `--identities` (synthetic people; by
default one per 50 frames), `--out` (default `.local/tongue-captures`).
It renders about 3 frames a second on a desktop GPU (EEVEE, two views each).

## Output

A recording in the same `vrft-tongue-capture-v1` format the daemon saves:
`.local/tongue-captures/<unixms>-synthetic-<pid>/` with `frames.gray8`
(800x400 stereo strips), `samples.jsonl` and `metadata.json` (plus a
`synthetic` block with the seed and Blender version). The Train tab lists it
beside real recordings, and training takes it with no changes.

Each frame is one of:

- **Tongue hidden** (visibility 0): relaxed, open, smiling, puckered or mid-speech mouth.
- **Tongue out**: extension 0.15 to 1, direction spread evenly over the
  horizontal/vertical disc. Horizontal +1 is the person's right, as in the
  capture poses.
- **Cheeks puffed**: left, right or both, 0.45 to 1.

Every line carries a `dot`, so training keeps every frame as it does for
follow-the-dot, and its report counts tongue-out frames under the
"Follow the dot" direction groups. A `synthetic` field records the
expression behind each frame.

## The scene

Procedural, with no assets: a lower-face sheet built around the mouth (lips,
jaw hinge, smile and pucker, cheek puffs, chin), a nose, teeth, a dark mouth
cavity and a tongue swept along a bending centreline. Two spot lights beside
the cameras stand in for the IR illuminators. Face shape, lips, skin, stubble,
tongue size, headset placement, exposure, vignetting, blur and noise are
randomised per person or per frame.

Calibrated against a real recording:

- Frames are mirrored: the person's left is the image's left, as in the real strips.
- The strip's right view sees the face about 18 px (at 400 px) further right
  than the left view. Synthetic frames measure the same.
- Mean brightness is about 55/255 in the real and synthetic frames.

`preview.py` tiles frames of any recording, real or synthetic, into one PNG
to compare them by eye:

```powershell
python tools/tongue-synth/preview.py .local/tongue-captures/<recording> preview.png --frames 8
```

## Limits

The faces are clearly procedural (smooth, no wrinkles or saliva), so treat
this as pretraining data or a small share of a training set, not a
replacement for real recordings. Always judge a model on a real recording
it didn't train on. Only visibility, extension, direction and cheek puffs are
labelled; curl, roll and the other shapes stay 0.
