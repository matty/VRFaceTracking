//! Packs rendered synthetic recordings into one set stored at the model's
//! input size, which personal training mixes in: a third of the download
//! the headset's 400 px views would take, and shrunk exactly as training
//! shrinks recorded frames. Developer tool:
//!
//! cargo run -p vrft-tongue --release --example pack_synthetic -- <output dir> <recording>... [--size 224] [--cameras mouth|all]
//!
//! `--cameras mouth` (the default) packs the mouth pair, which the stereo
//! tongue pair reads, at 224 px; it takes five-camera recordings too.
//! `--cameras all` packs every camera of five-camera recordings, for the
//! universal face model at 128 px (`--size 128`).

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use vrft_quest_pro_protocol::CameraLayout;
use vrft_tongue::preprocess::{AreaResize, VIEW};
use vrft_tongue::recordings::Recording;
use vrft_tongue::TARGETS;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags = ["--size", "--cameras"];
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .map(|index| {
                args.get(index + 1)
                    .with_context(|| format!("{name} needs a value"))
            })
            .transpose()
    };
    let size: usize = value("--size")?
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(224);
    let all = match value("--cameras")?.map(String::as_str) {
        None | Some("mouth") => false,
        Some("all") => true,
        Some(other) => bail!("--cameras is mouth or all, not {other}"),
    };
    let paths: Vec<PathBuf> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            !flags.contains(&a.as_str()) && !(*i > 0 && flags.contains(&args[i - 1].as_str()))
        })
        .map(|(_, a)| PathBuf::from(a))
        .collect();
    let [output, sources @ ..] = &paths[..] else {
        bail!(
            "usage: pack_synthetic <output dir> <recording>... [--size 224] [--cameras mouth|all]"
        );
    };
    if sources.is_empty() {
        bail!("name at least one recording to pack");
    }
    if output.exists() {
        bail!("{} already exists", output.display());
    }
    std::fs::create_dir_all(output)?;
    let resize = AreaResize::new(size);
    let mut frames = BufWriter::new(File::create(output.join("frames.gray8"))?);
    let mut samples = BufWriter::new(File::create(output.join("samples.jsonl"))?);
    let mut generators = vec![];
    let mut index = 0usize;
    let cameras: Vec<u8> = if all {
        CameraLayout::all().cameras
    } else {
        vrft_quest_pro_protocol::MOUTH_CAMERAS.to_vec()
    };
    let mut views = vec![vec![0u8; size * size]; cameras.len()];
    for source in sources {
        let recording = Recording::open(source)?;
        if recording.view != VIEW || !recording.synthetic {
            bail!("{} isn't a rendered synthetic recording", source.display());
        }
        if all && !recording.layout.is_all() {
            bail!("{} doesn't hold all five cameras", source.display());
        }
        generators.push(metadata(source)?["synthetic"].clone());
        let lines = lines(source)?;
        let indices: Vec<usize> = recording.samples.iter().map(|s| s.index).collect();
        let mut kept = indices.iter();
        let width = recording.layout.width();
        let read = |each: &mut dyn FnMut(&[u8])| -> Result<()> {
            if all {
                recording.read_whole(&indices, each)
            } else {
                recording.read_frames(&indices, each)
            }
        };
        read(&mut |strip: &[u8]| {
            for (view, out) in views.iter_mut().enumerate() {
                if all {
                    resize.view_of(strip, width, view, out);
                } else {
                    resize.view(strip, view, out);
                }
            }
            // Rows of every view side by side, as in a recording.
            for row in 0..size {
                for view in &views {
                    frames
                        .write_all(&view[row * size..][..size])
                        .expect("writing frames");
                }
            }
            let mut line = lines[*kept.next().expect("one line per frame")].clone();
            line["index"] = json!(index);
            line["sequence"] = json!(index);
            writeln!(samples, "{line}").expect("writing samples");
            index += 1;
        })?;
    }
    frames.flush()?;
    samples.flush()?;
    let mut metadata = CameraLayout {
        cameras,
        view: size,
    }
    .metadata();
    metadata["format"] = json!("vrft-tongue-capture-v1");
    metadata["mode"] = json!("synthetic");
    metadata["targets"] = json!(TARGETS);
    metadata["synthetic"] =
        json!({"packed": sources.len(), "frames": index, "sources": generators});
    std::fs::write(
        output.join("metadata.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )?;
    println!(
        "packed {index} frames at {size} px into {}",
        output.display()
    );
    Ok(())
}

fn metadata(dir: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(
        dir.join("metadata.json"),
    )?)?)
}

/// Every line of `samples.jsonl`, by frame index.
fn lines(dir: &Path) -> Result<Vec<Value>> {
    let mut out = vec![];
    for line in BufReader::new(File::open(dir.join("samples.jsonl"))?).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            out.push(serde_json::from_str(&line)?);
        }
    }
    Ok(out)
}
