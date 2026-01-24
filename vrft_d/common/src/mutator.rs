use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::calibration_manager::CalibrationManager;
use crate::{
    CalibrationData, CalibrationState, EuroFilter, UnifiedExpressions, UnifiedTrackingData,
};

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
    Native,
    /// Use the .NET VRCFT runtime for .NET modules (default)
    #[default]
    #[serde(alias = "VRCFT", alias = "vrcft", alias = "DotNet", alias = "dotnet")]
    Vrcft,
}

/// Module loading configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleConfig {
    /// Which module runtime to use (Native or Vrcft)
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

/// Mutator/processing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MutatorConfig {
    /// Whether the mutator is enabled
    pub enabled: bool,
    /// Smoothness factor for filtering (0.0 = no smoothing)
    pub smoothness: f32,
}

impl Default for MutatorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            smoothness: 0.0,
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
    Some(200.0)
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

pub struct UnifiedTrackingMutator {
    pub config: MutationConfig,
    pub calibration_manager: CalibrationManager,
    pub calibration_state: CalibrationState,

    shapes: Vec<EuroFilter>,
    gaze_left_x: EuroFilter,
    gaze_left_y: EuroFilter,
    gaze_right_x: EuroFilter,
    gaze_right_y: EuroFilter,
    pupil_left: EuroFilter,
    pupil_right: EuroFilter,
    openness_left: EuroFilter,
    openness_right: EuroFilter,

    min_pupil_l: f32,
    max_pupil_l: f32,
    min_pupil_r: f32,
    max_pupil_r: f32,
}

impl UnifiedTrackingMutator {
    pub fn new(config: MutationConfig) -> Self {
        let min_cutoff = if config.mutator.smoothness <= 0.0 {
            10.0
        } else {
            1.0 / (config.mutator.smoothness * 10.0)
        };
        let beta = if config.mutator.smoothness <= 0.0 {
            1.0
        } else {
            0.5 * (1.0 - config.mutator.smoothness)
        };

        Self {
            config,
            calibration_manager: CalibrationManager::new(std::path::PathBuf::from(".")),
            calibration_state: CalibrationState::Uncalibrated,
            shapes: vec![
                EuroFilter::new_with_config(min_cutoff, beta);
                UnifiedExpressions::Max as usize
            ],
            gaze_left_x: EuroFilter::new_with_config(min_cutoff, beta),
            gaze_left_y: EuroFilter::new_with_config(min_cutoff, beta),
            gaze_right_x: EuroFilter::new_with_config(min_cutoff, beta),
            gaze_right_y: EuroFilter::new_with_config(min_cutoff, beta),
            pupil_left: EuroFilter::new_with_config(min_cutoff, beta),
            pupil_right: EuroFilter::new_with_config(min_cutoff, beta),
            openness_left: EuroFilter::new_with_config(min_cutoff, beta),
            openness_right: EuroFilter::new_with_config(min_cutoff, beta),

            min_pupil_l: 999.0,
            max_pupil_l: 0.0,
            min_pupil_r: 999.0,
            max_pupil_r: 0.0,
        }
    }

    pub fn start_calibration(&mut self, duration_seconds: f32) {
        if !self.config.calibration.enabled {
            return;
        }
        self.calibration_state = CalibrationState::Collecting {
            timer: 0.0,
            duration: duration_seconds,
        };
        self.calibration_manager.data.clear();
    }

    pub fn has_calibration_data(&self) -> bool {
        self.calibration_manager
            .data
            .shapes
            .iter()
            .any(|p| p.max > 0.0)
    }

