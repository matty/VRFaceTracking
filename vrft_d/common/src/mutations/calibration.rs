use crate::calibration::{CalibrationData, CalibrationState};
use crate::calibration_manager::CalibrationManager;
use crate::mutation_trait::Mutation;
use crate::mutator::{CalibrationConfig, MutationConfig};
use crate::UnifiedTrackingData;
use anyhow::Result;
use std::any::Any;
use std::path::Path;

pub struct CalibrationMutation {
    pub manager: CalibrationManager,
    pub state: CalibrationState,
    config: CalibrationConfig,
}

impl CalibrationMutation {
    pub fn new(config: &MutationConfig) -> Self {
        Self {
            manager: CalibrationManager::new(std::path::PathBuf::from(".")),
            state: CalibrationState::Uncalibrated,
            config: config.calibration.clone(),
        }
    }

    pub fn start_calibration(&mut self, duration_seconds: f32) {
        if !self.config.enabled {
            return;
        }
        self.state = CalibrationState::Collecting {
            timer: 0.0,
            duration: duration_seconds,
        };
        self.manager.data.clear();
    }

    pub fn has_calibration_data(&self) -> bool {
        self.manager
            .data
            .shapes
            .iter()
            .any(|p| p.max > 0.0 || p.confidence > 0.0)
    }

    pub fn calibration_status(&self) -> (bool, f32, f32, f32) {
        match self.state {
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
        self.manager.data.clone()
    }

    pub fn save_calibration(&self, _path: &Path) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        self.manager.save()
    }

    pub fn load_calibration(&mut self, _path: &Path) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        self.manager.load()
    }
}

impl Mutation for CalibrationMutation {
    fn initialize(&mut self, config: &MutationConfig) -> Result<()> {
        self.config = config.calibration.clone();
        Ok(())
    }

    fn mutate(&mut self, data: &mut UnifiedTrackingData, dt: f32) {
        if !self.config.enabled {
            return;
        }

        if let CalibrationState::Collecting {
            mut timer,
            duration,
        } = self.state
        {
            timer += dt;
            if timer >= duration {
                self.state = CalibrationState::Calibrated;
            } else {
                self.state = CalibrationState::Collecting { timer, duration };
            }
        }

        for i in 0..data.shapes.len() {
            if i < self.manager.data.shapes.len() {
                let raw_weight = data.shapes[i].weight;

                self.manager.data.shapes[i].update_calibration(
                    raw_weight,
                    self.config.continuous,
                    dt,
                );

                data.shapes[i].weight =
                    self.manager.data.shapes[i].calculate_parameter(raw_weight, self.config.blend);
            }
        }
    }

    fn name(&self) -> &str {
        "Calibration"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
