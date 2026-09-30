//! Scores a tongue model pair on a recording it may not have trained on:
//! per pose, what it predicts against the labels, and overall visibility
//! accuracy and direction errors. Developer tool:
//!
//! cargo run -p vrft-tongue --release --example evaluate -- <model-dir> <recording-dir> [--every N] [--cpu]

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{bail, Result};
use vrft_tongue::recordings::Recording;
use vrft_tongue::{Accelerator, TongueModel};

#[derive(Default)]
struct Pose {
    step: u64,
    count: usize,
    label: [f32; 4],
    sum: [f64; 4],
    visible_right: usize,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name);
    let every = flag("--every")
        .and_then(|i| args.get(i + 1))
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(3usize);
    let accelerator = if flag("--cpu").is_some() {
        Accelerator::Cpu
    } else {
        Accelerator::Auto
    };
    let positional: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && !(*i > 0 && args[i - 1] == "--every"))
        .map(|(_, a)| a)
        .collect();
    let [model_dir, recording_dir] = positional[..] else {
        bail!("usage: evaluate <model-dir> <recording-dir> [--every N] [--cpu]");
    };
    let mut model = TongueModel::load(&PathBuf::from(model_dir), accelerator)?;
    let recording = Recording::open(&PathBuf::from(recording_dir))?;
    let samples: Vec<_> = recording.samples.iter().step_by(every.max(1)).collect();
    let indices: Vec<usize> = samples.iter().map(|s| s.index).collect();
    let threshold = 0.5;

    let mut poses: BTreeMap<String, Pose> = BTreeMap::new();
    let (mut correct, mut total) = (0usize, 0usize);
    let (mut direction_error, mut extension_error, mut visible) = (0.0f64, 0.0f64, 0usize);
    let mut signs = (0usize, 0usize);
    // (visibility, shown) per frame, for scores that need no threshold.
    let mut scores: Vec<(f32, bool)> = vec![];
    let mut outputs = vec![];
    recording.read_frames(&indices, |strip| outputs.push(model.predict(strip)))?;
    for (sample, output) in samples.iter().zip(outputs) {
        let output = output?;
        let pose = poses.entry(sample.pose.clone()).or_default();
        pose.step = sample.step;
        pose.count += 1;
        pose.label.copy_from_slice(&sample.targets[..4]);
        for (sum, value) in pose.sum.iter_mut().zip(&output) {
            *sum += *value as f64;
        }
        let shown = sample.targets[0] >= 0.5;
        let said = output[0] >= threshold;
        scores.push((output[0], shown));
        pose.visible_right += (shown == said) as usize;
        correct += (shown == said) as usize;
        total += 1;
        if shown {
            visible += 1;
            extension_error += (output[1] - sample.targets[1]).abs() as f64;
            direction_error += ((output[2] - sample.targets[2]).powi(2)
                + (output[3] - sample.targets[3]).powi(2))
            .sqrt() as f64;
            for (said, label) in output[2..4].iter().zip(&sample.targets[2..4]) {
                if label.abs() >= 0.5 {
                    signs.1 += 1;
                    signs.0 += (said.signum() == label.signum() && said.abs() >= 0.25) as usize;
                }
            }
        }
    }

    println!(
        "model {model_dir} on {} ({} frames, every {every})",
        recording.name(),
        total
    );
    println!(
        "{:<26} {:>4}  {:>11}  {:>11}  {:>12}  {:>12}  {:>6}",
        "pose", "n", "visible", "extension", "horizontal", "vertical", "vis ok"
    );
    let mut rows: Vec<_> = poses.into_iter().collect();
    rows.sort_by_key(|(_, p)| p.step);
    for (name, p) in rows {
        let mean = |k: usize| p.sum[k] / p.count as f64;
        println!(
            "{:<26} {:>4}  {:>4.2} ({:>4.2})  {:>4.2} ({:>4.2})  {:>+5.2} ({:>+5.2})  {:>+5.2} ({:>+5.2})  {:>5.0}%",
            name,
            p.count,
            mean(0),
            p.label[0],
            mean(1),
            p.label[1],
            mean(2),
            p.label[2],
            mean(3),
            p.label[3],
            100.0 * p.visible_right as f64 / p.count as f64
        );
    }
    println!(
        "visibility accuracy {:.1}%  |  tongue out: extension MAE {:.3}, direction error {:.3}, direction sign right {}/{}",
        100.0 * correct as f64 / total as f64,
        extension_error / visible.max(1) as f64,
        direction_error / visible.max(1) as f64,
        signs.0,
        signs.1
    );
    let (auc, best, at) = separation(&mut scores);
    println!(
        "visibility AUC {auc:.3}  |  best threshold {at:.2} gives {:.1}%",
        100.0 * best
    );
    Ok(())
}

/// How well visibility separates shown from hidden frames whatever the
/// threshold: the ROC AUC, and the best accuracy any threshold reaches with
/// that threshold.
fn separation(scores: &mut [(f32, bool)]) -> (f64, f64, f32) {
    scores.sort_by(|a, b| a.0.total_cmp(&b.0));
    let positives = scores.iter().filter(|(_, shown)| *shown).count();
    let negatives = scores.len() - positives;
    // Rank sum of the shown frames, ties sharing their mean rank.
    let mut rank_sum = 0.0;
    let mut start = 0;
    while start < scores.len() {
        let end = start + scores[start..].partition_point(|s| s.0 == scores[start].0);
        let mean_rank = (start + end + 1) as f64 / 2.0;
        rank_sum += mean_rank * scores[start..end].iter().filter(|s| s.1).count() as f64;
        start = end;
    }
    let pairs = (positives * negatives).max(1) as f64;
    let auc = (rank_sum - (positives * (positives + 1)) as f64 / 2.0) / pairs;
    // Everything at or above scores[i] counts as shown.
    let (mut best, mut at) = (negatives as f64 / scores.len().max(1) as f64, f32::INFINITY);
    let (mut hidden_below, mut shown_below) = (0, 0);
    let mut previous = None;
    for &(score, shown) in scores.iter() {
        if previous != Some(score) {
            let accuracy = (hidden_below + positives - shown_below) as f64 / scores.len() as f64;
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
