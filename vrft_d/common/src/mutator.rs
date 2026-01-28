use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::mutation_trait::Mutation;
use crate::mutations::{CalibrationMutation, NormalizationMutation, SmoothingMutation};
use crate::{CalibrationData, CalibrationState, UnifiedTrackingData};
use anyhow::Result;
use log::info;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum OutputMode {
    #[serde(alias = "VRChat", alias = "VRChatOSC")]
    #[default]
    VRChat,
    #[serde(alias = "Resonite")]
    Resonite,
    #[serde(alias = "Generic", alias = "GenericUDP")]
    Generic,
}

/// Which module runtime to use for loading tracking modules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum ModuleRuntime {
    /// Only load native (.dll/.so) modules from plugins/native
    #[serde(alias = "native")]
    Native,
    /// Use the .NET runtime for legacy modules (default)
    #[default]
    #[serde(alias = "VRCFT", alias = "vrcft", alias = "DotNet", alias = "dotnet")]
    Vrcft,
}

/// Module loading configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleConfig {
    /// Which module runtime to use (Native or .NET)
    pub runtime: ModuleRuntime,
    /// The active module/plugin to load
    #[serde(default = "default_active_module")]
    pub active: String,
}

impl Default for ModuleConfig {
    fn default() -> Self {
        Self {
            runtime: ModuleRuntime::default(),
            active: default_active_module(),
        }
    }
}

fn default_active_module() -> String {
    "vd_module.dll".to_string()
}

/// Configuration for a single pipeline step
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PipelineStepConfig {
    Smoothing {
        #[serde(default)]
        smoothness: Option<f32>,
    },
    Calibration {
        #[serde(default)]
        enabled: Option<bool>,
    },
    Normalization,
}

/// Mutator/processing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MutatorConfig {
    /// Whether the mutator is enabled
    pub enabled: bool,
    /// Smoothness factor for filtering (legacy, used if pipeline not specified)
    pub smoothness: f32,
    /// Optional explicit pipeline configuration
    pub pipeline: Option<Vec<PipelineStepConfig>>,
}

impl Default for MutatorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            smoothness: 0.0,
            pipeline: None,
        }
    }
}

/// Calibration configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CalibrationConfig {
    /// Whether calibration is enabled
    pub enabled: bool,
    /// Whether to continuously update calibration
    pub continuous: bool,
    /// Blend factor for calibration (0.0-1.0)
    pub blend: f32,
}

impl Default for CalibrationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            continuous: false,
            blend: 1.0,
        }
    }
}

/// OSC output configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OscConfig {
    /// Output mode (VRChat, Resonite, Generic)
    pub output_mode: OutputMode,
    /// OSC send address
    pub send_address: String,
    /// OSC send port
    pub send_port: u16,
}

impl Default for OscConfig {
    fn default() -> Self {
        Self {
            output_mode: OutputMode::default(),
            send_address: "127.0.0.1".to_string(),
            send_port: 9000,
        }
    }
}

/// Main application configuration with nested groups
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MutationConfig {
    /// Module loading settings
    pub module: ModuleConfig,
    /// Mutator/processing settings
    pub mutator: MutatorConfig,
    /// Calibration settings
    pub calibration: CalibrationConfig,
    /// OSC output settings
    pub osc: OscConfig,
    /// Maximum FPS limit
    #[serde(default = "default_max_fps")]
    pub max_fps: Option<f32>,
}

fn default_max_fps() -> Option<f32> {
    Some(60.0)
}

impl Default for MutationConfig {
    fn default() -> Self {
        Self {
            module: ModuleConfig::default(),
            mutator: MutatorConfig::default(),
            calibration: CalibrationConfig::default(),
            osc: OscConfig::default(),
            max_fps: default_max_fps(),
        }
    }
}

