# Synthetic five-camera face recordings

Renders labelled Quest Pro frames of all five inward cameras (the eyes, the mouth pair and the brow) with Blender. The faces are Google's [GNM head](https://github.com/google/GNM). The output trains the universal face model (`crates/tongue/src/universal/`) and, through its mouth pair, the stereo tongue pair.

It shares tools/tongue-synth's camera model, materials, IR lighting and sensor model by importing `render_tongue.py`; only the faces, expressions and labels are new.

Needs Blender 4.2 or later. It uses Blender's bundled NumPy; MPFB, GNM's own package and TensorFlow are not needed. A Python with `h5py` (`pip install h5py numpy`) converts GNM's semantic sampler once; without it the renders go ahead without semantic expressions, with a warning.

```powershell
./tools/face-synth/render.ps1 -Count 500                 # newest Blender Launcher stable build
./tools/face-synth/render.ps1 -Count 300 -Fast -Jobs 3   # a quick check
# or directly:
blender -b --factory-startup -P tools/face-synth/render_face.py -- --count 500 --seed 1
```

Options:

- `--count` frames, besides each person's face setup poses.
- `--seed`.
- `--identities` people (by default one per 40 frames).
- `--out` (default `.local/tongue-captures`).
- `--fast`: half-size renders, 8 samples.
- `--engine cycles` renders on the CPU, for machines without a GPU. EEVEE, the default, needs one.
- `--no-enrollment`.
- `--gnm` (where the GNM file is, or goes).
- `--semantic` (where the converted semantic sampler is, or goes) and `--no-semantic` (below).
- `--label-scales`: a JSON file overriding the label scales (see Tuning the labels).
- `--calibration`, `--camera-ids` and `--list-calibration` (see Cameras).

On four CPU cores, `--fast --engine cycles` renders about 5 frames a minute (25 views); EEVEE on a desktop GPU is much faster.

## GNM

The first run downloads the GNM head v3 weights from Hugging Face to `.local/gnm/gnm_head.npz`:

- source: `google/gnm-v3`, file `v3_0/gnm_head.npz`, 53 MB;
- licence: Apache-2.0, no account needed;
- `.local/` is never committed, and neither are the weights.

`gnm_head.py` evaluates the model in plain NumPy, in float32:

- template + identity basis x identity + expression basis x expression;
- only the expression coefficients in use are summed;
- no pose correctives, since the head is placed rigidly;
- the eyeballs turn about their joints for gaze.

GNM's coefficients are whitened, about N(0, 1) each:

- identities are drawn from N(0, 1), clipped at 2.5;
- every frame adds small expression noise, N(0, 0.25) by default, over the lower face and both eye regions.

The axes are metres, x the person's left, y up and z forward: the same as the headset frame the renderer works in.

If you use the renders in published work, cite GNM: Ploumpis et al., *GNM Head: A Generative aNthropometric Model of the human head*, arXiv:2607.23687, 2026.

### Expressions

GNM's expression basis is unlabelled PCA, in blocks:

| Coefficients | Region |
| --- | --- |
| 0-99 | left eye region |
| 100-199 | right eye region |
| 200-349 | lower face |
| 350 | `tongue_mean` |
| 351-381 | tongue |
| 382 | pupil |

There is no jaw joint: the jaw moves inside the lower-face block. Named expressions are *prototypes*, fitted once on the template:

