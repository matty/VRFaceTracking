# Personal Quest Pro tongue and cheek puff training

The same personal model tracks your tongue and puffs each cheek on its own. The headset's own face tracking puffs both cheeks together, so the model's cheek puffs replace it once it has learned them.

Open the desktop app's **Training** page: record, then train. The app splits this into three tabs, **Record**, **Train** and **Test**, with a tick on each one that's done. It opens on the tab that comes next and moves on by itself when a recording or training finishes; click a tab to go back to it. Training starts from the built-in model and mixes in a set of rendered training examples, so if either isn't downloaded yet, download it first: the app's Train tab offers **Download**. Everything runs on this PC, with nothing else to install; camera images are never uploaded.

## 1. Record

1. Run VRFT, start the camera APK and put the headset on, with any streaming app (Virtual Desktop, Steam Link or another) so you can see the preview. No tracking module is needed. If one is running, its own TongueOut is saved beside each frame. If something is missing, the Record tab says what and how to fix it.
2. Press **Start recording**. You get 17 poses, including one cheek puffed at a time and then both, with the tongue in. Each gives you four seconds to get ready and then records for four seconds, about two and a half minutes in total. The prompt shows a countdown and the next pose. Prompts are read aloud; untick **Read prompts aloud** to turn that off.
3. Aim for what the prompt asks, such as "tip only" or "half out", rather than the extreme. These graded poses teach the model the positions in between. Keep your tongue where both cameras can see it.
4. **Skip this pose** leaves out a pose you can't do or that went wrong, including frames already saved. **Pause** holds the timer. **Stop** keeps the poses recorded so far.

One complete basic recording is enough to train; the app then offers **Continue to training**. If tracking is still unreliable afterwards, refit the headset, record again and retrain. Under **Extra recordings** (folded away until you open it):

- **Follow the dot:** a dot glides between the centre and points across the whole pad (halfway, full and diagonal), and you follow it with the tip of your tongue. There are 12 rounds of about 7 seconds, each followed by a 3-second tongue-in rest: relaxed, talking, smiling or with the jaw open. It takes about 2½ minutes and saves around 2,500 frames, each labelled with where the dot was, including every position in between. The faint dashed line and rings show where the dot goes next. Each session takes a new random route, so repeating it keeps adding new data. This is the quickest way to improve direction tracking.
- **Direction poses:** halfway and diagonal directions, and full directions with a wide jaw. Try these if left/right or up/down jumps between extremes.
- **Expression poses:** a slight smile, lower teeth showing, vowels, puffed and sucked cheeks, the tongue in the other cheek, pressed lips and a tucked chin, all with the tongue hidden, plus matching tongue-out poses, including just the tip while smiling. Try these if the tongue appears when it shouldn't. The puffed cheeks count towards cheek puff training too, and the tongue pushing into a cheek teaches the model that a bulge from the tongue is not a puff.

Recordings made before cheek puffs were added have no cheek labels. To add cheek puffs without recording everything again, press **Record the cheek poses** on the Train tab: it records just the three cheek poses of the basic recording, about 24 seconds.

Curl, bend, roll, flat, squish and twist are never recorded, so personal models send 0 for them rather than guessing. As in the reference project, these cameras can't show roll, flat, squish and twist clearly enough to label them.

Recording pauses if the mouth cameras stop, and stops if they don't come back within ten seconds. The poses recorded so far are kept.

## 2. Train

