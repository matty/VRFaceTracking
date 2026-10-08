//! Personal training on the recordings the user ticks: fine-tuning a
//! `universal-face-v2` model such as the QFTPlus Model
//! (`universal_v2::train`). Models trained by earlier versions, tongue pairs
//! and `universal-face-v1`, still load, but are no longer trained.
//!
//! The daemon runs this in a child process (`vrft_d train-tongue`) and
//! reads `progress.json`; cancelling kills the process.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::backend::Accelerator;
use crate::recordings::Recording;
use vrft_quest_pro_protocol::{TrainerArchitecture, TrainingProgress, TrainingStage};

/// What to train, from the new model folder's `request.json`.
pub use vrft_quest_pro_protocol::TrainerRequest as Request;

pub struct Options {
    pub epochs: usize,
    /// `universal_v2::train::LEARNING_RATE` when unset.
    pub learning_rate: Option<f64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            epochs: 12,
            learning_rate: None,
        }
    }
}

/// Writes JSON via a temporary file. Windows refuses the rename while
/// another process has either file open (VRFT's status poll reads
/// progress.json every second, and antivirus may be scanning the new file);
/// those holds last milliseconds, so retry briefly.
pub(crate) fn write_json(path: &Path, value: &Value) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    for attempt in 0.. {
        match std::fs::rename(&temporary, path) {
            Ok(()) => return Ok(()),
            Err(error) if attempt < 40 && error.kind() == std::io::ErrorKind::PermissionDenied => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!()
}

pub(crate) struct Progress {
    pub(crate) output: PathBuf,
}

impl Progress {
    pub(crate) fn report(&self, progress: TrainingProgress) {
        let value = serde_json::to_value(&progress).expect("progress serializes");
        // progress.json only feeds the app; never lose a run over it.
        if let Err(error) = write_json(&self.output.join("progress.json"), &value) {
            println!("Could not update progress.json: {error}");
        }
        println!("{value}");
    }
}

/// Seconds left at `done`, from the pace since the run was `from` done,
/// `elapsed` seconds ago. `None` until a few seconds and some progress say
/// what the pace is.
pub(crate) fn estimate(elapsed: f64, from: f64, done: f64) -> Option<f64> {
    let progress = done - from;
    (elapsed >= 10.0 && progress > 0.0).then(|| (elapsed * (1.0 - done) / progress).round())
}

/// Runs a training request, writing the new model, `report.json` and
/// `progress.json` into `output`, which must be a new folder.
pub fn run(request: &Path, output: &Path, options: &Options) -> Result<()> {
    if options.epochs == 0 || options.learning_rate.is_some_and(|rate| rate <= 0.0) {
        bail!("Training settings must be positive");
    }
    std::fs::create_dir_all(output)?;
    // absolute() rather than canonicalize(), which adds \\?\ on Windows.
    let output = std::path::absolute(output)?;
    let existing = [
        "report.json",
        "progress.json",
        crate::universal_v2::FILE_NAME,
    ]
    .iter()
    .any(|file| output.join(file).exists());
    if existing {
        bail!("Use a new output directory; existing model runs are never overwritten");
    }
    let progress = Progress {
        output: output.clone(),
    };
    let result = (|| {
        let request: Request =
            serde_json::from_slice(&std::fs::read(request)?).context("invalid training request")?;
        if request.architecture != TrainerArchitecture::UniversalFaceV2 {
            bail!("Only universal-face-v2 models, such as the QFTPlus Model, are trained");
        }
        let accelerator: Accelerator = request
            .device
            .name()
            .parse()
            .map_err(|error: String| anyhow!(error))?;
        progress.report(TrainingProgress {
            fraction: Some(0.0),
            ..TrainingProgress::new(TrainingStage::Checking, "Checking your recordings")
        });
        let mut paths = vec![];
        for path in &request.recordings {
            let path = std::path::absolute(path)?;
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            bail!("Choose at least one recording to train on");
        }
        let recordings = paths
            .iter()
            .map(|path| Recording::open(path))
            .collect::<Result<Vec<_>>>()?;
        crate::universal_v2::train::run(
            &request,
            &recordings,
            &output,
            options,
            &progress,
            accelerator,
        )
    })();
    if let Err(error) = &result {
        progress.report(TrainingProgress::new(
            TrainingStage::Failed,
            format!("{error:#}"),
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_estimate_leaves_out_the_first_batch() {
        // The first batch took minutes and ended 1% in; 20 s later the run
        // is 5% in, so 4% took 20 s and the remaining 95% takes 475 s.
        assert_eq!(estimate(20.0, 0.01, 0.05), Some(475.0));
        // Too soon, or no batch finished since, to know the pace.
        assert_eq!(estimate(5.0, 0.01, 0.05), None);
        assert_eq!(estimate(20.0, 0.01, 0.01), None);
    }

    #[test]
    fn only_qft_format_models_train() {
        let dir = std::env::temp_dir().join(format!("vrft-train-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let request = dir.join("request.json");
        std::fs::write(
            &request,
            r#"{"base_model_dir": ".", "recordings": ["."], "architecture": "universal-face-v1"}"#,
        )
        .unwrap();
        let error = run(&request, &dir.join("run"), &Options::default()).unwrap_err();
        assert!(error.to_string().contains("universal-face-v2"), "{error}");
        let progress: TrainingProgress =
            serde_json::from_slice(&std::fs::read(dir.join("run/progress.json")).unwrap()).unwrap();
        assert_eq!(progress.stage, TrainingStage::Failed);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
