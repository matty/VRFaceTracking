//! Tongue checkpoints: the mouth-camera pair, PyTorch `.pt` files from
//! Qpro-Enhanced-FT read without Python, and personal models saved as
//! safetensors with the same PyTorch weight names and a JSON metadata entry.
//!
//! Checkpoints from before the cheek puff heads (the mouth-camera pair, and
//! personal models trained then) load with those heads added and disabled.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use burn::tensor::TensorData;
use burn_store::pytorch::reader::PickleValue;
use burn_store::pytorch::PytorchReader;
use safetensors::tensor::{Dtype, SafeTensors, TensorView};
use serde::{Deserialize, Serialize};

use crate::model::{add_missing_heads, Weights, ARCHITECTURE};
use crate::{CHEEK_COLUMNS, TARGETS, TONGUE_TARGETS};

/// Which of the pair a checkpoint is: the gate supplies visibility, the
/// direction model every other head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Gate,
    Direction,
}

impl Role {
    pub fn stem(self) -> &'static str {
        match self {
            Role::Gate => "qpro-stereo-tongue-v8-gate",
            Role::Direction => "qpro-stereo-tongue-v8-direction",
        }
    }

    /// The checkpoint for this role in `dir`: a personal safetensors file,
    /// else the PyTorch `.pt`.
    pub fn find(self, dir: &Path) -> Option<PathBuf> {
        ["safetensors", "pt"]
            .iter()
            .map(|extension| dir.join(format!("{}.{extension}", self.stem())))
            .find(|path| path.is_file())
    }

    pub fn safetensors(self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.safetensors", self.stem()))
    }
}

/// How camera visibility and the headset's own TongueOut are blended, and
/// the threshold above which the tongue counts as out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisibilityGate {
    pub camera_weight: f64,
    pub threshold: f64,
    /// Anything else the trainer recorded about the choice.
    #[serde(flatten)]
    pub details: serde_json::Map<String, serde_json::Value>,
}

impl Default for VisibilityGate {
    fn default() -> Self {
        // The reference defaults for checkpoints that carry no gate.
        Self {
            camera_weight: 0.95,
            threshold: 0.85,
            details: Default::default(),
        }
    }
}

/// The lowest TongueOut a visible tongue is sent with when TongueOut
/// follows the extension output.
pub const TONGUE_OUT_FLOOR: f32 = 0.1;

/// How TongueOut follows the direction model's extension output, once
/// training found that it separates the recorded amounts of tongue out.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TongueOutMap {
    pub scale: f64,
    pub offset: f64,
}

impl TongueOutMap {
    /// TongueOut for a visible tongue with this extension output.
    pub fn tongue_out(&self, extension: f32) -> f32 {
        (self.scale as f32 * extension.clamp(0.0, 1.0) + self.offset as f32)
            .clamp(TONGUE_OUT_FLOOR, 1.0)
    }
}

/// Everything a checkpoint says besides its weights.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub architecture: String,
    pub image_size: usize,
    #[serde(default)]
    pub visibility_gate: VisibilityGate,
    /// Heads personal training could not train; inference sends 0 for them.
    #[serde(default)]
    pub disabled_targets: Vec<String>,
    /// How a personal model was trained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal_training: Option<serde_json::Value>,
    /// On a direction model: how TongueOut follows extension. Without it,
    /// TongueOut is the larger of the tongue-out confidence and extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tongue_out: Option<TongueOutMap>,
}

pub struct Checkpoint {
    pub metadata: Metadata,
    pub weights: Weights,
}

const METADATA_KEY: &str = "vrft";

/// A pickled checkpoint value as JSON.
fn json(value: PickleValue) -> serde_json::Value {
    use serde_json::Value;
    match value {
        PickleValue::None => Value::Null,
        PickleValue::Bool(value) => Value::Bool(value),
        PickleValue::Int(value) => value.into(),
        PickleValue::Float(value) => value.into(),
        PickleValue::String(value) => Value::String(value),
        PickleValue::List(values) => Value::Array(values.into_iter().map(json).collect()),
        PickleValue::Dict(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key, json(value)))
                .collect(),
        ),
        PickleValue::Bytes(_) => Value::Null,
    }
}

