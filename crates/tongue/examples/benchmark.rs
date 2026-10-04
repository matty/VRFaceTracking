//! Times a tongue model the way the daemon runs it, one frame at a time, and
//! reports its latency and the process's memory. Developer tool:
//!
//! cargo run -p vrft-tongue --release --example benchmark -- (--pair <model dir> | --face <checkpoint>) [--device cpu|gpu] [--frames N] [--warmup N] [--recording <dir>] [--enrollment <face setup recording>] [--json]
//!
//! - `--pair`: the stereo gate and direction pair, on the 800 x 400 mouth
//!   pair; `--face`: the universal face model, on 2000 x 400 five-camera
//!   strips.
//! - Frames come from `--recording` (cycled), else from a fixed noise strip:
//!   the networks do the same work whatever the pixels are.
//! - On the CPU, `RAYON_NUM_THREADS` sets how many threads Burn uses.
//! - Memory is the process's resident set (working set on Windows): before
//!   loading, after loading and its peak.
//!
//! `tools/benchmark/compare.py` runs this beside QFT+'s model.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use serde_json::json;
use vrft_quest_pro_protocol::{CameraLayout, STRIP_BYTES};
use vrft_tongue::backend::Accelerator;
use vrft_tongue::infer::TongueModel;
use vrft_tongue::recordings::Recording;
use vrft_tongue::universal::infer::{Enrollment, FaceModel};

enum Model {
    Pair(Box<TongueModel>),
    Face(Box<FaceModel>),
}

impl Model {
    fn frame_bytes(&self) -> usize {
        match self {
            Model::Pair(_) => CameraLayout::mouth().frame_bytes(),
            Model::Face(_) => STRIP_BYTES,
        }
    }

    fn predict(&mut self, frame: &[u8]) -> Result<()> {
        match self {
            Model::Pair(model) => model.predict(frame).map(drop),
            Model::Face(model) => model.predict(frame).map(drop),
        }
    }

    fn device(&self) -> String {
        match self {
            Model::Pair(model) => model.info().device.clone(),
            Model::Face(model) => model.info().device.clone(),
        }
    }
}

/// The process's resident memory and its peak so far, in bytes.
#[cfg(target_os = "linux")]
fn memory() -> Option<(u64, u64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| -> Option<u64> {
        let line = status.lines().find(|line| line.starts_with(name))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    };
    Some((field("VmRSS:")?, field("VmHWM:")?))
}

#[cfg(windows)]
fn memory() -> Option<(u64, u64)> {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: the counters are sized for the call, and the pseudo-handle
    // from GetCurrentProcess needs no closing.
    unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) }.ok()?;
    Some((
        counters.WorkingSetSize as u64,
        counters.PeakWorkingSetSize as u64,
    ))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn memory() -> Option<(u64, u64)> {
    None
}

fn megabytes(bytes: Option<u64>) -> serde_json::Value {
    bytes.map_or(serde_json::Value::Null, |bytes| {
        json!((bytes as f64 / 1048576.0 * 10.0).round() / 10.0)
    })
}

fn frames_from(recording: Option<&Path>, bytes: usize) -> Result<Vec<Vec<u8>>> {
    let Some(dir) = recording else {
        // A fixed noise strip: xorshift, so every run times the same input.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let strip = (0..bytes)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 56) as u8
            })
            .collect();
        return Ok(vec![strip]);
    };
    let recording = Recording::open(dir)?;
    let indices: Vec<usize> = recording.samples.iter().take(64).map(|s| s.index).collect();
    let mut frames = vec![];
    if bytes == STRIP_BYTES {
        if recording.layout != CameraLayout::all() {
            bail!("{} doesn't hold all five cameras at 400 px", dir.display());
        }
        recording.read_whole(&indices, |strip| frames.push(strip.to_vec()))?;
    } else {
        recording.read_frames(&indices, |pair| frames.push(pair.to_vec()))?;
    }
    if frames.is_empty() {
        bail!("{} has no frames", dir.display());
    }
    Ok(frames)
}

fn percentile(sorted: &[f64], share: f64) -> f64 {
    let at = ((sorted.len() - 1) as f64 * share).round() as usize;
    sorted[at]
}

fn files_size(paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .sum()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|argument| argument == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let device: Accelerator = value("--device")
        .unwrap_or_else(|| "cpu".into())
        .parse()
        .map_err(anyhow::Error::msg)?;
    let frames_wanted: usize = value("--frames").map_or(Ok(300), |v| v.parse())?;
    let warmup: usize = value("--warmup").map_or(Ok(30), |v| v.parse())?;
    let recording = value("--recording").map(PathBuf::from);

    let before = memory();
    let started = Instant::now();
    let (mut model, name, files) = match (value("--pair"), value("--face")) {
        (Some(dir), None) => {
            let dir = PathBuf::from(dir);
            let model = TongueModel::load(&dir, device)?;
            let files = std::fs::read_dir(&dir)?
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.extension()
                        .is_some_and(|ext| ext == "safetensors" || ext == "pt")
                })
                .collect();
            (Model::Pair(Box::new(model)), "stereo pair", files)
        }
        (None, Some(path)) => {
            let path = PathBuf::from(path);
            let mut model = FaceModel::load(&path, device)?;
            if let Some(enrollment) = value("--enrollment") {
                model.enroll(&Enrollment::from_recording(Path::new(&enrollment))?)?;
            }
            let file = if path.is_dir() {
                path.join("universal-face-v1.safetensors")
            } else {
                path
            };
            (Model::Face(Box::new(model)), "universal face", vec![file])
        }
        _ => bail!("pass one of --pair <model dir> or --face <checkpoint>"),
    };
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    let loaded = memory();

    let frames =
        frames_from(recording.as_deref(), model.frame_bytes()).context("reading frames")?;
    for frame in frames.iter().cycle().take(warmup) {
        model.predict(frame)?;
    }
    let mut times = Vec::with_capacity(frames_wanted);
    for frame in frames.iter().cycle().take(frames_wanted) {
        let at = Instant::now();
        model.predict(frame)?;
        times.push(at.elapsed().as_secs_f64() * 1000.0);
    }
    let after = memory();
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    times.sort_by(f64::total_cmp);
    let threads = std::env::var("RAYON_NUM_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_else(rayon::current_num_threads);
    let round = |ms: f64| (ms * 1000.0).round() / 1000.0;
    let report = json!({
        "model": name,
        "runtime": "Burn 0.21",
        "device": model.device(),
        "threads": threads,
        "frames": times.len(),
        "input": if recording.is_some() { "recording" } else { "noise" },
        "load_ms": round(load_ms),
        "mean_ms": round(mean),
        "p50_ms": round(percentile(&times, 0.5)),
        "p95_ms": round(percentile(&times, 0.95)),
        "p99_ms": round(percentile(&times, 0.99)),
        "fps": round(1000.0 / mean),
        "model_mb": megabytes(Some(files_size(&files))),
        "rss_before_mb": megabytes(before.map(|m| m.0)),
        "rss_loaded_mb": megabytes(loaded.map(|m| m.0)),
        "rss_after_mb": megabytes(after.map(|m| m.0)),
        "peak_mb": megabytes(after.map(|m| m.1)),
    });
    if args.iter().any(|argument| argument == "--json") {
        println!("{report}");
    } else {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}
