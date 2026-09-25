"""Local personal training on every recording the user selects.

Uses the MIT-licensed model definitions in qpro_model.py. No sibling repository,
network service, or upload is needed. There is no held-out validation or test
set: both models train for a fixed number of passes and the final weights are
kept. Loss weights follow Qpro-Enhanced-FT's train_tongue_model.py.
"""
import argparse
import copy
import json
import math
import os
from pathlib import Path
import random
import time

import cv2
import numpy as np
import torch

if torch.version.hip:
    # Windows MIOpen HIPRTC cannot compile these tongue-model BatchNorm kernels.
    torch.backends.cudnn.enabled = False
from torch import nn
from torch.nn import functional as F
from torch.utils.data import DataLoader, Dataset, WeightedRandomSampler

from prepare_vrft_tongue import FRAME_BYTES, TARGET_NAMES, usable_samples
from tongue_inference import describe_device, load_checkpoint, synchronize_rocm

GATE = "qpro-stereo-tongue-v8-gate.pt"
DIRECTION = "qpro-stereo-tongue-v8-direction.pt"

# Labels may be graded (extension 0.25-1, directions +/-0.5 or 1, diagonals
# +/-0.7); any magnitude above this counts as an active label.
ACTIVE = .1
SIGNED_COLUMNS = (2, 3, 9)
# Directions a basic run must cover: (column, sign, name).
CORE_DIRECTIONS = ((2, -1, "left"), (2, 1, "right"), (3, 1, "up"), (3, -1, "down"))
# Hidden-tongue hard negatives get extra authority against false TongueOut
# from smiles, teeth, jaw opening and speech.
VISIBLE_WEIGHT, HIDDEN_WEIGHT = 1.25, 1.70
DIRECTION_VISIBILITY_WEIGHT = 1.8
REGRESSION_BETA = .08
COLUMN_WEIGHTS = (0., 1.4, 2.2, 2.2, 2.5, 2.5, 2.5, 2.2, 2.2, 2.)
# Each pose card is sampled equally, but a head still sees many visible zeros
# per active card; the boost stops "always predict zero" from looking good.
ACTIVE_BOOST = (0., 2., 6., 6., 4., 4., 4., 4., 4., 2.)

CAMERA_WEIGHTS = (.5, .65, .8, .9, .95, 1.)
THRESHOLDS = np.round(np.linspace(.1, .95, 86), 2)
THRESHOLD_LIMITS = (.3, .8)
# Frames the model trained on cannot choose the camera/native blend: the camera
# looks near-perfect on them. Keep the reference's preferred weight instead.
PREFERRED_CAMERA_WEIGHT = .8
GATE_FORMULA = "w * camera_visibility + (1-w) * native_TongueOut"
GATE_SELECTION = ("midpoint of the widest contiguous plateau of best F1 - 0.75*FPR thresholds; "
                  "weight ties prefer 0.8 then the higher camera weight; clamped to [0.3, 0.8]")
FOCUS_LABELS = {"gate": "Learning when your tongue is out", "direction": "Learning tongue direction"}
# Follow-the-dot frames are sampled by where the tongue points, so a route
# that happened to linger on one side does not dominate: the centre, then
# eight directions at half and full reach.
DIRECTION_SECTORS = ("right", "up right", "up", "up left", "left", "down left", "down", "down right")


def write_json(path, value, attempts=40):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, allow_nan=False), encoding="utf-8")
    # Windows refuses the rename while another process has either file open:
    # VRFT's status poll reads progress.json every second, and antivirus or
    # the indexer may be scanning the new file. Those holds last milliseconds.
    for attempt in range(attempts):
        try:
            os.replace(temporary, path)
            return
        except PermissionError:
            if attempt == attempts - 1:
                raise
            time.sleep(.05)


def progress(output, stage, message, **fields):
    value = dict(stage=stage, message=message, **fields)
    try:
        write_json(output / "progress.json", value)
    except OSError as error:
        # progress.json only feeds the preview; never lose a run over it.
        print(f"Could not update progress.json: {error}", flush=True)
    print(json.dumps(value, allow_nan=False), flush=True)


def recording_paths(request):
    """The request's recordings, each once, in order."""
    paths = []
    for path in request.get("recordings") or []:
        path = Path(path).resolve()
        if path not in paths:
            paths.append(path)
    if not paths:
        raise ValueError("Choose at least one recording to train on")
    return paths


def moving(sample):
    """Follow-the-dot frames each differ, unlike a held pose."""
    return "dot" in sample


