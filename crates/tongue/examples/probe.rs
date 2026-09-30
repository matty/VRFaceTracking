//! Judges training recordings in seconds rather than a training run: fits
//! simple classifiers on the base model pair's features of the `train`
//! recordings and scores them on a `test` recording, usually a real one the
//! train set leaves out. Training with the image layers locked scored
//! within a point of full training, so these features say what training
//! would learn. Developer tool:
//!
//! cargo run -p vrft-tongue --release --example probe -- <model-dir> <test-recording> <train-recording>... [--cpu]
//!
//! - visibility: logistic regression on the gate's features.
//! - direction: ridge regression of horizontal and vertical on the
//!   direction model's features, over tongue-out frames.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use burn::tensor::backend::Backend;
use burn::tensor::{Tensor, TensorData};
use vrft_tongue::backend::{Cpu, Gpu};
use vrft_tongue::dataset::Frames;
use vrft_tongue::model::{TongueNet, FEATURES};
use vrft_tongue::recordings::Recording;
use vrft_tongue::{Checkpoint, Role};

const RIDGE: f64 = 1.0;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cpu = args.iter().any(|a| a == "--cpu");
    let paths: Vec<PathBuf> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .collect();
    let [model_dir, test, train @ ..] = &paths[..] else {
        bail!("usage: probe <model-dir> <test-recording> <train-recording>... [--cpu]");
    };
    if train.is_empty() {
        bail!("name at least one train recording");
    }
    if cpu {
        run::<Cpu>(model_dir, test, train)
    } else {
        run::<Gpu>(model_dir, test, train)
    }
}

fn run<B: Backend>(model_dir: &Path, test: &Path, train: &[PathBuf]) -> Result<()> {
    let device = B::Device::default();
    let load = |role: Role| -> Result<(TongueNet<B>, usize)> {
        let path = role
            .find(model_dir)
            .ok_or_else(|| anyhow!("no {role:?} model in {}", model_dir.display()))?;
        let checkpoint = Checkpoint::load(&path)?;
        let size = checkpoint.metadata.image_size;
        Ok((TongueNet::from_weights(checkpoint.weights, &device)?, size))
    };
    let (gate, size) = load(Role::Gate)?;
    let (direction, _) = load(Role::Direction)?;
    let test_frames = Frames::load(&[Recording::open(test)?], Some(size))?;
    let train = train
        .iter()
        .map(|path| Recording::open(path))
        .collect::<Result<Vec<_>>>()?;
    let train_frames = Frames::load(&train, Some(size))?;
    println!(
        "probe: {} train frames, {} test frames",
        train_frames.len(),
        test_frames.len()
    );

    let gate_train = features(&gate, &train_frames, &device)?;
    let gate_test = features(&gate, &test_frames, &device)?;
    let shown = |frames: &Frames| -> Vec<bool> {
        frames.records.iter().map(|r| r.targets[0] >= 0.5).collect()
    };
    let (train_shown, test_shown) = (shown(&train_frames), shown(&test_frames));

    // Visibility, class-balanced.
    let scale = Standardise::fit(&gate_train);
    let x_train = scale.apply(&gate_train);
    let x_test = scale.apply(&gate_test);
    let y: Vec<f64> = train_shown.iter().map(|&s| s as u8 as f64).collect();
    let weights = balanced(&train_shown);
    let beta = logistic(&x_train, &y, &weights)?;
    let p_test: Vec<f64> = x_test.iter().map(|x| sigmoid(dot(&beta, x))).collect();
    let p_train: Vec<f64> = x_train.iter().map(|x| sigmoid(dot(&beta, x))).collect();
    let (auc_train, ..) = separation(&p_train, &train_shown);
    let (auc, best, at) = separation(&p_test, &test_shown);
    let at_half = p_test
        .iter()
        .zip(&test_shown)
        .filter(|(p, s)| (**p >= 0.5) == **s)
        .count() as f64
        / p_test.len() as f64;
    println!("visibility: train AUC {auc_train:.3} | test AUC {auc:.3}, {:.1}% at 0.5, best {:.1}% at {at:.3}",
        100.0 * at_half, 100.0 * best);
    let mut poses: BTreeMap<&str, (usize, f64, usize)> = BTreeMap::new();
    for ((record, p), s) in test_frames.records.iter().zip(&p_test).zip(&test_shown) {
        let entry = poses.entry(record.key.as_str()).or_default();
        entry.0 += 1;
        entry.1 += p;
        entry.2 += ((*p >= at) == *s) as usize;
    }
    for (pose, (n, sum, right)) in poses {
        println!(
            "  {pose:<28} n {n:>3}  visible {:.2}  right at best {:>3.0}%",
            sum / n as f64,
            100.0 * right as f64 / n as f64
        );
    }

    // Direction, on tongue-out frames only.
    let out = |frames: &Frames, shown: &[bool], all: Vec<Vec<f32>>| {
        let mut rows = vec![];
        let mut labels = vec![];
        for ((row, record), s) in all.into_iter().zip(&frames.records).zip(shown) {
            if *s {
                rows.push(row);
                labels.push([record.targets[2] as f64, record.targets[3] as f64]);
            }
        }
        (rows, labels)
    };
    let (d_train, l_train) = out(
        &train_frames,
        &train_shown,
        features(&direction, &train_frames, &device)?,
    );
    let (d_test, l_test) = out(
        &test_frames,
        &test_shown,
        features(&direction, &test_frames, &device)?,
    );
    if !d_train.is_empty() && !d_test.is_empty() {
        let scale = Standardise::fit(&d_train);
        let (x_train, x_test) = (scale.apply(&d_train), scale.apply(&d_test));
        let ones = vec![1.0; x_train.len()];
        let (mut signs, mut total) = (0, 0);
        let mut error = 0.0;
        for axis in 0..2 {
            let y: Vec<f64> = l_train.iter().map(|l| l[axis]).collect();
            let beta = ridge(&x_train, &y, &ones)?;
            for (x, label) in x_test.iter().zip(&l_test) {
                let said = dot(&beta, x).clamp(-1.0, 1.0);
                error += (said - label[axis]).powi(2);
                if label[axis].abs() >= 0.5 {
                    total += 1;
                    signs += (said.signum() == label[axis].signum() && said.abs() >= 0.25) as usize;
                }
            }
        }
        println!(
            "direction: test RMS error {:.3}, sign right {signs}/{total}",
            (error / (2 * l_test.len()) as f64).sqrt()
        );
    }

    Ok(())
}

