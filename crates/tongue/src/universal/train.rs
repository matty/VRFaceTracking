//! Training the universal face model.
//!
//! The front and the three tails start from the base pair's v8 encoder
//! (`encoder.network.*` of the direction model, the same shapes), never from
//! QFT+'s weights; the projection, tongue head, mouth and brow heads and
//! their missing vectors start fresh.
//!
//! Each batch holds frames of at most two faces (a recording session, or a
//! rendered identity). For each face, one frame per enrollment slot it has
//! is encoded with the batch and becomes that face's anchors, so the heads
//! learn to read a frame against the same person's poses. Each frame then
//! drops anchors at random, and sometimes all of them, so the learned
//! missing vectors are trained and the model works without enrollment too.
//!
//! Losses, each only where a label exists and the output trains:
//! visibility as class-weighted BCE (the pair's weights), extension and the
//! directions as smooth L1 on visible frames, and every mouth and brow
//! output as BCE against its soft label.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use burn::backend::Autodiff;
use burn::module::AutodiffModule;
use burn::optim::{AdamWConfig, GradientsParams, Optimizer};
use burn::tensor::activation::log_sigmoid;
use burn::tensor::backend::{AutodiffBackend, Backend};
use burn::tensor::{ElementConversion, Int, Tensor, TensorData};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rayon::prelude::*;
use serde_json::json;

use super::checkpoint::{FaceCheckpoint, FaceMetadata};
use super::data::{FaceFrames, FaceRecord};
use super::net::{activate, from_v8_encoder, Embeddings, FaceNet};
use super::{
    ANCHOR_SLOTS, BROW_EMBEDDING, CAMERAS, FACE_TARGETS, FILE_NAME, IMAGE_SIZE, MOUTH_EMBEDDING,
    MOUTH_START, SIGNED,
};
use crate::backend::{Accelerator, Cpu, Gpu, GPU_NAME};
use crate::checkpoint::{Checkpoint, Role};
use crate::dataset::{augment, Frames, Record, ACTIVE};
use crate::infer::guarded;
use crate::recordings::Recording;
use crate::train::{
    calibrate, calibration_frames, clip_gradients, write_json, Options, Progress, Request,
};
use crate::TARGETS;
use vrft_quest_pro_protocol::{ReportCalibration, TrainingProgress, TrainingReport, TrainingStage};

/// Learning rate when the request sets none: the heads start from nothing.
pub const LEARNING_RATE: f64 = 3e-4;
/// Each anchor is left out of a frame this often...
const ANCHOR_DROPOUT: f32 = 0.25;
/// ...and every anchor this often, as without enrollment.
const NO_ENROLLMENT: f32 = 0.2;
// Loss weights, as for the pair where they overlap.
const VISIBLE_WEIGHT: f32 = 1.25;
const HIDDEN_WEIGHT: f32 = 1.70;
const VISIBILITY_WEIGHT: f32 = 1.8;
const REGRESSION_BETA: f32 = 0.08;
/// Active labels count this much more than resting ones, so "always 0"
/// doesn't look good.
const ACTIVE_BOOST: f32 = 3.0;
const MAX_GRADIENT_NORM: f32 = 1.0;
const OUTPUTS: usize = FACE_TARGETS.len();

struct Job<'a> {
    request: &'a Request,
    recordings: &'a [Recording],
    output: &'a Path,
    options: &'a Options,
    progress: &'a Progress,
}

/// Trains the universal face model on `recordings`, writing the model,
/// `report.json` and `progress.json` into `output`.
pub(crate) fn run(
    request: &Request,
    recordings: &[Recording],
    output: &Path,
    options: &Options,
    progress: &Progress,
    accelerator: Accelerator,
) -> Result<()> {
    let job = Job {
        request,
        recordings,
        output,
        options,
        progress,
    };
    // The labels alone say whether there is enough to train on.
    FaceFrames::load(recordings, None)?.trainable()?;
    match accelerator {
        Accelerator::Cpu => guarded(|| job.train::<Autodiff<Cpu>>("CPU")),
        Accelerator::Gpu => guarded(|| job.train::<Autodiff<Gpu>>(GPU_NAME)),
        Accelerator::Auto => {
            let works = guarded(|| {
                let probe = Tensor::<Gpu, 1>::from_floats([1.0, 2.0], &Default::default()).sum();
                probe.into_scalar();
                Ok(())
            });
            match works {
                Ok(()) => guarded(|| job.train::<Autodiff<Gpu>>(GPU_NAME)),
                Err(error) => {
                    println!("GPU unavailable ({error:#}); training on the CPU");
                    guarded(|| job.train::<Autodiff<Cpu>>("CPU"))
                }
            }
        }
    }
}

