//! Packs rendered synthetic recordings into one set stored at the model's
//! input size, which personal training mixes in: a third of the download
//! the headset's 400 px views would take, and shrunk exactly as training
//! shrinks recorded frames. Developer tool:
//!
//! cargo run -p vrft-tongue --release --example pack_synthetic -- <output dir> <recording>... [--size 224]

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use vrft_tongue::preprocess::{AreaResize, VIEW};
use vrft_tongue::recordings::Recording;
use vrft_tongue::TARGETS;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let size_flag = args.iter().position(|a| a == "--size");
    let size: usize = match size_flag {
        Some(index) => args
            .get(index + 1)
            .context("--size needs a value")?
            .parse()?,
        None => 224,
    };
    let paths: Vec<PathBuf> = args
        .iter()
        .enumerate()
        .filter(|(i, _)| size_flag.is_none_or(|flag| *i != flag && *i != flag + 1))
        .map(|(_, a)| PathBuf::from(a))
        .collect();
    let [output, sources @ ..] = &paths[..] else {
        bail!("usage: pack_synthetic <output dir> <recording>... [--size 224]");
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
    let mut views = vec![vec![0u8; size * size]; 2];
    for source in sources {
        let recording = Recording::open(source)?;
        if recording.view != VIEW || !recording.synthetic {
            bail!("{} isn't a rendered synthetic recording", source.display());
        }
        generators.push(metadata(source)?["synthetic"].clone());
        let lines = lines(source)?;
        let indices: Vec<usize> = recording.samples.iter().map(|s| s.index).collect();
        let mut kept = indices.iter();
        recording.read_frames(&indices, |strip| {
            for (view, out) in views.iter_mut().enumerate() {
                resize.view(strip, view, out);
            }
            // Rows of both views side by side, as in a recording.
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
    let metadata = json!({
        "format": "vrft-tongue-capture-v1",
        "mode": "synthetic",
        "width": 2 * size,
        "height": size,
        "bytesPerFrame": 2 * size * size,
        "targets": TARGETS,
        "synthetic": {"packed": sources.len(), "frames": index, "sources": generators},
    });
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
