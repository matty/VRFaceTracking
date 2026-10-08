//! Personal fine-tuning of a `universal-face-v2` model such as QFT+'s, on the
//! wearer's five-camera recordings.
//!
//! QFT+ fine-tunes nothing on the wearer's PC: its one personal step is the
//! face setup, read when the model loads. This is the closest design to it
//! that learns from recordings:
//!
//! - The graph stays QFT+'s. Each recorded frame goes through it once, and
//!   the heads train on what it gives, read against the face setup in use as
//!   QFT+ reads them.
//! - Only what VRFT sends trains on labels: the mouth head's cheeks and the
//!   brow head's eight brows. Every other output of a frame is pulled toward
//!   what QFT+'s heads give for it, and every weight toward QFT+'s, so the
//!   heads stay QFT+'s where the recordings say nothing. The weights that
//!   read the anchors and presence flags, and the stand-ins for missing
//!   poses, stay QFT+'s: with one person's face setup they'd only shift a
//!   bias.
//! - The tongue's ridge map is fitted again on every labelled direction and
//!   the face setup's held tongue poses, as QFT+'s `tongue_map` fits it from
//!   the held poses alone.
//! - Some frames are held back ([`held_back`]). QFT+'s heads are pass 0, and
//!   a pass replaces the kept heads only when it scores better on them; the
//!   new tongue map is kept only when it reads their directions better than
//!   the face setup's. With too few held back, every frame trains and the
//!   last pass, or the new map, is kept.
//!
//! The result is a `universal-face-v2.npz` in QFT+'s format, its tongue map
//! in arrays QFT+'s loader ignores, beside a link to QFT+'s graph.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use rayon::prelude::*;
use serde_json::json;

use super::npz::{self, Array};
use super::{
    sigmoid, silu, solve, tongue_direction, Layer, TongueMapV2, UniversalV2, BROWS, CHEEKS,
    FILE_NAME, GRAPH_FILE, RIDGE, SLOTS, TONGUE_ARRAYS, TONGUE_DIRECTION, TONGUE_GAIN,
    TONGUE_REACH,
};
use crate::backend::Accelerator;
use crate::dataset::{held_back, sampling_key, select, ACTIVE};
use crate::recordings::{Recording, Sample};
use crate::train::{estimate, write_json, Options, Progress, Request};
use crate::universal::{Enrollment, BROW_OUTPUTS};
use vrft_quest_pro_protocol::{
    CameraLayout, ReportKept, TrainingProgress, TrainingReport, TrainingStage,
};

/// Learning rate when the request sets none.
pub const LEARNING_RATE: f64 = 1e-4;
/// How hard each weight is pulled back toward QFT+'s, decoupled as in AdamW.
const PULL: f32 = 1.0;
const BATCH: usize = 64;
/// Frames each thread works through at a time.
const CHUNK: usize = 8;
const LABEL_WEIGHT: f32 = 1.0;
/// Active labels count this much more than resting ones.
const ACTIVE_BOOST: f32 = 3.0;
/// The weight of what QFT+'s heads gave, for an output without a label.
const DISTILL_WEIGHT: f32 = 0.1;
const MAX_GRADIENT_NORM: f32 = 1.0;
/// A tongue this far out has a direction worth fitting.
const POINTING: f32 = 0.5;
/// Frames of each of the four straight directions a new tongue map needs.
const DIRECTION_FRAMES: usize = 3;
/// Fewer labelled frames held back than this can't score a model.
const HOLD_OUT_MIN: usize = 10;
const SUCK: [&str; 2] = ["cheek_suck_left", "cheek_suck_right"];
/// Of the run's progress: reading the recordings, then training the heads.
const READING: f64 = 0.45;
const TRAINING: f64 = 0.5;

/// One frame, as the heads read it, with its labels.
struct Example {
    q: Vec<f32>,
    w: Vec<f32>,
    /// `tanh` of the tongue head's direction.
    head_direction: [f32; 2],
    /// By [`CHEEKS`].
    cheeks: [Option<f32>; 4],
    /// By [`BROWS`].
    brows: [Option<f32>; 8],
    /// Horizontal and vertical, while the tongue points.
    direction: Option<[f32; 2]>,
    /// The recording and the pose (or direction) it shows, for balance.
    key: (usize, String),
    held_out: bool,
    /// Each pose weighs the same in its set.
    weight: f32,
    /// What QFT+'s heads gave.
    base_mouth: Vec<f32>,
    base_brows: [f32; 8],
}

impl Example {
    fn labelled(&self) -> bool {
        self.cheeks.iter().chain(&self.brows).any(Option::is_some)
    }
}

/// A frame's cheek and brow labels, and where the tongue points.
type Labels = ([Option<f32>; 4], [Option<f32>; 8], Option<[f32; 2]>);

/// The labels of a recorded frame, as QFT+ names its outputs.
fn labels(sample: &Sample) -> Labels {
    let face = |name: &str| sample.face.get(name).copied();
    let puff = |column: usize| sample.cheeks_labelled.then_some(sample.targets[column]);
    let cheeks = [puff(10), puff(11), face(SUCK[0]), face(SUCK[1])];
    let brows = BROW_OUTPUTS.map(face);
    let [visible, extension, horizontal, vertical] =
        [0, 1, 2, 3].map(|column| sample.targets[column]);
    let direction = (visible >= 0.5 && extension >= POINTING).then_some([horizontal, vertical]);
    (cheeks, brows, direction)
}

/// The face setup as the heads read it: its anchors, or QFT+'s stand-ins.
struct Setup {
    anchors: Vec<f32>,
    present: Vec<f32>,
    brow_neutral: Vec<f32>,
    brow_present: f32,
}

impl Setup {
    fn of(model: &UniversalV2) -> Self {
        let width = model.anchors.len() / SLOTS.len();
        let mut anchors = vec![0f32; model.anchors.len()];
        for (slot, &present) in model.present.iter().enumerate() {
            let source = if present > 0.0 {
                &model.anchors
            } else {
                &model.head_missing
            };
            anchors[slot * width..][..width].copy_from_slice(&source[slot * width..][..width]);
        }
        let brow_neutral = if model.brow_present > 0.0 {
            model.brow_neutral.clone()
        } else {
            model.brow_missing.clone()
        };
        Self {
            anchors,
            present: model.present.clone(),
            brow_neutral,
            brow_present: model.brow_present,
        }
    }

    fn mouth_width(&self) -> usize {
        self.anchors.len() / SLOTS.len()
    }
}