/// The features the last head layer reads, for every frame, unaugmented.
fn features<B: Backend>(
    model: &TongueNet<B>,
    frames: &Frames,
    device: &B::Device,
) -> Result<Vec<Vec<f32>>> {
    let size = frames.size;
    let mut out = Vec::with_capacity(frames.len());
    for images in frames.images.chunks(32) {
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
    Ok(out)
}

/// Per-feature mean and spread of one set, applied to any set; each row
/// gets a trailing 1 for the intercept.
struct Standardise {
    mean: Vec<f64>,
    spread: Vec<f64>,
}

impl Standardise {
    fn fit(rows: &[Vec<f32>]) -> Self {
        let n = rows.len().max(1) as f64;
        let mut mean = vec![0.0; FEATURES];
        for row in rows {
            for (m, v) in mean.iter_mut().zip(row) {
                *m += f64::from(*v) / n;
            }
        }
        let mut spread = vec![0.0; FEATURES];
        for row in rows {
            for ((s, v), m) in spread.iter_mut().zip(row).zip(&mean) {
                *s += (f64::from(*v) - m).powi(2) / n;
            }
        }
        let spread = spread.into_iter().map(|s| s.sqrt().max(1e-6)).collect();
        Self { mean, spread }
    }

    fn apply(&self, rows: &[Vec<f32>]) -> Vec<Vec<f64>> {
        rows.iter()
            .map(|row| {
                let mut x: Vec<f64> = row
                    .iter()
                    .zip(&self.mean)
                    .zip(&self.spread)
                    .map(|((v, m), s)| (f64::from(*v) - m) / s)
                    .collect();
                x.push(1.0);
                x
            })
            .collect()
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Weights that give each class the same total, summing to the count.
fn balanced(labels: &[bool]) -> Vec<f64> {
    let positives = labels.iter().filter(|l| **l).count().max(1) as f64;
    let negatives = labels.iter().filter(|l| !**l).count().max(1) as f64;
    let n = labels.len() as f64;
    labels
        .iter()
        .map(|&l| n / 2.0 / if l { positives } else { negatives })
        .collect()
}

/// Solves `a x = b` for symmetric positive definite `a` (n x n, row major).
fn solve(mut a: Vec<f64>, mut b: Vec<f64>) -> Result<Vec<f64>> {
    let n = b.len();
    for j in 0..n {
        let diagonal = a[j * n + j] - (0..j).map(|k| a[j * n + k].powi(2)).sum::<f64>();
        if diagonal.is_nan() || diagonal <= 0.0 {
            bail!("singular system");
        }
        let diagonal = diagonal.sqrt();
        a[j * n + j] = diagonal;
        for i in j + 1..n {
            let dot = (0..j).map(|k| a[i * n + k] * a[j * n + k]).sum::<f64>();
            a[i * n + j] = (a[i * n + j] - dot) / diagonal;
        }
    }
    for i in 0..n {
        let dot = (0..i).map(|k| a[i * n + k] * b[k]).sum::<f64>();
        b[i] = (b[i] - dot) / a[i * n + i];
    }
    for i in (0..n).rev() {
        let dot = (i + 1..n).map(|k| a[k * n + i] * b[k]).sum::<f64>();
        b[i] = (b[i] - dot) / a[i * n + i];
    }
    Ok(b)
}

/// `sum w (x x^T)` plus the ridge on every coefficient but the intercept.
fn gram(x: &[Vec<f64>], w: &[f64]) -> Vec<f64> {
    let n = x[0].len();
    let mut a = vec![0.0; n * n];
    for (row, &weight) in x.iter().zip(w) {
        for i in 0..n {
            let scaled = weight * row[i];
            for j in 0..=i {
                a[i * n + j] += scaled * row[j];
            }
        }
    }
    for i in 0..n {
        for j in 0..i {
            a[j * n + i] = a[i * n + j];
        }
        if i + 1 < n {
            a[i * n + i] += RIDGE;
        } else {
            a[i * n + i] += 1e-9;
        }
    }
    a
}

fn ridge(x: &[Vec<f64>], y: &[f64], w: &[f64]) -> Result<Vec<f64>> {
    let n = x[0].len();
    let mut rhs = vec![0.0; n];
    for ((row, &target), &weight) in x.iter().zip(y).zip(w) {
        for (r, v) in rhs.iter_mut().zip(row) {
            *r += weight * v * target;
        }
    }
    solve(gram(x, w), rhs)
}

/// Weighted logistic regression by Newton's method, with the ridge.
fn logistic(x: &[Vec<f64>], y: &[f64], w: &[f64]) -> Result<Vec<f64>> {
    let n = x[0].len();
    let mut beta = vec![0.0; n];
    for _ in 0..30 {
        let p: Vec<f64> = x.iter().map(|row| sigmoid(dot(&beta, row))).collect();
        let curvature: Vec<f64> = p
            .iter()
            .zip(w)
            .map(|(p, w)| w * (p * (1.0 - p)).max(1e-6))
            .collect();
        let mut gradient: Vec<f64> = beta.iter().map(|b| RIDGE * b).collect();
        gradient[n - 1] = 0.0;
        for (((row, p), target), weight) in x.iter().zip(&p).zip(y).zip(w) {
            for (g, v) in gradient.iter_mut().zip(row) {
                *g += weight * (p - target) * v;
            }
        }
        let step = solve(gram(x, &curvature), gradient)?;
        let size: f64 = step.iter().map(|s| s * s).sum::<f64>().sqrt();
        for (b, s) in beta.iter_mut().zip(&step) {
            *b -= s;
        }
        if size < 1e-6 {
            break;
        }
    }
    Ok(beta)
}

/// ROC AUC of `scores` for `labels`, and the best accuracy any threshold
/// reaches with that threshold.
fn separation(scores: &[f64], labels: &[bool]) -> (f64, f64, f64) {
    let mut pairs: Vec<(f64, bool)> = scores.iter().copied().zip(labels.iter().copied()).collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let positives = pairs.iter().filter(|p| p.1).count();
    let negatives = pairs.len() - positives;
    let mut rank_sum = 0.0;
    let mut start = 0;
    while start < pairs.len() {
        let end = start + pairs[start..].partition_point(|p| p.0 == pairs[start].0);
        let mean_rank = (start + end + 1) as f64 / 2.0;
        rank_sum += mean_rank * pairs[start..end].iter().filter(|p| p.1).count() as f64;
        start = end;
    }
    let auc = (rank_sum - (positives * (positives + 1)) as f64 / 2.0)
        / (positives * negatives).max(1) as f64;
    let (mut best, mut at) = (negatives as f64 / pairs.len().max(1) as f64, f64::INFINITY);
    let (mut hidden_below, mut shown_below) = (0, 0);
    let mut previous = None;
    for &(score, shown) in &pairs {
        if previous != Some(score) {
            let accuracy = (hidden_below + positives - shown_below) as f64 / pairs.len() as f64;
            if accuracy > best {
                (best, at) = (accuracy, score);
            }
        }
        previous = Some(score);
        if shown {
            shown_below += 1;
        } else {
            hidden_below += 1;
        }
    }
    (auc, best, at)
}