/// One batch's inputs: the frames, then each face's anchor frames.
struct Batch {
    /// `[rows, 5, size, size]` pixels, augmented.
    pixels: Vec<f32>,
    rows: usize,
    /// Per frame and slot, the row of its anchor (0 when absent).
    anchor_rows: Vec<i64>,
    present: Vec<f32>,
    /// Per frame, the row of its neutral anchor, and whether the brow head
    /// may read it.
    neutral_rows: Vec<i64>,
    brow_present: Vec<f32>,
    labels: Vec<f32>,
    labelled: Vec<bool>,
}

impl Job<'_> {
    fn train<B: AutodiffBackend>(&self, device_name: &str) -> Result<()> {
        let started = Instant::now();
        let device = B::Device::default();
        B::seed(&device, 42);
        let size = self.options.image_size.unwrap_or(IMAGE_SIZE);
        let frames = FaceFrames::load(self.recordings, Some(size))?;
        let enabled = frames.trainable()?;
        let (start, parent) = match &self.options.init {
            Some(path) => (FaceCheckpoint::load(path)?.weights, path.clone()),
            None => {
                let base = &self.request.base_model_dir;
                let source = Role::Direction.find(base).with_context(|| {
                    format!("the base model pair is missing from {}", base.display())
                })?;
                (from_v8_encoder(&Checkpoint::load(&source)?.weights), source)
            }
        };
        let mut model = FaceNet::<B>::from_weights(start, true, &device)?;
        let mut optimizer = AdamWConfig::new()
            .with_weight_decay(1e-4)
            .with_epsilon(1e-8)
            .init::<B, FaceNet<B>>();
        let rate = self.options.learning_rate.unwrap_or(LEARNING_RATE);
        let epochs = self.options.epochs;
        let mut rng = StdRng::seed_from_u64(42);
        let mut written: Option<Instant> = None;
        for epoch in 1..=epochs {
            let batches = frames.batches(self.options.batch_size, &mut rng);
            for (index, members) in batches.iter().enumerate() {
                let batch = self.batch(&frames, members, &mut rng);
                let raw = forward(&model, &batch, members.len(), size, &device);
                let loss = loss(raw, &batch.labels, &batch.labelled, &enabled, &device);
                let value: f32 = loss.clone().into_scalar().elem();
                if !value.is_finite() {
                    bail!("Training loss is not finite");
                }
                let mut grads = GradientsParams::from_grads(loss.backward(), &model);
                clip_gradients(&model, &mut grads, MAX_GRADIENT_NORM);
                model = optimizer.step(rate, model, grads);
                let done = ((epoch - 1) as f64 + (index + 1) as f64 / batches.len() as f64)
                    / epochs as f64;
                if written.is_none_or(|at| at.elapsed() >= Duration::from_secs(2)) {
                    written = Some(Instant::now());
                    self.progress.report(TrainingProgress {
                        focus: Some("face".into()),
                        epoch: Some(epoch as u32),
                        epochs: Some(epochs as u32),
                        fraction: Some((done * 0.95) as f32),
                        device: Some(device_name.into()),
                        ..TrainingProgress::new(
                            TrainingStage::Training,
                            format!("Learning your face (pass {epoch} of {epochs})"),
                        )
                    });
                }
            }
        }
        self.progress.report(TrainingProgress {
            focus: Some("face".into()),
            fraction: Some(0.96),
            device: Some(device_name.into()),
            ..TrainingProgress::new(
                TrainingStage::Calibrating,
                "Tuning when the tongue counts as out",
            )
        });
        let trained = model.valid();
        let predictions = predict_all(&trained, &frames, self.options.batch_size, &device)?;
        let pair = as_pair_frames(&frames);
        let indices = calibration_frames(&pair);
        let calibration = calibrate(
            &indices
                .iter()
                .map(|&index| f64::from(predictions[index][0]))
                .collect::<Vec<_>>(),
            &indices
                .iter()
                .map(|&index| &pair.records[index])
                .collect::<Vec<_>>(),
            false,
        );
        let disabled: Vec<String> = FACE_TARGETS
            .iter()
            .zip(enabled)
            .filter(|(_, yes)| !yes)
            .map(|(name, _)| name.to_string())
            .collect();
        let fit = fit_report(&predictions, &frames, &enabled);
        let mut metadata = FaceMetadata::new(size);
        metadata.visibility_gate = calibration.clone();
        metadata.disabled_targets = disabled.clone();
        metadata.training = Some(json!({
            "epochs": epochs,
            "learningRate": rate,
            "frames": frames.len(),
            "faces": frames.groups.len(),
            "anchorDropout": ANCHOR_DROPOUT,
            "noEnrollment": NO_ENROLLMENT,
            "parentCheckpoint": parent.display().to_string(),
            "trainingFit": fit,
        }));
        FaceCheckpoint {
            metadata,
            weights: trained.weights(),
        }
        .save(&self.output.join(FILE_NAME))?;
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
                .map(|recording| recording.dir.display().to_string())
                .collect(),
            frames: Some(frames.len() as u64),
            coverage: coverage(&frames),
            epochs: epochs as u32,
            supported_targets: FACE_TARGETS
                .iter()
                .zip(enabled)
                .filter(|(_, yes)| *yes)
                .map(|(name, _)| name.to_string())
                .collect(),
            disabled_targets: disabled,
            base_model_dir: self.request.base_model_dir.display().to_string(),
            calibration: ReportCalibration {
                camera_weight: calibration.camera_weight,
                threshold: calibration.threshold,
                plateau: calibration.details.get("plateau").cloned(),
                held_out: false,
            },
            seconds: Some((started.elapsed().as_secs_f64() * 10.0).round() / 10.0),
            held_out_frames: 0,
            kept: vec![],
            tongue_out: None,
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

    /// Augmented pixels of `members` and their faces' anchors, with each
    /// frame's anchors dropped at random.
    fn batch(&self, frames: &FaceFrames, members: &[usize], rng: &mut StdRng) -> Batch {
        let size = frames.size;
        let plane = CAMERAS * size * size;
        let mut rows: Vec<usize> = members.to_vec();
        // Each face's anchor rows, by slot.
        let mut anchors: Vec<(usize, [Option<usize>; ANCHOR_SLOTS.len()])> = vec![];
        for &member in members {
            let group = frames.records[member].group;
            if anchors.iter().any(|(g, _)| *g == group) {
                continue;
            }
            let mut slots = [None; ANCHOR_SLOTS.len()];
            for (slot, pool) in frames.groups[group].anchors.iter().enumerate() {
                if !pool.is_empty() {
                    slots[slot] = Some(rows.len());
                    rows.push(pool[rng.random_range(0..pool.len())]);
                }
            }
            anchors.push((group, slots));
        }
        let seeds: Vec<u64> = rows.iter().map(|_| rng.random()).collect();
        let mut pixels = vec![0f32; rows.len() * plane];
        pixels
            .par_chunks_mut(plane)
            .zip(rows.par_iter().zip(&seeds))
            .for_each(|(out, (&row, &seed))| {
                augment(
                    &frames.images[row],
                    size,
                    &mut StdRng::seed_from_u64(seed),
                    out,
                );
            });
        let mut batch = Batch {
            pixels,
            rows: rows.len(),
            anchor_rows: vec![],
            present: vec![],
            neutral_rows: vec![],
            brow_present: vec![],
            labels: vec![],
            labelled: vec![],
        };
        for &member in members {
            let record = &frames.records[member];
            let slots = anchors
                .iter()
                .find(|(group, _)| *group == record.group)
                .map(|(_, slots)| *slots)
                .unwrap_or_default();
            let none = rng.random::<f32>() < NO_ENROLLMENT;
            for slot in slots {
                let kept = slot.filter(|_| !none && rng.random::<f32>() >= ANCHOR_DROPOUT);
                batch.anchor_rows.push(kept.unwrap_or(0) as i64);
                batch.present.push(if kept.is_some() { 1.0 } else { 0.0 });
            }
            let neutral = batch.present[batch.present.len() - ANCHOR_SLOTS.len()] > 0.5;
            let neutral_row = batch.anchor_rows[batch.anchor_rows.len() - ANCHOR_SLOTS.len()];
            let upper = record.upper_face && frames.records[rows[neutral_row as usize]].upper_face;
            batch.neutral_rows.push(neutral_row);
            batch
                .brow_present
                .push(if neutral && upper { 1.0 } else { 0.0 });
            batch.labels.extend(record.labels);
            batch.labelled.extend(record.labelled);
        }
        batch
    }
}