/// The heads' weights, in the `.npz`'s layout.
#[derive(Clone)]
struct Heads {
    mouth: [Layer; 3],
    brow: [Layer; 2],
}

impl Heads {
    fn tensors(&self) -> [&Vec<f32>; 10] {
        let [m1, m2, m3] = &self.mouth;
        let [b1, b2] = &self.brow;
        [
            &m1.weight, &m1.bias, &m2.weight, &m2.bias, &m3.weight, &m3.bias, &b1.weight, &b1.bias,
            &b2.weight, &b2.bias,
        ]
    }

    fn tensors_mut(&mut self) -> [&mut Vec<f32>; 10] {
        let [m1, m2, m3] = &mut self.mouth;
        let [b1, b2] = &mut self.brow;
        [
            &mut m1.weight,
            &mut m1.bias,
            &mut m2.weight,
            &mut m2.bias,
            &mut m3.weight,
            &mut m3.bias,
            &mut b1.weight,
            &mut b1.bias,
            &mut b2.weight,
            &mut b2.bias,
        ]
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    let (a8, a_rest) = a.as_chunks::<8>();
    let (b8, b_rest) = b.as_chunks::<8>();
    let mut lanes = [0f32; 8];
    for (x, y) in a8.iter().zip(b8) {
        for ((lane, x), y) in lanes.iter_mut().zip(x).zip(y) {
            *lane += x * y;
        }
    }
    lanes.iter().sum::<f32>() + a_rest.iter().zip(b_rest).map(|(x, y)| x * y).sum::<f32>()
}

/// `y += alpha * x`.
fn axpy(alpha: f32, x: &[f32], y: &mut [f32]) {
    for (y, x) in y.iter_mut().zip(x) {
        *y += alpha * x;
    }
}

fn affine(layer: &Layer, x: &[f32]) -> Vec<f32> {
    layer
        .bias
        .iter()
        .enumerate()
        .map(|(row, bias)| bias + dot(&layer.weight[row * layer.inputs..][..layer.inputs], x))
        .collect()
}

fn silu_slope(x: f32) -> f32 {
    let s = sigmoid(x);
    s * (1.0 + x * (1.0 - s))
}

/// Binary cross-entropy from a logit.
fn bce(logit: f32, target: f32) -> f32 {
    logit.max(0.0) - logit * target + (-logit.abs()).exp().ln_1p()
}

/// The first layers as the face setup leaves them: the part that reads `q`
/// (`w` for the brows), and a bias that holds the rest, which is the same
/// for every frame.
struct Folded {
    mouth: Vec<f32>,
    mouth_bias: Vec<f32>,
    brow: Vec<f32>,
    brow_bias: Vec<f32>,
}

/// Where each part of the mouth head's input starts: `[q, anchors,
/// q - neutral, present]`.
fn mouth_parts(width: usize) -> [usize; 4] {
    let slots = SLOTS.len();
    [0, width, (slots + 1) * width, (slots + 2) * width]
}

fn fold(heads: &Heads, setup: &Setup) -> Folded {
    let width = setup.mouth_width();
    let [_, anchors_at, neutral_at, present_at] = mouth_parts(width);
    let first = &heads.mouth[0];
    let hidden = first.bias.len();
    let mut mouth = vec![0f32; hidden * width];
    let mut mouth_bias = vec![0f32; hidden];
    for row in 0..hidden {
        let weights = &first.weight[row * first.inputs..][..first.inputs];
        for (folded, (q, d)) in mouth[row * width..][..width].iter_mut().zip(
            weights[..width]
                .iter()
                .zip(&weights[neutral_at..present_at]),
        ) {
            *folded = q + d;
        }
        mouth_bias[row] = first.bias[row] + dot(&weights[anchors_at..neutral_at], &setup.anchors)
            - dot(&weights[neutral_at..present_at], &setup.anchors[..width])
            + dot(&weights[present_at..], &setup.present);
    }
    let brow_width = setup.brow_neutral.len();
    let first = &heads.brow[0];
    let hidden = first.bias.len();
    let mut brow = vec![0f32; hidden * brow_width];
    let mut brow_bias = vec![0f32; hidden];
    for row in 0..hidden {
        let weights = &first.weight[row * first.inputs..][..first.inputs];
        for (folded, (w, d)) in brow[row * brow_width..][..brow_width].iter_mut().zip(
            weights[..brow_width]
                .iter()
                .zip(&weights[brow_width..2 * brow_width]),
        ) {
            *folded = w + d;
        }
        brow_bias[row] = first.bias[row]
            - dot(&weights[brow_width..2 * brow_width], &setup.brow_neutral)
            + weights[2 * brow_width] * setup.brow_present;
    }
    Folded {
        mouth,
        mouth_bias,
        brow,
        brow_bias,
    }
}

/// One frame through both heads, keeping what the backward pass needs.
struct Pass {
    pre1: Vec<f32>,
    hidden1: Vec<f32>,
    pre2: Vec<f32>,
    hidden2: Vec<f32>,
    logits: Vec<f32>,
    brow_pre: Vec<f32>,
    brow_hidden: Vec<f32>,
    brow_logits: Vec<f32>,
}

fn forward(heads: &Heads, folded: &Folded, q: &[f32], w: &[f32]) -> Pass {
    let pre1: Vec<f32> = folded
        .mouth_bias
        .iter()
        .enumerate()
        .map(|(row, bias)| bias + dot(&folded.mouth[row * q.len()..][..q.len()], q))
        .collect();
    let hidden1: Vec<f32> = pre1.iter().map(|&x| silu(x)).collect();
    let pre2 = affine(&heads.mouth[1], &hidden1);
    let hidden2: Vec<f32> = pre2.iter().map(|&x| silu(x)).collect();
    let logits = affine(&heads.mouth[2], &hidden2);
    let brow_pre: Vec<f32> = folded
        .brow_bias
        .iter()
        .enumerate()
        .map(|(row, bias)| bias + dot(&folded.brow[row * w.len()..][..w.len()], w))
        .collect();
    let brow_hidden: Vec<f32> = brow_pre.iter().map(|&x| silu(x)).collect();
    let brow_logits = affine(&heads.brow[1], &brow_hidden);
    Pass {
        pre1,
        hidden1,
        pre2,
        hidden2,
        logits,
        brow_pre,
        brow_hidden,
        brow_logits,
    }
}

/// Where the mouth head's outputs QFT+ sends sit among its names.
struct Outputs {
    cheeks: [usize; 4],
}

/// Each mouth and brow output's target and weight.
type Targets = (Vec<(f32, f32)>, [(f32, f32); 8]);

/// Each output's target and weight: its label, else what QFT+'s heads gave.
fn targets(example: &Example, outputs: &Outputs) -> Targets {
    let weigh = |label: f32| LABEL_WEIGHT * if label > ACTIVE { ACTIVE_BOOST } else { 1.0 };
    let mut mouth: Vec<(f32, f32)> = example
        .base_mouth
        .iter()
        .map(|&base| (base, DISTILL_WEIGHT))
        .collect();
    for (&index, label) in outputs.cheeks.iter().zip(example.cheeks) {
        if let Some(label) = label {
            mouth[index] = (label, weigh(label));
        }
    }
    let brows = std::array::from_fn(|k| match example.brows[k] {
        Some(label) => (label, weigh(label)),
        None => (example.base_brows[k], DISTILL_WEIGHT),
    });
    (mouth, brows)
}

/// Gradients of what trains: the part of the mouth head's first layer that
/// reads `q` and its bias, the brow head's that reads `w`, and the rest.
struct Grads {
    mouth: Vec<f32>,
    mouth_bias: Vec<f32>,
    layer2: Vec<f32>,
    bias2: Vec<f32>,
    layer3: Vec<f32>,
    bias3: Vec<f32>,
    brow: Vec<f32>,
    brow_bias: Vec<f32>,
    brow2: Vec<f32>,
    brow_bias2: Vec<f32>,
}

impl Grads {
    fn zero(heads: &Heads, folded: &Folded) -> Self {
        let zeros = |v: &Vec<f32>| vec![0f32; v.len()];
        Self {
            mouth: zeros(&folded.mouth),
            mouth_bias: zeros(&folded.mouth_bias),
            layer2: zeros(&heads.mouth[1].weight),
            bias2: zeros(&heads.mouth[1].bias),
            layer3: zeros(&heads.mouth[2].weight),
            bias3: zeros(&heads.mouth[2].bias),
            brow: zeros(&folded.brow),
            brow_bias: zeros(&folded.brow_bias),
            brow2: zeros(&heads.brow[1].weight),
            brow_bias2: zeros(&heads.brow[1].bias),
        }
    }

