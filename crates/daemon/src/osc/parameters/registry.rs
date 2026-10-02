use super::base_param::BoolParam;
use super::eparam::EParam;
use super::legacy_eye::create_legacy_eye_parameters;
use super::legacy_lip::create_legacy_lip_parameters;
use super::native_param::create_native_parameters;
use super::{ParamType, Parameter};
use rosc::OscMessage;
use std::collections::{HashMap, HashSet};
use vrft_common::{UnifiedExpressions as E, UnifiedTrackingData};

// Brow Simple Shapes
fn brow_up_right(d: &UnifiedTrackingData) -> f32 {
    d.weight(E::BrowOuterUpRight) * 0.6 + d.weight(E::BrowInnerUpRight) * 0.4
}
fn brow_up_left(d: &UnifiedTrackingData) -> f32 {
    d.weight(E::BrowOuterUpLeft) * 0.6 + d.weight(E::BrowInnerUpLeft) * 0.4
}
fn brow_down_right(d: &UnifiedTrackingData) -> f32 {
    d.weight(E::BrowLowererRight) * 0.75 + d.weight(E::BrowPinchRight) * 0.25
}
fn brow_down_left(d: &UnifiedTrackingData) -> f32 {
    d.weight(E::BrowLowererLeft) * 0.75 + d.weight(E::BrowPinchLeft) * 0.25
}

// Mouth Simple Shapes
fn mouth_smile_right(d: &UnifiedTrackingData) -> f32 {
    d.weight(E::MouthCornerPullRight) * 0.8 + d.weight(E::MouthCornerSlantRight) * 0.2
}
fn mouth_smile_left(d: &UnifiedTrackingData) -> f32 {
    d.weight(E::MouthCornerPullLeft) * 0.8 + d.weight(E::MouthCornerSlantLeft) * 0.2
}
fn mouth_sad_right(d: &UnifiedTrackingData) -> f32 {
    d.weight(E::MouthFrownRight)
        .max(d.weight(E::MouthStretchRight))
}
fn mouth_sad_left(d: &UnifiedTrackingData) -> f32 {
    d.weight(E::MouthFrownLeft)
        .max(d.weight(E::MouthStretchLeft))
}

/// A shape computed from tracking data.
type ShapeFn = fn(&UnifiedTrackingData) -> f32;

/// The simple shapes above, also sent on their own.
const SIMPLE_EXPRESSIONS: &[(&str, ShapeFn)] = &[
    ("v2/BrowUpRight", brow_up_right),
    ("v2/BrowUpLeft", brow_up_left),
    ("v2/BrowDownRight", brow_down_right),
    ("v2/BrowDownLeft", brow_down_left),
    ("v2/MouthSmileRight", mouth_smile_right),
    ("v2/MouthSmileLeft", mouth_smile_left),
    ("v2/MouthSadRight", mouth_sad_right),
    ("v2/MouthSadLeft", mouth_sad_left),
];

/// How a combined parameter is computed from the shapes.
#[derive(Clone, Copy)]
enum Combined {
    /// `(a + b) / 2`
    Average(E, E),
    /// `a - b`
    Difference(E, E),
    Custom(ShapeFn),
}
use Combined::{Average, Custom, Difference};

