//! Personal training on the recordings the user ticks, a port of
//! Qpro-Enhanced-FT's fine-tuning (MIT license) as VRFT's Python trainer ran
//! it. Every run starts from the base pair and fine-tunes the gate, then the
//! direction model, for a fixed number of passes and keeps the final
//! weights. There is no held-out set, so no accuracy score is reported.
//!
//! The cheek puff heads are VRFT's own. The base pair has none, and the
//! fine-tuning learning rate is far too small to grow a head from nothing,
//! so each is fitted by least squares on the direction model's features
//! before fine-tuning starts, then again on the fine-tuned features.
//!
//! The daemon runs this in a child process (`vrft_d train-tongue`) and
//! reads `progress.json`; cancelling kills the process.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use burn::backend::Autodiff;
use burn::module::{AutodiffModule, ModuleVisitor, Param};
use burn::optim::{AdamWConfig, GradientsParams, Optimizer};
use burn::tensor::activation::log_sigmoid;
use burn::tensor::backend::{AutodiffBackend, Backend};
use burn::tensor::{ElementConversion, Tensor, TensorData};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rayon::prelude::*;
use serde_json::{json, Value};

use crate::backend::{Accelerator, Cpu, Gpu};
use crate::checkpoint::{Checkpoint, Metadata, Role, VisibilityGate};
use crate::dataset::Record;
use crate::dataset::{augment, Frames, ACTIVE};
use crate::infer::guarded;
use crate::model::{TongueNet, Weights, FEATURES, OUTPUT_BIAS, OUTPUT_WEIGHT};
use crate::recordings::Recording;
use crate::{CHEEK_COLUMNS, TARGETS};
use vrft_quest_pro_protocol::{ReportCalibration, TrainingProgress, TrainingReport, TrainingStage};

// Loss weights follow Qpro-Enhanced-FT's train_tongue_model.py.
/// Hidden-tongue hard negatives get extra authority against false
/// TongueOut from smiles, teeth, jaw opening and speech.
const VISIBLE_WEIGHT: f32 = 1.25;
const HIDDEN_WEIGHT: f32 = 1.70;
const DIRECTION_VISIBILITY_WEIGHT: f32 = 1.8;
const REGRESSION_BETA: f32 = 0.08;
const COLUMN_WEIGHTS: [f32; 12] = [0., 1.4, 2.2, 2.2, 2.5, 2.5, 2.5, 2.2, 2.2, 2., 2., 2.];
/// Each pose is sampled equally, but a head still sees many visible zeros
/// per active pose; the boost stops "always predict zero" from looking good.
const ACTIVE_BOOST: [f32; 12] = [0., 2., 6., 6., 4., 4., 4., 4., 4., 2., 3., 3.];
/// Cheek fits aim for these instead of 0 and 1, whose logits are infinite.
const CHEEK_FIT_RANGE: (f64, f64) = (0.03, 0.97);
/// Ridge strength of the cheek fits, relative to the features' mean energy.
const CHEEK_FIT_RIDGE: f64 = 0.05;
const MAX_GRADIENT_NORM: f32 = 1.0;

const THRESHOLD_LIMITS: (f64, f64) = (0.3, 0.8);
/// Frames the model trained on cannot choose the camera/native blend: the
/// camera looks near-perfect on them. Keep the reference's preferred weight.
const PREFERRED_CAMERA_WEIGHT: f64 = 0.8;
/// The weight when any frame was recorded without a tracking module, whose
/// TongueOut the blend would need: the camera alone.
const CAMERA_ONLY_WEIGHT: f64 = 1.0;
const GATE_FORMULA: &str = "w * camera_visibility + (1-w) * native_TongueOut";
const GATE_SELECTION: &str = "midpoint of the widest contiguous plateau of best F1 - 0.75*FPR \
    thresholds; weight ties prefer 0.8 then the higher camera weight; clamped to [0.3, 0.8]";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Gate,
    Direction,
}

impl Focus {
    fn role(self) -> Role {
        match self {
            Focus::Gate => Role::Gate,
            Focus::Direction => Role::Direction,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Focus::Gate => "gate",
            Focus::Direction => "direction",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Focus::Gate => "Learning when your tongue is out",
            Focus::Direction => "Learning tongue direction",
        }
    }
}

/// What to train, from the new model folder's `request.json`.
pub use vrft_quest_pro_protocol::TrainerRequest as Request;

pub struct Options {
    pub epochs: usize,
    pub batch_size: usize,
    pub learning_rate: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            epochs: 12,
            batch_size: 12,
            learning_rate: 1e-4,
        }
    }
}