    fn add(mut self, other: Self) -> Self {
        for (mine, theirs) in [
            (&mut self.mouth, &other.mouth),
            (&mut self.mouth_bias, &other.mouth_bias),
            (&mut self.layer2, &other.layer2),
            (&mut self.bias2, &other.bias2),
            (&mut self.layer3, &other.layer3),
            (&mut self.bias3, &other.bias3),
            (&mut self.brow, &other.brow),
            (&mut self.brow_bias, &other.brow_bias),
            (&mut self.brow2, &other.brow2),
            (&mut self.brow_bias2, &other.brow_bias2),
        ] {
            axpy(1.0, theirs, mine);
        }
        self
    }

    /// One frame's gradients, scaled by `scale`.
    #[allow(clippy::too_many_arguments)]
    fn backward(
        &mut self,
        heads: &Heads,
        example: &Example,
        pass: &Pass,
        mouth_targets: &[(f32, f32)],
        brow_targets: &[(f32, f32); 8],
        scale: f32,
    ) {
        let [_, layer2, layer3] = &heads.mouth;
        let mut hidden2 = vec![0f32; pass.hidden2.len()];
        for (output, (&logit, &(target, weight))) in
            pass.logits.iter().zip(mouth_targets).enumerate()
        {
            let slope = scale * weight * (sigmoid(logit) - target);
            if slope == 0.0 {
                continue;
            }
            let row = &layer3.weight[output * layer3.inputs..][..layer3.inputs];
            axpy(
                slope,
                &pass.hidden2,
                &mut self.layer3[output * layer3.inputs..][..layer3.inputs],
            );
            self.bias3[output] += slope;
            axpy(slope, row, &mut hidden2);
        }
        let mut hidden1 = vec![0f32; pass.hidden1.len()];
        for (unit, (&grad, &pre)) in hidden2.iter().zip(&pass.pre2).enumerate() {
            let slope = grad * silu_slope(pre);
            let row = &layer2.weight[unit * layer2.inputs..][..layer2.inputs];
            axpy(
                slope,
                &pass.hidden1,
                &mut self.layer2[unit * layer2.inputs..][..layer2.inputs],
            );
            self.bias2[unit] += slope;
            axpy(slope, row, &mut hidden1);
        }
        let width = example.q.len();
        for (unit, (&grad, &pre)) in hidden1.iter().zip(&pass.pre1).enumerate() {
            let slope = grad * silu_slope(pre);
            axpy(slope, &example.q, &mut self.mouth[unit * width..][..width]);
            self.mouth_bias[unit] += slope;
        }

        let brow2 = &heads.brow[1];
        let mut brow_hidden = vec![0f32; pass.brow_hidden.len()];
        for (output, (&logit, &(target, weight))) in
            pass.brow_logits.iter().zip(brow_targets).enumerate()
        {
            let slope = scale * weight * (sigmoid(logit) - target);
            let row = &brow2.weight[output * brow2.inputs..][..brow2.inputs];
            axpy(
                slope,
                &pass.brow_hidden,
                &mut self.brow2[output * brow2.inputs..][..brow2.inputs],
            );
            self.brow_bias2[output] += slope;
            axpy(slope, row, &mut brow_hidden);
        }
        let width = example.w.len();
        for (unit, (&grad, &pre)) in brow_hidden.iter().zip(&pass.brow_pre).enumerate() {
            let slope = grad * silu_slope(pre);
            axpy(slope, &example.w, &mut self.brow[unit * width..][..width]);
            self.brow_bias[unit] += slope;
        }
    }