/// The combined v2 parameters, in registration order.
#[rustfmt::skip]
const COMBINED: &[(&str, Combined)] = &[
    // Eyebrows Compacted
    ("v2/BrowUp", Custom(|d| (brow_up_right(d) + brow_up_left(d)) * 0.5)),
    ("v2/BrowDown", Custom(|d| (brow_down_right(d) + brow_down_left(d)) * 0.5)),
    ("v2/BrowInnerUp", Average(E::BrowInnerUpLeft, E::BrowInnerUpRight)),
    ("v2/BrowOuterUp", Average(E::BrowOuterUpLeft, E::BrowOuterUpRight)),
    ("v2/BrowExpressionRight", Custom(|d| {
        (d.weight(E::BrowInnerUpRight) * 0.5 + d.weight(E::BrowOuterUpRight) * 0.5).min(1.0)
            - brow_down_right(d)
    })),
    ("v2/BrowExpressionLeft", Custom(|d| {
        (d.weight(E::BrowInnerUpLeft) * 0.5 + d.weight(E::BrowOuterUpLeft) * 0.5).min(1.0)
            - brow_down_left(d)
    })),
    ("v2/BrowExpression", Custom(|d| {
        let right = (d.weight(E::BrowInnerUpRight) + d.weight(E::BrowOuterUpRight)) * 0.5;
        let left = (d.weight(E::BrowInnerUpLeft) + d.weight(E::BrowOuterUpLeft)) * 0.5;
        (right.min(1.0) - brow_down_right(d) + left.min(1.0) - brow_down_left(d)) * 0.5
    })),

    // Jaw Combined
    ("v2/JawX", Difference(E::JawRight, E::JawLeft)),
    ("v2/JawZ", Difference(E::JawForward, E::JawBackward)),

    // Cheeks Combined
    ("v2/CheekSquint", Average(E::CheekSquintLeft, E::CheekSquintRight)),
    ("v2/CheekPuffSuckLeft", Difference(E::CheekPuffLeft, E::CheekSuckLeft)),
    ("v2/CheekPuffSuckRight", Difference(E::CheekPuffRight, E::CheekSuckRight)),
    ("v2/CheekPuffSuck", Custom(|d| {
        (d.weight(E::CheekPuffRight) + d.weight(E::CheekPuffLeft)) / 2.0
            - (d.weight(E::CheekSuckRight) + d.weight(E::CheekSuckLeft)) / 2.0
    })),
    ("v2/CheekSuck", Average(E::CheekSuckLeft, E::CheekSuckRight)),

    // Mouth Direction
    ("v2/MouthUpperX", Difference(E::MouthUpperRight, E::MouthUpperLeft)),
    ("v2/MouthLowerX", Difference(E::MouthLowerRight, E::MouthLowerLeft)),
    ("v2/MouthX", Custom(|d| {
        (d.weight(E::MouthUpperRight) + d.weight(E::MouthLowerRight)) / 2.0
            - (d.weight(E::MouthUpperLeft) + d.weight(E::MouthLowerLeft)) / 2.0
    })),

    // Lip Combined
    ("v2/LipSuckUpper", Average(E::LipSuckUpperRight, E::LipSuckUpperLeft)),
    ("v2/LipSuckLower", Average(E::LipSuckLowerRight, E::LipSuckLowerLeft)),
    ("v2/LipSuck", Custom(|d| {
        (d.weight(E::LipSuckUpperRight)
            + d.weight(E::LipSuckUpperLeft)
            + d.weight(E::LipSuckLowerRight)
            + d.weight(E::LipSuckLowerLeft))
            / 4.0
    })),

    ("v2/LipFunnelUpper", Average(E::LipFunnelUpperRight, E::LipFunnelUpperLeft)),
    ("v2/LipFunnelLower", Average(E::LipFunnelLowerRight, E::LipFunnelLowerLeft)),
    ("v2/LipFunnel", Custom(|d| {
        (d.weight(E::LipFunnelUpperRight)
            + d.weight(E::LipFunnelUpperLeft)
            + d.weight(E::LipFunnelLowerRight)
            + d.weight(E::LipFunnelLowerLeft))
            / 4.0
    })),

    ("v2/LipPuckerUpper", Average(E::LipPuckerUpperRight, E::LipPuckerUpperLeft)),
    ("v2/LipPuckerLower", Average(E::LipPuckerLowerRight, E::LipPuckerLowerLeft)),
    ("v2/LipPuckerRight", Average(E::LipPuckerUpperRight, E::LipPuckerLowerRight)),
    ("v2/LipPuckerLeft", Average(E::LipPuckerUpperLeft, E::LipPuckerLowerLeft)),
    ("v2/LipPucker", Custom(|d| {
        (d.weight(E::LipPuckerUpperRight)
            + d.weight(E::LipPuckerUpperLeft)
            + d.weight(E::LipPuckerLowerRight)
            + d.weight(E::LipPuckerLowerLeft))
            / 4.0
    })),

    // Lip Suck/Funnel compacted
    ("v2/LipSuckFunnelUpper", Custom(|d| {
        (d.weight(E::LipSuckUpperRight) + d.weight(E::LipSuckUpperLeft)) / 2.0
            - (d.weight(E::LipFunnelUpperRight) + d.weight(E::LipFunnelUpperLeft)) / 2.0
    })),
    ("v2/LipSuckFunnelLower", Custom(|d| {
        (d.weight(E::LipSuckLowerRight) + d.weight(E::LipSuckLowerLeft)) / 2.0
            - (d.weight(E::LipFunnelLowerRight) + d.weight(E::LipFunnelLowerLeft)) / 2.0
    })),
    ("v2/LipSuckFunnelLowerLeft", Difference(E::LipSuckLowerLeft, E::LipFunnelLowerLeft)),
    ("v2/LipSuckFunnelLowerRight", Difference(E::LipSuckLowerRight, E::LipFunnelLowerRight)),
    ("v2/LipSuckFunnelUpperLeft", Difference(E::LipSuckUpperLeft, E::LipFunnelUpperLeft)),
    ("v2/LipSuckFunnelUpperRight", Difference(E::LipSuckUpperRight, E::LipFunnelUpperRight)),

    // Mouth Combined
    ("v2/MouthUpperUp", Average(E::MouthUpperUpRight, E::MouthUpperUpLeft)),
    ("v2/MouthLowerDown", Average(E::MouthLowerDownRight, E::MouthLowerDownLeft)),
    ("v2/MouthOpen", Custom(|d| {
        d.weight(E::MouthUpperUpRight) * 0.25
            + d.weight(E::MouthUpperUpLeft) * 0.25
            + d.weight(E::MouthLowerDownRight) * 0.25
            + d.weight(E::MouthLowerDownLeft) * 0.25
    })),

    ("v2/MouthStretch", Average(E::MouthStretchRight, E::MouthStretchLeft)),
    ("v2/MouthTightener", Average(E::MouthTightenerRight, E::MouthTightenerLeft)),
    ("v2/MouthPress", Average(E::MouthPressRight, E::MouthPressLeft)),
    ("v2/MouthDimple", Average(E::MouthDimpleRight, E::MouthDimpleLeft)),
    ("v2/NoseSneer", Average(E::NoseSneerRight, E::NoseSneerLeft)),

    // Mouth compacted
    ("v2/MouthTightenerStretch", Custom(|d| {
        (d.weight(E::MouthTightenerRight) + d.weight(E::MouthTightenerLeft)) / 2.0
            - (d.weight(E::MouthStretchRight) + d.weight(E::MouthStretchLeft)) / 2.0
    })),
    ("v2/MouthTightenerStretchLeft", Difference(E::MouthTightenerLeft, E::MouthStretchLeft)),
    ("v2/MouthTightenerStretchRight", Difference(E::MouthTightenerRight, E::MouthStretchRight)),

    // Lip Corners Combined
    ("v2/MouthCornerYLeft", Difference(E::MouthCornerSlantLeft, E::MouthFrownLeft)),
    ("v2/MouthCornerYRight", Difference(E::MouthCornerSlantRight, E::MouthFrownRight)),
    ("v2/MouthCornerY", Custom(|d| {
        (d.weight(E::MouthCornerSlantLeft) - d.weight(E::MouthFrownLeft)
            + d.weight(E::MouthCornerSlantRight)
            - d.weight(E::MouthFrownRight))
            * 0.5
    })),

    // SmileFrown
    ("v2/SmileFrownRight", Custom(|d| mouth_smile_right(d) - d.weight(E::MouthFrownRight))),
    ("v2/SmileFrownLeft", Custom(|d| mouth_smile_left(d) - d.weight(E::MouthFrownLeft))),
    ("v2/SmileFrown", Custom(|d| {
        mouth_smile_right(d) * 0.5 + mouth_smile_left(d) * 0.5
            - d.weight(E::MouthFrownRight) * 0.5
            - d.weight(E::MouthFrownLeft) * 0.5
    })),

    // SmileSad
    ("v2/SmileSadRight", Custom(|d| mouth_smile_right(d) - mouth_sad_right(d))),
    ("v2/SmileSadLeft", Custom(|d| mouth_smile_left(d) - mouth_sad_left(d))),
    ("v2/SmileSad", Custom(|d| {
        (mouth_smile_left(d) + mouth_smile_right(d)) / 2.0
            - (mouth_sad_left(d) + mouth_sad_right(d)) / 2.0
    })),

    // Tongue Combined
    ("v2/TongueX", Difference(E::TongueRight, E::TongueLeft)),
    ("v2/TongueY", Difference(E::TongueUp, E::TongueDown)),
    ("v2/TongueArchY", Difference(E::TongueCurlUp, E::TongueBendDown)),
    ("v2/TongueShape", Difference(E::TongueFlat, E::TongueSquish)),
];

