# Personal Quest Pro tongue and cheek puff training

The same personal model tracks your tongue and puffs each cheek on its own. The headset's own face tracking puffs both cheeks together, so the model's cheek puffs replace it once it has learned them.

Open the desktop app's **Training** page: record, then train. The app splits this into three tabs, **Record**, **Train** and **Test**, with a tick on each one that's done. It opens on the tab that comes next and moves on by itself when a recording or training finishes; click a tab to go back to it. Training starts from the built-in model, so if it isn't installed yet, download it first: the app's Train tab offers **Download**. Everything runs on this PC, with nothing else to install; camera images are never uploaded.

## 1. Record

1. Run VRFT, start the camera APK and put the headset on, with any streaming app (Virtual Desktop, Steam Link or another) so you can see the preview. No tracking module is needed. If one is running, its own TongueOut is saved beside each frame. If something is missing, the Record tab says what and how to fix it.
2. Press **Start recording**. You get 17 poses, including one cheek puffed at a time and then both, with the tongue in. Each gives you four seconds to get ready and then records for four seconds, about two and a half minutes in total. The prompt shows a countdown and the next pose. Prompts are read aloud; untick **Read prompts aloud** to turn that off.
3. Aim for what the prompt asks, such as "tip only" or "half out", rather than the extreme. These graded poses teach the model the positions in between. Keep your tongue where both cameras can see it.
4. **Skip this pose** leaves out a pose you can't do or that went wrong, including frames already saved. **Pause** holds the timer. **Stop** keeps the poses recorded so far.

One complete basic recording is enough to train; the app then offers **Continue to training**. If tracking is still unreliable afterwards, refit the headset, record again and retrain. Under **Extra recordings** (folded away until you open it):

- **Follow the dot:** a dot glides between the centre and points across the whole pad (halfway, full and diagonal), and you follow it with the tip of your tongue. There are 12 rounds of about 7 seconds, each followed by a 3-second tongue-in rest: relaxed, talking, smiling or with the jaw open. It takes about 2½ minutes and saves around 2,500 frames, each labelled with where the dot was, including every position in between. The faint dashed line and rings show where the dot goes next. Each session takes a new random route, so repeating it keeps adding new data. This is the quickest way to improve direction tracking.
- **Direction poses:** halfway and diagonal directions, and full directions with a wide jaw. Try these if left/right or up/down jumps between extremes.
- **Expression poses:** a slight smile, lower teeth showing, vowels, puffed and sucked cheeks, the tongue in the other cheek, pressed lips and a tucked chin, all with the tongue hidden, plus matching tongue-out poses. Try these if the tongue appears when it shouldn't. The puffed cheeks count towards cheek puff training too, and the tongue pushing into a cheek teaches the model that a bulge from the tongue is not a puff.

Recordings made before cheek puffs were added have no cheek labels. To add cheek puffs without recording everything again, press **Record the cheek poses** on the Train tab: it records just the three cheek poses of the basic recording, about 24 seconds.

Curl, bend, roll, flat, squish and twist are never recorded, so personal models send 0 for them rather than guessing. As in the reference project, these cameras can't show roll, flat, squish and twist clearly enough to label them.

Recording pauses if the mouth cameras stop, and stops if they don't come back within ten seconds. The poses recorded so far are kept.

## 2. Train

1. Every recording is listed, newest first; in the app, under **Your recordings** on the Record tab. **Delete** removes a recording's frames from this PC. **Review** shows the start, middle and end frame of each pose, so you can untick any pose that went wrong. This check is optional. All recordings are used for training unless you leave one out: in the app under **Advanced options** on the Train tab, and in the preview by unticking it.
2. The step shows whether the ticked recordings cover everything training needs: at least 20 tongue-out and 20 tongue-in frames, and 8 frames each of tongue left, right, up and down. If anything is missing it says what. Cheek puffs are optional: a cheek is learned once the ticked recordings have 8 frames with it puffed, and otherwise the step says cheek puffs won't be learned. Without them, the headset's cheek puffs are sent.
3. Press **Train my model**. A progress bar shows the two stages, learning when the tongue is out and then learning its direction, with an estimate of the time left. The first batch also sets up the GPU, which can take a few minutes while a game is using it, so the estimate appears once that's done and is timed from after it. Training can run while you play; it just takes longer, and the estimate allows for that. Your current model stays in use while it runs. **Cancel training** stops it without changing anything.
4. When training finishes, the new model **switches on automatically** and live tracking reloads within a second; the app moves on to the **Test** tab. Stick your tongue out and move it around: the dot shows what your avatar receives. The Mouth page's **Cheek puffs** card shows each cheek as sent and where it comes from; its switch sends the headset's cheek puffs instead.

