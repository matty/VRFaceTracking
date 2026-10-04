//! Replays frames through VRFT's port of QFT+'s universal face model and
//! prints each frame's outputs as JSON lines, for
//! `tools/benchmark/qftplus_parity.py --compare`. Developer tool:
//!
//! cargo run -p vrft-tongue --release --example universal_v2_replay -- <replay.json> [--model <universal-face-v2.npz>]

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use vrft_tongue::backend::Accelerator;
use vrft_tongue::universal_v2::{Native, UniversalV2};

const FRAME: usize = 400 * 2000;

#[derive(Deserialize)]
struct NativeSample {
    arrival: i64,
    values: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct Frame {
    index: usize,
    now: i64,
    native: Option<NativeSample>,
}

#[derive(Deserialize)]
struct Replay {
    recording: PathBuf,
    setup: BTreeMap<String, Vec<usize>>,
    frames: Vec<Frame>,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let replay_path = args
        .first()
        .context("usage: universal_v2_replay <replay.json> [--model <npz>]")?;
    let model_path = args
        .iter()
        .position(|a| a == "--model")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".local/qftplus/universal-face-v2.npz"));
    let replay: Replay = serde_json::from_str(&std::fs::read_to_string(replay_path)?)?;
    let frames = std::fs::read(replay.recording.join("frames.gray8"))?;
    let strip = |index: usize| frames[index * FRAME..(index + 1) * FRAME].to_vec();

    // QFT+ runs ONNX Runtime with one thread; so does this, for the same sums.
    std::env::set_var("VRFT_ONNX_THREADS", "1");
    let mut model = UniversalV2::load(&model_path, Accelerator::Cpu)?;
    let setup = replay
        .setup
        .iter()
        .map(|(slot, indices)| (slot.clone(), indices.iter().map(|&i| strip(i)).collect()))
        .collect();
    model.enroll(&setup)?;
    for frame in &replay.frames {
        let native = frame.native.as_ref().map(|n| Native {
            arrival: n.arrival,
            values: n.values.iter().map(|(k, v)| (k.clone(), *v)).collect(),
        });
        let out = model.update(&strip(frame.index), native.as_ref(), frame.now)?;
        println!(
            "{}",
            json!({
                "cheeks": out.cheeks,
                "brows": out.brows,
                "raises": out.raises,
                "tongue_visible": out.tongue_visible,
                "tongue_extension": out.tongue_extension,
                "tongue_horizontal": out.tongue_horizontal,
                "tongue_vertical": out.tongue_vertical,
            })
        );
    }
    Ok(())
}
