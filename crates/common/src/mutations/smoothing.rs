use crate::euro_filter::DEFAULT_D_CUTOFF;
use crate::mutation_trait::Mutation;
use crate::mutator::MutationConfig;
use crate::{EuroFilter, UnifiedExpressions, UnifiedTrackingData};
use log::warn;

pub struct SmoothingMutation {
    shapes: Vec<EuroFilter>,
    /// The left eye's, then the right's, in `UnifiedSingleEyeData::values_mut`
    /// order.
    eyes: [[EuroFilter; 4]; 2],
    /// In `UnifiedHeadData::values_mut` order; `None` when head pose isn't
    /// smoothed.
    head: Option<[EuroFilter; 6]>,
}

impl SmoothingMutation {
    pub fn new(config: &MutationConfig) -> Self {
        let (min_cutoff, beta, d_cutoff) = Self::params(config);
        let filter = EuroFilter::new_with_params(min_cutoff, beta, d_cutoff);

        Self {
            shapes: vec![filter; UnifiedExpressions::Max as usize],
            eyes: [[filter; 4]; 2],
            head: config.mutator.filter.head.then_some([filter; 6]),
        }
    }

    /// `min_cutoff` and `beta` for a `smoothness` preset. Values outside
    /// [0, 1] are clamped: above 1, beta would go negative and the filter
    /// would run away from fast moves.
    fn calculate_params(smoothness: f32) -> (f32, f32) {
        let smoothness = if smoothness.is_nan() {
            0.0
        } else {
            smoothness.clamp(0.0, 1.0)
        };
        if smoothness == 0.0 {
            return (10.0, 1.0);
        }
        (1.0 / (smoothness * 10.0), 0.5 * (1.0 - smoothness))
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
            pick(
                "d_cutoff",
                Some(filter.d_cutoff),
                positive,
                DEFAULT_D_CUTOFF,
            ),
        )
    }
}

/// Runs each value through its own filter.
fn filter_each<'a>(
    filters: &mut [EuroFilter],
    values: impl IntoIterator<Item = &'a mut f32>,
    dt: f32,
) {
    for (filter, value) in filters.iter_mut().zip(values) {
        *value = filter.filter(*value, dt);
    }
}

impl Mutation for SmoothingMutation {
    fn mutate(&mut self, data: &mut UnifiedTrackingData, dt: f32) {
        let [left, right] = &mut self.eyes;
        filter_each(left, data.eye.left.values_mut(), dt);
        filter_each(right, data.eye.right.values_mut(), dt);
        filter_each(
            &mut self.shapes,
            data.shapes.iter_mut().map(|shape| &mut shape.weight),
            dt,
        );
        if let Some(head) = &mut self.head {
            filter_each(head, data.head.values_mut(), dt);
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

    #[test]
    fn out_of_range_smoothness_is_clamped() {
        assert_eq!(SmoothingMutation::calculate_params(2.0), (0.1, 0.0));
        assert_eq!(SmoothingMutation::calculate_params(-1.0), (10.0, 1.0));
        assert_eq!(SmoothingMutation::calculate_params(f32::NAN), (10.0, 1.0));
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
