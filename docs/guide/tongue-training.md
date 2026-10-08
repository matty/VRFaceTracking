# Personal Quest Pro training: fine-tuning the QFTPlus Model

The QFTPlus Model, QFT+'s face model, is the base model: it reads all five Quest Pro cameras for the cheeks, brows and tongue. Personal training fine-tunes that model on your own recordings, here on this PC.

Open the desktop app's **Training** page. It splits this into five tabs, **QFTPlus Model**, **Face setup**, **Record**, **Fine-tune** and **Test**, with a tick on each one that's done. It opens on the first tab not done yet and moves on by itself when a recording or training finishes; click a tab to go back to it. Everything runs on this PC, with nothing else to install; camera images are never uploaded.

QFT+ itself fine-tunes nothing on your PC: its one personal step is the face setup, which its model reads every frame against. VRFT does the same, and fine-tuning goes one step further, as close to QFT+'s design as training on your own recordings can be: QFT+'s network stays as it is, and only the small heads on top of it and your tongue directions are fitted to you.

## 1. The QFTPlus Model

The QFTPlus Model is the model in use until you choose one of your own. It's QFT+'s face model, which reads all five cameras, with the mouth-camera pair beside it, which reads the mouth cameras whenever the headset sends only those. VRFT runs models in QFT+'s `universal-face-v2` format with QFT+'s per-frame logic ported to Rust (`crates/tongue/src/universal_v2/`). On the same frames it gives what QFT+ gives, to within 1e-6, in less time: 8.9 s against QFT+'s 15.6 s over a 452-frame replay (`tools/benchmark/qftplus_parity.py`).

QFT+'s weights are trained on Ava-256 (CC BY-NC 4.0) and private renders, so they're for **non-commercial use only**. VRFT never ships them, and downloads them only when you ask:

1. On the **QFTPlus Model** tab, or under **Model in use** on the **Test** tab, **Download** (about 334 MB) downloads the mouth-camera pair from the Qpro-Enhanced-FT v0.1.10 release into `models/quest-pro/`, then QFT+'s v0.4.0-rc.25.2 release package from QFT+'s GitHub. It checks each SHA-256, keeps only `universal-face-v2.area.onnx` and `universal-face-v2.npz` (each checked again) in `models/qftplus/`, and deletes the package. To use copies you already have, put `QproFaceTracking-0.1.10-poc.zip` or `QproFaceTracking.App-0.4.0-rc.25.2-full.nupkg` in `.local/` first. Files already there are never overwritten.
2. It loads within a second, with nothing to restart. From then on it runs on five-camera frames, unless the model in use has a face model of its own, or `VRFT_FACE_MODEL` names one. The mouth-camera pair reads the mouth cameras whenever the headset sends only those.

The bin beside **QFTPlus Model** under **Model in use** deletes QFT+'s two files again, keeping the mouth-camera pair, which then reads only the mouth cameras until you download it again. Models fine-tuned from it keep working: each has its own link to QFT+'s graph. For development, `python tools/benchmark/qftplus.py --fetch` keeps a copy in `.local/qftplus/`, which `VRFT_FACE_MODEL` can name.

It runs as QFT+ runs it:

- **Tongue:** visible and how far out come from the tracking module's own TongueOut, through QFT+'s event layer; its direction comes from the model, fitted to your tongue. The **Tongue visibility** and **Smoothing** settings don't apply: the event layer has its own thresholds and smoothing.
- **Cheeks:** puffs and sucks from the model, gated by the module's lips and tongue. Puffs split between the sides by your one-sided puffs.
- **Brows:** lowerer and pinch from the model. The inner and outer raises are the module's own, split between the sides by the model, so they're sent only while the module sends values.
- **Jaw:** left to the tracking module.

It needs ONNX Runtime, and a tracking module sending the headset's own face tracking, as it does in QFT+. Without one the tongue never shows and the raises stay the module's.

