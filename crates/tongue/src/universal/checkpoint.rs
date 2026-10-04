//! Universal face checkpoints: safetensors, like personal tongue models, with
//! the same `vrft` metadata entry describing the model.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::{ANCHOR_SLOTS, ARCHITECTURE, FACE_TARGETS};
use crate::checkpoint::{read_safetensors, write_safetensors, VisibilityGate};
use crate::model::Weights;

/// Everything a universal checkpoint says besides its weights.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceMetadata {
    pub architecture: String,
    pub image_size: usize,
    /// The outputs, in prediction order; always [`FACE_TARGETS`].
    pub outputs: Vec<String>,
    /// The enrollment slots the mouth head reads; always [`ANCHOR_SLOTS`].
    pub anchor_slots: Vec<String>,
    #[serde(default)]
    pub visibility_gate: VisibilityGate,
    /// Outputs training had too few labels for; inference sends 0 for them
    /// and VRFT leaves the tracking module's values alone.
    #[serde(default)]
    pub disabled_targets: Vec<String>,
    /// How it was trained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub training: Option<serde_json::Value>,
}

impl FaceMetadata {
    pub fn new(image_size: usize) -> Self {
        Self {
            architecture: ARCHITECTURE.into(),
            image_size,
            outputs: FACE_TARGETS.iter().map(|name| name.to_string()).collect(),
            anchor_slots: ANCHOR_SLOTS.iter().map(|name| name.to_string()).collect(),
            visibility_gate: VisibilityGate::default(),
            disabled_targets: vec![],
            training: None,
        }
    }

    /// Whether `name` was trained.
    pub fn enabled(&self, name: &str) -> bool {
        !self
            .disabled_targets
            .iter()
            .any(|disabled| disabled == name)
    }
}

pub struct FaceCheckpoint {
    pub metadata: FaceMetadata,
    pub weights: Weights,
}

impl FaceCheckpoint {
    pub fn load(path: &Path) -> Result<Self> {
        let (metadata, weights) = read_safetensors(path)
            .with_context(|| format!("could not read face model {}", path.display()))?;
        let metadata: FaceMetadata = serde_json::from_str(&metadata)
            .with_context(|| format!("{} is not a universal face model", path.display()))?;
        let checkpoint = Self { metadata, weights };
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    fn validate(&self) -> Result<()> {
        let metadata = &self.metadata;
        if metadata.architecture != ARCHITECTURE {
            bail!(
                "unsupported face model architecture {}",
                metadata.architecture
            );
        }
        if !(32..=400).contains(&metadata.image_size) {
            bail!("unsupported face model image size {}", metadata.image_size);
        }
        if !metadata.outputs.iter().map(String::as_str).eq(FACE_TARGETS)
            || !metadata
                .anchor_slots
                .iter()
                .map(String::as_str)
                .eq(ANCHOR_SLOTS)
        {
            bail!("unsupported face model outputs");
        }
        let gate = &metadata.visibility_gate;
        if !(0.0..=1.0).contains(&gate.camera_weight) || !(0.0..=1.0).contains(&gate.threshold) {
            bail!("face model visibility gate is out of range");
        }
        if let Some(unknown) = metadata
            .disabled_targets
            .iter()
            .find(|name| !FACE_TARGETS.contains(&name.as_str()) || *name == "visibility")
        {
            bail!("face model can't disable {unknown}");
        }
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        write_safetensors(path, &self.weights, &serde_json::to_string(&self.metadata)?)
    }
}
