use crate::mutation_trait::Mutation;
use crate::mutator::MutationConfig;
use crate::{UnifiedExpressions as E, UnifiedHeadData, UnifiedTrackingData};
use anyhow::Result;
use log::warn;

/// One head pose value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadAxis {
    Yaw,
    Pitch,
    Roll,
    PosX,
    PosY,
    PosZ,
}

impl HeadAxis {
    fn value(self, head: &mut UnifiedHeadData) -> &mut f32 {
        match self {
            Self::Yaw => &mut head.head_yaw,
            Self::Pitch => &mut head.head_pitch,
            Self::Roll => &mut head.head_roll,
            Self::PosX => &mut head.head_pos_x,
            Self::PosY => &mut head.head_pos_y,
            Self::PosZ => &mut head.head_pos_z,
        }
    }
}

/// What an adjustment group remaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustmentTarget {
    /// Shape weights, from 0 to 1.
    Shapes(&'static [E]),
    /// A head pose value, from -1 to 1.
    Head(HeadAxis),
}

impl AdjustmentTarget {
    /// The full range, which is also the `[floor, ceil]` that changes nothing.
    pub fn full_range(self) -> [f32; 2] {
        match self {
            Self::Shapes(_) => [0.0, 1.0],
            Self::Head(_) => [-1.0, 1.0],
        }
    }
}

/// A set of values that share one `[floor, ceil]`, keyed in
/// `mutator.adjustment.ranges` by `key`.
#[derive(Debug, Clone, Copy)]
pub struct AdjustmentGroup {
    pub key: &'static str,
    pub label: &'static str,
    pub target: AdjustmentTarget,
}

const fn shapes(key: &'static str, label: &'static str, shapes: &'static [E]) -> AdjustmentGroup {
    AdjustmentGroup {
        key,
        label,
        target: AdjustmentTarget::Shapes(shapes),
    }
}

const fn head(key: &'static str, label: &'static str, axis: HeadAxis) -> AdjustmentGroup {
    AdjustmentGroup {
        key,
        label,
        target: AdjustmentTarget::Head(axis),
    }
}

