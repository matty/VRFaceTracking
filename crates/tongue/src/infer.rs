//! Live tongue inference with the gate and direction pair.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use burn::tensor::backend::Backend;
use burn::tensor::{Tensor, TensorData};
use log::warn;

use crate::backend::{Accelerator, Cpu, Gpu};
use crate::checkpoint::{Checkpoint, Role};
use crate::model::TongueNet;
use crate::preprocess::{AreaResize, FRAME_BYTES};
use crate::TARGETS;

/// What the loaded pair reports, for the daemon's log and status.
#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub camera_weight: f32,
    pub threshold: f32,
    /// Where inference runs, such as "GPU (wgpu)" or "CPU".
    pub device: String,
    pub gate_size: usize,
    pub direction_size: usize,
    pub disabled_targets: Vec<String>,
}

struct Pair<B: Backend> {
    gate: TongueNet<B>,
    direction: TongueNet<B>,
    gate_resize: AreaResize,
    direction_resize: AreaResize,
    disabled: [bool; TARGETS.len()],
    device: B::Device,
    buffer: Vec<f32>,
}

impl<B: Backend> Pair<B> {
    fn new(gate: &Checkpoint, direction: &Checkpoint, device: B::Device) -> Result<Self> {
        let disabled = TARGETS.map(|name| {
            direction
                .metadata
                .disabled_targets
                .iter()
                .any(|disabled| disabled == name)
        });
        Ok(Self {
            gate: TongueNet::from_weights(gate.weights.clone(), &device)?.fold(),
            direction: TongueNet::from_weights(direction.weights.clone(), &device)?.fold(),
            gate_resize: AreaResize::new(gate.metadata.image_size),
            direction_resize: AreaResize::new(direction.metadata.image_size),
            disabled,
            device,
            buffer: vec![],
        })
    }

    fn run(&mut self, gate: bool, strip: &[u8]) -> Result<Vec<f32>> {
        let (net, resize) = if gate {
            (&self.gate, &self.gate_resize)
        } else {
            (&self.direction, &self.direction_resize)
        };
        resize.stereo(strip, &mut self.buffer);
        let size = resize.size();
        let input = Tensor::<B, 4>::from_data(
            TensorData::new(self.buffer.clone(), [1, 2, size, size]),
            &self.device,
        );
        net.forward(input)
            .into_data()
            .to_vec::<f32>()
            .map_err(|error| anyhow!("{error:?}"))
    }

    fn predict(&mut self, strip: &[u8]) -> Result<[f32; TARGETS.len()]> {
        let visibility = self.run(true, strip)?[0];
        let mut values: [f32; TARGETS.len()] = self
            .run(false, strip)?
            .try_into()
            .map_err(|_| anyhow!("tongue model returned the wrong number of values"))?;
        // Visibility comes from the gate, every other head from the direction model.
        values[0] = visibility;
        for (value, disabled) in values.iter_mut().zip(self.disabled) {
            if disabled {
                *value = 0.0;
            }
        }
        if values.iter().any(|value| !value.is_finite()) {
            bail!("tongue model returned non-finite values");
        }
        Ok(values)
    }
}

enum Engine {
    Gpu(Pair<Gpu>),
    Cpu(Pair<Cpu>),
}

pub struct TongueModel {
    engine: Engine,
    info: ModelInfo,
}

/// Runs `work`, turning a panic (as from a GPU driver) into an error.
pub(crate) fn guarded<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "unknown error".into());
        Err(anyhow!("{message}"))
    })
}

impl TongueModel {
    /// Loads the pair in `dir` and proves it runs on the chosen device.
    /// `Auto` uses the GPU when one works and falls back to the CPU.
    pub fn load(dir: &Path, accelerator: Accelerator) -> Result<Self> {
        let path = |role: Role| {
            role.find(dir)
                .ok_or_else(|| anyhow!("incomplete tongue model pair in {}", dir.display()))
        };
        let gate = Checkpoint::load(&path(Role::Gate)?)?;
        let direction = Checkpoint::load(&path(Role::Direction)?)?;
        let blank = vec![0u8; FRAME_BYTES];
        let engine = match accelerator {
            Accelerator::Cpu => None,
            Accelerator::Auto | Accelerator::Gpu => {
                let started = guarded(|| {
                    let mut pair = Pair::<Gpu>::new(&gate, &direction, Default::default())?;
                    pair.predict(&blank)?;
                    Ok(pair)
                });
                match started {
                    Ok(pair) => Some(Engine::Gpu(pair)),
                    Err(error) if accelerator == Accelerator::Auto => {
                        warn!("Tongue model: GPU unavailable ({error:#}); using the CPU");
                        None
                    }
                    Err(error) => {
                        return Err(error.context("the GPU could not run the tongue model"))
                    }
                }
            }
        };
        let engine = match engine {
            Some(engine) => engine,
            None => Engine::Cpu(Pair::new(&gate, &direction, Default::default())?),
        };
        let info = ModelInfo {
            camera_weight: gate.metadata.visibility_gate.camera_weight as f32,
            threshold: gate.metadata.visibility_gate.threshold as f32,
            device: match engine {
                Engine::Gpu(_) => "GPU (wgpu)".into(),
                Engine::Cpu(_) => "CPU".into(),
            },
            gate_size: gate.metadata.image_size,
            direction_size: direction.metadata.image_size,
            disabled_targets: direction.metadata.disabled_targets.clone(),
        };
        Ok(Self { engine, info })
    }

    pub fn info(&self) -> &ModelInfo {
        &self.info
    }

    /// Raw per-frame values in `TARGETS` order for one 800x400 gray8 stereo
    /// strip; smoothing is the caller's job.
    pub fn predict(&mut self, strip: &[u8]) -> Result<[f32; TARGETS.len()]> {
        if strip.len() != FRAME_BYTES {
            bail!("tongue frames must be 800x400 gray8");
        }
        guarded(|| match &mut self.engine {
            Engine::Gpu(pair) => pair.predict(strip),
            Engine::Cpu(pair) => pair.predict(strip),
        })
    }
}