| Prototype | How |
| --- | --- |
| `jaw_open` | ridge fit of the lower-face block to the jaw turning 20 degrees about a hinge behind the teeth (up to 12 standard deviations long: it is the lower face's largest movement) |
| `pucker`, `smile`, `lips_part`, `mouth_right` | ridge fits of the lower-face block to the lips moving forward and in, the corners out and up, apart, or sideways |
| `brow_inner_up_*`, `brow_outer_up_*`, `brow_lowerer_*`, `brow_pinch_*` | ridge fits of both eye blocks to one side's medial or lateral brow moving, the rest of the face held still |
| `tongue_out`, `tongue_up`, `tongue_down` | the tongue block's steepest direction for its tip to move out (or out and up or down), with no sideways part, 3 standard deviations long |

Other prototypes are at most 4 standard deviations long. Two kinds of movement are deformers instead, because GNM's PCA barely reaches them:

- **Cheeks:** a full puff from the lower face is about 1.6 mm. Puffs and sucks swell or sink the cheeks along their normals, as tongue-synth does, and so does a tongue pushed into a cheek.
- **Tongue sideways:** GNM's tongue reaches about 4 mm sideways at rest. A bend swings the tongue's front toward a corner from inside the mouth, and a lift tips its front up or down. Both turn fully at the tip, however far out it is, so a short tongue points as well as a long one.
- **Tongue length:** a stretch adds the last few millimetres of protrusion.

### Semantic expressions

GNM's semantic sampler draws whole expressions of a named class, learned from scans of real faces:

- surprise, disgust, suck, compress face, stretch face, happy, squint, platysma;
- blow, funneler, wide smile, corners down, pucker, wink left and right;
- mouth left and right, lips rolled in, snarl, tongue out (`tongue_center`).

15% of the random frames come from it, a class at a time, picked evenly. Their labels still come from the geometry, never from the class.

The sampler is a Keras model, the decoder half of a conditional VAE: five Dense layers, with a 64-dimensional latent and the class in and 383 expression coefficients out. `semantic_decoder.py` converts it to NumPy:

- it downloads `expression_decoder_model.h5` (1.5 MB, Apache-2.0) from GNM's repository at a pinned commit;
- checks its SHA-256;
- writes the layers to `.local/gnm/semantic_decoder.npz`.

`render_face.py` runs the converter itself when that file is missing, with `VRFT_PYTHON`, `python3`, `python` or `py`, whichever has `h5py`. `render.ps1` runs it once before starting its jobs.

The NumPy decoder matches GNM's own `ExpressionSampler` (TensorFlow) to within 5e-7 for the same latents.

### Frames

Each person gets the face setup's poses first, each marked with its slot (`anchor`) as the face setup records them:

- neutral, jaw open, kiss;
- both cheeks puffed, then each on its own;
- the tongue straight out, then up, down, left and right;
- cheeks sucked in.

Then random frames:

- neutral, speech, smile, pucker, jaw open, the mouth to one side;
- cheeks puffed (left, right, both) or sucked;
- the tongue in a cheek;
- brows (raised, inner or outer, frowned or pinched, on one side or both);
- the tongue out in any direction, at any length from the tip just past the lips to about 2 cm out, drawn evenly so short tongues are as common as long ones;
- the tongue to the left or right on its own, at any length;
- just its tip;
- GNM's semantic expressions (above), 15% of these frames.

Every frame carries its person as `identity`, so the universal model's anchors come from the same face.

## Labels

Every label is measured from the posed mesh, never copied from what the frame asked for. The 12 tongue targets come first; the rest go in each sample's `face` map. Directions and movements are measured in the headset frame as the cameras see it, except where noted.

| Label | Measured as | 1 means |
| --- | --- | --- |
| visibility | any tongue vertex past the lips, by more than 1 mm | out |
| extension | how far the tongue reaches past the lips' front at its own sideways position (the mouth curves back toward its corners) | interpolated through 2.5 mm -> 0.25, 8 mm -> 0.5, 20 mm -> 1, as the capture poses grade it |
| horizontal, vertical | which way the tongue's tip points from a point 12 mm inside the mouth, as yaw and elevation, against the person's own straight-out tongue at the same length. A straight tongue droops over the lower lip, more the shorter it is, and is labelled 0, as in the capture poses. Measured from inside the mouth, so even a short tongue has a lever to point with. Measured in the head's frame, so headset tilt doesn't turn into direction. Positive is the person's right and up. No direction under 3 mm past the lips | 40 degrees sideways, 50 up, 35 down. The face setup's left, right and up poses turn about 40 and 51 degrees. A straight tongue already points about 40 degrees below level, so down has less room |
| cheek_puff_left/right, cheek_suck_left/right | the cheek region's most-moved third, along the neutral face's normals, against the person's neutral, after a 1 mm dead zone (an open jaw stretches the cheeks in about a millimetre) | 6 mm out, or 6 mm in |
| brow_inner_up / brow_outer_up | the medial or lateral half of each brow rising, after a 0.5 mm dead zone | 4 mm |
| brow_lowerer | the whole brow falling | 4 mm |
| brow_pinch | the medial half moving toward the middle | 3 mm |
| jaw_open | the front teeth's gap against the neutral one | 22 mm wider |

The full scales for the cheeks, brows and jaw come from what GNM's semantic sampler does on a range of people (median over 6 people, 16 samples of each class):

| Class | Moves |
| --- | --- |
| BLOW | cheeks out 5.9 mm |
| SUCK | cheeks in 6.8 mm |
| STRETCH_FACE | front teeth 21.6 mm apart; inner brows up 3.7 mm |
| SURPRISE | inner brows up 1.6 mm |
| COMPRESS_FACE | brows down 3.6 mm |

The cheek deformers' full puff and suck move the cheek about 7 mm, to match.

A shown tongue whose part past the lips sinks more than 4 mm under the skin around the mouth (through a lip or the chin) is drawn again, up to 8 times, as in tongue-synth. `metadata.json` counts the redraws by pose (`rejected_poses`), and each sample keeps the depth (`synthetic.tongue_depth_mm`).

## Tuning the labels

The scales above are what each label's 1 means. They're in `gnm_head.SCALES`, and a JSON file passed as `--label-scales` overrides any of them, for example `{"suck_full_mm": 5.0}`.

Every sample keeps what its labels were graded from (`synthetic.measured`: millimetres moved, and the tongue's direction). So `relabel.py` grades a set again with other scales in seconds, without rendering it again:

```
python tools/face-synth/relabel.py <recordings or packed sets>... --label-scales scales.json
```

To tune the scales against real people, with a real five-camera recording and its face setup:

1. Train the universal model on the synthetic set alone.
2. Score it on the real recording: `evaluate_face <model> <recording> --enrollment <face setup>`.
3. Read the `gain` column, the least-squares slope of prediction against the real labels on frames where the output is active.
   - About 1: the synthetic 1 matches the real one.
   - 0.7: the model, which learned the synthetic scale, reads real full expressions as 0.7. The real movement is smaller than the synthetic full scale, so multiply that scale by about 0.7.
4. Relabel and train again, until the gains settle near 1.
5. Put the scales you settle on in `SCALES`.

## Cameras

- **Mouth pair (cameras 2 and 3):** from the headset's Fisheye62 factory calibration (`ft_calib.scio.json`, ids `cam07_left_mouth` and `cam08_right_mouth`), or tongue-synth's nominal values. Each person gets small random camera differences, as in tongue-synth.
- **Eye cameras (0 and 1) and brow camera (4):** this repository has no calibration for them, and neither does QFT+. If your headset's `ft_calib.scio.json` (or another calibration file in the same layout) lists them, find their ids with `--list-calibration` and pass `--camera-ids eye_left=<id>,eye_right=<id>,brow=<id>`. Otherwise these nominal values are used. They are **estimates, not measurements**, and need checking against a real five-camera recording:

| View | Position (x left, y up, z forward), mm | Looks at, mm | Lens | Flips |
| --- | --- | --- | --- | --- |
| 0 eye left | (24, -17, 8) | the left cornea, (31, -2, -8) | Fisheye62 like `cam07`, f = 330 px (about 60 degrees across) | vertical, as `cam07` |
| 1 eye right | (-24, -17, 8) | the right cornea, (-31, -2, -8) | like `cam08`, f = 330 px | horizontal and vertical, as `cam08` |
| 4 brow | (0, 0, 30) | the glabella, (0, 16, 0) | like `cam07`, f = 218 px | none: forehead up |

The eyeballs' centres sit at (0, -2, -20) mm on average, as in tongue-synth.

The face must clear each camera: the mouth pair by 2 cm, the eye and brow cameras by 8 mm (they sit in the lens rims and the nose bridge). A face that comes closer is moved back.

Each camera has an IR spot beside it. The mouth pair's sit 1.2 cm below the cameras, as in tongue-synth; the others sit close around theirs. The eyes, the mouth and the brow each auto-expose on their own.

To compare renders with a real recording, tile both with `python tools/tongue-synth/preview.py <recording> out.png --frames 8`, which reads five-camera recordings too. Check that each view shows the same part of the face the same way up.

## Output

A recording in the `vrft-tongue-capture-v1` format:

- **Frames:** `cameras: [0, 1, 2, 3, 4]`, 2000 x 400 strips, 800 KB a frame.
- **`samples.jsonl`:**
  - the 12 tongue `targets` (cheek puffs at 10 and 11) and the `face` labels;
  - `identity`, and `anchor` on face setup poses;
  - `dot`, so training keeps every frame;
  - a `synthetic` block: the prototype weights, deformers, gaze, the semantic class if any, the `measured` values the labels come from, and the tongue's depth.
- **`metadata.json`:** carries the generator, GNM version, cameras' sources, seed, the semantic sampler's classes, the label scales and the redraws.

Pack rendered recordings at a model's input size before training on them or shipping them:

```powershell
# The universal face model: all five cameras at 128 px.
cargo run -p vrft-tongue --release --example pack_synthetic -- <out> <recordings>... --size 128 --cameras all
# The stereo tongue pair: the mouth pair at 224 px, as tongue-synth's sets.
cargo run -p vrft-tongue --release --example pack_synthetic -- <out> <recordings>... --size 224
```

`--size` must be the input size of the model that trains on the pack (`image_size` in its checkpoint's metadata): 224 for the built-in pair, 128 for the universal model. A pair whose two models read different sizes can train on the unpacked renders, at the headset's 400 px.

## Validation

As for tongue-synth, judge a set by what users get, not by training on the set alone. Train on part of a real recording plus the set, and score on the recording's held-out poses. With `tools/tongue-remote`:

- **Universal face model:** needs a five-camera recording and its face setup.

  ```
  test real/<held-out part of a five-camera recording>
  base models/quest-pro
  architecture universal-face-v1
  enrollment real/<face setup recording>
  real-only 20 default all real/<training part>
  with-set  20 default all real/<training part>,synth/<packed 128 px set>
  ```

- **Tongue pair:** the same without the `architecture` and `enrollment` lines, with a 224 px pack.

Compare the scores (`evaluate_face`, or `evaluate` for the pair) with and without the set.

`python tools/face-synth/test_gnm_head.py` checks the poses and labels on the real GNM head:

- a neutral face labels nothing;
- each brow moves on its own side;
- puffs, sucks and the jaw label as asked;
- the tongue reads straight, left, right and up;
- labels are the measurements graded with the scales, and `relabel.py` grades a recording again;
- semantic expressions label as their class: blow puffs, suck sucks, `tongue_center` shows the tongue, stretch face opens the jaw, compress face lowers the brows, and a pucker isn't a cheek suck.

## Limits

- **Faces:** GNM gives shape only, no texture. The skin, lips, teeth and tongue look comes from tongue-synth's procedural IR materials. Eyebrows are darker dots on the brow regions; there are no eyelashes or hair.
- **Eye and brow cameras:** their poses, lenses and flips are nominal until checked against a real recording or a calibration that lists them.
- **Tongue:**
  - GNM's tongue is weak sideways and passes through closed lips if the tongue and lower-face coefficients are drawn independently. Here the tongue comes from fitted prototypes with the lips parted, sideways poses from a bend, and anything through the face is redrawn.
  - Curl, roll and the other tongue shapes stay 0.
- **Labels:** the full scales come from GNM's sampler, which is fitted to scans of real faces, not from real headset users. Tune them against a real face setup (Tuning the labels).