/// Writes JSON via a temporary file. Windows refuses the rename while
/// another process has either file open (VRFT's status poll reads
/// progress.json every second, and antivirus may be scanning the new file);
/// those holds last milliseconds, so retry briefly.
fn write_json(path: &Path, value: &Value) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    for attempt in 0.. {
        match std::fs::rename(&temporary, path) {
            Ok(()) => return Ok(()),
            Err(error) if attempt < 40 && error.kind() == std::io::ErrorKind::PermissionDenied => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!()
}

struct Progress {
    output: PathBuf,
}

impl Progress {
    fn report(&self, progress: TrainingProgress) {
        let value = serde_json::to_value(&progress).expect("progress serializes");
        // progress.json only feeds the app; never lose a run over it.
        if let Err(error) = write_json(&self.output.join("progress.json"), &value) {
            println!("Could not update progress.json: {error}");
        }
        println!("{value}");
    }
}

/// Overall progress across both models, with a time estimate.
struct Tracker<'a> {
    progress: &'a Progress,
    epochs: usize,
    device: String,
    started: Instant,
    written: Option<Instant>,
    /// When the first batch finished, and how far along the run was then.
    /// That batch also tunes the GPU's kernels, which can take minutes while
    /// a game shares the GPU, so the estimate is timed from after it.
    warmed: Option<(Instant, f64)>,
}

/// Seconds left at `done`, from the pace since the run was `from` done,
/// `elapsed` seconds ago. `None` until a few seconds and some progress say
/// what the pace is.
fn estimate(elapsed: f64, from: f64, done: f64) -> Option<f64> {
    let progress = done - from;
    (elapsed >= 10.0 && progress > 0.0).then(|| (elapsed * (1.0 - done) / progress).round())
}

impl Tracker<'_> {
    fn update(&mut self, focus: Focus, epoch: usize, part: f64, force: bool) {
        let step = if focus == Focus::Gate { 1 } else { 2 };
        let done = ((step - 1) * self.epochs + epoch - 1) as f64 + part;
        let done = done / (2 * self.epochs) as f64;
        if self.warmed.is_none() && part > 0.0 {
            self.warmed = Some((Instant::now(), done));
        }
        if !force
            && self
                .written
                .is_some_and(|at| at.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.written = Some(Instant::now());
        let remaining = self
            .warmed
            .and_then(|(at, from)| estimate(at.elapsed().as_secs_f64(), from, done));
        self.progress.report(TrainingProgress {
            focus: Some(focus.name().into()),
            epoch: Some(epoch as u32),
            epochs: Some(self.epochs as u32),
            fraction: Some(((done * 10000.0).round() / 10000.0) as f32),
            eta_seconds: remaining,
            device: Some(self.device.clone()),
            ..TrainingProgress::new(
                TrainingStage::Training,
                format!(
                    "{} (step {step} of 2, pass {epoch} of {})",
                    focus.label(),
                    self.epochs
                ),
            )
        });
    }
}

/// Runs a training request, writing the new pair, `report.json` and
/// `progress.json` into `output`, which must be a new folder.
pub fn run(request: &Path, output: &Path, options: &Options) -> Result<()> {
    if options.epochs == 0 || options.batch_size == 0 || options.learning_rate <= 0.0 {
        bail!("Training settings must be positive");
    }
    std::fs::create_dir_all(output)?;
    // absolute() rather than canonicalize(), which adds \\?\ on Windows.
    let output = std::path::absolute(output)?;
    let existing = [Role::Gate, Role::Direction]
        .iter()
        .any(|role| role.find(&output).is_some())
        || ["report.json", "progress.json"]
            .iter()
            .any(|file| output.join(file).exists());
    if existing {
        bail!("Use a new output directory; existing model runs are never overwritten");
    }
    let progress = Progress {
        output: output.clone(),
    };
    let result = (|| {
        let request: Request =
            serde_json::from_slice(&std::fs::read(request)?).context("invalid training request")?;
        let accelerator: Accelerator = request
            .device
            .name()
            .parse()
            .map_err(|error: String| anyhow!(error))?;
        progress.report(TrainingProgress {
            fraction: Some(0.0),
            ..TrainingProgress::new(TrainingStage::Checking, "Checking your recordings")
        });
        let mut paths = vec![];
        for path in &request.recordings {
            let path = std::path::absolute(path)?;
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            bail!("Choose at least one recording to train on");
        }
        let recordings = paths
            .iter()
            .map(|path| Recording::open(path))
            .collect::<Result<Vec<_>>>()?;
        let labels = Frames::load(&recordings, None)?;
        let enabled = labels.trainable_targets()?;
        let job = Job {
            request: &request,
            recordings: &recordings,
            coverage: labels.coverage(),
            enabled,
            output: &output,
            options,
            progress: &progress,
        };
        match accelerator {
            Accelerator::Cpu => job.run::<Autodiff<Cpu>>("CPU"),
            Accelerator::Gpu => job.run::<Autodiff<Gpu>>("GPU (wgpu)"),
            Accelerator::Auto => {
                // Prove the GPU works before committing to it.
                let works = guarded(|| {
                    let device = Default::default();
                    let probe = Tensor::<Gpu, 1>::from_floats([1.0, 2.0], &device).sum();
                    probe.into_scalar();
                    Ok(())
                });
                match works {
                    Ok(()) => job.run::<Autodiff<Gpu>>("GPU (wgpu)"),
                    Err(error) => {
                        println!("GPU unavailable ({error:#}); training on the CPU");
                        job.run::<Autodiff<Cpu>>("CPU")
                    }
                }
            }
        }
    })();
    if let Err(error) = &result {
        progress.report(TrainingProgress::new(
            TrainingStage::Failed,
            format!("{error:#}"),
        ));
    }
    result
}