1. Every recording is listed, newest first; in the app, under **Your recordings** on the Record tab. **Delete** removes a recording's frames from this PC. **Review** shows the start, middle and end frame of each pose, so you can untick any pose that went wrong. This check is optional. All recordings are used for training unless you leave one out: in the app under **Advanced options** on the Train tab, and in the preview by unticking it.
2. The step shows whether the ticked recordings cover everything training needs: at least 20 tongue-out and 20 tongue-in frames, and 8 frames each of tongue left, right, up and down. If anything is missing it says what. Cheek puffs are optional: a cheek is learned once the ticked recordings have 8 frames with it puffed, and otherwise the step says cheek puffs won't be learned. Without them, the headset's cheek puffs are sent.
3. Press **Train my model**. A progress bar shows the two stages, learning when the tongue is out and then learning its direction, with an estimate of the time left. The first batch also sets up the GPU, which can take a few minutes while a game is using it, so the estimate appears once that's done and is timed from after it. Training can run while you play; it just takes longer, and the estimate allows for that. Your current model stays in use while it runs. **Cancel training** stops it without changing anything.
4. When training finishes, the new model **switches on automatically** and live tracking reloads within a second; the app moves on to the **Test** tab. Stick your tongue out and move it around: the dot shows what your avatar receives. The Mouth page's **Cheek puffs** card shows each cheek as sent and where it comes from; its switch sends the headset's cheek puffs instead.

