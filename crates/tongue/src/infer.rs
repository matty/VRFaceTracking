//! Live tongue inference with the gate and direction pair.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use burn::tensor::backend::Backend;
use burn::tensor::{Tensor, TensorData};
use log::warn;

use crate::backend::{Accelerator, Cpu, Gpu, GPU_NAME};
use crate::checkpoint::{Checkpoint, Role};
use crate::model::TongueNet;
use crate::onnx::{self, live::LiveSession, live::Plan};
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
        let disabled = disabled_targets(direction);
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
        let direction = self.run(false, strip)?;
        combine(visibility, direction, self.disabled)
    }
}

/// Visibility from the gate, every other head from the direction model.
fn combine(
    visibility: f32,
    direction: Vec<f32>,
    disabled: [bool; TARGETS.len()],
) -> Result<[f32; TARGETS.len()]> {
    let mut values: [f32; TARGETS.len()] = direction
        .try_into()
        .map_err(|_| anyhow!("tongue model returned the wrong number of values"))?;
    values[0] = visibility;
    for (value, disabled) in values.iter_mut().zip(disabled) {
        if disabled {
            *value = 0.0;
        }
    }
    if values.iter().any(|value| !value.is_finite()) {
        bail!("tongue model returned non-finite values");
    }
    Ok(values)
}

/// The pair on ONNX Runtime.
struct OnnxPair {
    gate: LiveSession,
    direction: LiveSession,
    gate_resize: AreaResize,
    direction_resize: AreaResize,
    disabled: [bool; TARGETS.len()],
    buffer: Vec<f32>,
}

impl OnnxPair {
    fn new(
        gate: (&Checkpoint, &Path),
        direction: (&Checkpoint, &Path),
        accelerator: Accelerator,
    ) -> Result<Self> {
        let placement = onnx::runtime::placement(accelerator);
        let session = |(checkpoint, path): (&Checkpoint, &Path)| {
            let mut weights = checkpoint.weights.clone();
            weights.remove("signed_mask");
            weights.retain(|name, _| !name.ends_with("num_batches_tracked"));
            let graph = onnx::pair_graph(&weights, checkpoint.metadata.image_size)?;
            // Float: int8 moves the pair's directions by a few hundredths
            // (docs/internals/model-benchmark.md); VRFT_ONNX_INT8=all opts in.
            LiveSession::new(graph, path, &weights, placement, Plan::Float)
        };
        Ok(Self {
            gate: session(gate)?,
            direction: session(direction)?,
            gate_resize: AreaResize::new(gate.0.metadata.image_size),
            direction_resize: AreaResize::new(direction.0.metadata.image_size),
            disabled: disabled_targets(direction.0),
            buffer: vec![],
        })
    }

    fn run(&mut self, gate: bool, strip: &[u8]) -> Result<Vec<f32>> {
        let (session, resize) = if gate {
            (&mut self.gate, &self.gate_resize)
        } else {
            (&mut self.direction, &self.direction_resize)
        };
        resize.stereo(strip, &mut self.buffer);
        let size = resize.size();
        let mut values =
            session.run(&[("views", &[1, 2, size, size], &self.buffer)], &["values"])?;
        Ok(values.pop().unwrap_or_default())
    }

    fn predict(&mut self, strip: &[u8]) -> Result<[f32; TARGETS.len()]> {
        let visibility = self.run(true, strip)?[0];
        let direction = self.run(false, strip)?;
        combine(visibility, direction, self.disabled)
    }

    fn device(&self) -> String {
        self.direction.device()
    }

    fn settled(&self) -> bool {
        self.gate.settled() && self.direction.settled()
    }
}

fn disabled_targets(direction: &Checkpoint) -> [bool; TARGETS.len()] {
    TARGETS.map(|name| {
        direction
            .metadata
            .disabled_targets
            .iter()
            .any(|disabled| disabled == name)
    })
}

enum Engine {
    Gpu(Pair<Gpu>),
    Cpu(Pair<Cpu>),
    Onnx(Box<OnnxPair>),
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
        let (gate_path, direction_path) = (path(Role::Gate)?, path(Role::Direction)?);
        let gate = Checkpoint::load(&gate_path)?;
        let direction = Checkpoint::load(&direction_path)?;
        let blank = vec![0u8; FRAME_BYTES];
        let mut onnx = None;
        if onnx::runtime::wanted() {
            let started = guarded(|| {
                let mut pair = OnnxPair::new(
                    (&gate, &gate_path),
                    (&direction, &direction_path),
                    accelerator,
                )?;
                pair.predict(&blank)?;
                Ok(pair)
            });
            match started {
                Ok(pair) => onnx = Some(Engine::Onnx(Box::new(pair))),
                Err(error) => {
                    warn!("Tongue model: ONNX Runtime unavailable ({error:#}); using Burn")
                }
            }
        }
        let engine = match accelerator {
            _ if onnx.is_some() => onnx,
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
            device: match &engine {
                Engine::Gpu(_) => GPU_NAME.into(),
                Engine::Cpu(_) => "CPU".into(),
                Engine::Onnx(pair) => pair.device(),
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

    /// Whether the model has stopped changing how it runs (ONNX Runtime's
    /// calibration and switch to int8 are done).
    pub fn settled(&self) -> bool {
        match &self.engine {
            Engine::Onnx(pair) => pair.settled(),
            _ => true,
        }
    }

    /// Raw per-frame values in `TARGETS` order for one 800x400 gray8 stereo
    /// strip; smoothing is the caller's job.
    pub fn predict(&mut self, strip: &[u8]) -> Result<[f32; TARGETS.len()]> {
        if strip.len() != FRAME_BYTES {
            bail!("tongue frames must be 800x400 gray8");
        }
        let values = guarded(|| match &mut self.engine {
            Engine::Gpu(pair) => pair.predict(strip),
            Engine::Cpu(pair) => pair.predict(strip),
            Engine::Onnx(pair) => pair.predict(strip),
        });
        if let Engine::Onnx(pair) = &self.engine {
            self.info.device = pair.device();
        }
        values
    }
}