/// Every output before activation for a batch's frames, `[frames, 17]`.
fn forward<B: Backend>(
    model: &FaceNet<B>,
    batch: &Batch,
    count: usize,
    size: usize,
    device: &B::Device,
) -> Tensor<B, 2> {
    let views = Tensor::<B, 4>::from_data(
        TensorData::new(batch.pixels.clone(), [batch.rows, CAMERAS, size, size]),
        device,
    );
    let all = model.embed(views);
    let pick = |rows: &[i64]| {
        Tensor::<B, 1, Int>::from_data(TensorData::new(rows.to_vec(), [rows.len()]), device)
    };
    let slots = ANCHOR_SLOTS.len();
    let anchors = all
        .mouth
        .clone()
        .select(0, pick(&batch.anchor_rows))
        .reshape([count, slots, MOUTH_EMBEDDING]);
    let present = Tensor::<B, 2>::from_data(
        TensorData::new(batch.present.clone(), [count, slots]),
        device,
    );
    let brow_neutral = all.brow.clone().select(0, pick(&batch.neutral_rows));
    let brow_present = Tensor::<B, 2>::from_data(
        TensorData::new(batch.brow_present.clone(), [count, 1]),
        device,
    );
    let frames = Embeddings {
        mouth: all.mouth.narrow(0, 0, count),
        tongue: all.tongue.narrow(0, 0, count),
        brow: all.brow.narrow(0, 0, count),
    };
    debug_assert_eq!(brow_neutral.dims(), [count, BROW_EMBEDDING]);
    model.raw(&frames, anchors, present, brow_neutral, brow_present)
}