pub struct ParameterRegistry {
    parameters: Vec<Box<dyn Parameter>>,
}

impl ParameterRegistry {
    pub fn new() -> Self {
        let mut parameters: Vec<Box<dyn Parameter>> = Vec::new();

        // Head Tracking
        parameters.push(Box::new(EParam::simple("v2/Head/Yaw", |d| d.head.head_yaw)));
        parameters.push(Box::new(EParam::simple("v2/Head/Pitch", |d| {
            d.head.head_pitch
        })));
        parameters.push(Box::new(EParam::simple("v2/Head/Roll", |d| {
            d.head.head_roll
        })));
        parameters.push(Box::new(EParam::simple("v2/Head/PosX", |d| {
            d.head.head_pos_x
        })));
        parameters.push(Box::new(EParam::simple("v2/Head/PosY", |d| {
            d.head.head_pos_y
        })));
        parameters.push(Box::new(EParam::simple("v2/Head/PosZ", |d| {
            d.head.head_pos_z
        })));

        // Eye Gaze
        parameters.push(Box::new(EParam::simple("v2/EyeLeftX", |d| {
            d.eye.left.gaze.x
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeLeftY", |d| {
            d.eye.left.gaze.y
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeRightX", |d| {
            d.eye.right.gaze.x
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeRightY", |d| {
            d.eye.right.gaze.y
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeX", |d| {
            (d.eye.left.gaze.x + d.eye.right.gaze.x) / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeY", |d| {
            (d.eye.left.gaze.y + d.eye.right.gaze.y) / 2.0
        })));

        // Eye Pupils
        parameters.push(Box::new(EParam::simple("v2/PupilDilation", |d| {
            d.eye.dilation()
        })));
        parameters.push(Box::new(EParam::simple("v2/PupilDiameterLeft", |d| {
            d.eye.left.pupil_diameter_mm * 0.1
        })));
        parameters.push(Box::new(EParam::simple("v2/PupilDiameterRight", |d| {
            d.eye.right.pupil_diameter_mm * 0.1
        })));
        parameters.push(Box::new(EParam::simple("v2/PupilDiameter", |d| {
            (d.eye.left.pupil_diameter_mm + d.eye.right.pupil_diameter_mm) * 0.05
        })));

        // Eye Openness
        parameters.push(Box::new(EParam::simple("v2/EyeOpenLeft", |d| {
            d.eye.left.openness
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeOpenRight", |d| {
            d.eye.right.openness
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeOpen", |d| {
            (d.eye.left.openness + d.eye.right.openness) / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeClosedLeft", |d| {
            1.0 - d.eye.left.openness
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeClosedRight", |d| {
            1.0 - d.eye.right.openness
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeClosed", |d| {
            1.0 - (d.eye.left.openness + d.eye.right.openness) / 2.0
        })));

        // Eye Wide
        parameters.push(Box::new(EParam::simple("v2/EyeWide", |d| {
            d.weight(E::EyeWideLeft).max(d.weight(E::EyeWideRight))
        })));

        // Eye Lid
        parameters.push(Box::new(EParam::simple("v2/EyeLidLeft", |d| {
            d.eye.left.openness * 0.75 + d.weight(E::EyeWideLeft) * 0.25
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeLidRight", |d| {
            d.eye.right.openness * 0.75 + d.weight(E::EyeWideRight) * 0.25
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeLid", |d| {
            ((d.eye.left.openness + d.eye.right.openness) / 2.0) * 0.75
                + ((d.weight(E::EyeWideRight) + d.weight(E::EyeWideLeft)) / 2.0) * 0.25
        })));

        // Eye Squint
        parameters.push(Box::new(EParam::simple("v2/EyeSquint", |d| {
            d.weight(E::EyeSquintLeft).max(d.weight(E::EyeSquintRight))
        })));
        parameters.push(Box::new(EParam::simple("v2/EyesSquint", |d| {
            d.weight(E::EyeSquintLeft).max(d.weight(E::EyeSquintRight))
        })));

        // Combined Shapes
        for &(name, formula) in COMBINED {
            let param = match formula {
                Average(a, b) => EParam::simple(name, move |d| (d.weight(a) + d.weight(b)) / 2.0),
                Difference(a, b) => EParam::simple(name, move |d| d.weight(a) - d.weight(b)),
                Custom(get_value) => EParam::simple(name, get_value),
            };
            parameters.push(Box::new(param));
        }

        // All Base Expressions (v2/{ExpressionName})
        // Generate EParam for each UnifiedExpression
        for expr in (0..E::Max as usize).filter_map(|i| E::try_from(i).ok()) {
            parameters.push(Box::new(EParam::expression(
                &format!("v2/{:?}", expr),
                move |d| d.weight(expr),
            )));
        }

        // v2/ Simple Expressions (threshold 0.0 per reference — bool always false)
        for &(name, get_value) in SIMPLE_EXPRESSIONS {
            parameters.push(Box::new(EParam::expression(name, get_value)));
        }

        // Legacy Eye Parameters
        parameters.extend(create_legacy_eye_parameters());

        // Legacy SRanipal Lip Parameters
        parameters.extend(create_legacy_lip_parameters());

        // Native Tracking Paths
        parameters.extend(create_native_parameters());

        // Status Indicators
        // Like every parameter, these send on the first frame after an avatar
        // loads, so the avatar learns the tracking state straight away.

        // Eye tracking active: true if we have valid gaze data
        // Check if gaze values are non-zero or pupil has valid diameter
        parameters.push(Box::new(BoolParam::new("EyeTrackingActive", |d| {
            // Consider eye tracking active if we have any non-default gaze or pupil data
            d.eye.left.gaze.x != 0.0
                || d.eye.left.gaze.y != 0.0
                || d.eye.right.gaze.x != 0.0
                || d.eye.right.gaze.y != 0.0
                || d.eye.left.pupil_diameter_mm > 0.1
                || d.eye.right.pupil_diameter_mm > 0.1
        })));

        // Expression tracking active: true if any expression weights are active
        parameters.push(Box::new(BoolParam::new("ExpressionTrackingActive", |d| {
            // Check if any expression weight is above threshold
            d.shapes.iter().any(|s| s.weight > 0.01)
        })));

        // Lip tracking active: based on mouth-related expression activity
        parameters.push(Box::new(BoolParam::new("LipTrackingActive", |d| {
            // Check mouth/jaw expressions specifically
            [
                E::JawOpen,
                E::MouthClosed,
                E::MouthCornerPullLeft,
                E::MouthCornerPullRight,
                E::MouthFrownLeft,
                E::MouthFrownRight,
                E::TongueOut,
            ]
            .into_iter()
            .any(|e| d.weight(e) > 0.01)
        })));

        log::info!(
            "Parameter Registry initialized with {} parameters",
            parameters.len()
        );

        Self { parameters }
    }