struct Job<'a> {
    request: &'a Request,
    recordings: &'a [Recording],
    coverage: Value,
    enabled: [bool; TARGETS.len()],
    output: &'a Path,
    options: &'a Options,
    progress: &'a Progress,
}

impl Job<'_> {
    fn run<B: AutodiffBackend>(&self, device_name: &str) -> Result<()> {
        let device = B::Device::default();
        B::seed(&device, 42);
        let mut tracker = Tracker {
            progress: self.progress,
            epochs: self.options.epochs,
            device: device_name.into(),
            started: Instant::now(),
            written: None,
            warmed: None,
        };
        let base = &self.request.base_model_dir;
        let gate = guarded(|| self.train_one::<B>(base, Focus::Gate, None, &mut tracker, &device))?;
        let calibration = gate.visibility_gate.clone();
        guarded(|| {
            self.train_one::<B>(
                base,
                Focus::Direction,
                Some(calibration.clone()),
                &mut tracker,
                &device,
            )
        })?;
        let disabled = disabled_names(&self.enabled);
        let report = TrainingReport {
            name: self.request.name.clone().unwrap_or_else(|| {
                self.output
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            }),
            device: device_name.into(),
            recordings: self
                .recordings
                .iter()
                .map(|r| r.dir.display().to_string())
                .collect(),
            frames: self.coverage["frames"].as_u64(),
            coverage: self.coverage.clone(),
            epochs: self.options.epochs as u32,
            supported_targets: TARGETS
                .iter()
                .zip(self.enabled)
                .filter(|(_, yes)| *yes)
                .map(|(name, _)| name.to_string())
                .collect(),
            disabled_targets: disabled,
            base_model_dir: base.display().to_string(),
            calibration: ReportCalibration {
                camera_weight: calibration.camera_weight,
                threshold: calibration.threshold,
                plateau: calibration.details.get("plateau").cloned(),
            },
            seconds: Some((tracker.started.elapsed().as_secs_f64() * 10.0).round() / 10.0),
        };
        write_json(
            &self.output.join("report.json"),
            &serde_json::to_value(&report)?,
        )?;
        self.progress.report(TrainingProgress {
            fraction: Some(1.0),
            report: Some(report),
            ..TrainingProgress::new(TrainingStage::Complete, "Training complete")
        });
        Ok(())
    }

    /// Fine-tunes one checkpoint and saves it. Without a held-out set, the
    /// gate's visibility threshold is chosen from its own predictions on the
    /// unaugmented training frames; the direction model reuses the gate's
    /// calibration, which is the one inference reads.
    fn train_one<B: AutodiffBackend>(
        &self,
        base: &Path,
        focus: Focus,
        calibration: Option<VisibilityGate>,
        tracker: &mut Tracker,
        device: &B::Device,
    ) -> Result<Metadata> {
        let source = focus
            .role()
            .find(base)
            .with_context(|| format!("the base model pair is missing from {}", base.display()))?;
        let checkpoint = Checkpoint::load(&source)?;
        let size = checkpoint.metadata.image_size;
        let frames = Frames::load(self.recordings, Some(size))?;
        let batch_size = self.options.batch_size;
        let fit_cheeks =
            focus == Focus::Direction && CHEEK_COLUMNS.iter().any(|&column| self.enabled[column]);
        let mut weights = checkpoint.weights;
        if fit_cheeks {
            self.report_cheek_fit(tracker, 0.0);
            let base = TongueNet::<B::InnerBackend>::from_weights(weights.clone(), device)?;
            let features = features(&base, &frames, batch_size, device)?;
            self.fit_cheeks(&features, &frames, &mut weights)?;
        }
        let mut model = TongueNet::<B>::from_weights(weights, device)?;
        let mut optimizer = AdamWConfig::new()
            .with_weight_decay(1e-4)
            .with_epsilon(1e-8)
            .init::<B, TongueNet<B>>();
        let mut order_rng = StdRng::seed_from_u64(42);
        let mut augment_rng = StdRng::seed_from_u64(42);
        let plane = 2 * size * size;
        for epoch in 1..=self.options.epochs {
            tracker.update(focus, epoch, 0.0, true);
            let order = frames.balanced_order(&mut order_rng);
            let batches = order.len().div_ceil(batch_size);
            for (batch, indices) in order.chunks(batch_size).enumerate() {
                let count = indices.len();
                let mut pixels = vec![0f32; count * plane];
                let mut targets = Vec::with_capacity(count * TARGETS.len());
                let cheeks: Vec<bool> = indices
                    .iter()
                    .map(|&index| frames.records[index].cheeks_labelled)
                    .collect();
                // One seed per frame keeps runs repeatable while frames
                // augment in parallel.
                let seeds: Vec<u64> = indices.iter().map(|_| augment_rng.random()).collect();
                pixels
                    .par_chunks_mut(plane)
                    .zip(indices.par_iter().zip(&seeds))
                    .for_each(|(out, (&index, &seed))| {
                        let mut rng = StdRng::seed_from_u64(seed);
                        augment(&frames.images[index], size, &mut rng, out);
                    });
                for &index in indices {
                    targets.extend(frames.records[index].targets);
                }
                let images = Tensor::<B, 4>::from_data(
                    TensorData::new(pixels, [count, 2, size, size]),
                    device,
                );
                let raw = model.forward_raw(images);
                let prediction = model.activate(raw.clone());
                let loss = loss(
                    &self.enabled,
                    raw,
                    prediction,
                    &targets,
                    &cheeks,
                    focus,
                    device,
                );
                let value: f32 = loss.clone().into_scalar().elem();
                if !value.is_finite() {
                    bail!("Training loss is not finite");
                }
                let mut grads = GradientsParams::from_grads(loss.backward(), &model);
                clip_gradients(&model, &mut grads, MAX_GRADIENT_NORM);
                model = optimizer.step(self.options.learning_rate, model, grads);
                tracker.update(focus, epoch, (batch + 1) as f64 / batches as f64, false);
            }
        }
        let trained = model.valid();
        let mut weights = trained.weights();
        if fit_cheeks {
            self.report_cheek_fit(tracker, 1.0);
            let features = features(&trained, &frames, batch_size, device)?;
            self.fit_cheeks(&features, &frames, &mut weights)?;
        }
        let calibration = match calibration {
            Some(calibration) => calibration,
            None => {
                self.progress.report(TrainingProgress {
                    focus: Some(focus.name().into()),
                    fraction: Some(0.5),
                    device: Some(tracker.device.clone()),
                    ..TrainingProgress::new(
                        TrainingStage::Calibrating,
                        "Tuning when the tongue counts as out",
                    )
                });
                let camera = predict(&trained, &frames, batch_size, device)?;
                calibrate(&camera, &frames)
            }
        };
        let metadata = Metadata {
            architecture: checkpoint.metadata.architecture,
            image_size: size,
            visibility_gate: calibration,
            disabled_targets: disabled_names(&self.enabled),
            personal_training: Some(json!({
                "focus": focus.name(),
                "epochs": self.options.epochs,
                "frames": frames.len(),
                "recordings": self.recordings.iter().map(|r| r.dir.display().to_string()).collect::<Vec<_>>(),
                "parentCheckpoint": source.display().to_string(),
            })),
        };
        Checkpoint {
            metadata: metadata.clone(),
            weights,
        }
        .save(&focus.role().safetensors(self.output))?;
        Ok(metadata)
    }

    /// Reports fitting the cheek heads, before (`0`) or after (`1`)
    /// fine-tuning the direction model.
    fn report_cheek_fit(&self, tracker: &Tracker, fraction: f32) {
        self.progress.report(TrainingProgress {
            focus: Some(Focus::Direction.name().into()),
            fraction: Some(0.5 + fraction / 2.0),
            device: Some(tracker.device.clone()),
            ..TrainingProgress::new(TrainingStage::Calibrating, "Learning your cheek puffs")
        });
    }

    /// Fits the last layer's row of each trainable cheek head to the frames
    /// whose cheeks were labelled, with each pose weighted equally.
    fn fit_cheeks(
        &self,
        features: &[Vec<f32>],
        frames: &Frames,
        weights: &mut Weights,
    ) -> Result<()> {
        let labelled: Vec<(&[f32], &Record)> = features
            .iter()
            .zip(&frames.records)
            .filter(|(_, record)| record.cheeks_labelled)
            .map(|(row, record)| (row.as_slice(), record))
            .collect();
        let mut counts = std::collections::HashMap::<&str, f64>::new();
        for (_, record) in &labelled {
            *counts.entry(&record.key).or_default() += 1.0;
        }
        let values = |name: &str| {
            weights[name]
                .clone()
                .to_vec::<f32>()
                .map_err(|error| anyhow!("{error:?}"))
        };
        let (mut weight, mut bias) = (values(OUTPUT_WEIGHT)?, values(OUTPUT_BIAS)?);
        for column in CHEEK_COLUMNS {
            if !self.enabled[column] {
                continue;
            }
            let (low, high) = CHEEK_FIT_RANGE;
            let samples: Vec<Sample> = labelled
                .iter()
                .map(|&(features, record)| {
                    let target = f64::from(record.targets[column]).clamp(low, high);
                    Sample {
                        features,
                        weight: 1.0 / counts[record.key.as_str()],
                        target: (target / (1.0 - target)).ln(),
                    }
                })
                .collect();
            let fitted = ridge(&samples)?;
            for (feature, value) in fitted[..FEATURES].iter().enumerate() {
                weight[column * FEATURES + feature] = *value as f32;
            }
            bias[column] = fitted[FEATURES] as f32;
        }
        weights.insert(
            OUTPUT_WEIGHT.into(),
            TensorData::new(weight, [TARGETS.len(), FEATURES]),
        );
        weights.insert(OUTPUT_BIAS.into(), TensorData::new(bias, [TARGETS.len()]));
        Ok(())
    }
}

