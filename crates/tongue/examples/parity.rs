//! Compares Rust preprocessing and inference with golden PyTorch/OpenCV
//! outputs. Developer tool:
//!
//! cargo run -p vrft-tongue --release --example parity -- <golden-dir> <model-dir>
//!
//! The golden directory holds `frames.u8` (N 800x400 strips),
//! `resized<size>.u8` (N x 2 views from cv2.resize INTER_AREA) and
//! `outputs.json` (`gate`/`direction` raw model outputs per frame).

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use burn::tensor::backend::Backend;
use burn::tensor::{Tensor, TensorData};
use vrft_tongue::backend::{Cpu, Gpu};
use vrft_tongue::model::TongueNet;
use vrft_tongue::preprocess::{AreaResize, FRAME_BYTES};
use vrft_tongue::{Accelerator, Checkpoint, Role, TongueModel, TARGETS};

fn max_error(a: &[f32], b: &[f64]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (*a as f64 - b).abs())
        .fold(0.0, f64::max)
}

fn check_net<B: Backend>(
    name: &str,
    checkpoint: &Checkpoint,
    resized: &[u8],
    expected: &[Vec<f64>],
    device: B::Device,
) -> Result<()> {
    let net = TongueNet::<B>::from_weights(checkpoint.weights.clone(), &device)?;
    let size = checkpoint.metadata.image_size;
    let count = expected.len();
    let pixels: Vec<f32> = resized.iter().map(|&v| v as f32 / 255.0).collect();
    let input = Tensor::<B, 4>::from_data(TensorData::new(pixels, [count, 2, size, size]), &device);
    let started = Instant::now();
    let values = net.forward(input).into_data().to_vec::<f32>().unwrap();
    let elapsed = started.elapsed();
    // The golden outputs predate the cheek heads; max_error compares the
    // tongue heads they have.
    let worst = values
        .chunks(TARGETS.len())
        .zip(expected)
        .map(|(a, b)| max_error(a, b))
        .fold(0.0, f64::max);
    println!("{name}: max abs error {worst:.2e} over {count} frames ({elapsed:?} batched)");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [golden, models] = &args[..] else {
        anyhow::bail!("usage: parity <golden-dir> <model-dir>");
    };
    let outputs: serde_json::Value =
        serde_json::from_slice(&std::fs::read(golden.join("outputs.json"))?)?;
    let expected =
        |key: &str| -> Vec<Vec<f64>> { serde_json::from_value(outputs[key].clone()).unwrap() };
    let frames = std::fs::read(golden.join("frames.u8"))?;
    let count = frames.len() / FRAME_BYTES;

    let gate = Checkpoint::load(&Role::Gate.find(models).unwrap())?;
    let direction = Checkpoint::load(&Role::Direction.find(models).unwrap())?;
    for (role, checkpoint) in [("gate", &gate), ("direction", &direction)] {
        let size = checkpoint.metadata.image_size;
        let golden_resized = std::fs::read(golden.join(format!("resized{size}.u8")))?;
        let resize = AreaResize::new(size);
        let mut ours = vec![0u8; golden_resized.len()];
        for (index, frame) in frames.chunks(FRAME_BYTES).enumerate() {
            for view in 0..2 {
                let offset = (index * 2 + view) * size * size;
                resize.view(frame, view, &mut ours[offset..offset + size * size]);
            }
        }
        let different = ours
            .iter()
            .zip(&golden_resized)
            .filter(|(a, b)| a != b)
            .count();
        let worst = ours
            .iter()
            .zip(&golden_resized)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        println!(
            "resize {size}: {different} of {} pixels differ from OpenCV (max {worst})",
            ours.len()
        );
        let expected = expected(role);
        check_net::<Cpu>(
            &format!("{role} cpu"),
            checkpoint,
            &golden_resized,
            &expected,
            Default::default(),
        )?;
        check_net::<Gpu>(
            &format!("{role} gpu"),
            checkpoint,
            &golden_resized,
            &expected,
            Default::default(),
        )?;
    }

    for accelerator in [Accelerator::Cpu, Accelerator::Gpu] {
        let mut model = TongueModel::load(models, accelerator)?;
        let mut worst = 0f64;
        let started = Instant::now();
        for (index, frame) in frames.chunks(FRAME_BYTES).enumerate() {
            let values = model.predict(frame)?;
            let mut reference = expected("direction")[index].clone();
            reference[0] = expected("gate")[index][0];
            worst = worst.max(max_error(&values, &reference));
        }
        let per_frame = started.elapsed() / count as u32;
        println!(
            "end to end on {}: max abs error {worst:.2e}, {per_frame:?} per frame",
            model.info().device
        );
    }
    Ok(())
}
