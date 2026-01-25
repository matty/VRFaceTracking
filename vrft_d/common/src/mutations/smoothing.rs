use crate::mutation_trait::Mutation;
use crate::mutator::MutationConfig;
use crate::{EuroFilter, UnifiedExpressions, UnifiedTrackingData};
use anyhow::Result;
use std::any::Any;

pub struct SmoothingMutation {
    shapes: Vec<EuroFilter>,
    gaze_left_x: EuroFilter,
    gaze_left_y: EuroFilter,
    gaze_right_x: EuroFilter,
    gaze_right_y: EuroFilter,
    pupil_left: EuroFilter,
    pupil_right: EuroFilter,
    openness_left: EuroFilter,
    openness_right: EuroFilter,
}

impl SmoothingMutation {
    pub fn new(config: &MutationConfig) -> Self {
        let (min_cutoff, beta) = Self::calculate_params(config.mutator.smoothness);

        Self {
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
        }
    }

    fn calculate_params(smoothness: f32) -> (f32, f32) {
        let min_cutoff = if smoothness <= 0.0 {
            10.0
        } else {
            1.0 / (smoothness * 10.0)
        };
        let beta = if smoothness <= 0.0 {
            1.0
        } else {
            0.5 * (1.0 - smoothness)
        };
        (min_cutoff, beta)
    }
}

impl Mutation for SmoothingMutation {
    fn initialize(&mut self, config: &MutationConfig) -> Result<()> {
        let (_min_cutoff, _beta) = Self::calculate_params(config.mutator.smoothness);
        
        *self = Self::new(config);
        Ok(())
    }

    fn mutate(&mut self, data: &mut UnifiedTrackingData, _dt: f32) {
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
    }

    fn name(&self) -> &str {
        "Smoothing"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
