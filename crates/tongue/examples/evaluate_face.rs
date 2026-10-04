//! Scores a universal face model on a five-camera recording it may not have
//! trained on, read against a face setup: visibility accuracy, and for every
//! labelled output its mean absolute error over all frames and over the
//! frames where it is active, and its gain there (the least-squares slope of
//! prediction against label: under 1 reads short). Developer tool:
//!
//! cargo run -p vrft-tongue --release --example evaluate_face -- <model-dir or checkpoint> <recording-dir> [--enrollment <face setup recording>] [--every N] [--per-face] [--cpu]
//!
//! `--per-face` scores a rendered set face by face: each face's setup poses
//! enroll the model, and its other frames are scored.
//!
//! The model can also be a `universal-face-v2` `.npz`, such as QFT+'s. It is
//! scored on its heads' own readings, before its event layer: that layer, its
//! tongue's visibility and extension, and its brow raises read Meta's own
//! values, which a recording doesn't hold, so they aren't scored.
//!
//! The recording must hold all five cameras at the headset's 400 px; a
//! rendered set packed at the model's size can't be read back as strips.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use vrft_quest_pro_protocol::{CameraLayout, STRIP_BYTES};
use vrft_tongue::recordings::Recording;
use vrft_tongue::universal::data::labels;
use vrft_tongue::universal::{Enrollment, FaceModel, FACE_TARGETS, FILE_NAME};
use vrft_tongue::universal_v2::UniversalV2;
use vrft_tongue::Accelerator;

const OUTPUTS: usize = FACE_TARGETS.len();

enum Model {
    V1(Box<FaceModel>),
    V2(Box<UniversalV2>),
}

impl Model {
    fn enroll(&mut self, setup: &Enrollment) -> Result<()> {
        match self {
            Model::V1(model) => {
                model.enroll(setup)?;
                println!(
                    "enrolled {:?}, tongue map {}",
                    model.info().enrolled,
                    model.info().tongue_map
                );
            }
            Model::V2(model) => {
                model.enroll(&setup.frames)?;
                println!(
                    "enrolled {:?}, tongue map {}",
                    model.enrolled(),
                    model.has_tongue_map()
                );
            }
        }
        Ok(())
    }

    /// The outputs it reads, in `FACE_TARGETS` order.
    fn enabled(&self, output: usize) -> bool {
        match self {
            Model::V1(model) => model.enabled(output),
            // Direction, cheeks, brows.
            Model::V2(_) => matches!(output, 2..=7 | 9..=16),
        }
    }

    /// Its outputs for a strip, and whether it saw the tongue (None when it
    /// doesn't decide that itself).
    fn predict(&mut self, strip: &[u8]) -> Result<([f32; OUTPUTS], Option<bool>)> {
        match self {
            Model::V1(model) => {
                let values = model.predict(strip)?.values;
                Ok((values, Some(values[0] >= model.info().threshold)))
            }
            Model::V2(model) => {
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
                Ok((values, None))
            }
        }
    }
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
            "usage: evaluate_face <model> <recording-dir> [--enrollment <dir>] [--every N] [--cpu]"
        );
    };
    let model_path = PathBuf::from(model_path);
    let model_path = if model_path.is_dir() {
        model_path.join(FILE_NAME)
    } else {
        model_path
    };
    let mut model = if model_path.extension().is_some_and(|e| e == "npz") {
        Model::V2(Box::new(UniversalV2::load(&model_path, accelerator)?))
    } else {
        Model::V1(Box::new(FaceModel::load(&model_path, accelerator)?))
    };
    if let Some(setup) = value("--enrollment") {
        model.enroll(&Enrollment::from_recording(Path::new(setup))?)?;
    }
    let recording = Recording::open(&PathBuf::from(recording_dir))?;
    if recording.layout != CameraLayout::all() {
        bail!("{recording_dir} doesn't hold all five cameras at 400 px");
    }
    // With --per-face, each rendered face is read against its own face
    // setup frames, and only its other frames are scored.
    let per_face = args.iter().any(|a| a == "--per-face");
    let mut faces: Vec<Option<String>> = vec![None];
    if per_face {
        faces = recording
            .samples
            .iter()
            .map(|sample| sample.identity.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
    }
    let mut samples = vec![];
    let mut predictions = vec![];
    for face in &faces {
        let of_face =
            |sample: &&vrft_tongue::recordings::Sample| face.is_none() || sample.identity == *face;
        if per_face {
            let setup: Vec<_> = recording
                .samples
                .iter()
                .filter(of_face)
                .filter_map(|sample| Some((sample.index, sample.anchor.clone()?)))
                .collect();
            let mut frames: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
            let mut slots = setup.iter().map(|(_, slot)| slot);
            let indices: Vec<usize> = setup.iter().map(|(index, _)| *index).collect();
            recording.read_whole(&indices, |strip| {
                let slot = slots.next().expect("a slot per frame").clone();
                frames.entry(slot).or_default().push(strip.to_vec());
            })?;
            print!("{}: ", face.as_deref().unwrap_or("?"));
            model.enroll(&Enrollment { frames })?;
        }
        let scored: Vec<_> = recording
            .samples
            .iter()
            .filter(of_face)
            .filter(|sample| !per_face || sample.anchor.is_none())
            .step_by(every.max(1))
            .collect();
        let indices: Vec<usize> = scored.iter().map(|s| s.index).collect();
        recording.read_whole(&indices, |strip| {
            debug_assert_eq!(strip.len(), STRIP_BYTES);
            predictions.push(model.predict(strip));
        })?;
        samples.extend(scored);
    }
    let outputs = OUTPUTS;
    let mut error = vec![(0.0f64, 0usize); outputs];
    let mut active_error = vec![(0.0f64, 0usize); outputs];
    // Per output, sums of prediction x label and label squared over active
    // frames: their ratio is the least-squares gain of the prediction.
    let mut gain = vec![(0.0f64, 0.0f64); outputs];
    let (mut correct, mut total) = (0usize, 0usize);
    for (sample, prediction) in samples.iter().zip(predictions) {
        let (values, visible) = prediction?;
        let (label, labelled) = labels(sample);
        if let Some(visible) = visible {
            total += 1;
            if visible == (label[0] >= 0.5) {
                correct += 1;
            }
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
    if total > 0 {
        println!(
            "visibility accuracy {:.3} ({correct}/{total})",
            correct as f64 / total as f64
        );
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