def sampling_key(sample):
    """Group whose frames share one sampling weight: the pose, or for
    follow-the-dot frames the direction the label points."""
    if "dot" not in sample or sample["targets"][0] < .5:
        return sample["pose"]
    horizontal, vertical = sample["targets"][2:4]
    reach = math.hypot(horizontal, vertical)
    if reach < .25:
        return "Follow the dot: centre"
    sector = DIRECTION_SECTORS[round(math.atan2(vertical, horizontal) / (math.pi / 4)) % 8]
    return f"Follow the dot: {'half ' if reach < .75 else ''}{sector}"


def augment_batch(images):
    """A random geometry and exposure change per frame, on the training device.

    Both views of a frame get the same change, which preserves stereo
    correspondence: a small rotation, zoom and shift as from a refitted
    headset, then gamma, gain and offset, sometimes blur, sometimes sensor
    noise (independent per view, like a real sensor).
    """
    count, device = images.shape[0], images.device
    def uniform(low, high):
        return torch.empty(count, device=device).uniform_(low, high)
    angle = uniform(-6, 6) * (math.pi / 180)
    scale = uniform(.92, 1.08)
    cos, sin = torch.cos(angle) / scale, torch.sin(angle) / scale
    # affine_grid coordinates span 2, so 1/12 shifts by 1/24 of the image.
    theta = torch.stack([torch.stack([cos, -sin, uniform(-1 / 12, 1 / 12)], 1),
                         torch.stack([sin, cos, uniform(-1 / 12, 1 / 12)], 1)], 1)
    grid = F.affine_grid(theta, list(images.shape), align_corners=False)
    images = F.grid_sample(images, grid, mode="bilinear", padding_mode="border",
                           align_corners=False).clamp(0, 1)
    shape = (count, 1, 1, 1)
    images = images ** uniform(.8, 1.25).view(shape)
    images = (images * uniform(.88, 1.12).view(shape) + uniform(-.04, .04).view(shape)).clamp(0, 1)
    blurred = F.avg_pool2d(images, 3, stride=1, padding=1, count_include_pad=False)
    images = torch.where(torch.rand(shape, device=device) < .15, blurred, images)
    noisy = (images + torch.randn_like(images) * .008).clamp(0, 1)
    return torch.where(torch.rand(shape, device=device) < .2, noisy, images)


class Frames(Dataset):
    """Evenly subsampled frames of every usable pose. With images=False only
    the labels are read, which is enough for coverage checks."""
    def __init__(self, directories, size, limit_per_pose=90, images=True):
        self.records = []
        self.arrays = []
        self.size = size
        for directory in directories:
            directory = Path(directory)
            samples, raw = usable_samples(directory)
            groups = {}
            for sample in samples:
                groups.setdefault(sample["step"], []).append(sample)
            # Even temporal subsampling prevents long held poses dominating.
            selected = []
            for values in groups.values():
                if moving(values[0]):
                    selected.extend(values)
                    continue
                if len(values) < 8:
                    continue
                indices = np.linspace(0, len(values) - 1, min(limit_per_pose, len(values)), dtype=int)
                selected.extend(values[i] for i in indices)
            selected.sort(key=lambda sample: sample["index"])
            if not images:
                self.records.extend(dict(sample, session=directory.name) for sample in selected)
                continue
            with raw.open("rb") as stream:
                for sample in selected:
                    stream.seek(sample["index"] * FRAME_BYTES)
                    strip = np.frombuffer(stream.read(FRAME_BYTES), np.uint8).reshape(400, 800)
                    self.arrays.append(np.stack([
                        cv2.resize(strip[:, v * 400:(v + 1) * 400], (size, size),
                                   interpolation=cv2.INTER_AREA) for v in range(2)]))
                    self.records.append(dict(sample, session=directory.name))
        if not self.records:
            raise ValueError("No usable poses in these recordings. Record a full basic run, then train again")
        self.targets = np.asarray([s["targets"] for s in self.records], np.float32)
        self.native = np.asarray([s["native_tongue_out"] for s in self.records], np.float32)
        self.follow = np.asarray(["dot" in s for s in self.records], bool)
        self.pose_keys = [sampling_key(s) for s in self.records]

    def __len__(self):
        return len(self.records)

    def __getitem__(self, index):
        image = torch.from_numpy(self.arrays[index].copy()).float() / 255.0
        return image, torch.from_numpy(self.targets[index]), self.native[index]


