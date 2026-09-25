# Personal Quest Pro tongue training

Open [the local preview](http://127.0.0.1:27275/) and use the **Tongue** tab: **1. Record**, then **2. Train**. Run `setup-quest-pro-tongue.ps1` once first for the built-in model and Python dependencies. Everything runs on this PC; camera images are never uploaded.

## 1. Record

1. Run VRFT normally with the Virtual Desktop tracking module, start the camera APK and put the headset on. Open the preview in Virtual Desktop. If something is missing, the Record step says what and how to fix it.
2. Press **Start recording**. You get 14 poses. Each gives you four seconds to get ready and then records for four seconds, about two minutes in total. The prompt shows a countdown and the next pose. Prompts are read aloud; untick **Read prompts aloud** to turn that off.
3. Aim for what the prompt asks, such as "tip only" or "half out", rather than the extreme. These graded poses teach the model the positions in between. Keep your tongue where both cameras can see it.
4. **Skip this pose** leaves out a pose you can't do or that went wrong, including frames already saved. **Pause** holds the timer. **Stop** keeps the poses recorded so far.

One complete basic recording is enough to train. If tracking is still unreliable afterwards, refit the headset, record again and retrain. Under **Extra recordings (optional)**:

- **Follow the dot:** a dot glides between the centre and points across the whole pad (halfway, full and diagonal), and you follow it with the tip of your tongue. There are 12 rounds of about 7 seconds, each followed by a 3-second tongue-in rest: relaxed, talking, smiling or with the jaw open. It takes about 2½ minutes and saves around 2,500 frames, each labelled with where the dot was, including every position in between. The faint dashed line and rings show where the dot goes next. Each session takes a new random route, so repeating it keeps adding new data. This is the quickest way to improve direction tracking.
- **Direction poses:** halfway and diagonal directions, and full directions with a wide jaw. Try these if left/right or up/down jumps between extremes.
- **Expression poses:** a slight smile, lower teeth showing, vowels, puffed and sucked cheeks, the tongue in the other cheek, pressed lips and a tucked chin, all with the tongue hidden, plus matching tongue-out poses. Try these if the tongue appears when it shouldn't.

Curl, bend, roll, flat, squish and twist are never recorded, so personal models send 0 for them rather than guessing. As in the reference project, these cameras can't show roll, flat, squish and twist clearly enough to label them.

Recording stops if the mouth cameras or the headset's face tracking are lost. The poses recorded so far are kept.

## 2. Train

1. The Train step lists every recording, newest first. Ticked recordings are used, and all are ticked by default. Untick one to leave it out, or **Delete** it to remove its frames from this PC. **Review** shows the start, middle and end frame of each pose, so you can untick any pose that went wrong. This check is optional.
2. The step shows whether the ticked recordings cover everything training needs: at least 20 tongue-out and 20 tongue-in frames, and 8 frames each of tongue left, right, up and down. If anything is missing it says what.
3. Press **Train my model**. A progress bar shows the two stages, learning when the tongue is out and then learning its direction, with an estimate of the time left. Your current model stays in use while it runs. **Cancel training** stops it without changing anything.
4. When training finishes, the new model **switches on automatically** and live tracking reloads within a second. Stick your tongue out and move it around: the dot under the camera view shows what your avatar receives.

**Model in use** switches between your saved models and the built-in model at any time. Choose **Built-in model** to undo a personal model.

**Training options** sets the model name (the date and time by default), the device and the number of training passes (12 by default). **Automatic** uses a GPU if the installed PyTorch runtime supports one, otherwise the CPU. The setup script installs CPU PyTorch by default; to use a GPU, run `setup-quest-pro-tongue.ps1 -Accelerator cuda` for NVIDIA (PyTorch CUDA 12.8 wheels) or `setup-quest-pro-tongue.ps1 -Accelerator rocm` for AMD Radeon on Windows (AMD's ROCm 7.2.1 PyTorch 2.9.1 wheels, which need Python 3.12). Either option runs one training and one inference step on the GPU to confirm it works. PyTorch reports ROCm GPUs as CUDA devices, so **GPU** covers both. Live inference uses the same runtime.

## How training works

Every run starts from the installed built-in pair (the visibility gate and the direction model) and fine-tunes both on all ticked recordings for the chosen number of passes. Each pose is sampled equally, so long or repeated poses don't dominate. Follow-the-dot frames are grouped by where the label points (the centre, then eight directions at half and full reach). Held poses keep at most 90 evenly spaced frames per pose per recording; follow-the-dot frames are all used, since each one differs. Every training frame gets a random small rotation, zoom and shift, as from a refitted headset, plus brightness and contrast changes, occasional blur and sensor noise. Both camera views of a frame get the same change. The final weights are kept. Nothing is held out for validation or testing, so no accuracy score is reported. Judge the model by trying it live.

The visibility threshold is chosen from the gate's own predictions on the recordings: it is the middle of the widest range of thresholds that best separate tongue out from tongue in, clamped to 0.3 to 0.8. The camera/native blend weight is fixed at 0.8, because frames the model trained on can't show how far to trust the camera over the headset's own TongueOut. The **When to show the tongue** setting on the Settings tab still lets you choose cameras only, headset only or both.

Recordings train visibility, extension and horizontal/vertical direction. All other outputs are always sent as 0.

## Local files

- `.local/tongue-captures/<recording>/`: `frames.gray8`, `samples.jsonl`, `metadata.json`, and optional files listing skipped (`excluded_steps.json`) and unticked (`review.json`) poses. Follow-the-dot samples also store `dot`, the dot's position at that frame; their labels use its position 0.35 seconds earlier, and `metadata.json` keeps every route so labels can be recomputed.
- `.local/tongue-models/<run>/`: the gate and direction checkpoints, `request.json`, `progress.json`, `training.log` and `report.json`. Failed or cancelled runs never become selectable.
- `.local/tongue-active.json`: the ID of the model in use. The built-in checkpoints in `models/quest-pro/` are never modified.

Raw recordings, personal weights, reports and the selection are ignored by Git. If `VRFT_TONGUE_MODEL_DIR` is set, it takes priority. New models are still saved but not switched on, and model selection is disabled.

## Command line

`prepare_vrft_tongue.py` converts recordings into arrays for external training workflows. It respects skipped and unticked poses and excludes samples without native tracking.

```powershell
& .local/tongue-python/Scripts/python.exe prepare_vrft_tongue.py `
  '.local/tongue-captures/my-recording' `
  --output '.local/tongue-training/core-224' --size 224
```

`train_vrft_tongue.py` accepts `--request request.json --output <new-model-folder> [--epochs 12]`. The request contains `name`, `device` (`auto`, `cpu` or `cuda`), `base_model_dir`, and `recordings`, a list of recording folders. Use a new output folder for each run. The browser writes these files for you.

## Developer checks

```powershell
& .local/tongue-python/Scripts/python.exe -m unittest -v test_tongue_training
cargo test -p vrft-daemon
node --check crates/daemon/src/quest_pro_training.js
```

The Python suite includes a small synthetic end-to-end training, checkpoint and binary-inference test. It checks that the software works, not how well tracking works on people.
