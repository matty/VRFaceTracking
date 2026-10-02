use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::mutation_trait::Mutation;
use crate::mutations::{AdjustmentMutation, CorrectorsMutation, SmoothingMutation};
use crate::UnifiedTrackingData;
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

/// Deprecated module runtime selector.
///
/// Runtime (native vs .NET) is now auto-detected from each plugin's PE header,
/// so this enum is no longer used for load decisions. It is retained only so
/// older `config.json` files that still specify `module.runtime` keep parsing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum ModuleRuntime {
    /// Legacy "native Rust module" selector (ignored).
    #[serde(alias = "native")]
    Native,
    /// Legacy ".NET/VRCFT module" selector (ignored, default).
    #[default]
    #[serde(alias = "VRCFT", alias = "vrcft", alias = "DotNet", alias = "dotnet")]
    Vrcft,
}

/// Module loading configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleConfig {
    /// Deprecated: runtime is now auto-detected from the plugin's PE header.
    /// Retained only so older configs that still specify it continue to parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ModuleRuntime>,
    /// The active module/plugin to load
    #[serde(default = "default_active_module")]
    pub active: String,
    /// Where the app finds modules to install; the VRCFT registry unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_url: Option<String>,
}

impl Default for ModuleConfig {
    fn default() -> Self {
        Self {
            runtime: None,
            active: default_active_module(),
            registry_url: None,
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
    /// Runs the correctors with `mutator.correctors`, whether or not that
    /// is enabled.
    Correctors {},
    /// Runs the adjustment with `mutator.adjustment`, whether or not that
    /// is enabled.
    Adjustment {},
    /// Removed calibration step, retained only so older `config.json` files
    /// that still list it keep parsing. It produces no pipeline stage, and any
    /// options it used to carry are ignored.
    Calibration {},
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
    /// Raw Euro filter settings that override the `smoothness` preset
    pub filter: FilterConfig,
    /// Fixes that make module data conform to Unified Expressions
    pub correctors: CorrectorsConfig,
    /// Per-group range remapping of shapes and head pose
    pub adjustment: AdjustmentConfig,
}

impl Default for MutatorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            smoothness: 0.0,
            pipeline: None,
            filter: FilterConfig::default(),
            correctors: CorrectorsConfig::default(),
            adjustment: AdjustmentConfig::default(),
        }
    }
}

// Shared with the desktop app, which shows and changes them.
pub use vrft_protocol::{
    AdjustmentTuning as AdjustmentConfig, CorrectorsTuning as CorrectorsConfig,
    FilterTuning as FilterConfig,
};

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
    /// OSC output settings
    pub osc: OscConfig,
    /// Maximum FPS limit
    #[serde(default = "default_max_fps")]
    pub max_fps: Option<f32>,
    /// Each daemon extension's block, by extension id, such as
    /// `"quest-pro": { "enabled": false }`. The daemon reads these as
    /// `vrft_extension::ExtensionConfig`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

fn default_max_fps() -> Option<f32> {
    Some(60.0)
}

impl MutationConfig {
    /// The time between frames `max_fps` allows, or `None` without a usable
    /// limit. `max_fps` comes straight from user-editable config, so it may
    /// be zero, negative or not a number.
    pub fn frame_interval(&self) -> Option<std::time::Duration> {
        self.max_fps
            .filter(|fps| fps.is_finite() && *fps > 0.0)
            .and_then(|fps| std::time::Duration::try_from_secs_f32(1.0 / fps).ok())
    }
}

impl Default for MutationConfig {
    fn default() -> Self {
        Self {
            module: ModuleConfig::default(),
            mutator: MutatorConfig::default(),
            osc: OscConfig::default(),
            max_fps: default_max_fps(),
            extensions: BTreeMap::new(),
        }
    }
}

/// Factory function to create a mutation from pipeline step config.
///
/// Returns `None` for steps that no longer map to a mutation.
fn create_mutation_from_step(
    step: &PipelineStepConfig,
    config: &MutationConfig,
) -> Option<Box<dyn Mutation>> {
    match step {
        PipelineStepConfig::Smoothing { smoothness } => {
            let mut cfg = config.clone();
            if let Some(s) = smoothness {
                cfg.mutator.smoothness = *s;
            }
            Some(Box::new(SmoothingMutation::new(&cfg)))
        }
        PipelineStepConfig::Correctors {} => Some(Box::new(CorrectorsMutation::new(config))),
        PipelineStepConfig::Adjustment {} => Some(Box::new(AdjustmentMutation::new(config))),
        PipelineStepConfig::Calibration {} => {
            info!("Ignoring removed 'calibration' pipeline step");
            None
        }
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
                .filter_map(|step| create_mutation_from_step(step, &config))
                .collect()
        } else {
            info!("Using default mutation pipeline");
            // VRCFaceTracking's order: adjustment, then correctors, then the
            // filter, so smoothing sees the final values.
            let mut steps: Vec<Box<dyn Mutation>> = Vec::new();
            if config.mutator.adjustment.enabled {
                steps.push(Box::new(AdjustmentMutation::new(&config)));
            }
            if config.mutator.correctors.enabled {
                steps.push(Box::new(CorrectorsMutation::new(&config)));
            }
            steps.push(Box::new(SmoothingMutation::new(&config)));
            steps
        };

        Self { config, pipeline }
    }

    pub fn mutate(&mut self, data: &mut UnifiedTrackingData, dt: f32) {
        self.mutate_with(data, dt, |_, _| {});
    }

    /// Like [`mutate`](Self::mutate), showing `after_step` each step's name
    /// and the values it left.
    pub fn mutate_with(
        &mut self,
        data: &mut UnifiedTrackingData,
        dt: f32,
        mut after_step: impl FnMut(&str, &UnifiedTrackingData),
    ) {
        if !self.config.mutator.enabled {
            return;
        }

        for mutation in &mut self.pipeline {
            mutation.mutate(data, dt);
            after_step(mutation.name(), data);
        }
    }

    /// Each step in order, and whether it runs. The default pipeline lists
    /// the steps it leaves out too, in their places, as not running.
    pub fn steps(&self) -> Vec<(String, bool)> {
        let enabled = self.config.mutator.enabled;
        if self.config.mutator.pipeline.is_some() {
            return self
                .pipeline
                .iter()
                .map(|step| (step.name().to_string(), enabled))
                .collect();
        }
        let mutator = &self.config.mutator;
        [
            ("Adjustment", mutator.adjustment.enabled),
            ("Correctors", mutator.correctors.enabled),
            ("Smoothing", true),
        ]
        .into_iter()
        .map(|(name, on)| (name.to_string(), enabled && on))
        .collect()
    }
}

