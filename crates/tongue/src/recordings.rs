//! Guided recordings, as the daemon saves them under
//! `.local/tongue-captures/<recording>/`. Recordings from before the cheek
//! puff heads carry tongue labels alone, so their cheeks count as unlabelled.
//! Rendered sets from earlier versions use the same format, stored already
//! shrunk to a model's input size.
//!
//! A frame holds the views of the cameras `metadata.json` lists in `cameras`,
//! side by side: the mouth pair (2 and 3) in recordings from before the
//! five-camera stream, all five since. The tongue pair reads the mouth pair
//! of either.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::preprocess::VIEW;
use crate::{TARGETS, TONGUE_TARGETS};
use vrft_quest_pro_protocol::{CameraLayout, MOUTH_CAMERAS};

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
    /// A follow-the-dot frame, whose label moves with the dot.
    pub moving: bool,
    /// Labels for the universal face model's outputs beyond the tongue, by
    /// name (see `universal::FACE_TARGETS`): each one given is labelled.
    pub face: BTreeMap<String, f32>,
    /// The enrollment slot this frame shows, such as `neutral`, when it was
    /// recorded as one (the one-minute face setup, or a rendered set's
    /// enrollment poses).
    pub anchor: Option<String>,
    /// Whose face this is, in a rendered set of many; recordings are one
    /// person's.
    pub identity: Option<String>,
}

#[derive(Deserialize)]
struct Metadata {
    format: Option<String>,
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
    dot: Option<serde_json::Value>,
    #[serde(default)]
    face: BTreeMap<String, f32>,
    anchor: Option<String>,
    identity: Option<String>,
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
    /// less in a rendered set stored at the model's input size.
    pub view: usize,
    /// The cameras each frame holds.
    pub layout: CameraLayout,
}

impl Recording {
    pub fn open(dir: &Path) -> Result<Self> {
        let raw: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("metadata.json"))
                .with_context(|| format!("{} is not a recording", dir.display()))?,
        )?;
        let metadata: Metadata = serde_json::from_value(raw.clone())?;
        let names = metadata.targets.unwrap_or_default();
        let names = names.iter().map(String::as_str);
        let cheeks_labelled = names.clone().eq(TARGETS);
        let layout = CameraLayout::from_metadata(&raw)
            .map_err(|error| anyhow::anyhow!("{error}: {}", dir.display()))?;
        let view = layout.view;
        let frame_bytes = layout.frame_bytes();
        if metadata.format.as_deref() != Some("vrft-tongue-capture-v1")
            || !(32..=VIEW).contains(&view)
            || !MOUTH_CAMERAS.iter().all(|&camera| layout.has(camera))
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
            if line
                .face
                .values()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
            {
                bail!(
                    "Invalid face labels for sample {index} in {}",
                    dir.display()
                );
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
                moving: line.dot.is_some_and(|dot| !dot.is_null()),
                face: line.face,
                anchor: line.anchor,
                identity: line.identity,
            });
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            samples,
            view,
            layout,
        })
    }

    pub fn name(&self) -> String {
        self.dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Reads the frames at `indices` (ascending) as strips of the two mouth
    /// views side by side: 800x400 as the headset sends them.
    pub fn read_frames(&self, indices: &[usize], mut each: impl FnMut(&[u8])) -> Result<()> {
        let mouth = self.layout.cameras == MOUTH_CAMERAS;
        self.read_whole(indices, |frame| {
            if mouth {
                each(frame);
            } else {
                let pair = self
                    .layout
                    .select(frame, &MOUTH_CAMERAS)
                    .expect("opened recordings hold the mouth pair");
                each(&pair);
            }
        })
    }

    /// Reads the frames at `indices` (ascending) whole, with every camera
    /// in [`layout`](Self::layout).
    pub fn read_whole(&self, indices: &[usize], mut each: impl FnMut(&[u8])) -> Result<()> {
        let frame_bytes = self.layout.frame_bytes();
        let mut file = File::open(self.dir.join("frames.gray8"))?;
        let mut frame = vec![0u8; frame_bytes];
        for &index in indices {
            file.seek(SeekFrom::Start((index * frame_bytes) as u64))?;
            file.read_exact(&mut frame)
                .with_context(|| format!("Truncated frame in {}", self.dir.display()))?;
            each(&frame);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrft_quest_pro_protocol::STRIP_BYTES;

    fn write(dir: &Path, metadata: serde_json::Value, frames: &[u8]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("metadata.json"), metadata.to_string()).unwrap();
        let line = serde_json::json!({"index": 0, "step": 0, "pose": "Neutral",
            "targets": vec![0.0; TARGETS.len()]});
        std::fs::write(dir.join("samples.jsonl"), format!("{line}\n")).unwrap();
        std::fs::write(dir.join("frames.gray8"), frames).unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "vrft-recordings-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn five_camera_recordings_read_as_their_mouth_pair() {
        let dir = temp("five");
        let strip: Vec<u8> = (0..STRIP_BYTES).map(|i| ((i % 2000) / 400) as u8).collect();
        let mut metadata = CameraLayout::all().metadata();
        metadata["format"] = "vrft-tongue-capture-v1".into();
        metadata["targets"] = serde_json::json!(TARGETS);
        write(&dir, metadata, &strip);
        let recording = Recording::open(&dir).unwrap();
        assert!(recording.layout.is_all());
        let mut pairs = vec![];
        recording
            .read_frames(&[0], |pair| pairs.push(pair.to_vec()))
            .unwrap();
        assert_eq!(pairs[0].len(), crate::preprocess::FRAME_BYTES);
        assert_eq!((pairs[0][0], pairs[0][400]), (2, 3));
        recording
            .read_whole(&[0], |frame| assert_eq!(frame, &strip[..]))
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recordings_without_the_mouth_pair_are_refused() {
        let dir = temp("eyes");
        let layout = CameraLayout {
            cameras: vec![0, 1],
            view: 400,
        };
        let mut metadata = layout.metadata();
        metadata["format"] = "vrft-tongue-capture-v1".into();
        metadata["targets"] = serde_json::json!(TARGETS);
        write(&dir, metadata, &vec![0; layout.frame_bytes()]);
        assert!(Recording::open(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