/// VRCFaceTracking's Parameter Adjustment groups, with the same shapes in
/// each. `mouth_dimple` is ours: upstream leaves the dimples out.
pub const ADJUSTMENT_GROUPS: &[AdjustmentGroup] = &[
    shapes(
        "brow_raiser",
        "Eyebrow Raiser",
        &[
            E::BrowInnerUpLeft,
            E::BrowInnerUpRight,
            E::BrowOuterUpLeft,
            E::BrowOuterUpRight,
        ],
    ),
    shapes(
        "brow_lowerer",
        "Eyebrow Lowerer",
        &[
            E::BrowLowererLeft,
            E::BrowLowererRight,
            E::BrowPinchLeft,
            E::BrowPinchRight,
        ],
    ),
    shapes(
        "eye_squint",
        "Eye Squint",
        &[E::EyeSquintLeft, E::EyeSquintRight],
    ),
    shapes("eye_wide", "Eye Wide", &[E::EyeWideLeft, E::EyeWideRight]),
    shapes(
        "cheek",
        "Cheek Puff / Suck",
        &[
            E::CheekPuffLeft,
            E::CheekPuffRight,
            E::CheekSuckLeft,
            E::CheekSuckRight,
        ],
    ),
    shapes(
        "cheek_squint",
        "Cheek Squint",
        &[E::CheekSquintLeft, E::CheekSquintRight],
    ),
    shapes(
        "jaw",
        "Jaw",
        &[E::JawOpen, E::JawClench, E::JawMandibleRaise],
    ),
    shapes("mouth_closed", "Mouth Closed", &[E::MouthClosed]),
    shapes("jaw_sideways", "Jaw Sideways", &[E::JawLeft, E::JawRight]),
    shapes(
        "jaw_forward_backward",
        "Jaw Forward / Backward",
        &[E::JawForward, E::JawBackward],
    ),
    shapes(
        "lip_funnel",
        "Lip Funnel",
        &[
            E::LipFunnelLowerLeft,
            E::LipFunnelLowerRight,
            E::LipFunnelUpperLeft,
            E::LipFunnelUpperRight,
        ],
    ),
    shapes(
        "lip_suck",
        "Lip Suck",
        &[
            E::LipSuckCornerLeft,
            E::LipSuckCornerRight,
            E::LipSuckUpperLeft,
            E::LipSuckUpperRight,
            E::LipSuckLowerLeft,
            E::LipSuckLowerRight,
        ],
    ),
    shapes(
        "lip_pucker",
        "Lip Pucker",
        &[
            E::LipPuckerLowerLeft,
            E::LipPuckerLowerRight,
            E::LipPuckerUpperLeft,
            E::LipPuckerUpperRight,
        ],
    ),
    shapes(
        "mouth_open",
        "Mouth Open",
        &[
            E::MouthUpperDeepenLeft,
            E::MouthUpperDeepenRight,
            E::MouthUpperUpLeft,
            E::MouthUpperUpRight,
            E::MouthLowerDownLeft,
            E::MouthLowerDownRight,
        ],
    ),
    shapes(
        "mouth_smile",
        "Mouth Smile",
        &[
            E::MouthCornerPullLeft,
            E::MouthCornerPullRight,
            E::MouthCornerSlantLeft,
            E::MouthCornerSlantRight,
        ],
    ),
    shapes(
        "mouth_frown",
        "Mouth Frown",
        &[E::MouthFrownLeft, E::MouthFrownRight],
    ),
    shapes(
        "mouth_stretch",
        "Mouth Stretch",
        &[E::MouthStretchLeft, E::MouthStretchRight],
    ),
    shapes(
        "mouth_dimple",
        "Mouth Dimple",
        &[E::MouthDimpleLeft, E::MouthDimpleRight],
    ),
    shapes(
        "mouth_tightener",
        "Mouth Tightener",
        &[E::MouthTightenerLeft, E::MouthTightenerRight],
    ),
    shapes(
        "mouth_press",
        "Mouth Press",
        &[E::MouthPressLeft, E::MouthPressRight],
    ),
    shapes(
        "mouth_sideways",
        "Mouth Sideways",
        &[
            E::MouthUpperLeft,
            E::MouthUpperRight,
            E::MouthLowerLeft,
            E::MouthLowerRight,
        ],
    ),
    shapes(
        "mouth_raiser",
        "Mouth Raiser",
        &[E::MouthRaiserLower, E::MouthRaiserUpper],
    ),
    shapes(
        "nose",
        "Nose",
        &[
            E::NasalConstrictLeft,
            E::NasalConstrictRight,
            E::NasalDilationLeft,
            E::NasalDilationRight,
        ],
    ),
    shapes(
        "nose_sneer",
        "Nose Sneer",
        &[E::NoseSneerLeft, E::NoseSneerRight],
    ),
    shapes(
        "neck",
        "Neck",
        &[
            E::NeckFlexLeft,
            E::NeckFlexRight,
            E::SoftPalateClose,
            E::ThroatSwallow,
        ],
    ),
    shapes("tongue_out", "Tongue Out", &[E::TongueOut]),
    shapes(
        "tongue_directions",
        "Tongue Directions",
        &[
            E::TongueBendDown,
            E::TongueCurlUp,
            E::TongueDown,
            E::TongueUp,
            E::TongueLeft,
            E::TongueRight,
        ],
    ),
    shapes(
        "tongue_other",
        "Tongue Miscellaneous",
        &[
            E::TongueTwistLeft,
            E::TongueTwistRight,
            E::TongueFlat,
            E::TongueSquish,
            E::TongueRoll,
        ],
    ),
    head("head_yaw", "Head Rotation (Side-to-Side)", HeadAxis::Yaw),
    head(
        "head_pitch",
        "Head Rotation (Up-Down Tilt)",
        HeadAxis::Pitch,
    ),
    head("head_roll", "Head Rotation (Side Tilt)", HeadAxis::Roll),
    head("head_pos_x", "Head Position (Side-to-Side)", HeadAxis::PosX),
    head("head_pos_y", "Head Position (Up-Down)", HeadAxis::PosY),
    head("head_pos_z", "Head Position (Forward-Back)", HeadAxis::PosZ),
];

/// The group with `key`.
pub fn adjustment_group(key: &str) -> Option<&'static AdjustmentGroup> {
    ADJUSTMENT_GROUPS.iter().find(|group| group.key == key)
}

/// Whether `[floor, ceil]` can be used: finite, with `floor` below `ceil`.
pub fn valid_range([floor, ceil]: [f32; 2]) -> bool {
    floor.is_finite() && ceil.is_finite() && floor < ceil
}

struct Adjustment {
    target: AdjustmentTarget,
    floor: f32,
    ceil: f32,
}

/// VRCFaceTracking's "Parameter Adjustment": stretches each configured
/// group's `[floor, ceil]` to its full range.
///
/// Unlike upstream, results are clamped to the range VRChat expects, and a
/// head value is mapped onto -1..1 (upstream maps it onto 0..1, which moves
/// the head even at the default range).
pub struct AdjustmentMutation {
    adjustments: Vec<Adjustment>,
}

