use crate::mutation_trait::Mutation;
use crate::mutator::{CorrectorsConfig, MutationConfig};
use crate::{UnifiedExpressions as E, UnifiedTrackingData};

/// Pairs that the eyelid blend pulls towards each other, as (left, right).
const BLENDED_PAIRS: [(E, E); 6] = [
    (E::EyeWideLeft, E::EyeWideRight),
    (E::EyeSquintLeft, E::EyeSquintRight),
    (E::BrowPinchLeft, E::BrowPinchRight),
    (E::BrowLowererLeft, E::BrowLowererRight),
    (E::BrowInnerUpLeft, E::BrowInnerUpRight),
    (E::BrowOuterUpLeft, E::BrowOuterUpRight),
];

/// Each lip suck, and the opening on the same side that limits it.
const LIP_SUCK_LIMITS: [(E, E); 4] = [
    (E::LipSuckLowerLeft, E::MouthLowerDownLeft),
    (E::LipSuckLowerRight, E::MouthLowerDownRight),
    (E::LipSuckUpperLeft, E::MouthUpperUpLeft),
    (E::LipSuckUpperRight, E::MouthUpperUpRight),
];

/// VRCFaceTracking's "Unified Correctors": fixes combinations of shapes
/// that Unified Expressions doesn't allow, and optionally ties the two
/// sides of the upper face together.
pub struct CorrectorsMutation {
    config: CorrectorsConfig,
}

impl CorrectorsMutation {
    pub fn new(config: &MutationConfig) -> Self {
        Self {
            config: config.mutator.correctors.clone(),
        }
    }
}

fn set_weight(data: &mut UnifiedTrackingData, shape: E, value: f32) {
    if let Some(weight) = data.weight_mut(shape) {
        *weight = value;
    }
}

/// `left` and `right` each moved `blend / 2` of the way to the other, so a
/// blend of 1 makes both their average. Both use the values from before.
fn blend_pair(left: f32, right: f32, blend: f32) -> (f32, f32) {
    let other = blend * 0.5;
    (
        left * (1.0 - other) + right * other,
        right * (1.0 - other) + left * other,
    )
}

impl Mutation for CorrectorsMutation {
    fn mutate(&mut self, data: &mut UnifiedTrackingData, _dt: f32) {
        if self.config.mouth_closed_clamp {
            let closed = data.weight(E::MouthClosed).min(data.weight(E::JawOpen));
            set_weight(data, E::MouthClosed, closed);
        }

        if self.config.lip_suck_limiter {
            for (suck, opening) in LIP_SUCK_LIMITS {
                let limited = data.weight(suck) * (1.0 - data.weight(opening));
                set_weight(data, suck, limited);
            }
        }

        let blend = self.config.eyelid_blend;
        if blend.is_finite() && blend > 0.0 {
            let blend = blend.min(1.0);
            let eye = &mut data.eye;
            (eye.left.openness, eye.right.openness) =
                blend_pair(eye.left.openness, eye.right.openness, blend);
            (eye.left.pupil_diameter_mm, eye.right.pupil_diameter_mm) = blend_pair(
                eye.left.pupil_diameter_mm,
                eye.right.pupil_diameter_mm,
                blend,
            );
            for (left, right) in BLENDED_PAIRS {
                let (l, r) = blend_pair(data.weight(left), data.weight(right), blend);
                set_weight(data, left, l);
                set_weight(data, right, r);
            }
        }

        if self.config.eye_look_symmetrize {
            let y = (data.eye.left.gaze.y + data.eye.right.gaze.y) * 0.5;
            data.eye.left.gaze.y = y;
            data.eye.right.gaze.y = y;
        }
    }

    fn name(&self) -> &str {
        "Correctors"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mutation(config: CorrectorsConfig) -> CorrectorsMutation {
        CorrectorsMutation { config }
    }

    #[test]
    fn mouth_closed_never_exceeds_jaw_open() {
        let mut data = UnifiedTrackingData::default();
        set_weight(&mut data, E::MouthClosed, 0.8);
        set_weight(&mut data, E::JawOpen, 0.3);
        mutation(CorrectorsConfig::default()).mutate(&mut data, 0.016);
        assert_eq!(data.weight(E::MouthClosed), 0.3);
    }

    #[test]
    fn lip_suck_falls_as_the_lip_opens() {
        let mut data = UnifiedTrackingData::default();
        set_weight(&mut data, E::LipSuckLowerLeft, 1.0);
        set_weight(&mut data, E::MouthLowerDownLeft, 0.75);
        set_weight(&mut data, E::LipSuckUpperRight, 0.5);
        mutation(CorrectorsConfig::default()).mutate(&mut data, 0.016);
        assert_eq!(data.weight(E::LipSuckLowerLeft), 0.25);
        assert_eq!(data.weight(E::LipSuckUpperRight), 0.5);
    }

    #[test]
    fn switched_off_fixes_leave_the_data_alone() {
        let mut data = UnifiedTrackingData::default();
        set_weight(&mut data, E::MouthClosed, 0.8);
        set_weight(&mut data, E::LipSuckLowerLeft, 1.0);
        set_weight(&mut data, E::MouthLowerDownLeft, 1.0);
        let before = data.clone();
        mutation(CorrectorsConfig {
            mouth_closed_clamp: false,
            lip_suck_limiter: false,
            ..CorrectorsConfig::default()
        })
        .mutate(&mut data, 0.016);
        assert_eq!(data, before);
    }

    #[test]
    fn full_eyelid_blend_averages_both_sides_symmetrically() {
        let mut data = UnifiedTrackingData::default();
        data.eye.left.openness = 1.0;
        data.eye.right.openness = 0.0;
        data.eye.left.pupil_diameter_mm = 3.0;
        data.eye.right.pupil_diameter_mm = 5.0;
        set_weight(&mut data, E::BrowInnerUpLeft, 0.2);
        set_weight(&mut data, E::BrowInnerUpRight, 0.6);
        mutation(CorrectorsConfig {
            eyelid_blend: 1.0,
            ..CorrectorsConfig::default()
        })
        .mutate(&mut data, 0.016);
        assert_eq!(data.eye.left.openness, 0.5);
        assert_eq!(data.eye.right.openness, 0.5);
        // Pupils are in millimetres, so they must not be clamped to 0..1.
        assert_eq!(data.eye.left.pupil_diameter_mm, 4.0);
        assert_eq!(data.eye.right.pupil_diameter_mm, 4.0);
        assert!((data.weight(E::BrowInnerUpLeft) - 0.4).abs() < 1e-6);
        assert!((data.weight(E::BrowInnerUpRight) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn half_eyelid_blend_moves_each_side_a_quarter_of_the_way() {
        let (l, r) = blend_pair(1.0, 0.0, 0.5);
        assert_eq!((l, r), (0.75, 0.25));
    }

    #[test]
    fn symmetrize_averages_vertical_gaze_only() {
        let mut data = UnifiedTrackingData::default();
        data.eye.left.gaze = glam::Vec2::new(0.1, 0.2);
        data.eye.right.gaze = glam::Vec2::new(-0.3, 0.4);
        mutation(CorrectorsConfig {
            eye_look_symmetrize: true,
            ..CorrectorsConfig::default()
        })
        .mutate(&mut data, 0.016);
        assert!((data.eye.left.gaze.y - 0.3).abs() < 1e-6);
        assert!((data.eye.right.gaze.y - 0.3).abs() < 1e-6);
        assert_eq!(data.eye.left.gaze.x, 0.1);
        assert_eq!(data.eye.right.gaze.x, -0.3);
    }
}