    /// As the heads' own tensors ([`Heads::tensors`]): the folded first
    /// layers' gradients go to both the `q` and `q - neutral` parts (`w` and
    /// `w - neutral` for the brows), and nothing to the frozen ones.
    fn expand(self, heads: &Heads, setup: &Setup) -> Vec<Vec<f32>> {
        let width = setup.mouth_width();
        let [_, _, neutral_at, present_at] = mouth_parts(width);
        let first = &heads.mouth[0];
        let mut mouth = vec![0f32; first.weight.len()];
        for (unit, row) in mouth.chunks_mut(first.inputs).enumerate() {
            let grad = &self.mouth[unit * width..][..width];
            row[..width].copy_from_slice(grad);
            let neutral = &mut row[neutral_at..present_at];
            neutral.copy_from_slice(grad);
            axpy(-self.mouth_bias[unit], &setup.anchors[..width], neutral);
        }
        let brow_width = setup.brow_neutral.len();
        let first = &heads.brow[0];
        let mut brow = vec![0f32; first.weight.len()];
        for (unit, row) in brow.chunks_mut(first.inputs).enumerate() {
            let grad = &self.brow[unit * brow_width..][..brow_width];
            row[..brow_width].copy_from_slice(grad);
            let neutral = &mut row[brow_width..2 * brow_width];
            neutral.copy_from_slice(grad);
            axpy(-self.brow_bias[unit], &setup.brow_neutral, neutral);
        }
        vec![
            mouth,
            self.mouth_bias,
            self.layer2,
            self.bias2,
            self.layer3,
            self.bias3,
            brow,
            self.brow_bias,
            self.brow2,
            self.brow_bias2,
        ]
    }
}

/// Adam, with each weight pulled toward where it started.
struct Adam {
    moments: Vec<(Vec<f32>, Vec<f32>)>,
    steps: i32,
}

impl Adam {
    fn new(heads: &Heads) -> Self {
        Self {
            moments: heads
                .tensors()
                .iter()
                .map(|t| (vec![0f32; t.len()], vec![0f32; t.len()]))
                .collect(),
            steps: 0,
        }
    }