**Model in use** (on the app's Test tab) switches between your saved models and the built-in model at any time. Choose **Built-in model** to undo a personal model.

**Advanced options** in the app, or **Training options** in the preview, sets the model name (the date and time by default), the device and the number of training passes (12 by default). **Automatic** trains on the GPU (NVIDIA, AMD or Intel, through DX12 or Vulkan) when one works, and on the CPU otherwise. **GPU** fails rather than falling back. On a recent GPU a basic recording trains in roughly ten minutes; the CPU takes several times longer. Live inference uses the GPU the same way.

## How training works

Every run starts from the installed built-in pair (the visibility gate and the direction model) and fine-tunes both on all ticked recordings for the chosen number of passes. Each pose is sampled equally, so long or repeated poses don't dominate. Follow-the-dot frames are grouped by where the label points (the centre, then eight directions at half and full reach). Held poses keep at most 90 evenly spaced frames per pose per recording; follow-the-dot frames are all used, since each one differs. Every training frame gets a random small rotation, zoom and shift, as from a refitted headset, plus brightness and contrast changes, occasional blur and sensor noise. Both camera views of a frame get the same change. The final weights are kept. Nothing is held out for validation or testing, so no accuracy score is reported. Judge the model by trying it live.

The visibility threshold is chosen from the gate's own predictions on the recordings: it is the middle of the widest range of thresholds that best separate tongue out from tongue in, clamped to 0.3 to 0.8. The camera/native blend weight is fixed at 0.8, because frames the model trained on can't show how far to trust the camera over the tracking module's own TongueOut. If any recording was made without a tracking module, the gate is tuned on the cameras alone (weight 1.0). Live, whenever no tracking module TongueOut is arriving, every setting uses the cameras alone. The **When to show the tongue** setting on the app's Mouth page (or the preview's Settings tab) still lets you choose cameras only, headset only or both.

Recordings train visibility, extension and horizontal/vertical direction. All other tongue outputs are always sent as 0.

### Cheek puffs

The model has two more outputs, one per cheek, on the direction model. Unlike the tongue's, they are labelled on every frame, with the tongue in or out: 1 for the puffed cheek in the cheek poses and in **Cheeks puffed**, 0 everywhere else, including the tongue pushing into a cheek. The built-in model has no cheek outputs, and fine-tuning changes weights too gently to grow new ones from nothing, so each cheek's output is first fitted by least squares on the direction model's features for the labelled frames (each pose weighted equally), fine-tuned with the rest of the model, then fitted again on the fine-tuned features. Models trained before cheek puffs load with them switched off.

## Local files

- `.local/tongue-captures/<recording>/`: `frames.gray8`, `samples.jsonl` (twelve labels per frame, the last two the left and right cheek puffs; ten in recordings made before cheek puffs), `metadata.json`, and optional files listing skipped (`excluded_steps.json`) and unticked (`review.json`) poses. Follow-the-dot samples also store `dot`, the dot's position at that frame; their labels use its position 0.35 seconds earlier, and `metadata.json` keeps every route so labels can be recomputed.
- `.local/tongue-models/<run>/`: the gate and direction checkpoints (`.safetensors`), `request.json`, `progress.json`, `training.log` and `report.json`. Failed or cancelled runs never become selectable. Models trained by older versions (`.pt`) still load.
- `.local/tongue-active.json`: the ID of the model in use. The built-in checkpoints in `models/quest-pro/` are never modified.

Raw recordings, personal weights, reports and the selection are ignored by Git. If `VRFT_TONGUE_MODEL_DIR` is set, it takes priority. New models are still saved but not switched on, and model selection is disabled.

## Command line

`vrft_d.exe train-tongue --request request.json --output <new-model-folder> [--epochs 12]` runs one training outside the app. The request contains `name`, `device` (`auto`, `cpu` or `gpu`), `base_model_dir`, and `recordings`, a list of recording folders. Use a new output folder for each run. The app writes these files for you.

## Developer checks

```powershell
cargo test -p vrft-tongue --release
cargo test -p vrft-daemon
node --check extensions/quest-pro/daemon/src/training.js
```

`vrft-tongue`'s tests include a small synthetic end-to-end training on the CPU, checking the report, the saved checkpoints and inference. They check that the software works, not how well tracking works on people. The loss is checked against values from the reference PyTorch trainer, and `cargo run -p vrft-tongue --release --example parity` compares preprocessing and inference with saved PyTorch and OpenCV outputs.