def coverage(dataset):
    target = dataset.targets
    visible = target[:, 0] >= .5
    result = {"frames": len(dataset), "negative": int((~visible).sum()),
              "positive": int(visible.sum()), "poses": {}, "targets": {}}
    follow = getattr(dataset, "follow", None)
    if follow is not None:
        result["sources"] = {"follow": int(follow.sum()), "poses": int((~follow).sum())}
    for pose in sorted(set(dataset.pose_keys)):
        result["poses"][pose] = dataset.pose_keys.count(pose)
    # Visible-frame label support per head, including the graded levels seen.
    for column, name in enumerate(TARGET_NAMES[1:], 1):
        values = target[visible, column]
        active = values[np.abs(values) > ACTIVE]
        result["targets"][name] = dict(
            positive=int((values > ACTIVE).sum()), negative=int((values < -ACTIVE).sum()),
            levels=sorted({round(float(value), 2) for value in active}))
    return result


def supported_targets(dataset):
    y = dataset.targets
    visible = y[:, 0] >= .5
    if visible.sum() < 20 or (~visible).sum() < 20:
        raise ValueError("Training needs at least 20 tongue-out and 20 tongue-in frames. "
                         "Record a full basic run, then train again")
    enabled = [True, True]
    for column in range(2, 10):
        values = y[visible, column]
        active = np.count_nonzero(values > ACTIVE) >= 8
        if column in SIGNED_COLUMNS:
            active = active and np.count_nonzero(values < -ACTIVE) >= 8
        enabled.append(bool(active))
    return enabled


def trainable_targets(dataset):
    """Heads with enough examples to train; basic directions are required."""
    enabled = supported_targets(dataset)
    y = dataset.targets[dataset.targets[:, 0] >= .5]
    missing = [name for column, sign, name in CORE_DIRECTIONS
               if np.count_nonzero(y[:, column] * sign > ACTIVE) < 8]
    if missing:
        raise ValueError(f"No usable tongue {', '.join(missing)} poses. Record a full basic run, then train again")
    return enabled


def disabled_names(enabled):
    """Heads masked out of the loss; inference outputs 0.0 for them."""
    return [name for name, yes in zip(TARGET_NAMES, enabled) if not yes]


def visibility_loss(prediction, target):
    """Class-weighted BCE on the post-sigmoid visibility head.

    Written out because F.binary_cross_entropy rejects CUDA autocast. Convert
    to float32 before clamping: float16 rounds 1 - 1e-5 back to exactly 1,
    which would still make log1p(-p) infinite.
    """
    probability = prediction[:, 0].float().clamp(1e-5, 1 - 1e-5)
    expected = target[:, 0].float()
    loss = -(expected * torch.log(probability) + (1 - expected) * torch.log1p(-probability))
    return (loss * torch.where(expected >= .5, VISIBLE_WEIGHT, HIDDEN_WEIGHT)).mean()


def regression_weights(target, enabled):
    """Per-element regression weights: visible frames only, enabled heads only."""
    target = target.float()
    columns = target.new_tensor(COLUMN_WEIGHTS) * target.new_tensor([float(yes) for yes in enabled])
    visible = (target[:, :1] >= .5).float()
    active = (target.abs() > ACTIVE).float()
    weights = visible * columns * (1 + target.new_tensor(ACTIVE_BOOST) * active)
    return weights


def loss_for(prediction, target, enabled, focus):
    visibility = visibility_loss(prediction, target)
    if focus == "gate":
        return visibility
    prediction, target = prediction.float(), target.float()
    weights = regression_weights(target, enabled)
    error = F.smooth_l1_loss(prediction, target, reduction="none", beta=REGRESSION_BETA)
    regression = (error * weights).sum() / weights.sum().clamp_min(1)
    return DIRECTION_VISIBILITY_WEIGHT * visibility + regression


def predict(model, dataset, device, batch_size):
    model.eval()
    result = []
    with torch.inference_mode():
        for images, *_ in DataLoader(dataset, batch_size=batch_size):
            result.append(model(images.to(device)).float().cpu().numpy())
    values = np.concatenate(result)
    if not np.isfinite(values).all():
        raise ValueError("Non-finite model predictions")
    return values


def classify(probability, expected, threshold):
    predicted = probability >= threshold
    actual = expected >= .5
    tp, fp = int((predicted & actual).sum()), int((predicted & ~actual).sum())
    fn, tn = int((~predicted & actual).sum()), int((~predicted & ~actual).sum())
    return dict(precision=tp / max(1, tp + fp), recall=tp / max(1, tp + fn),
                f1=2 * tp / max(1, 2 * tp + fp + fn),
                false_positive_rate=fp / max(1, fp + tn),
                false_negative_rate=fn / max(1, fn + tp), tp=tp, fp=fp, fn=fn, tn=tn)