**Model in use** (on the app's Test tab) switches between your saved models and the built-in model at any time. Choose **Built-in model** to undo a personal model.

**Advanced options** in the app, or **Training options** in the preview, sets the model name (the date and time by default), the device and the number of training passes (12 by default). **Automatic** trains on the GPU (NVIDIA, AMD or Intel, through DX12 or Vulkan) when one works, and on the CPU otherwise. **GPU** fails rather than falling back. On a recent GPU a basic recording trains in roughly ten minutes; the CPU takes several times longer. Live inference runs on ONNX Runtime on the CPU for **Automatic** and **CPU**, and through DirectML for **GPU**. Without ONNX Runtime it picks the device the same way as training.

## How training works

Every run starts from the installed built-in pair (the visibility gate and the direction model) and fine-tunes both on all ticked recordings for the chosen number of passes. Each pose is sampled equally, so long or repeated poses don't dominate. Follow-the-dot frames are grouped by where the label points (the centre, then eight directions at half and full reach). Held poses keep at most 90 evenly spaced frames per pose per recording; follow-the-dot frames are all used, since each one differs. Every training frame gets a random small rotation, zoom and shift, as from a refitted headset, plus brightness and contrast changes, occasional blur and sensor noise. Both camera views of a frame get the same change.

Some of your recorded frames are held back from training to score it: the last fifth of each held pose (poses under 10 frames train in full) and every fifth 36-frame block of follow-the-dot frames. Rendered examples always train. Each model is scored on them before training and after every pass, and the best is kept, so a run never ends worse than it started. The gate is scored by how well it tells tongue out from tongue in (the class-weighted cross-entropy it trains on), the direction model by its error on extension and direction for tongue-out frames. `report.json` lists every score under `kept`, with the pass that won (0 is the model training started from). Held-back frames come from the same poses as the frames trained on, so they flatter the model: they catch a run that got worse, not how well it will track another day. If fewer than 10 tongue-out or 10 tongue-in frames would be held back, every frame trains and the last pass is kept. Judge the model by trying it live.

The visibility threshold is chosen from the gate's predictions on the held-back frames, or on the training frames when none are held back: it is the middle of the widest range of thresholds that best separate tongue out from tongue in, clamped to 0.3 to 0.8. When every held-back frame has a tracking module TongueOut, they also choose the camera/native blend weight, from 0.5 to 1 (1 is the cameras alone), keeping 0.8 when it does as well as any. Otherwise the weight is 0.8, because frames the model trained on can't show how far to trust the camera over the tracking module's own TongueOut. If any recording was made without a tracking module, the gate is tuned on the cameras alone (weight 1.0). The rendered training examples count toward neither: the threshold and the weight come from your recordings alone. Live, whenever no tracking module TongueOut is arriving, every setting uses the cameras alone. The **When to show the tongue** setting on the app's Mouth page (or the preview's Settings tab) still lets you choose cameras only, headset only or both.

Live, a tongue counts as out once the blend reaches the threshold and stays out until it drops 0.08 below it. A dip below that keeps the last confident values for up to 0.22 seconds of camera frames, so one missed frame doesn't flicker the tongue off; a tongue that is in never shows because of it.

TongueOut is normally the larger of the tongue-out confidence and the extension output, so a clearly visible tongue always reads as mostly out. The held-back straight-ahead poses (just the tip at 0.25, halfway, three-quarters and all the way out) check whether extension tells those amounts apart: at least three amounts with 3 frames each, their mean outputs rising, the highest at least 0.25 above the lowest, and a correlation of at least 0.7. If so, TongueOut follows extension through a straight line fitted to those amounts, never below 0.1 while the tongue is out, so just the tip reads as just the tip. `report.json`'s `tongue_out` shows the check; VRFT's log says when a model uses it.

Recordings train visibility, extension and horizontal/vertical direction. All other tongue outputs are always sent as 0.

### Rendered training examples

Every training run also mixes in 2,000 rendered frames of synthetic people (made with [tools/tongue-synth](../../tools/tongue-synth/README.md)): tongues out in every direction and extension, and smiles, speech, open jaws, teeth and cheek puffs with the tongue in. Your recordings teach the model your face; the examples keep it knowing the poses and faces your recordings don't show, which fine-tuning on a short recording otherwise forgets. On held-out poses of one real recording, adding them raised frames judged right from 67-80% to 94-95%. With them, training takes two to three times as long.

They download with the built-in model, once (about 123 MB, VRFaceTracking's `tongue-synthetic-v4` release), are checked against a fixed SHA-256 hash, and unpack to `models/quest-pro/tongue-synthetic-v4/`, stored at the model's 224 px input size. To use a copy you already have, put `tongue-synthetic-v4.zip` in `.local/` first.

### Cheek puffs

The model has two more outputs, one per cheek, on the direction model. Unlike the tongue's, they are labelled on every frame, with the tongue in or out: 1 for the puffed cheek in the cheek poses and in **Cheeks puffed**, 0 everywhere else, including the tongue pushing into a cheek. The built-in model has no cheek outputs, and fine-tuning changes weights too gently to grow new ones from nothing, so each cheek's output is first fitted by least squares on the direction model's features for the labelled frames (each pose weighted equally), fine-tuned with the rest of the model, then fitted again on the fine-tuned features. Models trained before cheek puffs load with them switched off.

## Universal face model (five cameras)

With the headset app's **All five cameras** on, VRFT can also run a *universal face model* (`universal-face-v1`, `crates/tongue/src/universal/`). It reads all five cameras, the eyes, the mouth and the brow, and adds the brows (inner up, outer up, lowerer and pinch, each side), cheek suck and jaw open to the tongue and cheek puffs. It follows the design of QFT+'s universal face model, rebuilt in VRFT's own code and trained from VRFT's own v8 encoder rather than QFT+'s weights:

- every camera view, shrunk to 128 px, goes through the first three stages of the v8 encoder, shared by all five;
- the mouth cameras then go through two separate copies of the encoder's last stage: one gives a 512-wide mouth embedding, the other the tongue's visibility, extension and direction;
- the eye and brow cameras go through a third copy, which gives a 480-wide brow embedding;
- the mouth outputs (cheek puffs, cheek suck, jaw open) read the mouth embedding against your own face setup poses, and the brows read the brow embedding against your own neutral face. Any pose you didn't record is replaced by a learned stand-in, so the model works without a face setup too.

**Face setup.** Under **Extra recordings**, **Face setup** takes about a minute, and needs the headset app's **All five cameras** on. Its poses are QFT+'s own enrolment's:

- a relaxed neutral face;
- mouth wide open, a kiss, both cheeks puffed, then each cheek on its own;
- the tongue straight out, then up, down, left and right;
- cheeks sucked in;
- the jaw side to side, then a sentence read out loud, both with the tongue in.

Each held pose is checked as it ends, as QFT+ checks its own (`extensions/quest-pro/daemon/src/face_check.rs`). The mouth cameras, each shrunk to 25 × 25 blocks, must have sent at least 3 frames, no more than 30 % of them repeats, at a mean brightness from 10 to 230. Every pose but the relaxed face must differ from it by at least 3 levels on the median frame and hold still while it does. With a tracking module sending, the headset's own tracking must see the open mouth (JawDrop 0.4), the kiss (LipPucker 0.3) and the tongue poses (TongueOut 0.5). A pose that fails is asked for once more ("Once more: …", saying why) while the setup is under 80 seconds in, and a failed attempt's frames are left out like a skipped pose's. When it ends, the Training page lists any pose that still didn't come through, and the recording keeps the checks in `face_setup.json`.

A face setup is put in use (`.local/face-enrollment.json`) only if it gets past its last pose with a clean relaxed face: stopping in the jaw sweep or the reading is fine. One that's stopped before then, or whose relaxed face failed, is kept as a recording, but the face setup in use stays. The Face setup row shows the one in use and whether it fitted your tongue directions. The model reloads with a new face setup within a second, and again when you tick or untick its poses in Review. Deleting the face setup in use leaves the model running without one. The face setup does three things:

- its poses become the model's reference poses for you;
- the five held tongue poses fit how your tongue reads each way (a ridge regression on the mouth embedding, gains capped at 2), which then sets the tongue's direction;
- its frames are labelled, so they also train a personal model.

**Training.** Under **Advanced options**, set **Model** to **Universal face**. Training starts from the built-in pair's encoder, and each pose is sampled equally within each face. Each training frame is read against frames of the same face's setup poses, from the same recording session or the same rendered person. Poses are left out at random, and sometimes all of them, so the model also learns to work without them.

An output is trained only once the recordings label it enough: 8 active and 20 resting frames, for example. Anything not trained is sent from the tracking module as before.

Brows need five-camera recordings: a face setup, or rendered sets. Recordings of the mouth cameras alone still train the tongue and cheeks. The tongue's visibility threshold is chosen as for the pair.

A universal model trains for many more passes than fine-tuning the pair; train it on a GPU (see [tools/tongue-remote](../../tools/tongue-remote/README.md)).

**Live.** While five-camera frames arrive and the model in use has a universal face model (`universal-face-v1.safetensors` in its folder, or beside the built-in pair, or named by `VRFT_FACE_MODEL`), or QFT+'s model is downloaded (below), it replaces the pair for the tongue and cheek puffs, and sends the brows, cheek suck and jaw it was trained for. When the headset falls back to the mouth stream, the pair takes over. Its values replace the tracking module's only while they are fresh. **Face expressions** (`face_expressions` in the Quest Pro settings) turns the brows, suck and jaw off.

### QFT+'s face model (the base face model)

VRFT's own universal face model is on hold, and QFT+'s model is the base face model instead. VRFT can run a model in QFT+'s `universal-face-v2` format, such as QFT+'s own, with QFT+'s per-frame logic ported to Rust (`crates/tongue/src/universal_v2/`). On the same frames it gives what QFT+ gives, to within 1e-6, in less time: 8.9 s against QFT+'s 15.6 s over a 452-frame replay (`tools/benchmark/qftplus_parity.py`).

QFT+'s weights are trained on Ava-256 (CC BY-NC 4.0) and private renders, so they're for **non-commercial use only**. VRFT never ships them, and downloads them only when you ask:

1. On the Training page, under the models, **QFT+ face model** offers a **Download** (194 MB). It downloads QFT+'s v0.4.0-rc.25.2 release package from QFT+'s GitHub, checks its SHA-256, keeps only `universal-face-v2.area.onnx` and `universal-face-v2.npz` (each checked again) in `models/qftplus/`, and deletes the package. To use a copy you already have, put `QproFaceTracking.App-0.4.0-rc.25.2-full.nupkg` in `.local/` first. Like the built-in pair, files already there are never overwritten.
2. It loads within a second, with nothing to restart. From then on it runs on five-camera frames, unless the model in use has a face model of its own (`universal-face-v1.safetensors` or `universal-face-v2.npz` in its folder, or beside the built-in pair), or `VRFT_FACE_MODEL` names one. The built-in pair, or the trained pair in use, still reads the mouth cameras whenever the headset sends only those, so the built-in model is still needed.
3. Record a face setup on the Training page. The model reads its poses as QFT+'s enrolment does: the six anchors, the one-sided puffs and the tongue directions. It runs without one too.

**Remove** deletes the two files again; the model in use then reads the mouth cameras as before. For development, `python tools/benchmark/qftplus.py --fetch` keeps a copy in `.local/qftplus/`, which `VRFT_FACE_MODEL` can name.

It runs as QFT+ runs it:

- **Tongue:** visible and how far out come from the tracking module's own TongueOut, through QFT+'s event layer; its direction comes from the model, fitted to your face setup's tongue poses. The **Tongue visibility** and **Smoothing** settings don't apply: the event layer has its own thresholds and smoothing.
- **Cheeks:** puffs and sucks from the model, gated by the module's lips and tongue. Puffs split between the sides by your one-sided puffs.
- **Brows:** lowerer and pinch from the model. The inner and outer raises are the module's own, split between the sides by the model, so they're sent only while the module sends values.
- **Jaw:** left to the tracking module.

It needs ONNX Runtime, and a tracking module sending the headset's own face tracking, as it does in QFT+. Without one the tongue never shows and the raises stay the module's.

## Local files

- `.local/tongue-captures/<recording>/`: `frames.gray8` (each frame the views of the cameras `metadata.json` lists in `cameras`, side by side: the mouth pair, 800 × 400, or, while the headset sends all five cameras, the whole 2000 × 400 strip; recordings without `cameras` hold the mouth pair, and training reads the mouth pair of either), `samples.jsonl` (twelve labels per frame, the last two the left and right cheek puffs; ten in recordings made before cheek puffs), `metadata.json`, and optional files listing skipped (`excluded_steps.json`) and unticked (`review.json`) poses. Follow-the-dot samples also store `dot`, the dot's position at that frame; their labels use its position 0.35 seconds earlier, and `metadata.json` keeps every route so labels can be recomputed.
- `.local/tongue-models/<run>/`: the gate and direction checkpoints (`.safetensors`), `request.json`, `progress.json`, `training.log` and `report.json`. Failed or cancelled runs never become selectable. Models trained by older versions (`.pt`) still load.
- `.local/tongue-models/<run>/universal-face-v1.safetensors`: a universal face model, with the same `report.json`. A folder with only that runs it beside the built-in pair.
- `.local/face-enrollment.json`: the face setup in use, `{"recording": "<recording id>"}`. A face setup recording also keeps `face_setup.json`, each pose's check.
- `.local/tongue-active.json`: the ID of the model in use. The built-in checkpoints in `models/quest-pro/` are never modified.

Raw recordings, personal weights, reports and the selection are ignored by Git. If `VRFT_TONGUE_MODEL_DIR` is set, it takes priority. New models are still saved but not switched on, and model selection is disabled.

## Command line

`vrft_d.exe train-tongue --request request.json --output <new-model-folder> [--epochs 12]` runs one training outside the app. The request contains `name`, `device` (`auto`, `cpu` or `gpu`), `base_model_dir`, `recordings`, a list of recording folders, and optionally `architecture`: `universal-face-v1` trains the universal face model instead of the pair. Use a new output folder for each run. The app writes these files for you.

## Developer checks

```powershell
cargo test -p vrft-tongue --release
cargo test -p vrft-daemon
node --check extensions/quest-pro/daemon/src/training.js
```

`vrft-tongue`'s tests include a small synthetic end-to-end training on the CPU, checking the report, the saved checkpoints and inference. They check that the software works, not how well tracking works on people. The loss is checked against values from the reference PyTorch trainer, and `cargo run -p vrft-tongue --release --example parity` compares preprocessing and inference with saved PyTorch and OpenCV outputs.