impl Checkpoint {
    pub fn load(path: &Path) -> Result<Self> {
        let checkpoint = match path.extension().and_then(|e| e.to_str()) {
            Some("safetensors") => Self::load_safetensors(path),
            _ => Self::load_pytorch(path),
        }
        .and_then(|mut checkpoint| {
            if add_missing_heads(&mut checkpoint.weights)? {
                checkpoint.disable_cheeks();
            }
            Ok(checkpoint)
        })
        .with_context(|| format!("could not read tongue model {}", path.display()))?;
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    /// Marks the cheek heads untrained, as for a model from before them.
    fn disable_cheeks(&mut self) {
        for column in CHEEK_COLUMNS {
            let name = TARGETS[column].to_string();
            if !self.metadata.disabled_targets.contains(&name) {
                self.metadata.disabled_targets.push(name);
            }
        }
    }

    fn validate(&self) -> Result<()> {
        let metadata = &self.metadata;
        if metadata.architecture != ARCHITECTURE {
            bail!(
                "unsupported tongue model architecture {}",
                metadata.architecture
            );
        }
        if !(32..=512).contains(&metadata.image_size) {
            bail!(
                "unsupported tongue model image size {}",
                metadata.image_size
            );
        }
        let gate = &metadata.visibility_gate;
        if !(0.0..=1.0).contains(&gate.camera_weight) || !(0.0..=1.0).contains(&gate.threshold) {
            bail!("tongue model visibility gate is out of range");
        }
        if metadata.tongue_out.is_some_and(|map| {
            !map.offset.is_finite() || !(map.scale.is_finite() && map.scale > 0.0)
        }) {
            bail!("tongue model TongueOut map is invalid");
        }
        if let Some(unknown) = metadata
            .disabled_targets
            .iter()
            .find(|name| !TARGETS.contains(&name.as_str()))
        {
            bail!("unknown disabled tongue target {unknown}");
        }
        if metadata
            .disabled_targets
            .iter()
            .any(|name| name == "visibility")
        {
            bail!("a tongue checkpoint cannot disable visibility");
        }
        Ok(())
    }

    fn load_pytorch(path: &Path) -> Result<Self> {
        fn field<T: serde::de::DeserializeOwned>(path: &Path, key: &str) -> Option<T> {
            let value = PytorchReader::read_pickle_data(path, Some(key)).ok()?;
            serde_json::from_value(json(value)).ok()
        }
        let targets: Vec<String> =
            field(path, "targetNames").context("checkpoint has no targetNames")?;
        if targets != TARGETS && targets != TARGETS[..TONGUE_TARGETS] {
            bail!("unsupported tongue model target schema");
        }
        // disabledTargets is written by personal training; older personal
        // checkpoints only carry the supportedTargets mask, and the
        // mouth-camera pair carries neither.
        let disabled_targets = match field::<Vec<String>>(path, "disabledTargets") {
            Some(names) => names,
            None => match field::<Vec<bool>>(path, "supportedTargets") {
                Some(mask) if mask.len() == targets.len() => TARGETS
                    .iter()
                    .zip(mask)
                    .filter(|(_, yes)| !yes)
                    .map(|(name, _)| name.to_string())
                    .collect(),
                Some(_) => bail!("invalid supported target mask"),
                None => vec![],
            },
        };
        let metadata = Metadata {
            architecture: field(path, "architecture").context("checkpoint has no architecture")?,
            image_size: field(path, "imageSize").context("checkpoint has no imageSize")?,
            visibility_gate: field(path, "visibilityGate").unwrap_or_default(),
            disabled_targets,
            personal_training: field(path, "personalTraining"),
            tongue_out: None,
        };
        let weights = PytorchReader::with_top_level_key(path, "modelState")?
            .into_tensors()
            .into_iter()
            .map(|(name, snapshot)| Ok((name, snapshot.to_data()?)))
            .collect::<Result<Weights>>()?;
        Ok(Self { metadata, weights })
    }

    fn load_safetensors(path: &Path) -> Result<Self> {
        let (metadata, weights) = read_safetensors(path)?;
        let metadata: Metadata = serde_json::from_str(&metadata)?;
        Ok(Self { metadata, weights })
    }

    /// Writes a safetensors checkpoint, via a temporary file so a reader
    /// never sees a partial model.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        write_safetensors(path, &self.weights, &serde_json::to_string(&self.metadata)?)
    }
}

/// A VRFT safetensors checkpoint's metadata JSON and float32 weights.
pub(crate) fn read_safetensors(path: &Path) -> Result<(String, Weights)> {
    let bytes = std::fs::read(path)?;
    let (_, header) = SafeTensors::read_metadata(&bytes)?;
    let metadata = header
        .metadata()
        .as_ref()
        .and_then(|entries| entries.get(METADATA_KEY))
        .context("not a VRFT tongue model")?
        .clone();
    let tensors = SafeTensors::deserialize(&bytes)?;
    let mut weights = Weights::new();
    for (name, view) in tensors.tensors() {
        if view.dtype() != Dtype::F32 {
            bail!("{name} is not float32");
        }
        let values: Vec<f32> = view
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect();
        weights.insert(name, TensorData::new(values, view.shape().to_vec()));
    }
    Ok((metadata, weights))
}

/// Writes float32 `weights` and a metadata JSON as a safetensors file, via a
/// temporary file so a reader never sees a partial model.
pub(crate) fn write_safetensors(path: &Path, weights: &Weights, metadata: &str) -> Result<()> {
    let mut names: Vec<&String> = weights.keys().collect();
    names.sort();
    let bytes: Vec<(String, Vec<u8>, Vec<usize>)> = names
        .into_iter()
        .map(|name| {
            let data = &weights[name];
            let values = data
                .as_slice::<f32>()
                .map_err(|error| anyhow::anyhow!("{name}: {error:?}"))?;
            let raw = values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect();
            Ok((name.clone(), raw, data.shape.as_slice().to_vec()))
        })
        .collect::<Result<_>>()?;
    let views = bytes
        .iter()
        .map(|(name, raw, shape)| {
            Ok((
                name.as_str(),
                TensorView::new(Dtype::F32, shape.clone(), raw)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let info = HashMap::from([(METADATA_KEY.to_string(), metadata.to_string())]);
    let serialized = safetensors::serialize(views, Some(info))?;
    let temporary = path.with_extension("safetensors.tmp");
    std::fs::write(&temporary, serialized)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}
