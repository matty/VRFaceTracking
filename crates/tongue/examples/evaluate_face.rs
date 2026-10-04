//! Scores a universal face model on a five-camera recording it may not have
//! trained on, read against a face setup: visibility accuracy, and for every
//! labelled output its mean absolute error over all frames and over the
//! frames where it is active, and its gain there (the least-squares slope of
//! prediction against label: under 1 reads short). Developer tool:
//!
//! cargo run -p vrft-tongue --release --example evaluate_face -- <model-dir or checkpoint> <recording-dir> [--enrollment <face setup recording>] [--every N] [--cpu]
//!
//! The recording must hold all five cameras at the headset's 400 px; a
//! rendered set packed at the model's size can't be read back as strips.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use vrft_quest_pro_protocol::{CameraLayout, STRIP_BYTES};
use vrft_tongue::recordings::Recording;
use vrft_tongue::universal::data::labels;
use vrft_tongue::universal::{Enrollment, FaceModel, FACE_TARGETS, FILE_NAME};
use vrft_tongue::Accelerator;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let every: usize = value("--every")
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(3);
    let accelerator = if args.iter().any(|a| a == "--cpu") {
        Accelerator::Cpu
    } else {
        Accelerator::Auto
    };
    let positional: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            !a.starts_with("--")
                && !(*i > 0 && matches!(args[i - 1].as_str(), "--every" | "--enrollment"))
        })
        .map(|(_, a)| a)
        .collect();
    let [model_path, recording_dir] = positional[..] else {
        bail!(
            "usage: evaluate_face <model> <recording-dir> [--enrollment <dir>] [--every N] [--cpu]"
        );
    };
    let model_path = PathBuf::from(model_path);
    let model_path = if model_path.is_dir() {
        model_path.join(FILE_NAME)
    } else {
        model_path
    };
    let mut model = FaceModel::load(&model_path, accelerator)?;
    if let Some(setup) = value("--enrollment") {
        model.enroll(&Enrollment::from_recording(Path::new(setup))?)?;
        println!(
            "enrolled {:?}, tongue map {}",
            model.info().enrolled,
            model.info().tongue_map
        );
    }
    let threshold = model.info().threshold;
    let recording = Recording::open(&PathBuf::from(recording_dir))?;
    if recording.layout != CameraLayout::all() {
        bail!("{recording_dir} doesn't hold all five cameras at 400 px");
    }
    let samples: Vec<_> = recording.samples.iter().step_by(every.max(1)).collect();
    let indices: Vec<usize> = samples.iter().map(|s| s.index).collect();
    let mut predictions = vec![];
    recording.read_whole(&indices, |strip| {
        debug_assert_eq!(strip.len(), STRIP_BYTES);
        predictions.push(model.predict(strip));
    })?;
    let outputs = FACE_TARGETS.len();
    let mut error = vec![(0.0f64, 0usize); outputs];
    let mut active_error = vec![(0.0f64, 0usize); outputs];
    // Per output, sums of prediction x label and label squared over active
    // frames: their ratio is the least-squares gain of the prediction.
    let mut gain = vec![(0.0f64, 0.0f64); outputs];
    let (mut correct, mut total) = (0usize, 0usize);
    for (sample, prediction) in samples.iter().zip(predictions) {
        let values = prediction?.values;
        let (label, labelled) = labels(sample);
        total += 1;
        if (values[0] >= threshold) == (label[0] >= 0.5) {
            correct += 1;
        }
        for output in 1..outputs {
            if !labelled[output] || !model.enabled(output) {
                continue;
            }
            let difference = f64::from((values[output] - label[output]).abs());
            error[output].0 += difference;
            error[output].1 += 1;
            if label[output].abs() > 0.1 {
                active_error[output].0 += difference;
                active_error[output].1 += 1;
                gain[output].0 += f64::from(values[output] * label[output]);
                gain[output].1 += f64::from(label[output] * label[output]);
            }
        }
    }
    println!(
        "visibility accuracy {:.3} ({correct}/{total}) at threshold {threshold:.2}",
        correct as f64 / total.max(1) as f64
    );
    println!(
        "{:<22} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "output", "mae", "frames", "active", "frames", "gain"
    );
    for (output, name) in FACE_TARGETS.iter().enumerate().skip(1) {
        let (sum, count) = error[output];
        if count == 0 {
            continue;
        }
        let (active, active_count) = active_error[output];
        let (along, squared) = gain[output];
        println!(
            "{name:<22} {:>8.3} {count:>8} {:>8.3} {active_count:>8} {:>8.3}",
            sum / count as f64,
            if active_count > 0 {
                active / active_count as f64
            } else {
                f64::NAN
            },
            if squared > 0.0 {
                along / squared
            } else {
                f64::NAN
            }
        );
    }
    Ok(())
}
