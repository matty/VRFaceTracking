//! Writes a model's float ONNX graph, as ONNX Runtime runs it before
//! switching to int8, for inspection with tools such as Netron. Developer
//! tool:
//!
//! cargo run -p vrft-tongue --release --example export_onnx -- <universal-face-v1.safetensors | gate or direction checkpoint> <out.onnx>

use std::path::PathBuf;

use anyhow::{Context, Result};
use vrft_tongue::checkpoint::Checkpoint;
use vrft_tongue::onnx;
use vrft_tongue::universal::checkpoint::FaceCheckpoint;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (input, output) = match &args[..] {
        [input, output] => (PathBuf::from(input), PathBuf::from(output)),
        _ => anyhow::bail!("usage: export_onnx <checkpoint> <out.onnx>"),
    };
    let graph = match FaceCheckpoint::load(&input) {
        Ok(face) => onnx::face_graph(&face.weights, face.metadata.image_size)?,
        Err(_) => {
            let checkpoint = Checkpoint::load(&input).context("not a face or tongue checkpoint")?;
            let mut weights = checkpoint.weights.clone();
            weights.remove("signed_mask");
            weights.retain(|name, _| !name.ends_with("num_batches_tracked"));
            onnx::pair_graph(&weights, checkpoint.metadata.image_size)?
        }
    };
    std::fs::write(&output, graph.to_model())?;
    println!("wrote {}", output.display());
    Ok(())
}