/// One frame of a cheek fit.
struct Sample<'a> {
    features: &'a [f32],
    weight: f64,
    /// The label's logit.
    target: f64,
}

/// Weighted ridge regression of each sample's target on its features plus
/// a constant: the `FEATURES` coefficients, then the intercept, which is
/// not penalised.
fn ridge(samples: &[Sample]) -> Result<Vec<f64>> {
    let n = FEATURES + 1;
    let mut gram = vec![0f64; n * n];
    let mut moment = vec![0f64; n];
    let mut row = vec![0f64; n];
    for sample in samples {
        for (value, feature) in row.iter_mut().zip(sample.features) {
            *value = f64::from(*feature);
        }
        row[FEATURES] = 1.0;
        for i in 0..n {
            let scaled = sample.weight * row[i];
            moment[i] += scaled * sample.target;
            for j in 0..=i {
                gram[i * n + j] += scaled * row[j];
            }
        }
    }
    let energy = (0..FEATURES).map(|i| gram[i * n + i]).sum::<f64>() / FEATURES as f64;
    for i in 0..FEATURES {
        gram[i * n + i] += CHEEK_FIT_RIDGE * energy.max(1e-9);
    }
    // A trace on the intercept too, so the system stays solvable.
    gram[n * n - 1] += 1e-9;
    // Cholesky, gram = L L^T, in place in the lower triangle.
    for j in 0..n {
        let diagonal = gram[j * n + j] - (0..j).map(|k| gram[j * n + k].powi(2)).sum::<f64>();
        if diagonal.is_nan() || diagonal <= 0.0 {
            bail!("The cheek puff fit has no solution");
        }
        let diagonal = diagonal.sqrt();
        gram[j * n + j] = diagonal;
        for i in j + 1..n {
            let dot = (0..j)
                .map(|k| gram[i * n + k] * gram[j * n + k])
                .sum::<f64>();
            gram[i * n + j] = (gram[i * n + j] - dot) / diagonal;
        }
    }
    let mut solution = moment;
    for i in 0..n {
        let dot = (0..i).map(|k| gram[i * n + k] * solution[k]).sum::<f64>();
        solution[i] = (solution[i] - dot) / gram[i * n + i];
    }
    for i in (0..n).rev() {
        let dot = (i + 1..n)
            .map(|k| gram[k * n + i] * solution[k])
            .sum::<f64>();
        solution[i] = (solution[i] - dot) / gram[i * n + i];
    }
    if solution.iter().any(|value| !value.is_finite()) {
        bail!("The cheek puff fit is not finite");
    }
    Ok(solution)
}