    /// Reset all parameters based on new avatar's parameter list
    pub fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> usize {
        log::debug!(
            "registry.reset() starting: {} avatar params, {} param types",
            avatar_params.len(),
            param_types.len()
        );

        // Log sample of avatar params for debugging
        let sample: Vec<_> = avatar_params.iter().take(10).collect();
        log::debug!("Sample avatar params: {:?}", sample);

        let start_time = std::time::Instant::now();
        let mut relevant_count = 0usize;
        let total_params = self.parameters.len();

        for (idx, param) in self.parameters.iter_mut().enumerate() {
            let param_start = std::time::Instant::now();
            relevant_count += param.reset(avatar_params, param_types);

            // Log every 100th parameter for progress tracking
            if idx % 100 == 0 {
                log::debug!(
                    "Processing param {}/{} ({}ms elapsed)",
                    idx,
                    total_params,
                    start_time.elapsed().as_millis()
                );
            }

            // Warn if any individual param takes too long
            let param_elapsed = param_start.elapsed();
            if param_elapsed.as_millis() > 50 {
                log::warn!(
                    "Slow param reset at index {}: {}ms",
                    idx,
                    param_elapsed.as_millis()
                );
            }
        }

        let total_elapsed = start_time.elapsed();
        log::info!(
            "Parameter Registry: {} parameters marked relevant to avatar (took {}ms)",
            relevant_count,
            total_elapsed.as_millis()
        );

        // Debug: Count FT-related params
        let ft_params: Vec<_> = avatar_params
            .iter()
            .filter(|p| p.contains("/FT/") || p.contains("/v2/"))
            .collect();

        log::debug!(
            "Avatar has {} total params ({} bool, {} float, {} int), {} with FT/v2 prefix",
            avatar_params.len(),
            param_types
                .values()
                .filter(|t| **t == ParamType::Bool)
                .count(),
            param_types
                .values()
                .filter(|t| **t == ParamType::Float)
                .count(),
            param_types
                .values()
                .filter(|t| **t == ParamType::Int)
                .count(),
            ft_params.len()
        );

        relevant_count
    }

    /// Process all parameters and collect OSC messages
    pub fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage> {
        self.parameters
            .iter_mut()
            .flat_map(|p| p.process(data))
            .collect()
    }
}

impl Default for ParameterRegistry {
    fn default() -> Self {
        Self::new()
    }
}