## 2. Face setup

The face setup, and every recording, needs the headset app's **All five cameras** on: switch it on on the app's **Headset** page (which restarts the stream) or in the headset app. Until it's on, the tabs say so, with a button to the Headset page.

**Face setup** takes about a minute. Its poses are QFT+'s own enrolment's:

- a relaxed neutral face;
- mouth wide open, a kiss, both cheeks puffed, then each cheek on its own;
- the tongue straight out, then up, down, left and right;
- cheeks sucked in;
- the jaw side to side, then a sentence read out loud, both with the tongue in.

Each held pose is checked as it ends, as QFT+ checks its own (`extensions/quest-pro/daemon/src/face_check.rs`). The mouth cameras, each shrunk to 25 × 25 blocks, must have sent at least 3 frames, no more than 30 % of them repeats, at a mean brightness from 10 to 230. Every pose but the relaxed face must differ from it by at least 3 levels on the median frame and hold still while it does. With a tracking module sending, the headset's own tracking must see the open mouth (JawDrop 0.4), the kiss (LipPucker 0.3) and the tongue poses (TongueOut 0.5). A pose that fails is asked for once more ("Once more: …", saying why) while the setup is under 80 seconds in, and a failed attempt's frames are left out like a skipped pose's. When it ends, the tab lists any pose that still didn't come through, and the recording keeps the checks in `face_setup.json`.

A face setup is put in use (`.local/face-enrollment.json`) only if it gets past its last pose with a clean relaxed face: stopping in the jaw sweep or the reading is fine. One that's stopped before then, or whose relaxed face failed, is kept as a recording, but the face setup in use stays. The tab shows the one in use and whether it fitted your tongue directions. The model reloads with a new face setup within a second, and again when you tick or untick its poses in Review. Deleting the face setup in use leaves the model running without one. The model reads the face setup as QFT+'s enrolment does:

- the six poses become the anchors the mouth head reads every frame against, and the relaxed face the brow head's;
- the one-sided puffs split a puff between the sides;
- the five held tongue poses fit how your tongue reads each way (a ridge regression on the mouth embedding, gains capped at 2), which then sets the tongue's direction.

Training needs a face setup in use: it reads your recordings against it.

## 3. Record

1. Run VRFT, start the camera APK with **All five cameras** on and put the headset on, with any streaming app (Virtual Desktop, Steam Link or another) so you can see the preview. No tracking module is needed. If something is missing, the Record tab says what and how to fix it.
2. Press **Start recording**. The face recording is QFT+'s own: the targets and look-alikes of its short benchmark (`guided_session.py`), which is what QFT+ collects to train its model. About five minutes:
   - a relaxed face, 15 seconds;
   - 16 targets, each at three amounts (halfway, most of the way, full), held for 1½ seconds after a second to get into it, with a 1½-second rest after each: both cheeks puffed, each cheek on its own, cheeks sucked in, the tongue out straight, left, right, up, down, up and left, and up and right, both brows raised, each brow on its own, a worried look (the middle of the brows up) and a frown;
   - seven look-alikes of 10 seconds that mustn't read as a target: chewing, the tongue pushing into each cheek, lips sucked in, a pout, smiling and talking, and squinting;
   - two sentences read out loud, 15 seconds each.
3. Aim for the amount the prompt asks rather than the extreme. Prompts are read aloud; untick **Read prompts aloud** to turn that off.
4. **Skip this pose** leaves out a pose you can't do or that went wrong, including frames already saved. **Pause** holds the timer. **Stop** keeps the poses recorded so far.

Each frame is labelled by its prompt: the puffed and sucked cheeks, the brows that move and those that stay still, and where the tongue points. A brow you were asked to keep still is left unlabelled when only one is raised, since not everyone can. The relaxed face labels every cheek and brow at rest, and the look-alikes and reading label the cheeks at rest. A target saves at most 12 frames a second, and the longer steps 6, since frames that close together add disk rather than information.