/// The reference trainer's loss: class-weighted BCE on visibility, plus
/// for the direction model a weighted smooth L1 on visible frames and
/// trainable heads. The cheek heads count on every frame whose cheeks were
/// labelled, tongue out or not.
fn loss<B: Backend>(
    enabled: &[bool; TARGETS.len()],
    raw: Tensor<B, 2>,
    prediction: Tensor<B, 2>,
    targets: &[f32],
    cheeks_labelled: &[bool],
    focus: Focus,
    device: &B::Device,
) -> Tensor<B, 1> {
    let heads = TARGETS.len();
    let count = targets.len() / heads;
    // Class-weighted BCE on the visibility head, from its logit. Clamping the
    // post-sigmoid probability instead zeroes the gradient of any frame whose
    // output saturates, and over a full run the gate could saturate at "out"
    // for every frame with nothing left to pull it back.
    let expected: Vec<f32> = targets.chunks(heads).map(|row| row[0]).collect();
    let class_weight: Vec<f32> = expected
        .iter()
        .map(|&e| {
            if e >= 0.5 {
                VISIBLE_WEIGHT
            } else {
                HIDDEN_WEIGHT
            }
        })
        .collect();
    let expected = Tensor::<B, 1>::from_floats(expected.as_slice(), device);
    let logit = raw.narrow(1, 0, 1).reshape([count]);
    let bce = (expected.clone() * log_sigmoid(logit.clone())
        + (expected.neg() + 1.0) * log_sigmoid(logit.neg()))
    .neg();
    let visibility = (bce * Tensor::<B, 1>::from_floats(class_weight.as_slice(), device)).mean();
    if focus == Focus::Gate {
        return visibility;
    }
    // Smooth L1 on labelled frames and trainable heads only.
    let weights: Vec<f32> = targets
        .chunks(heads)
        .zip(cheeks_labelled)
        .flat_map(|(row, &cheeks)| {
            let visible = row[0] >= 0.5;
            (0..heads).map(move |column| {
                let labelled = if CHEEK_COLUMNS.contains(&column) {
                    cheeks
                } else {
                    visible
                };
                let labelled = if labelled { 1.0 } else { 0.0 };
                let trainable = if enabled[column] { 1.0 } else { 0.0 };
                let active = if row[column].abs() > ACTIVE { 1.0 } else { 0.0 };
                labelled
                    * COLUMN_WEIGHTS[column]
                    * trainable
                    * (1.0 + ACTIVE_BOOST[column] * active)
            })
        })
        .collect();
    let total_weight: f32 = weights.iter().sum();
    let weights = Tensor::<B, 2>::from_data(TensorData::new(weights, [count, heads]), device);
    let targets =
        Tensor::<B, 2>::from_data(TensorData::new(targets.to_vec(), [count, heads]), device);
    let difference = (prediction - targets).abs();
    let quadratic = difference.clone().powi_scalar(2) * (0.5 / REGRESSION_BETA);
    let linear = difference.clone() - 0.5 * REGRESSION_BETA;
    let error = linear.mask_where(difference.lower_elem(REGRESSION_BETA), quadratic);
    let regression = (error * weights).sum() / total_weight.max(1.0);
    visibility * DIRECTION_VISIBILITY_WEIGHT + regression
}

