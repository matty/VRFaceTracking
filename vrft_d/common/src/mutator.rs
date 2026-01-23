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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MutationConfig {
    /// Which module runtime to use (Native or Vrcft)
    #[serde(default)]
    pub module_runtime: ModuleRuntime,

    pub smoothness: f32,
    pub mutator_enabled: bool,
    pub calibration_enabled: bool,
    #[serde(default = "default_calibration_continuous")]
    pub calibration_continuous: bool,
    #[serde(default = "default_calibration_blend")]
    pub calibration_blend: f32,

    #[serde(default, alias = "transport_type")]
    pub output_mode: OutputMode,
    #[serde(default = "default_osc_address")]
    pub osc_send_address: String,
    #[serde(default = "default_osc_port")]
    pub osc_send_port: u16,

    #[serde(default = "default_active_plugin")]
    pub active_plugin: String,

    #[serde(default = "default_max_fps")]
    pub max_fps: Option<f32>,
}

fn default_active_plugin() -> String {
    "vd_module.dll".to_string()
}

fn default_calibration_continuous() -> bool {
    false
}

fn default_calibration_blend() -> f32 {
    1.0
}

fn default_osc_address() -> String {
    "127.0.0.1".to_string()
}

fn default_osc_port() -> u16 {
    9000
}

fn default_max_fps() -> Option<f32> {
    Some(200.0)
}

impl Default for MutationConfig {
    fn default() -> Self {
        Self {
            module_runtime: ModuleRuntime::default(),
            smoothness: 0.0,
            mutator_enabled: true,
            calibration_enabled: false,
            calibration_continuous: default_calibration_continuous(),
            calibration_blend: default_calibration_blend(),
            output_mode: OutputMode::default(),
            osc_send_address: default_osc_address(),
            osc_send_port: default_osc_port(),
            active_plugin: default_active_plugin(),
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
        let min_cutoff = if config.smoothness <= 0.0 {
            10.0
        } else {
            1.0 / (config.smoothness * 10.0)
        };
        let beta = if config.smoothness <= 0.0 {
            1.0
        } else {
            0.5 * (1.0 - config.smoothness)
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
        if !self.config.calibration_enabled {
            return Ok(());
        }
        self.calibration_manager.save_current_profile()
    }

    pub fn load_calibration(&mut self, _path: &Path) -> anyhow::Result<()> {
        self.calibration_manager.load_profile("default")
    }

    pub fn switch_profile(&mut self, new_profile_id: &str) -> anyhow::Result<()> {
        let should_save = self.config.calibration_enabled && self.has_calibration_data();
        self.calibration_manager
            .switch_profile(new_profile_id, should_save)
    }

    pub fn mutate(&mut self, data: &mut UnifiedTrackingData, dt: f32) {
        if !self.config.mutator_enabled {
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

        if self.config.calibration_enabled {
            for i in 0..data.shapes.len() {
                if i < self.calibration_manager.data.shapes.len() {
                    let raw_weight = data.shapes[i].weight;

                    self.calibration_manager.data.shapes[i].update_calibration(
                        raw_weight,
                        self.config.calibration_continuous,
                        dt,
                    );

                    data.shapes[i].weight = self.calibration_manager.data.shapes[i]
                        .calculate_parameter(raw_weight, self.config.calibration_blend);
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
