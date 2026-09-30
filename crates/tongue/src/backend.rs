//! Where tongue models run.

use std::str::FromStr;

/// GPU through wgpu (DX12 or Vulkan), so NVIDIA, AMD and Intel all work
/// without CUDA or ROCm. The `cuda` feature uses NVIDIA's CUDA instead, for
/// rented Linux machines that have no Vulkan driver.
#[cfg(not(feature = "cuda"))]
pub type Gpu = burn::backend::Wgpu;
#[cfg(feature = "cuda")]
pub type Gpu = burn::backend::Cuda;
/// How progress and reports name the GPU backend.
#[cfg(not(feature = "cuda"))]
pub const GPU_NAME: &str = "GPU (wgpu)";
#[cfg(feature = "cuda")]
pub const GPU_NAME: &str = "GPU (CUDA)";
pub type Cpu = burn::backend::Flex;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Accelerator {
    /// The GPU when one works, else the CPU.
    #[default]
    Auto,
    Cpu,
    Gpu,
}

impl FromStr for Accelerator {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Ok(Self::Auto),
            "cpu" => Ok(Self::Cpu),
            // "cuda" is what the Python helper accepted.
            "gpu" | "cuda" => Ok(Self::Gpu),
            other => Err(format!(
                "unknown tongue device {other}; use auto, cpu or gpu"
            )),
        }
    }
}

impl Accelerator {
    /// `VRFT_TONGUE_DEVICE`, defaulting to automatic.
    pub fn from_env() -> Self {
        std::env::var("VRFT_TONGUE_DEVICE")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or_default()
    }
}