/// The training loss; see the module's documentation.
fn loss<B: Backend>(
    raw: Tensor<B, 2>,
    labels: &[f32],
    labelled: &[bool],
    enabled: &[bool; OUTPUTS],
    device: &B::Device,
) -> Tensor<B, 1> {
    let count = labels.len() / OUTPUTS;
    let mut visibility = vec![0f32; labels.len()];
    let mut regression = vec![0f32; labels.len()];
    let mut classification = vec![0f32; labels.len()];
    for row in 0..count {
        let at = |output: usize| row * OUTPUTS + output;
        let visible = labels[at(0)] >= 0.5;
        visibility[at(0)] = if visible {
            VISIBLE_WEIGHT
        } else {
            HIDDEN_WEIGHT
        };
        for output in 1..OUTPUTS {
            if !labelled[at(output)] || !enabled[output] {
                continue;
            }
            let weight =
                1.0 + ACTIVE_BOOST * f32::from(u8::from(labels[at(output)].abs() > ACTIVE));
            if output < MOUTH_START {
                regression[at(output)] = weight;
            } else {
                classification[at(output)] = weight;
            }
        }
    }
    let normalise = |weights: &mut Vec<f32>, scale: f32| {
        let total: f32 = weights.iter().sum();
        let factor = scale / total.max(1.0);
        weights.iter_mut().for_each(|weight| *weight *= factor);
    };
    // Visibility averages over frames; the rest over the labels there are.
    let factor = VISIBILITY_WEIGHT / count as f32;
    visibility.iter_mut().for_each(|weight| *weight *= factor);
    normalise(&mut regression, 1.0);
    normalise(&mut classification, 1.0);
    let tensor = |values: Vec<f32>| {
        Tensor::<B, 2>::from_data(TensorData::new(values, [count, OUTPUTS]), device)
    };
    let targets = tensor(labels.to_vec());
    let bce = (targets.clone() * log_sigmoid(raw.clone())
        + (targets.clone().neg() + 1.0) * log_sigmoid(raw.clone().neg()))
    .neg();
    let difference = (activate(raw) - targets).abs();
    let quadratic = difference.clone().powi_scalar(2) * (0.5 / REGRESSION_BETA);
    let linear = difference.clone() - 0.5 * REGRESSION_BETA;
    let smooth = linear.mask_where(difference.lower_elem(REGRESSION_BETA), quadratic);
    (bce.clone() * (tensor(visibility) + tensor(classification))).sum()
        + (smooth * tensor(regression)).sum()
}

