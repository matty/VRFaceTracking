//! Guided tongue recordings, as the daemon saves them under
//! `.local/tongue-captures/<recording>/`. Recordings from before the cheek
//! puff heads carry tongue labels alone, so their cheeks count as unlabelled.
//! Synthetic sets use the same format, and the one the app downloads is
//! stored already shrunk to the model's input size.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::preprocess::VIEW;
use crate::{TARGETS, TONGUE_TARGETS};

/// Heads that can never be negative.
const UNSIGNED_COLUMNS: [usize; 9] = [0, 1, 4, 5, 6, 7, 8, 10, 11];

#[derive(Clone, Debug)]
pub struct Sample {
    /// Frame index in `frames.gray8`.
    pub index: usize,
    /// The recording step (pose) this frame belongs to.
    pub step: u64,
    pub pose: String,
    /// Cheek puffs are 0 when not `cheeks_labelled`.
    pub targets: [f32; TARGETS.len()],
    /// Whether the cheek puffs were labelled, which they were in every
    /// recording made since they were added.
    pub cheeks_labelled: bool,
    /// The tracking module's TongueOut at this frame, if one was running.
    pub native: Option<f32>,
    /// A follow-the-dot frame, whose label moves with the dot.
    pub moving: bool,
}

#[derive(Deserialize)]
struct Metadata {
    format: Option<String>,
    mode: Option<String>,
    width: Option<usize>,
    height: Option<usize>,
    #[serde(rename = "bytesPerFrame")]
    bytes_per_frame: Option<usize>,
    targets: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Line {
    index: Option<usize>,
    step: u64,
    pose: String,
    targets: Vec<f32>,
    native_tongue_out: Option<f32>,
    dot: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Excluded {
    excluded_steps: Vec<u64>,
}

pub struct Recording {
    pub dir: PathBuf,
    /// Frames usable for training: not in a skipped or unticked pose.
    pub samples: Vec<Sample>,
    /// Each camera view's width and height: 400 as the headset sends them,
    /// less in a synthetic set stored at the model's input size.
    pub view: usize,
    /// Rendered rather than recorded.
    pub synthetic: bool,
}

impl Recording {
    pub fn open(dir: &Path) -> Result<Self> {
        let metadata: Metadata = serde_json::from_slice(
            &std::fs::read(dir.join("metadata.json"))
                .with_context(|| format!("{} is not a recording", dir.display()))?,
        )?;
        let names = metadata.targets.unwrap_or_default();
        let names = names.iter().map(String::as_str);
        let cheeks_labelled = names.clone().eq(TARGETS);
        let view = metadata.height.unwrap_or(VIEW);
        let frame_bytes = 2 * view * view;
        if metadata.format.as_deref() != Some("vrft-tongue-capture-v1")
            || !(32..=VIEW).contains(&view)
            || metadata.width.is_some_and(|width| width != 2 * view)
            || metadata.bytes_per_frame != Some(frame_bytes)
            || !(cheeks_labelled || names.eq(TARGETS[..TONGUE_TARGETS].iter().copied()))
        {
            bail!("Unsupported capture format: {}", dir.display());
        }
        let labels = if cheeks_labelled {
            TARGETS.len()
        } else {
            TONGUE_TARGETS
        };
        let mut lines = vec![];
        for line in BufReader::new(File::open(dir.join("samples.jsonl"))?).lines() {
            let line = line?;
            if !line.trim().is_empty() {
                lines.push(serde_json::from_str::<Line>(&line)?);
            }
        }
        let frames = std::fs::metadata(dir.join("frames.gray8"))?.len();
        if frames != (lines.len() * frame_bytes) as u64 {
            bail!("Frame and label counts differ in {}", dir.display());
        }
        let mut excluded = HashSet::new();
        for file in ["excluded_steps.json", "review.json"] {
            let path = dir.join(file);
            if path.exists() {
                let list: Excluded = serde_json::from_slice(&std::fs::read(path)?)?;
                excluded.extend(list.excluded_steps);
            }
        }
        let mut samples = vec![];
        for (index, line) in lines.into_iter().enumerate() {
            let invalid = || format!("Invalid sample {index} in {}", dir.display());
            if line.index != Some(index) {
                bail!(invalid());
            }
            if line.targets.len() != labels {
                bail!(invalid());
            }
            let mut targets = [0.0; TARGETS.len()];
            targets[..labels].copy_from_slice(&line.targets);
            if targets
                .iter()
                .any(|value| !value.is_finite() || value.abs() > 1.0)
            {
                bail!("Invalid targets for sample {index} in {}", dir.display());
            }
            if UNSIGNED_COLUMNS.iter().any(|&column| targets[column] < 0.0) {
                bail!(
                    "Negative unsigned target for sample {index} in {}",
                    dir.display()
                );
            }
            if let Some(native) = line.native_tongue_out {
                if !native.is_finite() || !(0.0..=1.0).contains(&native) {
                    bail!(
                        "Invalid native TongueOut for sample {index} in {}",
                        dir.display()
                    );
                }
            }
            if excluded.contains(&line.step) {
                continue;
            }
            samples.push(Sample {
                index,
                step: line.step,
                pose: line.pose,
                targets,
                cheeks_labelled,
                native: line.native_tongue_out,
                moving: line.dot.is_some_and(|dot| !dot.is_null()),
            });
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            samples,
            view,
            synthetic: metadata.mode.as_deref() == Some("synthetic"),
        })
    }

    pub fn name(&self) -> String {
        self.dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Reads the frames at `indices` (ascending) as strips of the two views
    /// side by side: 800x400 as the headset sends them.
    pub fn read_frames(&self, indices: &[usize], mut each: impl FnMut(&[u8])) -> Result<()> {
        let frame_bytes = 2 * self.view * self.view;
        let mut file = File::open(self.dir.join("frames.gray8"))?;
        let mut strip = vec![0u8; frame_bytes];
        for &index in indices {
            file.seek(SeekFrom::Start((index * frame_bytes) as u64))?;
            file.read_exact(&mut strip)
                .with_context(|| format!("Truncated frame in {}", self.dir.display()))?;
            each(&strip);
        }
        Ok(())
    }
}