fn disabled_names(enabled: &[bool; TARGETS.len()]) -> Vec<String> {
    TARGETS
        .iter()
        .zip(enabled)
        .filter(|(_, yes)| !**yes)
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Scales every gradient so their combined L2 norm is at most `max_norm`,
/// like `torch.nn.utils.clip_grad_norm_` (Burn's own clipping is per
/// parameter).
fn clip_gradients<B: AutodiffBackend, M: AutodiffModule<B>>(
    model: &M,
    grads: &mut GradientsParams,
    max_norm: f32,
) {
    struct SquaredNorm<'a, B: AutodiffBackend> {
        grads: &'a GradientsParams,
        total: Option<Tensor<B::InnerBackend, 1>>,
    }
    impl<B: AutodiffBackend> ModuleVisitor<B> for SquaredNorm<'_, B> {
        fn visit_float<const D: usize>(&mut self, param: &Param<Tensor<B, D>>) {
            if let Some(grad) = self.grads.get::<B::InnerBackend, D>(param.id) {
                let squared = grad.powi_scalar(2).sum();
                self.total = Some(match self.total.take() {
                    Some(total) => total + squared,
                    None => squared,
                });
            }
        }
    }
    struct Scale<'a> {
        grads: &'a mut GradientsParams,
        factor: f32,
    }
    impl<B: AutodiffBackend> ModuleVisitor<B> for Scale<'_> {
        fn visit_float<const D: usize>(&mut self, param: &Param<Tensor<B, D>>) {
            if let Some(grad) = self.grads.remove::<B::InnerBackend, D>(param.id) {
                self.grads.register(param.id, grad * self.factor);
            }
        }
    }
    let mut norm = SquaredNorm::<B> { grads, total: None };
    model.visit(&mut norm);
    let Some(total) = norm.total else { return };
    let total: f32 = total.into_scalar().elem();
    let factor = max_norm / (total.sqrt() + 1e-6);
    if factor < 1.0 {
        model.visit(&mut Scale { grads, factor });
    }
}

/// Camera visibility for every frame, unaugmented, in inference mode.
fn predict<B: Backend>(
    model: &TongueNet<B>,
    frames: &Frames,
    batch_size: usize,
    device: &B::Device,
) -> Result<Vec<f64>> {
    let size = frames.size;
    let mut visibility = Vec::with_capacity(frames.len());
    for images in frames.images.chunks(batch_size) {
        let pixels: Vec<f32> = images
            .iter()
            .flat_map(|image| image.iter().map(|&value| value as f32 / 255.0))
            .collect();
        let input = Tensor::<B, 4>::from_data(
            TensorData::new(pixels, [images.len(), 2, size, size]),
            device,
        );
        let values = model
            .forward(input)
            .into_data()
            .to_vec::<f32>()
            .map_err(|error| anyhow!("{error:?}"))?;
        visibility.extend(values.chunks(TARGETS.len()).map(|row| row[0] as f64));
    }
    if visibility.iter().any(|value| !value.is_finite()) {
        bail!("Non-finite model predictions");
    }
    Ok(visibility)
}

/// The features the last head layer reads, for every frame, unaugmented.
fn features<B: Backend>(
    model: &TongueNet<B>,
    frames: &Frames,
    batch_size: usize,
    device: &B::Device,
) -> Result<Vec<Vec<f32>>> {
    let size = frames.size;
    let mut out = Vec::with_capacity(frames.len());
    for images in frames.images.chunks(batch_size) {
        let pixels: Vec<f32> = images
            .iter()
            .flat_map(|image| image.iter().map(|&value| value as f32 / 255.0))
            .collect();
        let input = Tensor::<B, 4>::from_data(
            TensorData::new(pixels, [images.len(), 2, size, size]),
            device,
        );
        let values = model
            .features(input)
            .into_data()
            .to_vec::<f32>()
            .map_err(|error| anyhow!("{error:?}"))?;
        out.extend(values.chunks(FEATURES).map(<[f32]>::to_vec));
    }
    if out.iter().flatten().any(|value| !value.is_finite()) {
        bail!("Non-finite model features");
    }
    Ok(out)
}

struct Classification {
    f1: f64,
    false_positive_rate: f64,
}

fn classify(probability: &[f64], expected: &[f64], threshold: f64) -> Classification {
    let (mut tp, mut fp, mut fn_, mut tn) = (0usize, 0usize, 0usize, 0usize);
    for (&p, &e) in probability.iter().zip(expected) {
        match (p >= threshold, e >= 0.5) {
            (true, true) => tp += 1,
            (true, false) => fp += 1,
            (false, true) => fn_ += 1,
            (false, false) => tn += 1,
        }
    }
    Classification {
        f1: 2.0 * tp as f64 / (2 * tp + fp + fn_).max(1) as f64,
        false_positive_rate: fp as f64 / (fp + tn).max(1) as f64,
    }
}