One full face recording is enough to train; the tab then offers **Continue to training**. Under **Extra recordings**:

- **Follow the dot:** a dot glides between the centre and points across the whole pad, and you follow it with the tip of your tongue, 12 rounds with tongue-in rests between. Each frame is labelled with where the dot was, every position in between included, for fitting your tongue's directions.
- **Direction poses:** halfway and diagonal directions, and full directions with a wide jaw.

Recording pauses if the cameras stop, and stops if they don't come back within ten seconds. The poses recorded so far are kept. Recordings from older versions are still listed: basic and expression poses can't be recorded any more, and recordings of the mouth cameras alone can't be trained on.

## 4. Fine-tune

1. Every recording is listed, newest first, under **Your recordings**. **Delete** removes a recording's frames from this PC. **Review** shows the start, middle and end frame of each pose, so you can untick any pose that went wrong. All five-camera recordings are trained on unless you untick one.
2. The tab shows whether the ticked recordings cover what training needs: 20 frames of a relaxed face, and 8 each of the left cheek puffed, the right cheek puffed, the cheeks sucked in, the brows raised and the brows lowered. If anything is missing it says what, and **Record just those** records only the face recording's poses for it.
3. Press **Fine-tune**. It reads every frame through QFT+'s model, then fine-tunes; a minute or two for each recording. Training can run while you play; it runs at below-normal priority while VRChat runs. Your current model stays in use while it runs. **Cancel training** stops it without changing anything.
4. When training finishes, the new model **switches on automatically** and live tracking reloads within a second; the app moves on to the **Test** tab.

**Model in use** (on the Test tab) switches between your saved models and the QFTPlus Model at any time. Choose **QFTPlus Model** to undo a personal model. Each trained model can be renamed, deleted, or exported with the recordings and face setup it was trained on, to a folder another PC can import (**Import folder** or **Import zip**).

**Advanced options** sets the model name (the date and time by default), the device and the number of training passes (12 by default, up to 60). **Automatic** reads the recordings through QFT+'s model on the GPU (DirectML) when one works, and on the CPU otherwise; **GPU** fails rather than falling back. The fine-tuning itself runs on the CPU.

## How fine-tuning works

`crates/tongue/src/universal_v2/train.rs`:

1. QFT+'s model loads from `models/qftplus/` and reads the face setup in use, as it does live. Each selected frame of the ticked recordings goes through QFT+'s graph once, which stays as it is, giving its mouth embedding (512 values), tongue head and brow embedding (480). Held poses keep at most 90 evenly spaced frames per pose; follow-the-dot frames are all used.
2. Some frames are held back to score the run: the last fifth of each held pose (poses under 10 frames train in full) and every fifth 36-frame block of follow-the-dot frames.
3. The heads train on the rest for the chosen number of passes (Adam, learning rate 1e-4, 64 frames a step, each pose weighing the same):
   - What VRFT sends trains on the labels: the mouth head's four cheek outputs and the brow head's eight brows, by binary cross-entropy, with active labels counting three times. Every other output of a frame, and every output without a label, is pulled toward what QFT+'s heads give for that frame (one tenth the weight), so the heads stay QFT+'s where the recordings say nothing.
   - Every weight is pulled back toward QFT+'s each step, as AdamW's weight decay pulls toward zero.
   - The weights that read the face setup's anchors and presence flags, and QFT+'s stand-ins for missing poses, stay QFT+'s: with one person's face setup they would only shift a bias.