    fn step(&mut self, heads: &mut Heads, start: &Heads, grads: &[Vec<f32>], rate: f32) {
        const BETAS: (f32, f32) = (0.9, 0.999);
        self.steps += 1;
        let first = 1.0 - BETAS.0.powi(self.steps);
        let second = 1.0 - BETAS.1.powi(self.steps);
        for (((param, start), grad), (mean, square)) in heads
            .tensors_mut()
            .into_iter()
            .zip(start.tensors())
            .zip(grads)
            .zip(&mut self.moments)
        {
            param
                .par_iter_mut()
                .zip(start.par_iter())
                .zip(grad.par_iter())
                .zip(mean.par_iter_mut())
                .zip(square.par_iter_mut())
                .for_each(|((((p, s), g), m), v)| {
                    *m = BETAS.0 * *m + (1.0 - BETAS.0) * g;
                    *v = BETAS.1 * *v + (1.0 - BETAS.1) * g * g;
                    let update = (*m / first) / ((*v / second).sqrt() + 1e-8);
                    *p -= rate * (update + PULL * (*p - s));
                });
        }
    }
}

fn clip(grads: &mut [Vec<f32>], limit: f32) {
    let norm = grads
        .iter()
        .flatten()
        .map(|g| (g * g) as f64)
        .sum::<f64>()
        .sqrt() as f32;
    if norm > limit {
        let scale = limit / norm;
        for g in grads.iter_mut().flatten() {
            *g *= scale;
        }
    }
}

/// The heads' mean cross-entropy on the labels of `examples`; lower is
/// better.
fn score(heads: &Heads, setup: &Setup, outputs: &Outputs, examples: &[&Example]) -> f64 {
    let folded = fold(heads, setup);
    let (total, weight) = examples
        .par_iter()
        .map(|example| {
            let pass = forward(heads, &folded, &example.q, &example.w);
            let (mouth, brows) = targets(example, outputs);
            let labelled = outputs
                .cheeks
                .iter()
                .zip(example.cheeks)
                .filter(|(_, label)| label.is_some())
                .map(|(&index, _)| (pass.logits[index], mouth[index]))
                .chain(
                    example
                        .brows
                        .iter()
                        .zip(pass.brow_logits.iter().zip(brows))
                        .filter(|(label, _)| label.is_some())
                        .map(|(_, (&logit, target))| (logit, target)),
                );
            labelled.fold((0f64, 0f64), |(total, sum), (logit, (target, weight))| {
                let weight = (weight * example.weight) as f64;
                (total + weight * bce(logit, target) as f64, sum + weight)
            })
        })
        .reduce(|| (0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
    if weight > 0.0 {
        total / weight
    } else {
        0.0
    }
}

/// Gives each pose (or follow-the-dot direction) the same total weight
/// within `examples`.
fn balance(examples: &mut [&mut Example]) {
    let mut counts: HashMap<(usize, String), usize> = HashMap::new();
    for example in examples.iter() {
        *counts.entry(example.key.clone()).or_default() += 1;
    }
    let per_pose = examples.len() as f32 / counts.len().max(1) as f32;
    for example in examples.iter_mut() {
        example.weight = per_pose / counts[&example.key] as f32;
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

/// QFT+'s `tongue_map` on any labelled directions, the held poses' among
/// them, in its primal form: the same weights, from a solve the size of `q`
/// rather than of the frames. The gains come from the frames pointing
/// straight left, right, down and up. `None` without a few of each.
fn fit_tongue(samples: &[(&[f32], [f32; 2])]) -> Option<TongueMapV2> {
    let straight = |direction: [f32; 2]| {
        samples
            .iter()
            .filter(move |(_, label)| {
                (label[0] - direction[0]).abs() < 1e-3 && (label[1] - direction[1]).abs() < 1e-3
            })
            .map(|(q, _)| *q)
    };
    let ways = [[-1.0, 0.0], [1.0, 0.0], [0.0, -1.0], [0.0, 1.0]];
    if ways
        .iter()
        .any(|&way| straight(way).count() < DIRECTION_FRAMES)
    {
        return None;
    }
    let (rows, dim) = (samples.len(), samples[0].0.len());
    let mut mean = vec![0f64; dim];
    for (q, _) in samples {
        for (m, &v) in mean.iter_mut().zip(*q) {
            *m += v as f64;
        }
    }
    mean.iter_mut().for_each(|m| *m /= rows as f64);
    let mut scale = vec![0f64; dim];
    for (q, _) in samples {
        for ((s, &v), m) in scale.iter_mut().zip(*q).zip(&mean) {
            *s += (v as f64 - m).powi(2);
        }
    }
    scale
        .iter_mut()
        .for_each(|s| *s = (*s / rows as f64).sqrt() + 1e-3);
    let normal = |q: &[f32]| -> Vec<f64> {
        q.iter()
            .zip(&mean)
            .zip(&scale)
            .map(|((&v, m), s)| (v as f64 - m) / s)
            .collect()
    };
    // (x'x + RIDGE * dim * I) weights = x'y
    let (gram, moments) = samples
        .par_chunks(64)
        .map(|chunk| {
            let mut gram = vec![0f64; dim * dim];
            let mut moments = vec![[0f64; 2]; dim];
            for (q, label) in chunk {
                let x = normal(q);
                for (j, &a) in x.iter().enumerate() {
                    for (g, &b) in gram[j * dim..][..dim].iter_mut().zip(&x) {
                        *g += a * b;
                    }
                    moments[j][0] += a * label[0] as f64;
                    moments[j][1] += a * label[1] as f64;
                }
            }
            (gram, moments)
        })
        .reduce(
            || (vec![0f64; dim * dim], vec![[0f64; 2]; dim]),
            |(mut gram, mut moments), (other_gram, other_moments)| {
                gram.iter_mut().zip(other_gram).for_each(|(a, b)| *a += b);
                for (a, b) in moments.iter_mut().zip(other_moments) {
                    a[0] += b[0];
                    a[1] += b[1];
                }
                (gram, moments)
            },
        );
    let system: Vec<Vec<f64>> = (0..dim)
        .map(|j| {
            let mut row = gram[j * dim..][..dim].to_vec();
            row[j] += RIDGE * dim as f64;
            row
        })
        .collect();
    let solved = solve(system, moments)?;
    let weights: Vec<f64> = solved.iter().flatten().copied().collect();
    let reach = |way: [f32; 2], axis: usize| -> f64 {
        let mut read: Vec<f64> = straight(way)
            .map(|q| {
                normal(q)
                    .iter()
                    .enumerate()
                    .map(|(j, x)| x * weights[j * 2 + axis])
                    .sum()
            })
            .collect();
        median(&mut read)
    };
    let gain = |reach: f64| {
        if reach >= TONGUE_REACH {
            (1.0 / reach).min(TONGUE_GAIN)
        } else {
            1.0
        }
    };
    Some(TongueMapV2 {
        mean: mean.iter().map(|&v| v as f32).collect(),
        scale: scale.iter().map(|&v| v as f32).collect(),
        weights: weights.iter().map(|&v| v as f32).collect(),
        gains: [
            gain(-reach(ways[0], 0)),
            gain(reach(ways[1], 0)),
            gain(-reach(ways[2], 1)),
            gain(reach(ways[3], 1)),
        ],
    })
}

/// The mean error of a direction reading, by axis, on `examples`.
fn direction_error(examples: &[&Example], read: impl Fn(&Example) -> [f32; 2]) -> f64 {
    let total: f64 = examples
        .iter()
        .filter_map(|example| {
            let label = example.direction?;
            let [h, v] = read(example);
            Some(((h - label[0]).abs() + (v - label[1]).abs()) as f64 / 2.0)
        })
        .sum();
    total / examples.len().max(1) as f64
}

fn reading(map: Option<&TongueMapV2>, example: &Example) -> [f32; 2] {
    match map {
        Some(map) => {
            let (h, v) = tongue_direction(&example.q, map);
            [h as f32, v as f32]
        }
        None => example.head_direction,
    }
}

fn round(value: f64) -> f64 {
    (value * 1e5).round() / 1e5
}

struct Job<'a> {
    progress: &'a Progress,
    started: Instant,
    written: Option<Instant>,
}

impl Job<'_> {
    /// Reports how far the run is, at most every 2 seconds unless `force`.
    fn report(&mut self, stage: TrainingStage, message: String, done: f64, force: bool) {
        if !force
            && self
                .written
                .is_some_and(|at| at.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.written = Some(Instant::now());
        self.progress.report(TrainingProgress {
            focus: Some("face".into()),
            fraction: Some(done as f32),
            eta_seconds: estimate(self.started.elapsed().as_secs_f64(), 0.0, done),
            ..TrainingProgress::new(stage, message)
        });
    }
}

/// Fine-tunes the `universal-face-v2` model in the request's base folder on
/// `recordings`, read against the request's face setup, writing the new
/// model, `report.json` and `progress.json` into `output`.
pub(crate) fn run(
    request: &Request,
    recordings: &[Recording],
    output: &Path,
    options: &Options,
    progress: &Progress,
    accelerator: Accelerator,
) -> Result<()> {
    let mut job = Job {
        progress,
        started: Instant::now(),
        written: None,
    };
    for recording in recordings {
        if recording.layout != CameraLayout::all() {
            bail!(
                "{} holds only the mouth cameras; the face model needs recordings made with All five cameras on",
                recording.name()
            );
        }
    }
    let base_dir = &request.base_model_dir;
    let base = base_dir.join(FILE_NAME);
    let mut model = UniversalV2::load(&base, accelerator)
        .with_context(|| format!("loading the model to fine-tune from {}", base.display()))?;
    let face_setup = match &request.face_setup {
        Some(dir) => {
            let dir = std::path::absolute(dir)?;
            let setup = Enrollment::from_recording(&dir)?;
            model.enroll(&setup.frames)?;
            Some((dir, setup))
        }
        None if model.allows_no_enrollment => None,
        None => bail!("This model needs a face setup to fine-tune"),
    };
    let setup = Setup::of(&model);
    let base_map = model.tongue_map.clone();
    // The face setup's held tongue poses, unless its recording trains anyway.
    let mut holds: Vec<(Vec<f32>, [f32; 2])> = vec![];
    if let Some((dir, setup)) = &face_setup {
        if !recordings.iter().any(|recording| recording.dir == *dir) {
            for (slot, [h, v]) in TONGUE_DIRECTION {
                for strip in setup.frames.get(slot).into_iter().flatten() {
                    holds.push((model.run(strip)?.0, [h as f32, v as f32]));
                }
            }
        }
    }

    let selected: Vec<Vec<&Sample>> = recordings
        .iter()
        .map(|recording| select(&recording.samples))
        .collect();
    let total: usize = selected.iter().map(Vec::len).sum();
    if total == 0 {
        bail!("The recordings have no frames to train on");
    }
    let mut examples: Vec<Example> = Vec::with_capacity(total);
    for (index, (recording, selected)) in recordings.iter().zip(&selected).enumerate() {
        let held = held_back(selected);
        let indices: Vec<usize> = selected.iter().map(|sample| sample.index).collect();
        let mut failure = None;
        let mut position = 0;
        recording.read_whole(&indices, |strip| {
            if failure.is_some() {
                return;
            }
            match model.run(strip) {
                Ok((q, t, w)) => {
                    let sample = selected[position];
                    let (cheeks, brows, direction) = labels(sample);
                    examples.push(Example {
                        q,
                        w,
                        head_direction: [t[1].tanh(), t[2].tanh()],
                        cheeks,
                        brows,
                        direction,
                        key: (index, sampling_key(sample)),
                        held_out: held[position],
                        weight: 1.0,
                        base_mouth: vec![],
                        base_brows: [0.0; 8],
                    });
                }
                Err(error) => failure = Some(error),
            }
            position += 1;
            job.report(
                TrainingStage::Training,
                format!(
                    "Reading your recordings ({} of {total} frames)",
                    examples.len()
                ),
                READING * examples.len() as f64 / total as f64,
                false,
            );
        })?;
        if let Some(error) = failure {
            return Err(error);
        }
    }

    // Too few held back to score: everything trains.
    let heads_held = examples
        .iter()
        .filter(|e| e.held_out && e.labelled())
        .count()
        >= HOLD_OUT_MIN;
    let tongue_held = examples
        .iter()
        .filter(|e| e.held_out && e.direction.is_some())
        .count()
        >= HOLD_OUT_MIN;
    let held_out_frames = if heads_held || tongue_held {
        examples.iter().filter(|e| e.held_out).count()
    } else {
        0
    };
    let trained_on = {
        let (mut training, mut scoring): (Vec<&mut Example>, Vec<&mut Example>) = examples
            .iter_mut()
            .partition(|e| !(e.held_out && heads_held));
        balance(&mut training);
        balance(&mut scoring);
        training.len()
    };

    let start = Heads {
        mouth: model.head.clone(),
        brow: model.brow.clone(),
    };
    let names = &model.names;
    let outputs = Outputs {
        cheeks: CHEEKS.map(|name| names.iter().position(|n| n == name).unwrap_or(0)),
    };
    let folded = fold(&start, &setup);
    examples.par_iter_mut().for_each(|example| {
        let pass = forward(&start, &folded, &example.q, &example.w);
        example.base_mouth = pass.logits.iter().map(|&x| sigmoid(x)).collect();
        example.base_brows = std::array::from_fn(|k| sigmoid(pass.brow_logits[k]));
    });
    let train: Vec<usize> = (0..examples.len())
        .filter(|&i| !(examples[i].held_out && heads_held))
        .collect();
    let held: Vec<&Example> = examples
        .iter()
        .filter(|e| e.held_out && heads_held)
        .collect();

    let epochs = options.epochs;
    let rate = options.learning_rate.unwrap_or(LEARNING_RATE) as f32;
    let mut heads = start.clone();
    let mut adam = Adam::new(&heads);
    let mut rng = StdRng::seed_from_u64(42);
    let mut scores = vec![round(score(&heads, &setup, &outputs, &held))];
    let mut kept = (0, heads.clone());
    let mut order = train.clone();
    let batches = train.len().div_ceil(BATCH).max(1);
    for epoch in 1..=epochs {
        order.shuffle(&mut rng);
        for (index, batch) in order.chunks(BATCH).enumerate() {
            let folded = fold(&heads, &setup);
            let norm: f32 = batch.iter().map(|&i| examples[i].weight).sum();
            let grads = batch
                .par_chunks(CHUNK)
                .map(|chunk| {
                    let mut grads = Grads::zero(&heads, &folded);
                    for &i in chunk {
                        let example = &examples[i];
                        let pass = forward(&heads, &folded, &example.q, &example.w);
                        let (mouth, brows) = targets(example, &outputs);
                        grads.backward(
                            &heads,
                            example,
                            &pass,
                            &mouth,
                            &brows,
                            example.weight / norm,
                        );
                    }
                    grads
                })
                .reduce(|| Grads::zero(&heads, &folded), Grads::add);
            let mut grads = grads.expand(&heads, &setup);
            clip(&mut grads, MAX_GRADIENT_NORM);
            adam.step(&mut heads, &start, &grads, rate);
            let done = ((epoch - 1) as f64 + (index + 1) as f64 / batches as f64) / epochs as f64;
            job.report(
                TrainingStage::Training,
                format!("Fine-tuning QFT+'s heads (pass {epoch} of {epochs})"),
                READING + TRAINING * done,
                false,
            );
        }
        if heads_held {
            let scored = round(score(&heads, &setup, &outputs, &held));
            if scored < scores[kept.0] {
                kept = (epoch, heads.clone());
            }
            scores.push(scored);
        }
    }
    let (kept_pass, heads) = if heads_held { kept } else { (epochs, heads) };

    job.report(
        TrainingStage::Calibrating,
        "Fitting your tongue directions".into(),
        READING + TRAINING + 0.02,
        true,
    );
    let fit_from = |include_held: bool| -> Option<TongueMapV2> {
        let samples: Vec<(&[f32], [f32; 2])> = holds
            .iter()
            .map(|(q, direction)| (q.as_slice(), *direction))
            .chain(examples.iter().filter_map(|e| {
                (include_held || !e.held_out)
                    .then_some(())
                    .and(e.direction.map(|direction| (e.q.as_slice(), direction)))
            }))
            .collect();
        fit_tongue(&samples)
    };
    let tongue_test: Vec<&Example> = examples
        .iter()
        .filter(|e| e.held_out && e.direction.is_some())
        .collect();
    let (tongue_map, tongue_kept) = if tongue_held {
        let before = round(direction_error(&tongue_test, |e| {
            reading(base_map.as_ref(), e)
        }));
        match fit_from(false) {
            Some(fitted) => {
                let after = round(direction_error(&tongue_test, |e| reading(Some(&fitted), e)));
                let better = after < before;
                (
                    better.then_some(fitted),
                    Some(ReportKept {
                        focus: "tongue".into(),
                        epoch: better as u32,
                        scores: vec![before, after],
                    }),
                )
            }
            None => (None, None),
        }
    } else {
        (fit_from(true), None)
    };

    job.report(
        TrainingStage::Calibrating,
        "Saving your model".into(),
        0.98,
        true,
    );
    let mut arrays = npz::read(&base)?;
    let Some(Array::Text(meta)) = arrays.remove("meta") else {
        bail!("{} has no metadata", base.display());
    };
    let mut meta: serde_json::Value = serde_json::from_str(&meta)?;
    let name = request.name.clone().unwrap_or_else(|| {
        output
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    meta["provenance"] = json!(format!(
        "{}; heads fine-tuned on one wearer's recordings by VRFaceTracking",
        meta["provenance"].as_str().unwrap_or_default()
    ));
    let shape = |name: &str| -> Vec<usize> {
        match arrays.get(name) {
            Some(Array::Float { shape, .. }) => shape.clone(),
            _ => vec![],
        }
    };
    let mut written: Vec<(String, Array)> = vec![("meta".into(), Array::Text(meta.to_string()))];
    let trained = [
        "head_w1", "head_b1", "head_w2", "head_b2", "head_w3", "head_b3", "brow_w1", "brow_b1",
        "brow_w2", "brow_b2",
    ];
    for (name, values) in trained.iter().zip(heads.tensors()) {
        written.push((
            name.to_string(),
            Array::Float {
                shape: shape(name),
                values: values.clone(),
            },
        ));
    }
    let mut others: Vec<String> = arrays
        .keys()
        .filter(|name| !trained.contains(&name.as_str()) && !TONGUE_ARRAYS.contains(&name.as_str()))
        .cloned()
        .collect();
    others.sort();
    for name in others {
        let array = arrays.remove(&name).expect("listed");
        written.push((name, array));
    }
    if let Some(map) = &tongue_map {
        let float = |shape: Vec<usize>, values: Vec<f32>| Array::Float { shape, values };
        let dim = map.mean.len();
        for (name, array) in TONGUE_ARRAYS.iter().zip([
            float(vec![dim], map.mean.clone()),
            float(vec![dim], map.scale.clone()),
            float(vec![dim, 2], map.weights.clone()),
            float(vec![4], map.gains.map(|g| g as f32).to_vec()),
        ]) {
            written.push((name.to_string(), array));
        }
    }
    npz::write(&output.join(FILE_NAME), &written)?;
    let graph = base_dir.join(GRAPH_FILE);
    if std::fs::hard_link(&graph, output.join(GRAPH_FILE)).is_err() {
        std::fs::copy(&graph, output.join(GRAPH_FILE))
            .with_context(|| format!("copying {}", graph.display()))?;
    }
    // It loads as any QFT+ model does.
    UniversalV2::load(&output.join(FILE_NAME), Accelerator::Cpu)
        .context("the fine-tuned model doesn't load")?;

    let count = |label: &dyn Fn(&Example) -> Option<f32>| -> (u64, u64) {
        examples
            .iter()
            .filter(|e| !(e.held_out && heads_held))
            .filter_map(label)
            .fold((0, 0), |(labelled, active), value| {
                (labelled + 1, active + (value > ACTIVE) as u64)
            })
    };
    let mut labelled = BTreeMap::new();
    let (mut supported, mut disabled) = (vec![], vec![]);
    let outputs_labelled = CHEEKS
        .iter()
        .enumerate()
        .map(|(k, name)| (name, count(&|e: &Example| e.cheeks[k])))
        .chain(
            BROWS
                .iter()
                .enumerate()
                .map(|(k, name)| (name, count(&|e: &Example| e.brows[k]))),
        );
    for (name, (frames, active)) in outputs_labelled {
        labelled.insert(
            name.to_string(),
            json!({"frames": frames, "active": active}),
        );
        if frames > 0 {
            supported.push(name.to_string());
        } else {
            disabled.push(name.to_string());
        }
    }
    let directions = examples.iter().filter(|e| e.direction.is_some()).count() + holds.len();
    if tongue_map.is_some() {
        supported.push("TongueDirection".into());
    }
    let report = TrainingReport {
        name,
        device: model.device().to_string(),
        recordings: recordings
            .iter()
            .map(|r| r.dir.display().to_string())
            .collect(),
        frames: Some(examples.len() as u64),
        coverage: json!({
            "frames": examples.len(),
            "trained_on": trained_on,
            "labelled": labelled,
            "tongue_directions": directions,
        }),
        epochs: epochs as u32,
        supported_targets: supported,
        disabled_targets: disabled,
        base_model_dir: base_dir.display().to_string(),
        seconds: Some((job.started.elapsed().as_secs_f64() * 10.0).round() / 10.0),
        held_out_frames: held_out_frames as u64,
        kept: [
            heads_held.then(|| ReportKept {
                focus: "face".into(),
                epoch: kept_pass as u32,
                scores,
            }),
            tongue_kept,
        ]
        .into_iter()
        .flatten()
        .collect(),
        face_setup: face_setup.map(|(dir, _)| dir.display().to_string()),
        ..TrainingReport::default()
    };
    write_json(&output.join("report.json"), &serde_json::to_value(&report)?)?;
    progress.report(TrainingProgress {
        fraction: Some(1.0),
        report: Some(report),
        ..TrainingProgress::new(TrainingStage::Complete, "Training complete")
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    fn layer(rng: &mut StdRng, outputs: usize, inputs: usize, spread: f32) -> Layer {
        Layer {
            weight: (0..outputs * inputs)
                .map(|_| rng.random_range(-spread..spread))
                .collect(),
            bias: (0..outputs).map(|_| rng.random_range(-0.1..0.1)).collect(),
            inputs,
        }
    }

    /// Small heads of QFT+'s shape: `q` of 4 per slot, `w` of 3.
    fn small(rng: &mut StdRng) -> (Heads, Setup) {
        let (q, w) = (4, 3);
        let heads = Heads {
            mouth: [
                layer(rng, 6, 8 * q + 6, 0.4),
                layer(rng, 5, 6, 0.4),
                layer(rng, 7, 5, 0.4),
            ],
            brow: [layer(rng, 4, 2 * w + 1, 0.4), layer(rng, 8, 4, 0.4)],
        };
        let setup = Setup {
            anchors: (0..6 * q).map(|_| rng.random_range(-1.0..1.0)).collect(),
            present: vec![1.0, 1.0, 0.0, 1.0, 1.0, 0.0],
            brow_neutral: (0..w).map(|_| rng.random_range(-1.0..1.0)).collect(),
            brow_present: 1.0,
        };
        (heads, setup)
    }

    fn example(rng: &mut StdRng, outputs: usize) -> Example {
        Example {
            q: (0..4).map(|_| rng.random_range(-1.0..1.0)).collect(),
            w: (0..3).map(|_| rng.random_range(-1.0..1.0)).collect(),
            head_direction: [0.0; 2],
            cheeks: [Some(0.9), None, Some(0.0), None],
            brows: [
                Some(0.2),
                None,
                None,
                Some(1.0),
                None,
                None,
                Some(0.0),
                None,
            ],
            direction: None,
            key: (0, "pose".into()),
            held_out: false,
            weight: 1.0,
            base_mouth: (0..outputs).map(|_| rng.random_range(0.0..1.0)).collect(),
            base_brows: std::array::from_fn(|_| rng.random_range(0.0..1.0)),
        }
    }

    fn loss(heads: &Heads, setup: &Setup, example: &Example, outputs: &Outputs) -> f64 {
        let pass = forward(heads, &fold(heads, setup), &example.q, &example.w);
        let (mouth, brows) = targets(example, outputs);
        pass.logits
            .iter()
            .zip(&mouth)
            .chain(pass.brow_logits.iter().zip(&brows))
            .map(|(&logit, &(target, weight))| (weight * bce(logit, target)) as f64)
            .sum()
    }

    #[test]
    fn folding_the_first_layers_reads_as_qfts_heads_do() {
        let mut rng = StdRng::seed_from_u64(1);
        let (heads, setup) = small(&mut rng);
        let example = example(&mut rng, 7);
        let pass = forward(&heads, &fold(&heads, &setup), &example.q, &example.w);
        // head_forward: [q, anchors, q - neutral, present].
        let mut z = example.q.clone();
        z.extend(&setup.anchors);
        z.extend(example.q.iter().zip(&setup.anchors).map(|(q, n)| q - n));
        z.extend(&setup.present);
        let h1: Vec<f32> = heads.mouth[0].apply(&z).into_iter().map(silu).collect();
        let h2: Vec<f32> = heads.mouth[1].apply(&h1).into_iter().map(silu).collect();
        for (a, b) in heads.mouth[2].apply(&h2).iter().zip(&pass.logits) {
            assert!((a - b).abs() < 1e-5, "{a} {b}");
        }
        let mut z = example.w.clone();
        z.extend(
            example
                .w
                .iter()
                .zip(&setup.brow_neutral)
                .map(|(w, n)| w - n),
        );
        z.push(setup.brow_present);
        let h: Vec<f32> = heads.brow[0].apply(&z).into_iter().map(silu).collect();
        for (a, b) in heads.brow[1].apply(&h).iter().zip(&pass.brow_logits) {
            assert!((a - b).abs() < 1e-5, "{a} {b}");
        }
    }

    #[test]
    fn gradients_match_finite_differences_and_leave_the_anchor_weights() {
        let mut rng = StdRng::seed_from_u64(2);
        let (heads, setup) = small(&mut rng);
        let outputs = Outputs {
            cheeks: [0, 1, 2, 3],
        };
        let example = example(&mut rng, 7);
        let folded = fold(&heads, &setup);
        let pass = forward(&heads, &folded, &example.q, &example.w);
        let (mouth, brows) = targets(&example, &outputs);
        let mut grads = Grads::zero(&heads, &folded);
        grads.backward(&heads, &example, &pass, &mouth, &brows, 1.0);
        let grads = grads.expand(&heads, &setup);
        let [_, anchors_at, neutral_at, present_at] = mouth_parts(4);
        for (tensor, grad) in grads.iter().enumerate() {
            for index in (0..grad.len()).step_by(3) {
                let nudged = |delta: f32| {
                    let mut heads = heads.clone();
                    heads.tensors_mut()[tensor][index] += delta;
                    loss(&heads, &setup, &example, &outputs)
                };
                let numeric = ((nudged(1e-3) - nudged(-1e-3)) / 2e-3) as f32;
                let frozen = match tensor {
                    0 => {
                        let column = index % (8 * 4 + 6);
                        (anchors_at..neutral_at).contains(&column) || column >= present_at
                    }
                    6 => index % 7 == 6,
                    _ => false,
                };
                if frozen {
                    assert_eq!(grad[index], 0.0, "tensor {tensor} index {index} is frozen");
                } else {
                    assert!(
                        (numeric - grad[index]).abs() < 2e-3,
                        "tensor {tensor} index {index}: {numeric} vs {}",
                        grad[index]
                    );
                }
            }
        }
    }

    #[test]
    fn the_primal_tongue_fit_is_qfts_and_reads_straight_poses_fully() {
        let mut rng = StdRng::seed_from_u64(3);
        let axes: Vec<[f32; 2]> = (0..16)
            .map(|_| [rng.random_range(-1.0..1.0), rng.random_range(-1.0..1.0)])
            .collect();
        let mut holds: HashMap<&str, Vec<Vec<f32>>> = HashMap::new();
        let mut samples: Vec<(Vec<f32>, [f32; 2])> = vec![];
        for (slot, [h, v]) in TONGUE_DIRECTION {
            for _ in 0..6 {
                let q: Vec<f32> = axes
                    .iter()
                    .map(|a| a[0] * h as f32 + a[1] * v as f32 + rng.random_range(-0.05..0.05))
                    .collect();
                holds.entry(slot).or_default().push(q.clone());
                samples.push((q, [h as f32, v as f32]));
            }
        }
        let qft = super::super::tongue_map(&holds).unwrap();
        let samples: Vec<(&[f32], [f32; 2])> =
            samples.iter().map(|(q, d)| (q.as_slice(), *d)).collect();
        let fitted = fit_tongue(&samples).unwrap();
        for (a, b) in qft.weights.iter().zip(&fitted.weights) {
            assert!((a - b).abs() < 1e-4, "{a} {b}");
        }
        assert_eq!(
            qft.gains.map(|g| (g * 1e4).round()),
            fitted.gains.map(|g| (g * 1e4).round())
        );
        for (q, [h, v]) in &samples {
            let (rh, rv) = tongue_direction(q, &fitted);
            assert!((rh as f32 - h).abs() < 0.35 && (rv as f32 - v).abs() < 0.35);
        }
        assert!(
            fit_tongue(&samples[..12]).is_none(),
            "every straight direction is needed"
        );
    }

    #[test]
    fn training_lowers_the_labelled_loss() {
        let mut rng = StdRng::seed_from_u64(4);
        let (start, setup) = small(&mut rng);
        let outputs = Outputs {
            cheeks: [0, 1, 2, 3],
        };
        let examples: Vec<Example> = (0..64)
            .map(|_| {
                let mut e = example(&mut rng, 7);
                let on = e.q[0] > 0.0;
                e.cheeks = [Some(on as u8 as f32), Some(0.0), None, None];
                e.brows = [
                    Some((e.w[1] > 0.0) as u8 as f32),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                ];
                e
            })
            .collect();
        let all: Vec<&Example> = examples.iter().collect();
        let before = score(&start, &setup, &outputs, &all);
        let mut heads = start.clone();
        let mut adam = Adam::new(&heads);
        for _ in 0..150 {
            let folded = fold(&heads, &setup);
            let mut grads = Grads::zero(&heads, &folded);
            for example in &examples {
                let pass = forward(&heads, &folded, &example.q, &example.w);
                let (mouth, brows) = targets(example, &outputs);
                grads.backward(&heads, example, &pass, &mouth, &brows, 1.0 / 64.0);
            }
            let mut grads = grads.expand(&heads, &setup);
            clip(&mut grads, MAX_GRADIENT_NORM);
            adam.step(&mut heads, &start, &grads, 1e-2);
        }
        let after = score(&heads, &setup, &outputs, &all);
        assert!(after < before * 0.8, "{before} -> {after}");
    }
}