/// Every output for every frame, unaugmented, each face read against one
/// frame of each of its slots.
fn predict_all<B: Backend>(
    model: &FaceNet<B>,
    frames: &FaceFrames,
    batch_size: usize,
    device: &B::Device,
) -> Result<Vec<[f32; OUTPUTS]>> {
    let size = frames.size;
    let mut out = Vec::with_capacity(frames.len());
    for (group_index, group) in frames.groups.iter().enumerate() {
        let anchors: Vec<Option<usize>> = group
            .anchors
            .iter()
            .map(|pool| pool.first().copied())
            .collect();
        for members in group.members.chunks(batch_size) {
            let mut rows: Vec<usize> = members.to_vec();
            let mut slot_rows = [None; ANCHOR_SLOTS.len()];
            for (slot, anchor) in anchors.iter().enumerate() {
                if let Some(anchor) = anchor {
                    slot_rows[slot] = Some(rows.len());
                    rows.push(*anchor);
                }
            }
            let mut batch = Batch {
                pixels: rows
                    .iter()
                    .flat_map(|&row| frames.images[row].iter().map(|&v| v as f32 / 255.0))
                    .collect(),
                rows: rows.len(),
                anchor_rows: vec![],
                present: vec![],
                neutral_rows: vec![],
                brow_present: vec![],
                labels: vec![],
                labelled: vec![],
            };
            for &member in members {
                for slot in slot_rows {
                    batch.anchor_rows.push(slot.unwrap_or(0) as i64);
                    batch.present.push(if slot.is_some() { 1.0 } else { 0.0 });
                }
                batch.neutral_rows.push(slot_rows[0].unwrap_or(0) as i64);
                let upper = frames.records[member].upper_face
                    && slot_rows[0].is_some_and(|row| frames.records[rows[row]].upper_face);
                batch.brow_present.push(if upper { 1.0 } else { 0.0 });
            }
            let values = activate(forward(model, &batch, members.len(), size, device))
                .into_data()
                .to_vec::<f32>()
                .map_err(|error| anyhow!("{error:?}"))?;
            for (member, row) in members.iter().zip(values.chunks(OUTPUTS)) {
                debug_assert_eq!(frames.records[*member].group, group_index);
                out.push((*member, row.try_into().expect("a row per frame")));
            }
        }
    }
    out.sort_by_key(|(member, _)| *member);
    let out: Vec<[f32; OUTPUTS]> = out.into_iter().map(|(_, row)| row).collect();
    if out.iter().flatten().any(|value| !value.is_finite()) {
        bail!("Non-finite model predictions");
    }
    Ok(out)
}

/// The face frames as the pair's calibration reads them: visibility, the
/// tracking module's TongueOut, and whether they were rendered.
fn as_pair_frames(frames: &FaceFrames) -> Frames {
    Frames {
        records: frames
            .records
            .iter()
            .map(|record: &FaceRecord| {
                let mut targets = [0.0; TARGETS.len()];
                targets[0] = record.labels[0];
                Record {
                    targets,
                    cheeks_labelled: false,
                    native: record.native,
                    moving: false,
                    key: record.key.clone(),
                    synthetic: record.synthetic,
                    held_out: false,
                }
            })
            .collect(),
        images: vec![],
        size: 0,
    }
}