impl AdjustmentMutation {
    pub fn new(config: &MutationConfig) -> Self {
        let mut adjustments = Vec::new();
        for (key, &range) in &config.mutator.adjustment.ranges {
            let Some(group) = adjustment_group(key) else {
                warn!("Ignoring unknown adjustment group '{key}'");
                continue;
            };
            if !valid_range(range) {
                warn!("Ignoring adjustment '{key}': {range:?} needs a floor below its ceil");
                continue;
            }
            if range == group.target.full_range() {
                continue;
            }
            let [floor, ceil] = range;
            adjustments.push(Adjustment {
                target: group.target,
                floor,
                ceil,
            });
        }
        Self { adjustments }
    }
}

impl Mutation for AdjustmentMutation {
    fn initialize(&mut self, config: &MutationConfig) -> Result<()> {
        *self = Self::new(config);
        Ok(())
    }

    fn mutate(&mut self, data: &mut UnifiedTrackingData, _dt: f32) {
        for adjustment in &self.adjustments {
            let unit =
                |value: f32| (value - adjustment.floor) / (adjustment.ceil - adjustment.floor);
            match adjustment.target {
                AdjustmentTarget::Shapes(shapes) => {
                    for &shape in shapes {
                        if let Some(s) = data.shapes.get_mut(shape as usize) {
                            s.weight = unit(s.weight).clamp(0.0, 1.0);
                        }
                    }
                }
                AdjustmentTarget::Head(axis) => {
                    let value = axis.value(&mut data.head);
                    *value = (unit(*value) * 2.0 - 1.0).clamp(-1.0, 1.0);
                }
            }
        }
    }

    fn name(&self) -> &str {
        "Adjustment"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adjust(ranges: &[(&str, [f32; 2])], data: &mut UnifiedTrackingData) {
        let mut config = MutationConfig::default();
        config.mutator.adjustment.ranges = ranges
            .iter()
            .map(|(key, range)| (key.to_string(), *range))
            .collect();
        AdjustmentMutation::new(&config).mutate(data, 0.016);
    }

    #[test]
    fn group_keys_are_unique_and_cover_every_shape() {
        let mut keys: Vec<_> = ADJUSTMENT_GROUPS.iter().map(|g| g.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), ADJUSTMENT_GROUPS.len());

        let mut covered = vec![0; E::Max as usize];
        for group in ADJUSTMENT_GROUPS {
            if let AdjustmentTarget::Shapes(shapes) = group.target {
                for &shape in shapes {
                    covered[shape as usize] += 1;
                }
            }
        }
        assert!(
            covered.iter().all(|&count| count == 1),
            "each shape must be in exactly one group: {covered:?}"
        );
    }

    #[test]
    fn a_shape_range_is_stretched_to_the_full_range_and_clamped() {
        let mut data = UnifiedTrackingData::default();
        data.shapes[E::JawOpen as usize].weight = 0.4;
        data.shapes[E::JawClench as usize].weight = 0.9;
        data.shapes[E::MouthClosed as usize].weight = 0.4;
        adjust(&[("jaw", [0.0, 0.8])], &mut data);
        assert!((data.shapes[E::JawOpen as usize].weight - 0.5).abs() < 1e-6);
        assert_eq!(data.shapes[E::JawClench as usize].weight, 1.0);
        assert_eq!(data.shapes[E::MouthClosed as usize].weight, 0.4);
    }

    #[test]
    fn a_raised_floor_cuts_out_small_values() {
        let mut data = UnifiedTrackingData::default();
        data.shapes[E::EyeWideLeft as usize].weight = 0.1;
        data.shapes[E::EyeWideRight as usize].weight = 0.6;
        adjust(&[("eye_wide", [0.2, 1.0])], &mut data);
        assert_eq!(data.shapes[E::EyeWideLeft as usize].weight, 0.0);
        assert!((data.shapes[E::EyeWideRight as usize].weight - 0.5).abs() < 1e-6);
    }

    #[test]
    fn head_ranges_map_onto_minus_one_to_one() {
        let mut data = UnifiedTrackingData::default();
        data.head.head_yaw = 0.25;
        data.head.head_pitch = 0.3;
        adjust(
            &[("head_yaw", [-0.5, 0.5]), ("head_pitch", [-1.0, 1.0])],
            &mut data,
        );
        assert!((data.head.head_yaw - 0.5).abs() < 1e-6);
        assert_eq!(data.head.head_pitch, 0.3);
    }

    #[test]
    fn unknown_or_invalid_ranges_are_ignored() {
        let mut data = UnifiedTrackingData::default();
        data.shapes[E::JawOpen as usize].weight = 0.4;
        let before = data.clone();
        adjust(
            &[
                ("not_a_group", [0.0, 0.5]),
                ("jaw", [0.5, 0.5]),
                ("tongue_out", [f32::NAN, 1.0]),
            ],
            &mut data,
        );
        assert_eq!(data, before);
    }
}