/// Factory function to create a mutation from pipeline step config
fn create_mutation_from_step(
    step: &PipelineStepConfig,
    config: &MutationConfig,
) -> Box<dyn Mutation> {
    match step {
        PipelineStepConfig::Smoothing { smoothness } => {
            let mut cfg = config.clone();
            if let Some(s) = smoothness {
                cfg.mutator.smoothness = *s;
            }
            Box::new(SmoothingMutation::new(&cfg))
        }
        PipelineStepConfig::Calibration { enabled } => {
            let mut cfg = config.clone();
            if let Some(e) = enabled {
                cfg.calibration.enabled = *e;
            }
            Box::new(CalibrationMutation::new(&cfg))
        }
        PipelineStepConfig::Normalization => Box::new(NormalizationMutation::new(config)),
    }
}

pub struct UnifiedTrackingMutator {
    pub config: MutationConfig,
    pipeline: Vec<Box<dyn Mutation>>,
}

impl UnifiedTrackingMutator {
    pub fn new(config: MutationConfig) -> Self {
        let pipeline = if let Some(ref steps) = config.mutator.pipeline {
            info!(
                "Building mutation pipeline from config ({} steps)",
                steps.len()
            );
            steps
                .iter()
                .map(|step| create_mutation_from_step(step, &config))
                .collect()
        } else {
            info!("Using default mutation pipeline");
            vec![
                Box::new(SmoothingMutation::new(&config)) as Box<dyn Mutation>,
                Box::new(CalibrationMutation::new(&config)),
                Box::new(NormalizationMutation::new(&config)),
            ]
        };

        Self { config, pipeline }
    }

    fn get_calibration_mutation(&self) -> Option<&CalibrationMutation> {
        for m in &self.pipeline {
            if let Some(c) = m.as_any().downcast_ref::<CalibrationMutation>() {
                return Some(c);
            }
        }
        None
    }

    fn get_calibration_mutation_mut(&mut self) -> Option<&mut CalibrationMutation> {
        for m in &mut self.pipeline {
            if let Some(c) = m.as_any_mut().downcast_mut::<CalibrationMutation>() {
                return Some(c);
            }
        }
        None
    }

    pub fn start_calibration(&mut self, duration_seconds: f32) {
        if let Some(c) = self.get_calibration_mutation_mut() {
            c.start_calibration(duration_seconds);
        }
    }

    pub fn has_calibration_data(&self) -> bool {
        if let Some(c) = self.get_calibration_mutation() {
            c.has_calibration_data()
        } else {
            false
        }
    }

    pub fn calibration_status(&self) -> (bool, f32, f32, f32) {
        if let Some(c) = self.get_calibration_mutation() {
            c.calibration_status()
        } else {
            (false, 0.0, 0.0, 0.0)
        }
    }

    pub fn get_calibration_state(&self) -> CalibrationState {
        if let Some(c) = self.get_calibration_mutation() {
            c.state.clone()
        } else {
            CalibrationState::Uncalibrated
        }
    }

    pub fn get_calibration_data(&self) -> CalibrationData {
        if let Some(c) = self.get_calibration_mutation() {
            c.get_calibration_data()
        } else {
            CalibrationData::default()
        }
    }

    pub fn save_calibration(&self, path: &Path) -> Result<()> {
        if let Some(c) = self.get_calibration_mutation() {
            c.save_calibration(path)
        } else {
            Ok(())
        }
    }

    pub fn load_calibration(&mut self, path: &Path) -> Result<()> {
        if let Some(c) = self.get_calibration_mutation_mut() {
            c.load_calibration(path)
        } else {
            Ok(())
        }
    }

    pub fn mutate(&mut self, data: &mut UnifiedTrackingData, dt: f32) {
        if !self.config.mutator.enabled {
            return;
        }

        for mutation in &mut self.pipeline {
            mutation.mutate(data, dt);
        }
    }
}

pub trait IntegrationAdapter: Send + Sync {
    fn initialize(&mut self) -> anyhow::Result<()>;
    fn send(&self, data: &UnifiedTrackingData) -> anyhow::Result<()>;
}