    pub fn calibration_status(&self) -> (bool, f32, f32, f32) {
        match self.calibration_state {
            CalibrationState::Collecting { timer, duration } => {
                let progress = if duration > 0.0 {
                    (timer / duration).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (true, timer, duration, progress)
            }
            _ => (false, 0.0, 0.0, 0.0),
        }
    }

    pub fn get_calibration_data(&self) -> CalibrationData {
        self.calibration_manager.data.clone()
    }

    pub fn save_calibration(&self, _path: &Path) -> anyhow::Result<()> {
        if !self.config.calibration.enabled {
            return Ok(());
        }
        self.calibration_manager.save_current_profile()
    }

    pub fn load_calibration(&mut self, _path: &Path) -> anyhow::Result<()> {
        if !self.config.calibration.enabled {
            return Ok(());
        }
        self.calibration_manager.load_profile("default")
    }

    pub fn switch_profile(&mut self, new_profile_id: &str) -> anyhow::Result<()> {
        if !self.config.calibration.enabled {
            return Ok(());
        }
        let should_save = self.has_calibration_data();
        self.calibration_manager
            .switch_profile(new_profile_id, should_save)
    }

    pub fn mutate(&mut self, data: &mut UnifiedTrackingData, dt: f32) {
        if !self.config.mutator.enabled {
            return;
        }

        if let CalibrationState::Collecting {
            mut timer,
            duration,
        } = self.calibration_state
        {
            timer += dt;
            if timer >= duration {
                self.calibration_state = CalibrationState::Calibrated;
            } else {
                self.calibration_state = CalibrationState::Collecting { timer, duration };
            }
        }

        if self.config.calibration.enabled {
            for i in 0..data.shapes.len() {
                if i < self.calibration_manager.data.shapes.len() {
                    let raw_weight = data.shapes[i].weight;

                    self.calibration_manager.data.shapes[i].update_calibration(
                        raw_weight,
                        self.config.calibration.continuous,
                        dt,
                    );

                    data.shapes[i].weight = self.calibration_manager.data.shapes[i]
                        .calculate_parameter(raw_weight, self.config.calibration.blend);
                }
            }
        }

        data.eye.left.openness = self.openness_left.filter(data.eye.left.openness);
        data.eye.right.openness = self.openness_right.filter(data.eye.right.openness);

        data.eye.left.gaze.x = self.gaze_left_x.filter(data.eye.left.gaze.x);
        data.eye.left.gaze.y = self.gaze_left_y.filter(data.eye.left.gaze.y);
        data.eye.right.gaze.x = self.gaze_right_x.filter(data.eye.right.gaze.x);
        data.eye.right.gaze.y = self.gaze_right_y.filter(data.eye.right.gaze.y);

        data.eye.left.pupil_diameter_mm = self.pupil_left.filter(data.eye.left.pupil_diameter_mm);
        data.eye.right.pupil_diameter_mm =
            self.pupil_right.filter(data.eye.right.pupil_diameter_mm);

        for i in 0..data.shapes.len() {
            if i < self.shapes.len() {
                data.shapes[i].weight = self.shapes[i].filter(data.shapes[i].weight);
            }
        }

        let curr_l = data.eye.left.pupil_diameter_mm;
        let curr_r = data.eye.right.pupil_diameter_mm;

        if curr_l > 0.0 {
            if curr_l < self.min_pupil_l {
                self.min_pupil_l = curr_l;
            }
            if curr_l > self.max_pupil_l {
                self.max_pupil_l = curr_l;
            }
        }
        if curr_r > 0.0 {
            if curr_r < self.min_pupil_r {
                self.min_pupil_r = curr_r;
            }
            if curr_r > self.max_pupil_r {
                self.max_pupil_r = curr_r;
            }
        }

        if (self.max_pupil_l - self.min_pupil_l) > 0.001 {
            data.eye.left.pupil_diameter_mm =
                (curr_l - self.min_pupil_l) / (self.max_pupil_l - self.min_pupil_l);
        } else {
            data.eye.left.pupil_diameter_mm = 0.5;
        }

        if (self.max_pupil_r - self.min_pupil_r) > 0.001 {
            data.eye.right.pupil_diameter_mm =
                (curr_r - self.min_pupil_r) / (self.max_pupil_r - self.min_pupil_r);
        } else {
            data.eye.right.pupil_diameter_mm = 0.5;
        }
    }
}

pub trait IntegrationAdapter: Send + Sync {
    fn initialize(&mut self) -> anyhow::Result<()>;
    fn send(&self, data: &UnifiedTrackingData) -> anyhow::Result<()>;
}
