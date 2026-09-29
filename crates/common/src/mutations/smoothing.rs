use crate::mutation_trait::Mutation;
use crate::mutator::MutationConfig;
use crate::{EuroFilter, UnifiedExpressions, UnifiedTrackingData};
use anyhow::Result;
use log::warn;

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
    /// Yaw, pitch, roll, then position x, y, z; `None` when head pose isn't
    /// smoothed.
    head: Option<[EuroFilter; 6]>,
}

impl SmoothingMutation {
    pub fn new(config: &MutationConfig) -> Self {
        let (min_cutoff, beta, d_cutoff) = Self::params(config);
        let filter = EuroFilter::new_with_params(min_cutoff, beta, d_cutoff);

        Self {
            shapes: vec![filter; UnifiedExpressions::Max as usize],
            gaze_left_x: filter,
            gaze_left_y: filter,
            gaze_right_x: filter,
            gaze_right_y: filter,
            pupil_left: filter,
            pupil_right: filter,
            openness_left: filter,
            openness_right: filter,
            head: config.mutator.filter.head.then_some([filter; 6]),
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

    /// `min_cutoff`, `beta` and `d_cutoff`: the `smoothness` preset, with any
    /// raw values set in `mutator.filter` in its place. A raw value the
    /// filter can't use is ignored.
    fn params(config: &MutationConfig) -> (f32, f32, f32) {
        let (preset_cutoff, preset_beta) = Self::calculate_params(config.mutator.smoothness);
        let filter = &config.mutator.filter;
        let pick =
            |name: &str, value: Option<f32>, usable: fn(f32) -> bool, preset: f32| match value {
                Some(v) if usable(v) => v,
                Some(v) => {
                    warn!("Ignoring mutator.filter.{name} = {v}; using {preset}");
                    preset
                }
                None => preset,
            };
        let positive = |v: f32| v.is_finite() && v > 0.0;
        let non_negative = |v: f32| v.is_finite() && v >= 0.0;
        (
            pick("min_cutoff", filter.min_cutoff, positive, preset_cutoff),
            pick("beta", filter.beta, non_negative, preset_beta),
            pick("d_cutoff", Some(filter.d_cutoff), positive, 0.1),
        )
    }
}

impl Mutation for SmoothingMutation {
    fn initialize(&mut self, config: &MutationConfig) -> Result<()> {
        *self = Self::new(config);
        Ok(())
    }

    fn mutate(&mut self, data: &mut UnifiedTrackingData, dt: f32) {
        data.eye.left.openness = self.openness_left.filter(data.eye.left.openness, dt);
        data.eye.right.openness = self.openness_right.filter(data.eye.right.openness, dt);

        data.eye.left.gaze.x = self.gaze_left_x.filter(data.eye.left.gaze.x, dt);
        data.eye.left.gaze.y = self.gaze_left_y.filter(data.eye.left.gaze.y, dt);
        data.eye.right.gaze.x = self.gaze_right_x.filter(data.eye.right.gaze.x, dt);
        data.eye.right.gaze.y = self.gaze_right_y.filter(data.eye.right.gaze.y, dt);

        data.eye.left.pupil_diameter_mm =
            self.pupil_left.filter(data.eye.left.pupil_diameter_mm, dt);
        data.eye.right.pupil_diameter_mm = self
            .pupil_right
            .filter(data.eye.right.pupil_diameter_mm, dt);

        for i in 0..data.shapes.len() {
            if i < self.shapes.len() {
                data.shapes[i].weight = self.shapes[i].filter(data.shapes[i].weight, dt);
            }
        }

        if let Some([yaw, pitch, roll, x, y, z]) = &mut self.head {
            let head = &mut data.head;
            head.head_yaw = yaw.filter(head.head_yaw, dt);
            head.head_pitch = pitch.filter(head.head_pitch, dt);
            head.head_roll = roll.filter(head.head_roll, dt);
            head.head_pos_x = x.filter(head.head_pos_x, dt);
            head.head_pos_y = y.filter(head.head_pos_y, dt);
            head.head_pos_z = z.filter(head.head_pos_z, dt);
        }
    }

    fn name(&self) -> &str {
        "Smoothing"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_filter_values_replace_the_preset() {
        let mut config = MutationConfig::default();
        config.mutator.smoothness = 0.5;
        assert_eq!(SmoothingMutation::params(&config), (0.2, 0.25, 0.1));

        config.mutator.filter.min_cutoff = Some(1.5);
        config.mutator.filter.d_cutoff = 0.3;
        assert_eq!(SmoothingMutation::params(&config), (1.5, 0.25, 0.3));

        config.mutator.filter.beta = Some(0.0);
        assert_eq!(SmoothingMutation::params(&config).1, 0.0);
    }

    #[test]
    fn unusable_raw_values_fall_back() {
        let mut config = MutationConfig::default();
        config.mutator.smoothness = 0.5;
        config.mutator.filter.min_cutoff = Some(0.0);
        config.mutator.filter.beta = Some(-1.0);
        config.mutator.filter.d_cutoff = f32::NAN;
        assert_eq!(SmoothingMutation::params(&config), (0.2, 0.25, 0.1));
    }

    /// Runs a step in head yaw through smoothing and returns the yaw after it.
    fn yaw_after_step(head: bool) -> f32 {
        let mut config = MutationConfig::default();
        config.mutator.smoothness = 0.8;
        config.mutator.filter.head = head;
        let mut smoothing = SmoothingMutation::new(&config);
        let mut data = UnifiedTrackingData::default();
        smoothing.mutate(&mut data, 1.0 / 60.0);
        data.head.head_yaw = 1.0;
        smoothing.mutate(&mut data, 1.0 / 60.0);
        data.head.head_yaw
    }

    #[test]
    fn head_pose_is_smoothed_unless_switched_off() {
        assert!(yaw_after_step(true) < 0.5);
        assert_eq!(yaw_after_step(false), 1.0);
    }
}