/// Contiguous (start, end) runs whose score equals the best.
fn best_runs(scores: &[f64], best: f64) -> Vec<(usize, usize)> {
    let mut runs = vec![];
    let mut start = None;
    for (index, &score) in scores.iter().enumerate() {
        if score >= best - 1e-9 {
            start.get_or_insert(index);
        } else if let Some(first) = start.take() {
            runs.push((first, index - 1));
        }
    }
    if let Some(first) = start {
        runs.push((first, scores.len() - 1));
    }
    runs
}

/// The visibility threshold at the preferred camera weight (the camera alone
/// when any frame has no tracking module TongueOut): the midpoint of
/// the widest contiguous plateau of best `F1 - 0.75 * FPR` thresholds,
/// clamped to [0.3, 0.8]. Many thresholds usually tie, and breaking ties
/// toward the largest pinned the shipped gate at the grid edge, where live
/// detection flickers across the daemon's hysteresis band.
fn calibrate(camera: &[f64], frames: &Frames) -> VisibilityGate {
    let thresholds: Vec<f64> = (0..86).map(|i| (10 + i) as f64 / 100.0).collect();
    let native: Option<Vec<f64>> = frames
        .records
        .iter()
        .map(|record| record.native.map(f64::from))
        .collect();
    let weight = if native.is_some() {
        PREFERRED_CAMERA_WEIGHT
    } else {
        CAMERA_ONLY_WEIGHT
    };
    let expected: Vec<f64> = frames.records.iter().map(|r| r.targets[0] as f64).collect();
    let fused: Vec<f64> = match &native {
        Some(native) => camera
            .iter()
            .zip(native)
            .map(|(camera, native)| weight * camera + (1.0 - weight) * native)
            .collect(),
        None => camera.to_vec(),
    };
    let scores: Vec<f64> = thresholds
        .iter()
        .map(|&threshold| {
            let metrics = classify(&fused, &expected, threshold);
            metrics.f1 - 0.75 * metrics.false_positive_rate
        })
        .collect();
    let best = scores.iter().copied().fold(f64::MIN, f64::max);
    let center = (THRESHOLD_LIMITS.0 + THRESHOLD_LIMITS.1) / 2.0;
    // Reversed so equal keys keep the first plateau, as Python's max does.
    let (start, end) = best_runs(&scores, best)
        .into_iter()
        .rev()
        .max_by(|a, b| {
            let key = |&(start, end): &(usize, usize)| {
                let middle = (thresholds[start] + thresholds[end]) / 2.0;
                (
                    end - start + 1,
                    -((middle - center).abs() * 1e6).round() as i64,
                )
            };
            key(a).cmp(&key(b))
        })
        .expect("at least one threshold scores best");
    let middle = (thresholds[start] + thresholds[end]) / 2.0;
    let threshold = middle.clamp(THRESHOLD_LIMITS.0, THRESHOLD_LIMITS.1);
    let mut details = serde_json::Map::new();
    details.insert(
        "plateau".into(),
        json!([thresholds[start], thresholds[end]]),
    );
    details.insert("objective".into(), json!(best));
    details.insert("formula".into(), json!(GATE_FORMULA));
    details.insert("selection".into(), json!(GATE_SELECTION));
    VisibilityGate {
        camera_weight: weight,
        threshold: (threshold * 10000.0).round() / 10000.0,
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate_frames(native: impl Fn(usize) -> Option<f32>) -> Frames {
        let records = (0..20)
            .map(|index| {
                let mut targets = [0.0; TARGETS.len()];
                targets[0] = if index % 2 == 0 { 1.0 } else { 0.0 };
                crate::dataset::Record {
                    targets,
                    cheeks_labelled: false,
                    native: native(index),
                    moving: false,
                    key: index.to_string(),
                }
            })
            .collect();
        Frames {
            records,
            images: vec![],
            size: 0,
        }
    }

    #[test]
    fn gate_uses_the_camera_alone_without_tracking_module_values() {
        let camera: Vec<f64> = (0..20)
            .map(|index| if index % 2 == 0 { 0.9 } else { 0.1 })
            .collect();
        let with_module = calibrate(
            &camera,
            &gate_frames(|index| Some((index % 2 == 0) as u8 as f32)),
        );
        assert_eq!(with_module.camera_weight, PREFERRED_CAMERA_WEIGHT);
        let partly = calibrate(&camera, &gate_frames(|index| (index > 3).then_some(0.0)));
        assert_eq!(partly.camera_weight, CAMERA_ONLY_WEIGHT);
        assert!((0.3..=0.8).contains(&partly.threshold));
    }

    /// Values from the Python trainer's `loss_for` on the same inputs.
    #[test]
    #[allow(clippy::excessive_precision)]
    fn loss_matches_the_reference_trainer() {
        let prediction: [[f32; 10]; 6] = [
            [
                0.053837169,
                0.14501242,
                -0.3854835,
                -0.85148132,
                0.47445291,
                0.10410466,
                0.74467927,
                0.71932942,
                0.58497906,
                0.69814193,
            ],
            [
                0.45592821,
                0.76954156,
                -0.6302864,
                -0.17735612,
                0.098759234,
                0.46346471,
                0.20804471,
                0.90426457,
                0.81258678,
                0.67474115,
            ],
            [
                0.63347936,
                0.24328196,
                0.80874896,
                -0.87820435,
                0.21278948,
                0.21894364,
                0.31347024,
                0.58972877,
                0.15912975,
                -0.6933676,
            ],
            [
                0.28479168,
                0.77465469,
                -0.64914823,
                -0.80720663,
                0.10998122,
                0.27856165,
                0.38176334,
                0.053891994,
                0.28931558,
                0.017313838,
            ],
            [
                0.57417989,
                0.43548846,
                -0.10204667,
                -0.58551693,
                0.73053765,
                0.39159277,
                0.19232744,
                0.39162904,
                0.43713251,
                0.83214509,
            ],
            [
                0.73452115,
                0.31829557,
                -0.12768465,
                0.79530239,
                0.78909653,
                0.6122005,
                0.33857763,
                0.87443459,
                0.69279367,
                -0.40083748,
            ],
        ];
        let targets: [[f32; 10]; 6] = [
            [1., 1., 0.5, 0., 0., 0., 0., 0., 0., 0.],
            [0.; 10],
            [1., 0.25, -1., 0.7, 0., 0., 0., 0., 0., 0.],
            [1., 1., 0., -0.5, 0., 0., 0., 0., 0., 0.],
            [0.; 10],
            [1., 0.75, 0.7, -0.7, 0., 0., 0., 0., 0., 0.],
        ];
        // The reference has no cheek heads: pad them as unlabelled.
        let widen = |rows: [[f32; 10]; 6]| -> Vec<f32> {
            rows.iter()
                .flat_map(|row| row.iter().copied().chain([0.9, 0.9]))
                .collect()
        };
        let mut enabled = [false; TARGETS.len()];
        enabled[..4].fill(true);
        enabled[10..].fill(true);
        let device = Default::default();
        let targets = widen(targets);
        // Only the visibility logit is read, and the reference's
        // probabilities are all inside (0, 1).
        let logits: Vec<f32> = widen(prediction)
            .chunks(TARGETS.len())
            .flat_map(|row| {
                let p = row[0];
                std::iter::once((p / (1.0 - p)).ln()).chain(row[1..].iter().copied())
            })
            .collect();
        let value = |focus, cheeks: [bool; 6]| {
            let prediction = Tensor::<Cpu, 2>::from_data(
                TensorData::new(widen(prediction), [6, TARGETS.len()]),
                &device,
            );
            let raw = Tensor::<Cpu, 2>::from_data(
                TensorData::new(logits.clone(), [6, TARGETS.len()]),
                &device,
            );
            let value: f32 = loss(&enabled, raw, prediction, &targets, &cheeks, focus, &device)
                .into_scalar()
                .elem();
            value
        };
        assert!((value(Focus::Gate, [false; 6]) - 1.4441112).abs() < 1e-5);
        assert!((value(Focus::Direction, [false; 6]) - 3.5815976).abs() < 1e-5);
        // Labelled cheeks count on hidden-tongue frames too: 0.9 against 0.9
        // adds nothing, but the weight they add dilutes the tongue's error.
        assert!(value(Focus::Direction, [true; 6]) < 3.5815976);
    }

    #[test]
    fn ridge_recovers_a_linear_map() {
        let mut rng = StdRng::seed_from_u64(3);
        let rows: Vec<Vec<f32>> = (0..400)
            .map(|_| {
                (0..FEATURES)
                    .map(|_| rng.random_range(-0.5f32..0.5))
                    .collect()
            })
            .collect();
        let samples: Vec<Sample> = rows
            .iter()
            .map(|features| Sample {
                features,
                weight: 1.0,
                target: 2.0 * f64::from(features[3]) - 1.5,
            })
            .collect();
        let fitted = ridge(&samples).unwrap();
        for sample in &samples {
            let predicted: f64 = fitted[FEATURES]
                + fitted[..FEATURES]
                    .iter()
                    .zip(sample.features)
                    .map(|(w, x)| w * f64::from(*x))
                    .sum::<f64>();
            assert!(
                (predicted - sample.target).abs() < 0.2,
                "{predicted} vs {}",
                sample.target
            );
        }
    }

    #[test]
    fn best_runs_finds_every_plateau() {
        assert_eq!(best_runs(&[1., 1., 0., 1.], 1.0), vec![(0, 1), (3, 3)]);
        assert_eq!(best_runs(&[0., 1., 1.], 1.0), vec![(1, 2)]);
    }

    #[test]
    fn classification_counts_rates() {
        let metrics = classify(&[0.9, 0.9, 0.1, 0.1], &[1., 0., 1., 0.], 0.5);
        assert!((metrics.f1 - 0.5).abs() < 1e-9);
        assert!((metrics.false_positive_rate - 0.5).abs() < 1e-9);
    }

    #[test]
    fn the_estimate_leaves_out_the_first_batch() {
        // The first batch took minutes and ended 1% in; 20 s later the run
        // is 5% in, so 4% took 20 s and the remaining 95% takes 475 s.
        assert_eq!(estimate(20.0, 0.01, 0.05), Some(475.0));
        // Too soon, or no batch finished since, to know the pace.
        assert_eq!(estimate(5.0, 0.01, 0.05), None);
        assert_eq!(estimate(20.0, 0.01, 0.01), None);
    }
}
