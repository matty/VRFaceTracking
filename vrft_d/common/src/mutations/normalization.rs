use crate::mutation_trait::Mutation;
use crate::mutator::MutationConfig;
use crate::UnifiedTrackingData;
use anyhow::Result;
use std::any::Any;

pub struct NormalizationMutation {
    min_pupil_l: f32,
    max_pupil_l: f32,
    min_pupil_r: f32,
    max_pupil_r: f32,
}

impl NormalizationMutation {
    pub fn new(_config: &MutationConfig) -> Self {
        Self {
            min_pupil_l: 999.0,
            max_pupil_l: 0.0,
            min_pupil_r: 999.0,
            max_pupil_r: 0.0,
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