pub trait IntegrationAdapter: Send + Sync {
    fn initialize(&mut self) -> anyhow::Result<()>;
    fn send(&self, data: &UnifiedTrackingData) -> anyhow::Result<()>;
}

#[cfg(test)]
mod module_config_tests {
    use super::*;

    #[test]
    fn old_config_with_runtime_field_still_parses() {
        let json = r#"{ "runtime": "Native", "active": "vd_module.dll" }"#;
        let cfg: ModuleConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.active, "vd_module.dll");
        assert_eq!(cfg.runtime, Some(ModuleRuntime::Native));
    }

    #[test]
    fn config_without_runtime_field_parses() {
        let json = r#"{ "active": "vd_module.dll" }"#;
        let cfg: ModuleConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.runtime, None);
    }

    #[test]
    fn old_config_with_calibration_still_parses_and_is_ignored() {
        let json = r#"{
            "mutator": {
                "enabled": true,
                "pipeline": [
                    { "type": "smoothing", "smoothness": 0.5 },
                    { "type": "calibration", "enabled": true }
                ]
            },
            "calibration": { "enabled": true, "continuous": true, "blend": 1.0 }
        }"#;
        let cfg: MutationConfig = serde_json::from_str(json).unwrap();

        let mutator = UnifiedTrackingMutator::new(cfg);
        assert_eq!(
            mutator.pipeline.len(),
            1,
            "the removed calibration step must not add a pipeline stage"
        );
        assert_eq!(mutator.pipeline[0].name(), "Smoothing");
    }

    fn step_names(mutator: &UnifiedTrackingMutator) -> Vec<&str> {
        mutator.pipeline.iter().map(|step| step.name()).collect()
    }

    #[test]
    fn default_pipeline_runs_enabled_steps_in_vrcft_order() {
        let cfg = MutationConfig::default();
        assert_eq!(
            step_names(&UnifiedTrackingMutator::new(cfg)),
            ["Correctors", "Smoothing"]
        );

        let json = r#"{ "mutator": {
            "adjustment": { "enabled": true, "ranges": { "jaw": [0.0, 0.8] } },
            "correctors": { "enabled": false }
        } }"#;
        let cfg: MutationConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            step_names(&UnifiedTrackingMutator::new(cfg)),
            ["Adjustment", "Smoothing"]
        );
    }

    #[test]
    fn explicit_pipeline_steps_run_even_when_their_section_is_disabled() {
        let json = r#"{ "mutator": {
            "pipeline": [{ "type": "adjustment" }, { "type": "correctors" }, { "type": "smoothing" }],
            "correctors": { "enabled": false }
        } }"#;
        let cfg: MutationConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            step_names(&UnifiedTrackingMutator::new(cfg)),
            ["Adjustment", "Correctors", "Smoothing"]
        );
    }

    #[test]
    fn steps_list_the_default_pipelines_steps_that_are_off() {
        let steps = UnifiedTrackingMutator::new(MutationConfig::default()).steps();
        assert_eq!(
            steps,
            [
                ("Adjustment".to_string(), false),
                ("Correctors".to_string(), true),
                ("Smoothing".to_string(), true),
            ]
        );

        let json = r#"{ "mutator": { "enabled": false } }"#;
        let cfg: MutationConfig = serde_json::from_str(json).unwrap();
        let steps = UnifiedTrackingMutator::new(cfg).steps();
        assert!(steps.iter().all(|(_, runs)| !runs));
    }

    #[test]
    fn mutate_with_shows_each_step_that_runs() {
        let mut mutator = UnifiedTrackingMutator::new(MutationConfig::default());
        let mut seen = Vec::new();
        mutator.mutate_with(&mut UnifiedTrackingData::default(), 0.016, |name, _| {
            seen.push(name.to_string())
        });
        assert_eq!(seen, ["Correctors", "Smoothing"]);
    }

    #[test]
    fn old_mutator_config_parses_with_default_tuning() {
        let json = r#"{ "mutator": { "enabled": true, "smoothness": 0.3 } }"#;
        let cfg: MutationConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.mutator.filter, FilterConfig::default());
        assert_eq!(cfg.mutator.correctors, CorrectorsConfig::default());
        assert_eq!(cfg.mutator.adjustment, AdjustmentConfig::default());
    }

    #[test]
    fn default_config_omits_runtime_when_serialized() {
        let cfg = ModuleConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("runtime"), "serialized default was: {json}");
    }
}
