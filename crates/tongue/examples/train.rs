//! Trains a tongue model pair from a request file without the app, as
//! `vrft_d train-tongue` does: for GPU servers, where the daemon doesn't
//! build. Developer tool:
//!
//! cargo run -p vrft-tongue --release --features cuda --example train -- --request <file> --output <new folder> [--epochs N] [--learning-rate R] [--layers all|head|output] [--init <face checkpoint>]
//!
//! A request with `"architecture": "universal-face-v1"` trains the universal
//! face model instead of the pair; `--init` starts it from another face
//! checkpoint rather than the base pair's encoder.

use anyhow::{Context, Result};
use vrft_tongue::train::{run, Options};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|argument| argument == flag)
            .and_then(|index| args.get(index + 1))
    };
    let request = value("--request").context("--request is required")?;
    let output = value("--output").context("--output is required")?;
    let mut options = Options::default();
    if let Some(epochs) = value("--epochs") {
        options.epochs = epochs.parse().context("--epochs must be a whole number")?;
    }
    if let Some(rate) = value("--learning-rate") {
        options.learning_rate = Some(rate.parse().context("--learning-rate must be a number")?);
    }
    if let Some(layers) = value("--layers") {
        options.trainable = layers.parse().map_err(anyhow::Error::msg)?;
    }
    options.init = value("--init").map(Into::into);
    run(request.as_ref(), output.as_ref(), &options)
}