/// Mean absolute error per trained output on the frames it trained on: a
/// check that it learned, not an accuracy score.
fn fit_report(
    predictions: &[[f32; OUTPUTS]],
    frames: &FaceFrames,
    enabled: &[bool; OUTPUTS],
) -> serde_json::Value {
    let mut fit = serde_json::Map::new();
    for (output, name) in FACE_TARGETS.iter().enumerate() {
        if !enabled[output] {
            continue;
        }
        let errors: Vec<f32> = predictions
            .iter()
            .zip(&frames.records)
            .filter(|(_, record)| record.labelled[output])
            .map(|(row, record)| (row[output] - record.labels[output]).abs())
            .collect();
        if !errors.is_empty() {
            let mean = errors.iter().sum::<f32>() / errors.len() as f32;
            fit.insert(name.to_string(), json!((mean * 1000.0).round() / 1000.0));
        }
    }
    serde_json::Value::Object(fit)
}

/// Labelled, active and resting frames per output, and the faces and slots.
fn coverage(frames: &FaceFrames) -> serde_json::Value {
    let mut outputs = serde_json::Map::new();
    for (output, name) in FACE_TARGETS.iter().enumerate() {
        let labelled = frames.records.iter().filter(|r| r.labelled[output]);
        let active = labelled
            .clone()
            .filter(|r| r.labels[output].abs() > ACTIVE)
            .count();
        let total = labelled.count();
        let mut entry = json!({"labelled": total, "active": active});
        if SIGNED.contains(&output) {
            entry["negative"] = json!(frames
                .records
                .iter()
                .filter(|r| r.labelled[output] && r.labels[output] < -ACTIVE)
                .count());
        }
        outputs.insert(name.to_string(), entry);
    }
    let slots: serde_json::Map<String, serde_json::Value> = ANCHOR_SLOTS
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            let faces = frames
                .groups
                .iter()
                .filter(|group| !group.anchors[slot].is_empty())
                .count();
            (name.to_string(), json!(faces))
        })
        .collect();
    let positive = frames.records.iter().filter(|r| r.labels[0] >= 0.5).count();
    json!({
        "frames": frames.len(),
        "positive": positive,
        "negative": frames.len() - positive,
        "faces": frames.groups.len(),
        "facesWithSlot": slots,
        "upperFace": frames.records.iter().filter(|r| r.upper_face).count(),
        "synthetic": frames.records.iter().filter(|r| r.synthetic).count(),
        "outputs": outputs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlabelled_and_untrained_outputs_add_nothing() {
        let device = Default::default();
        let raw = Tensor::<Cpu, 2>::zeros([2, OUTPUTS], &device);
        let mut labels = vec![0.0; 2 * OUTPUTS];
        labels[0] = 1.0;
        let labelled = vec![false; 2 * OUTPUTS];
        let base: f32 = loss(raw.clone(), &labels, &labelled, &[true; OUTPUTS], &device)
            .into_scalar()
            .elem();
        // Visibility alone: BCE of 0.5 against 1 and 0, class weighted.
        let expected = VISIBILITY_WEIGHT * (VISIBLE_WEIGHT + HIDDEN_WEIGHT) * 2f32.ln() / 2.0;
        assert!((base - expected).abs() < 1e-5, "{base} vs {expected}");
        let mut known = labelled.clone();
        let brow = super::super::BROW_START;
        known[brow] = true;
        labels[brow] = 1.0;
        let mut disabled = [true; OUTPUTS];
        disabled[brow] = false;
        let off: f32 = loss(raw.clone(), &labels, &known, &disabled, &device)
            .into_scalar()
            .elem();
        assert!(
            (off - base).abs() < 1e-6,
            "an untrained output adds nothing"
        );
        let on: f32 = loss(raw, &labels, &known, &[true; OUTPUTS], &device)
            .into_scalar()
            .elem();
        assert!(on > base, "a labelled, trained output adds its BCE");
    }
}
