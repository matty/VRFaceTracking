use crate::mutation_trait::Mutation;
use crate::mutator::MutationConfig;
use crate::UnifiedTrackingData;
use anyhow::Result;
use std::any::Any;

const DEFAULT_MIN_PUPIL_MM: f32 = 2.0;
const DEFAULT_MAX_PUPIL_MM: f32 = 8.0;

pub struct NormalizationMutation {
    min_pupil_l: f32,
    max_pupil_l: f32,
    min_pupil_r: f32,
    max_pupil_r: f32,
}

impl NormalizationMutation {
    pub fn new(_config: &MutationConfig) -> Self {
        Self {
            min_pupil_l: DEFAULT_MIN_PUPIL_MM,
            max_pupil_l: DEFAULT_MAX_PUPIL_MM,
            min_pupil_r: DEFAULT_MIN_PUPIL_MM,
            max_pupil_r: DEFAULT_MAX_PUPIL_MM,
        }
    }
}

impl Mutation for NormalizationMutation {
    fn initialize(&mut self, _config: &MutationConfig) -> Result<()> {
        Ok(())
    }

    fn mutate(&mut self, data: &mut UnifiedTrackingData, _dt: f32) {
        let curr_l = data.eye.left.pupil_diameter_mm;
        let curr_r = data.eye.right.pupil_diameter_mm;

        // Expand bounds if hardware reports outside
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

        let range_l = self.max_pupil_l - self.min_pupil_l;
        data.eye.left.pupil_diameter_mm = if range_l > 0.001 {
            ((curr_l - self.min_pupil_l) / range_l).clamp(0.0, 1.0)
        } else {
            0.5
        };

        let range_r = self.max_pupil_r - self.min_pupil_r;
        data.eye.right.pupil_diameter_mm = if range_r > 0.001 {
            ((curr_r - self.min_pupil_r) / range_r).clamp(0.0, 1.0)
        } else {
            0.5
        };
    }

    fn name(&self) -> &str {
        "Normalization"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