4. QFT+'s heads are pass 0, scored on the held-back frames' labels (the same cross-entropy), and a pass replaces the kept heads only when it scores better, so a run never ends worse than it started. If fewer than 10 labelled frames would be held back, every frame trains and the last pass is kept.
5. The tongue's directions are fitted again: QFT+'s ridge regression from the mouth embedding to the direction (the same ridge, solved over the embedding rather than the frames), on every labelled frame with the tongue at least halfway out and the face setup's held poses, with each direction's gain from the frames pointing straight that way. It needs at least 3 frames pointing straight left, right, up and down. The new fit is kept only when it reads the held-back frames' directions better than the face setup's; with fewer than 10 of those held back, it's kept.
6. The result is a `universal-face-v2.npz` in QFT+'s own format: the same metadata (with a note added to its provenance), the fine-tuned heads, and the tongue fit in arrays QFT+'s loader ignores (`tongue_mean`, `tongue_scale`, `tongue_weights`, `tongue_gains`). Beside it, `universal-face-v2.area.onnx` is a hard link to QFT+'s graph (a copy where the drive can't link), which the `.npz` still names by its SHA-256. The model is loaded once more before it's saved.

`report.json` lists the frames read and trained on, how many frames labelled each output, the outputs trained (`supported_targets`) and those no recording labelled (`disabled_targets`), and under `kept` each pass's score with the pass that won (`face`, where 0 is QFT+'s heads), and the tongue fit's error before and after (`tongue`, where 1 is the new fit). Held-back frames come from the same poses as the frames trained on, so they flatter the model: they catch a run that got worse, not how well it will track another day. Judge the model by trying it live.

Live, a fine-tuned model runs exactly as the QFTPlus Model does, with the face setup in use read against its own heads; its tongue fit is used rather than the face setup's.

## Older models

Models trained by earlier versions stay selectable and run as before: tongue pairs (`.safetensors`, or `.pt` from older still) and VRFT's own universal face model (`universal-face-v1.safetensors`). They are no longer trained: training fine-tunes the QFTPlus Model.

## Local files

- `.local/tongue-captures/<recording>/`: `frames.gray8` (each frame the views of the cameras `metadata.json` lists in `cameras`, side by side: all five, 2000 × 400, in recordings made now), `samples.jsonl` (each frame's labels: twelve tongue and cheek puff labels, and `face`, the cheek suck and brow labels by name), `metadata.json`, and optional files listing skipped (`excluded_steps.json`) and unticked (`review.json`) poses.
- `.local/tongue-models/<run>/`: `universal-face-v2.npz` and `universal-face-v2.area.onnx`, `request.json`, `progress.json`, `training.log` and `report.json`. Failed or cancelled runs never become selectable.
- `.local/face-enrollment.json`: the face setup in use, `{"recording": "<recording id>"}`. A face setup recording also keeps `face_setup.json`, each pose's check.
- `.local/tongue-active.json`: the ID of the model in use. The downloaded models in `models/quest-pro/` and `models/qftplus/` are never modified.

Raw recordings, personal weights, reports and the selection are ignored by Git. If `VRFT_TONGUE_MODEL_DIR` is set, it takes priority. New models are still saved but not switched on, and model selection is disabled.

## Command line

`vrft_d.exe train-tongue --request request.json --output <new-model-folder> [--epochs 12] [--learning-rate 1e-4]` runs one training outside the app. To fine-tune the QFTPlus Model, the request contains `name`, `device` (`auto`, `cpu` or `gpu`), `"architecture": "universal-face-v2"`, `base_model_dir` (the folder holding QFT+'s `.npz` and graph, such as `models/qftplus`), `face_setup` (the face setup's recording folder) and `recordings`, a list of five-camera recording folders. Use a new output folder for each run. The app writes these files for you.

## Developer checks

```powershell
cargo test -p vrft-tongue --release
cargo test -p vrft-daemon
node --check extensions/quest-pro/daemon/src/training.js
```

`vrft-tongue`'s tests check the heads' gradients against finite differences, that the tongue fit gives QFT+'s weights, and fine-tune a small stand-in for QFT+'s model end to end on synthetic five-camera recordings (with ONNX Runtime). They check that the software works, not how well tracking works on people.