def gate_objective(metrics):
    return metrics["f1"] - .75 * metrics["false_positive_rate"]


def best_runs(scores, best, tolerance=1e-9):
    """Contiguous (start, end) index runs whose score equals the best score."""
    runs, start = [], None
    for index, score in enumerate(scores):
        if score >= best - tolerance:
            start = index if start is None else start
        elif start is not None:
            runs.append((start, index - 1))
            start = None
    if start is not None:
        runs.append((start, len(scores) - 1))
    return runs


def calibrate_gate(camera, native, expected, weights=CAMERA_WEIGHTS, thresholds=THRESHOLDS):
    """Choose the camera/native blend weight and visibility threshold.

    Many thresholds usually tie. Breaking ties toward the largest
    threshold and weight pinned the shipped gate at the grid edge, where live
    detection flickers across the daemon's hysteresis band. Instead use the
    weight with the widest contiguous plateau of best-scoring thresholds (ties:
    weight nearest 0.8, then the higher camera weight) and that plateau's
    midpoint, clamped to [0.3, 0.8].
    """
    camera, native = np.asarray(camera, np.float64), np.asarray(native, np.float64)
    expected = np.asarray(expected, np.float64)
    table = {}
    for weight in weights:
        fused = weight * camera + (1 - weight) * native
        table[weight] = [gate_objective(classify(fused, expected, float(t))) for t in thresholds]
    best = max(max(scores) for scores in table.values())
    center = sum(THRESHOLD_LIMITS) / 2
    candidates = []
    for weight, scores in table.items():
        for start, end in best_runs(scores, best):
            middle = (float(thresholds[start]) + float(thresholds[end])) / 2
            key = (end - start + 1, -round(abs(weight - PREFERRED_CAMERA_WEIGHT), 6),
                   weight, -round(abs(middle - center), 6))
            candidates.append((key, weight, start, end, middle))
    _, weight, start, end, middle = max(candidates, key=lambda candidate: candidate[0])
    threshold = min(max(middle, THRESHOLD_LIMITS[0]), THRESHOLD_LIMITS[1])
    return dict(cameraWeight=float(weight), threshold=round(float(threshold), 4),
                plateau=[float(thresholds[start]), float(thresholds[end])],
                objective=float(best), formula=GATE_FORMULA, selection=GATE_SELECTION)


def calibrate(prediction, dataset):
    """Visibility threshold at the preferred camera weight."""
    return calibrate_gate(prediction[:, 0], dataset.native, dataset.targets[:, 0],
                          weights=(PREFERRED_CAMERA_WEIGHT,))


class Tracker:
    """Overall progress across both models, with a time estimate."""
    def __init__(self, output, epochs, device):
        self.output, self.epochs, self.device = output, epochs, describe_device(device)
        self.started = time.monotonic()
        self.written = 0.

    def update(self, focus, epoch, part, force=False):
        now = time.monotonic()
        if not force and now - self.written < 2:
            return
        self.written = now
        step = 1 if focus == "gate" else 2
        done = ((step - 1) * self.epochs + epoch - 1 + part) / (2 * self.epochs)
        elapsed = now - self.started
        remaining = round(elapsed * (1 - done) / done) if done >= .02 else None
        progress(self.output, "training",
                 f"{FOCUS_LABELS[focus]} (step {step} of 2, pass {epoch} of {self.epochs})",
                 focus=focus, epoch=epoch, epochs=self.epochs, fraction=round(done, 4),
                 eta_seconds=remaining, device=self.device)


