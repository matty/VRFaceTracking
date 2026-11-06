//! Scores a `universal-face-v2` model, such as QFT+'s, on a five-camera
//! recording it may not have trained on, read against a face setup: for every
//! labelled output, its mean absolute error over all frames and over the
//! frames where it is active, and its gain there (the least-squares slope of
//! prediction against label: under 1 reads short). Developer tool:
//!
//! cargo run -p vrft-tongue --release --example evaluate_face -- <model.npz> <recording-dir> [--enrollment <face setup recording>] [--every N] [--cpu]
//!
//! It is scored on its heads' own readings, before its event layer: that
//! layer, its tongue's visibility and extension, and its brow raises read
//! Meta's own values, which a recording doesn't hold, so they aren't scored.
//!
//! The recording must hold all five cameras at the headset's 400 px.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use vrft_quest_pro_protocol::{CameraLayout, STRIP_BYTES};
use vrft_tongue::recordings::{Recording, Sample};
use vrft_tongue::universal_v2::{Enrollment, UniversalV2, FACE_TARGETS};
use vrft_tongue::{Accelerator, CHEEK_COLUMNS};

const OUTPUTS: usize = FACE_TARGETS.len();
/// The recorded pose that labels both cheeks sucked in.
const SUCK_POSE: &str = "Cheeks sucked in";

fn target_index(name: &str) -> Option<usize> {
    FACE_TARGETS.iter().position(|target| *target == name)
}

/// A frame's labels: the tongue's when it was labelled, the cheek puffs
/// (and, from the pose, the cheeks sucked in) when the cheeks were, then
/// anything named in its `face` labels.
fn labels(sample: &Sample) -> ([f32; OUTPUTS], [bool; OUTPUTS]) {
    let mut labels = [0.0; OUTPUTS];
    let mut labelled = [false; OUTPUTS];
    let visible = sample.targets[0] >= 0.5;
    labels[0] = sample.targets[0];
    labelled[0] = true;
    for column in 1..4 {
        labels[column] = sample.targets[column];
        labelled[column] = visible;
    }
    if sample.cheeks_labelled {
        for (output, column) in ["cheek_puff_left", "cheek_puff_right"]
            .into_iter()
            .zip(CHEEK_COLUMNS)
        {
            let index = target_index(output).expect("a face output");
            labels[index] = sample.targets[column];
            labelled[index] = true;
        }
        let sucked = if sample.pose == SUCK_POSE { 1.0 } else { 0.0 };
        for output in ["cheek_suck_left", "cheek_suck_right"] {
            let index = target_index(output).expect("a face output");
            labels[index] = sucked;
            labelled[index] = true;
        }
    }
    for (name, value) in &sample.face {
        if let Some(index) = target_index(name) {
            labels[index] = *value;
            labelled[index] = true;
        }
    }
    (labels, labelled)
}

/// The outputs it reads, in `FACE_TARGETS` order: direction, cheeks, brows.
fn scored(output: usize) -> bool {
    matches!(output, 2..=7 | 9..=16)
}

/// Its outputs for a strip.
fn predict(model: &mut UniversalV2, strip: &[u8]) -> Result<[f32; OUTPUTS]> {
    let reading = model.read(strip)?;
    let mut values = [0.0; OUTPUTS];
    values[2] = reading.tongue.0 as f32;
    values[3] = reading.tongue.1 as f32;
    for (k, value) in reading.cheeks.iter().enumerate() {
        values[4 + k] = *value as f32;
    }
    for (k, value) in reading.brows.iter().enumerate() {
        values[9 + k] = *value as f32;
    }
    Ok(values)
}

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
            "usage: evaluate_face <model.npz> <recording-dir> [--enrollment <dir>] [--every N] [--cpu]"
        );
    };
    let mut model = UniversalV2::load(&PathBuf::from(model_path), accelerator)?;
    if let Some(setup) = value("--enrollment") {
        let setup = Enrollment::from_recording(Path::new(setup))?;
        model.enroll(&setup.frames)?;
        println!(
            "enrolled {:?}, tongue map {}",
            model.enrolled(),
            model.has_tongue_map()
        );
    }
    let recording = Recording::open(&PathBuf::from(recording_dir))?;
    if recording.layout != CameraLayout::all() {
        bail!("{recording_dir} doesn't hold all five cameras at 400 px");
    }
    let samples: Vec<_> = recording.samples.iter().step_by(every.max(1)).collect();
    let indices: Vec<usize> = samples.iter().map(|s| s.index).collect();
    let mut predictions = vec![];
    recording.read_whole(&indices, |strip| {
        debug_assert_eq!(strip.len(), STRIP_BYTES);
        predictions.push(predict(&mut model, strip));
    })?;
    let mut error = vec![(0.0f64, 0usize); OUTPUTS];
    let mut active_error = vec![(0.0f64, 0usize); OUTPUTS];
    // Per output, sums of prediction x label and label squared over active
    // frames: their ratio is the least-squares gain of the prediction.
    let mut gain = vec![(0.0f64, 0.0f64); OUTPUTS];
    for (sample, prediction) in samples.iter().zip(predictions) {
        let values = prediction?;
        let (label, labelled) = labels(sample);
        for output in 1..OUTPUTS {
            if !labelled[output] || !scored(output) {
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
