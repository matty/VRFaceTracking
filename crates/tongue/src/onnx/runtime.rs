//! ONNX Runtime, loaded at run time from its library beside the program, so
//! VRFT starts without it and falls back to Burn.
//!
//! The library is found, in order, at `VRFT_ONNXRUNTIME`, beside the running
//! program, or in `.local/onnxruntime/` under the working directory (a
//! development checkout). `VRFT_ONNX_THREADS` sets the CPU threads a model
//! uses (default [`default_threads`]).

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Result};
use log::{info, warn};
use ort::session::builder::GraphOptimizationLevel;
use ort::value::TensorRef;

use crate::backend::Accelerator;

#[cfg(windows)]
const LIBRARY: &str = "onnxruntime.dll";
#[cfg(target_os = "macos")]
const LIBRARY: &str = "libonnxruntime.dylib";
#[cfg(not(any(windows, target_os = "macos")))]
const LIBRARY: &str = "libonnxruntime.so";

/// CPU threads per model: half the cores, at most 4, so a model keeps well
/// inside the camera's frame time and leaves the game most of the CPU.
pub fn default_threads() -> usize {
    let cores = std::thread::available_parallelism().map_or(2, |cores| cores.get());
    (cores / 2).clamp(1, 4)
}

/// Where the ONNX Runtime library is, if anywhere.
pub fn library() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("VRFT_ONNXRUNTIME") {
        return Some(PathBuf::from(path));
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(LIBRARY)));
    let local = std::env::current_dir()
        .ok()
        .map(|dir| dir.join(".local").join("onnxruntime").join(LIBRARY));
    [beside, local]
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
}

/// Loads the library once; later calls return the first outcome.
pub fn available() -> Result<()> {
    static LOADED: OnceLock<Result<String, String>> = OnceLock::new();
    let loaded = LOADED.get_or_init(|| {
        let path = library().ok_or_else(|| format!("{LIBRARY} not found"))?;
        let builder =
            ort::init_from(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        builder.with_name("vrft").commit();
        info!("ONNX Runtime loaded from {}", path.display());
        Ok(path.display().to_string())
    });
    match loaded {
        Ok(_) => Ok(()),
        Err(error) => Err(anyhow!("ONNX Runtime unavailable: {error}")),
    }
}

/// Whether to run models on ONNX Runtime: unless `VRFT_INFERENCE=burn`.
pub fn wanted() -> bool {
    !std::env::var("VRFT_INFERENCE").is_ok_and(|value| value.eq_ignore_ascii_case("burn"))
}

/// Where ONNX Runtime runs a model for a device setting. `Auto` is the CPU:
/// an int8 model takes a few milliseconds there, and the game keeps the GPU.
/// `Gpu` asks for DirectML.
pub fn placement(accelerator: Accelerator) -> Accelerator {
    match accelerator {
        Accelerator::Gpu => Accelerator::Gpu,
        Accelerator::Auto | Accelerator::Cpu => Accelerator::Cpu,
    }
}

fn threads() -> usize {
    std::env::var("VRFT_ONNX_THREADS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|&threads| threads > 0)
        .unwrap_or_else(default_threads)
}

/// One model on ONNX Runtime.
pub struct Session {
    inner: ort::session::Session,
    /// Where it runs, such as "CPU (ONNX Runtime)" or "GPU (DirectML)".
    pub device: String,
}

fn builder(threads: usize) -> Result<ort::session::builder::SessionBuilder> {
    let error = |error: ort::Error<ort::session::builder::SessionBuilder>| anyhow!("{error}");
    ort::session::Session::builder()
        .map_err(|error| anyhow!("{error}"))?
        .with_optimization_level(GraphOptimizationLevel::All)
        .map_err(error)?
        .with_intra_threads(threads)
        .map_err(error)?
        .with_inter_threads(1)
        .map_err(error)?
        .with_parallel_execution(false)
        .map_err(error)?
        // Threads sleep between frames rather than spin, so two models (the
        // pair) and the game don't fight over cores.
        .with_intra_op_spinning(false)
        .map_err(error)?
        .with_inter_op_spinning(false)
        .map_err(error)
}

/// A CPU session: no memory arena or pattern planning, which hold memory a
/// one-frame model doesn't need.
fn cpu_builder(threads: usize) -> Result<ort::session::builder::SessionBuilder> {
    let error = |error: ort::Error<ort::session::builder::SessionBuilder>| anyhow!("{error}");
    builder(threads)?
        .with_memory_pattern(false)
        .map_err(error)?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(error)
}

impl Session {
    /// `model` on the GPU (DirectML, Windows only) or the CPU, as
    /// `accelerator` asks; `Auto` tries the GPU first.
    pub fn new(model: &[u8], accelerator: Accelerator) -> Result<Self> {
        available()?;
        let commit = |builder: ort::session::builder::SessionBuilder| {
            let mut builder = builder;
            builder
                .commit_from_memory(model)
                .map_err(|error| anyhow!("{error}"))
        };
        if accelerator != Accelerator::Cpu {
            match Self::gpu(model) {
                Ok(session) => return Ok(session),
                Err(error) if accelerator == Accelerator::Auto => {
                    warn!("ONNX Runtime GPU unavailable ({error:#}); using the CPU");
                }
                Err(error) => return Err(error),
            }
        }
        let threads = threads();
        let inner = commit(cpu_builder(threads)?)?;
        Ok(Self {
            inner,
            device: format!("CPU (ONNX Runtime, {threads} threads)"),
        })
    }

    #[cfg(windows)]
    fn gpu(model: &[u8]) -> Result<Self> {
        let mut builder = builder(1)?
            // DirectML needs these off.
            .with_memory_pattern(false)
            .map_err(|error| anyhow!("{error}"))?
            .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
            .map_err(|error| anyhow!("{error}"))?;
        let inner = builder
            .commit_from_memory(model)
            .map_err(|error| anyhow!("{error}"))?;
        Ok(Self {
            inner,
            device: "GPU (DirectML)".into(),
        })
    }

    #[cfg(not(windows))]
    fn gpu(_model: &[u8]) -> Result<Self> {
        bail!("ONNX Runtime uses the GPU through DirectML, on Windows only")
    }

    /// Runs the model on float `inputs` (name, shape, values) and returns
    /// the float `outputs` asked for, in that order.
    pub fn run(
        &mut self,
        inputs: &[(&str, &[usize], &[f32])],
        outputs: &[&str],
    ) -> Result<Vec<Vec<f32>>> {
        let mut values = Vec::with_capacity(inputs.len());
        for (name, shape, data) in inputs {
            let tensor = TensorRef::from_array_view((shape.to_vec(), *data))
                .map_err(|error| anyhow!("{error}"))?;
            values.push((
                std::borrow::Cow::Borrowed(*name),
                ort::session::SessionInputValue::from(tensor),
            ));
        }
        let results = self.inner.run(values).map_err(|error| anyhow!("{error}"))?;
        outputs
            .iter()
            .map(|name| {
                let value = results
                    .get(*name)
                    .ok_or_else(|| anyhow!("the model has no output {name}"))?;
                let (_, data) = value
                    .try_extract_tensor::<f32>()
                    .map_err(|error| anyhow!("{error}"))?;
                if data.iter().any(|value| !value.is_finite()) {
                    bail!("the model's {name} isn't finite");
                }
                Ok(data.to_vec())
            })
            .collect()
    }
}