def train_one(source, focus, recordings, output, args, device, enabled, tracker, gate=None):
    """Fine-tune one checkpoint for a fixed number of passes and keep the result.

    Without a held-out set, the gate's visibility threshold is chosen from its
    own predictions on the unaugmented training frames; the direction model
    reuses the gate's calibration, which is the one inference reads.
    """
    model, size, original = load_checkpoint(str(source), device)
    frames = Frames(recordings, size)
    counts = {pose: frames.pose_keys.count(pose) for pose in set(frames.pose_keys)}
    sampler = WeightedRandomSampler([1 / counts[p] for p in frames.pose_keys],
                                    len(frames), replacement=True,
                                    generator=torch.Generator().manual_seed(42))
    loader = DataLoader(frames, batch_size=args.batch_size, sampler=sampler)
    optimizer = torch.optim.AdamW(model.parameters(), lr=args.learning_rate, weight_decay=1e-4)
    # Mixed precision on CUDA/ROCm GPUs; the loss is computed in float32.
    use_amp = device.type == "cuda"
    scaler = torch.amp.GradScaler("cuda", enabled=use_amp)
    for epoch in range(1, args.epochs + 1):
        tracker.update(focus, epoch, 0., force=True)
        model.train()
        # Tiny personal batches should not rewrite population BatchNorm stats.
        for layer in model.modules():
            if isinstance(layer, nn.BatchNorm2d):
                layer.eval()
        for batch, (images, target, _) in enumerate(loader, 1):
            optimizer.zero_grad(set_to_none=True)
            images = augment_batch(images.to(device))
            with torch.autocast(device_type=device.type, enabled=use_amp):
                prediction = model(images)
            loss = loss_for(prediction, target.to(device), enabled, focus)
            if not torch.isfinite(loss):
                raise ValueError("Training loss is not finite")
            scaler.scale(loss).backward()
            scaler.unscale_(optimizer)
            nn.utils.clip_grad_norm_(model.parameters(), 1.)
            scaler.step(optimizer)
            scaler.update()
            tracker.update(focus, epoch, batch / len(loader))
    if gate is None:
        progress(output, "calibrating", "Tuning when the tongue counts as out",
                 focus=focus, fraction=.5, device=tracker.device)
        gate = calibrate(predict(model, frames, device, args.batch_size), frames)
    checkpoint = dict(original)
    checkpoint.update(modelState=copy.deepcopy({k: v.cpu() for k, v in model.state_dict().items()}),
                      visibilityGate=gate, supportedTargets=list(enabled),
                      disabledTargets=disabled_names(enabled),
                      personalTraining=dict(focus=focus, epochs=args.epochs, frames=len(frames),
                                            recordings=[str(path) for path in recordings],
                                            parentCheckpoint=str(Path(source).resolve())))
    torch.save(checkpoint, output / (GATE if focus == "gate" else DIRECTION))
    return checkpoint


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--epochs", type=int, default=12)
    parser.add_argument("--batch-size", type=int, default=12)
    parser.add_argument("--learning-rate", type=float, default=1e-4)
    args = parser.parse_args()
    if args.epochs < 1 or args.batch_size < 1 or args.learning_rate <= 0:
        parser.error("Training settings must be positive")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if any((output / filename).exists() for filename in (GATE, DIRECTION, "report.json", "progress.json")):
        raise ValueError("Use a new output directory; existing model runs are never overwritten")
    device = None
    try:
        torch.set_num_threads(min(4, os.cpu_count() or 1))
        torch.manual_seed(42); np.random.seed(42); random.seed(42)
        request = json.loads(args.request.read_text(encoding="utf-8"))
        recordings = recording_paths(request)
        progress(output, "checking", "Checking your recordings", fraction=0.)
        labels = Frames(recordings, 32, images=False)
        coverage_report = coverage(labels)
        enabled = trainable_targets(labels)
        source = Path(request["base_model_dir"])
        requested_device = request.get("device", "auto")
        if requested_device not in ("auto", "cpu", "cuda"):
            raise ValueError("Device must be auto, cpu, or cuda")
        selected = torch.device("cuda" if requested_device == "auto" and torch.cuda.is_available()
                                else "cpu" if requested_device == "auto" else requested_device)
        if selected.type == "cuda" and not torch.cuda.is_available():
            raise ValueError("This Python runtime has no available CUDA or ROCm GPU; choose Automatic or rerun "
                             "setup-quest-pro-tongue.ps1 -Accelerator cuda (NVIDIA) or rocm (AMD)")
        device = selected
        tracker = Tracker(output, args.epochs, device)
        gate = train_one(source / GATE, "gate", recordings, output, args, device, enabled, tracker)
        train_one(source / DIRECTION, "direction", recordings, output, args, device, enabled, tracker,
                  gate=gate["visibilityGate"])
        calibration = gate["visibilityGate"]
        report = dict(name=request.get("name", output.name), device=describe_device(device),
                      recordings=[str(path) for path in recordings], frames=coverage_report["frames"],
                      coverage=coverage_report, epochs=args.epochs,
                      supported_targets=[n for n, yes in zip(TARGET_NAMES, enabled) if yes],
                      disabled_targets=disabled_names(enabled), base_model_dir=str(source),
                      calibration=dict(camera_weight=calibration["cameraWeight"],
                                       threshold=calibration["threshold"],
                                       plateau=calibration["plateau"]),
                      seconds=round(time.monotonic() - tracker.started, 1))
        write_json(output / "report.json", report)
        progress(output, "complete", "Training complete", fraction=1., report=report)
    except Exception as error:
        progress(output, "failed", str(error))
        raise
    finally:
        if device is not None:
            synchronize_rocm(device)


if __name__ == "__main__":
    main()
