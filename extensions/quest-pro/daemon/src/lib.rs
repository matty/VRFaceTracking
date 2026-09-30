//! Quest Pro support for the daemon: the headset's mouth and eye cameras,
//! independent eye gaze, pupil size, and camera-based tongue and cheek puff
//! tracking with personal model training. It adds to whichever tracking
//! module is active rather than replacing it.
mod builtin;
mod camera;
mod capture;
mod eye;
mod priority;
mod pupil;
mod settings;
mod training;
mod transfer;

use anyhow::Context as _;
use vrft_extension::{DaemonExtension, HostContext, Started};

pub use vrft_quest_pro_protocol::ID;

#[derive(Default)]
pub struct QuestPro;

impl DaemonExtension for QuestPro {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        vrft_quest_pro_protocol::NAME
    }

    fn run_subcommand(&self, name: &str, arguments: &[String]) -> Option<anyhow::Result<()>> {
        (name == "train-tongue").then(|| train_tongue(arguments))
    }

    fn start(self: Box<Self>, host: HostContext) -> anyhow::Result<Started> {
        let running = camera::start(&host.root, host.running);
        Ok(Started {
            routes: Some(running.routes),
            page: camera::BROWSER_PAGES,
            status: Some(running.status),
            frame_hook: Some(Box::new(running.overlay)),
        })
    }
}

/// `vrft_d train-tongue --request <file> --output <new folder> [--epochs N]
/// [--learning-rate R] [--layers all|head|output]`: one personal tongue
/// training run. The daemon starts this as a child
/// process and follows its `progress.json`.
fn train_tongue(arguments: &[String]) -> anyhow::Result<()> {
    let value = |flag: &str| {
        arguments
            .iter()
            .position(|argument| argument == flag)
            .and_then(|index| arguments.get(index + 1))
    };
    let request = value("--request").context("--request is required")?;
    let output = value("--output").context("--output is required")?;
    let mut options = vrft_tongue::train::Options::default();
    if let Some(epochs) = value("--epochs") {
        options.epochs = epochs.parse().context("--epochs must be a whole number")?;
    }
    if let Some(rate) = value("--learning-rate") {
        options.learning_rate = rate.parse().context("--learning-rate must be a number")?;
    }
    if let Some(layers) = value("--layers") {
        options.trainable = layers.parse().map_err(anyhow::Error::msg)?;
    }
    vrft_tongue::train::run(request.as_ref(), output.as_ref(), &options)
}
