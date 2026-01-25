use super::base_param::FloatParam;
use super::eparam::EParam;
use super::{ParamType, Parameter};
use common::{UnifiedExpressions, UnifiedTrackingData};
use rosc::OscMessage;
use std::collections::{HashMap, HashSet};

pub struct ParameterRegistry {
    parameters: Vec<Box<dyn Parameter>>,
}

impl ParameterRegistry {
    pub fn new() -> Self {
        let mut parameters: Vec<Box<dyn Parameter>> = Vec::new();

        // Helper to get shape weight
        fn w(data: &UnifiedTrackingData, expr: UnifiedExpressions) -> f32 {
            data.shapes[expr as usize].weight
        }

        // ===== Head Tracking =====
        parameters.push(Box::new(FloatParam::new("v2/Head/Yaw", |d| {
            d.head.head_yaw
        })));
        parameters.push(Box::new(FloatParam::new("v2/Head/Pitch", |d| {
            d.head.head_pitch
        })));
        parameters.push(Box::new(FloatParam::new("v2/Head/Roll", |d| {
            d.head.head_roll
        })));
        parameters.push(Box::new(FloatParam::new("v2/Head/PosX", |d| {
            d.head.head_pos_x
        })));
        parameters.push(Box::new(FloatParam::new("v2/Head/PosY", |d| {
            d.head.head_pos_y
        })));
        parameters.push(Box::new(FloatParam::new("v2/Head/PosZ", |d| {
            d.head.head_pos_z
        })));

        // ===== Eye Gaze =====
        parameters.push(Box::new(EParam::simple("v2/EyeLeftX", |d| {
            -d.eye.left.gaze.x
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeLeftY", |d| {
            -d.eye.left.gaze.y
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeRightX", |d| {
            -d.eye.right.gaze.x
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeRightY", |d| {
            -d.eye.right.gaze.y
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeX", |d| {
            -(d.eye.left.gaze.x + d.eye.right.gaze.x) / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeY", |d| {
            -(d.eye.left.gaze.y + d.eye.right.gaze.y) / 2.0
        })));

        // ===== Eye Pupils =====
        parameters.push(Box::new(EParam::simple("v2/PupilDilation", |d| {
            (d.eye.left.pupil_diameter_mm + d.eye.right.pupil_diameter_mm) / 2.0
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

        // ===== Eye Openness =====
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

        // ===== Eye Wide =====
        parameters.push(Box::new(EParam::simple("v2/EyeWide", |d| {
            w(d, UnifiedExpressions::EyeWideLeft).max(w(d, UnifiedExpressions::EyeWideRight))
        })));

        // ===== Eye Lid =====
        parameters.push(Box::new(EParam::simple("v2/EyeLidLeft", |d| {
            d.eye.left.openness * 0.75 + w(d, UnifiedExpressions::EyeWideLeft) * 0.25
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeLidRight", |d| {
            d.eye.right.openness * 0.75 + w(d, UnifiedExpressions::EyeWideRight) * 0.25
        })));
        parameters.push(Box::new(EParam::simple("v2/EyeLid", |d| {
            ((d.eye.left.openness + d.eye.right.openness) / 2.0) * 0.75
                + ((w(d, UnifiedExpressions::EyeWideRight) + w(d, UnifiedExpressions::EyeWideLeft))
                    / 2.0)
                    * 0.25
        })));

        // ===== Eye Squint =====
        parameters.push(Box::new(EParam::simple("v2/EyeSquint", |d| {
            w(d, UnifiedExpressions::EyeSquintLeft).max(w(d, UnifiedExpressions::EyeSquintRight))
        })));
        parameters.push(Box::new(EParam::simple("v2/EyesSquint", |d| {
            w(d, UnifiedExpressions::EyeSquintLeft).max(w(d, UnifiedExpressions::EyeSquintRight))
        })));

        // ===== Brow Simple Shapes =====
        fn brow_up_right(d: &UnifiedTrackingData) -> f32 {
            w(d, UnifiedExpressions::BrowOuterUpRight) * 0.6
                + w(d, UnifiedExpressions::BrowInnerUpRight) * 0.4
        }
        fn brow_up_left(d: &UnifiedTrackingData) -> f32 {
            w(d, UnifiedExpressions::BrowOuterUpLeft) * 0.6
                + w(d, UnifiedExpressions::BrowInnerUpLeft) * 0.4
        }
        fn brow_down_right(d: &UnifiedTrackingData) -> f32 {
            w(d, UnifiedExpressions::BrowLowererRight) * 0.75
                + w(d, UnifiedExpressions::BrowPinchRight) * 0.25
        }
        fn brow_down_left(d: &UnifiedTrackingData) -> f32 {
            w(d, UnifiedExpressions::BrowLowererLeft) * 0.75
                + w(d, UnifiedExpressions::BrowPinchLeft) * 0.25
        }

        // ===== Eyebrows Compacted =====
        parameters.push(Box::new(EParam::simple("v2/BrowUp", |d| {
            (brow_up_right(d) + brow_up_left(d)) * 0.5
        })));
        parameters.push(Box::new(EParam::simple("v2/BrowDown", |d| {
            (brow_down_right(d) + brow_down_left(d)) * 0.5
        })));
        parameters.push(Box::new(EParam::simple("v2/BrowInnerUp", |d| {
            (w(d, UnifiedExpressions::BrowInnerUpLeft) + w(d, UnifiedExpressions::BrowInnerUpRight))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/BrowOuterUp", |d| {
            (w(d, UnifiedExpressions::BrowOuterUpLeft) + w(d, UnifiedExpressions::BrowOuterUpRight))
                / 2.0
        })));

        parameters.push(Box::new(EParam::simple("v2/BrowExpressionRight", |d| {
            (w(d, UnifiedExpressions::BrowInnerUpRight) * 0.5
                + w(d, UnifiedExpressions::BrowOuterUpRight) * 0.5)
                .min(1.0)
                - brow_down_right(d)
        })));
        parameters.push(Box::new(EParam::simple("v2/BrowExpressionLeft", |d| {
            (w(d, UnifiedExpressions::BrowInnerUpLeft) * 0.5
                + w(d, UnifiedExpressions::BrowOuterUpLeft) * 0.5)
                .min(1.0)
                - brow_down_left(d)
        })));
        parameters.push(Box::new(EParam::simple("v2/BrowExpression", |d| {
            let right = (w(d, UnifiedExpressions::BrowInnerUpRight)
                + w(d, UnifiedExpressions::BrowOuterUpRight))
                * 0.5;
            let left = (w(d, UnifiedExpressions::BrowInnerUpLeft)
                + w(d, UnifiedExpressions::BrowOuterUpLeft))
                * 0.5;
            (right.min(1.0) - brow_down_right(d) + left.min(1.0) - brow_down_left(d)) * 0.5
        })));

        // ===== Jaw Combined =====
        parameters.push(Box::new(EParam::simple("v2/JawX", |d| {
            w(d, UnifiedExpressions::JawRight) - w(d, UnifiedExpressions::JawLeft)
        })));
        parameters.push(Box::new(EParam::simple("v2/JawZ", |d| {
            w(d, UnifiedExpressions::JawForward) - w(d, UnifiedExpressions::JawBackward)
        })));

        // ===== Cheeks Combined =====
        parameters.push(Box::new(EParam::simple("v2/CheekSquint", |d| {
            (w(d, UnifiedExpressions::CheekSquintLeft) + w(d, UnifiedExpressions::CheekSquintRight))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/CheekPuffSuckLeft", |d| {
            w(d, UnifiedExpressions::CheekPuffLeft) - w(d, UnifiedExpressions::CheekSuckLeft)
        })));
        parameters.push(Box::new(EParam::simple("v2/CheekPuffSuckRight", |d| {
            w(d, UnifiedExpressions::CheekPuffRight) - w(d, UnifiedExpressions::CheekSuckRight)
        })));
        parameters.push(Box::new(EParam::simple("v2/CheekPuffSuck", |d| {
            (w(d, UnifiedExpressions::CheekPuffRight) + w(d, UnifiedExpressions::CheekPuffLeft))
                / 2.0
                - (w(d, UnifiedExpressions::CheekSuckRight)
                    + w(d, UnifiedExpressions::CheekSuckLeft))
                    / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/CheekSuck", |d| {
            (w(d, UnifiedExpressions::CheekSuckLeft) + w(d, UnifiedExpressions::CheekSuckRight))
                / 2.0
        })));

        // ===== Mouth Direction =====
        parameters.push(Box::new(EParam::simple("v2/MouthUpperX", |d| {
            w(d, UnifiedExpressions::MouthUpperRight) - w(d, UnifiedExpressions::MouthUpperLeft)
        })));
        parameters.push(Box::new(EParam::simple("v2/MouthLowerX", |d| {
            w(d, UnifiedExpressions::MouthLowerRight) - w(d, UnifiedExpressions::MouthLowerLeft)
        })));
        parameters.push(Box::new(EParam::simple("v2/MouthX", |d| {
            (w(d, UnifiedExpressions::MouthUpperRight) + w(d, UnifiedExpressions::MouthLowerRight))
                / 2.0
                - (w(d, UnifiedExpressions::MouthUpperLeft)
                    + w(d, UnifiedExpressions::MouthLowerLeft))
                    / 2.0
        })));

        // ===== Lip Combined =====
        parameters.push(Box::new(EParam::simple("v2/LipSuckUpper", |d| {
            (w(d, UnifiedExpressions::LipSuckUpperRight)
                + w(d, UnifiedExpressions::LipSuckUpperLeft))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/LipSuckLower", |d| {
            (w(d, UnifiedExpressions::LipSuckLowerRight)
                + w(d, UnifiedExpressions::LipSuckLowerLeft))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/LipSuck", |d| {
            (w(d, UnifiedExpressions::LipSuckUpperRight)
                + w(d, UnifiedExpressions::LipSuckUpperLeft)
                + w(d, UnifiedExpressions::LipSuckLowerRight)
                + w(d, UnifiedExpressions::LipSuckLowerLeft))
                / 4.0
        })));

        parameters.push(Box::new(EParam::simple("v2/LipFunnelUpper", |d| {
            (w(d, UnifiedExpressions::LipFunnelUpperRight)
                + w(d, UnifiedExpressions::LipFunnelUpperLeft))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/LipFunnelLower", |d| {
            (w(d, UnifiedExpressions::LipFunnelLowerRight)
                + w(d, UnifiedExpressions::LipFunnelLowerLeft))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/LipFunnel", |d| {
            (w(d, UnifiedExpressions::LipFunnelUpperRight)
                + w(d, UnifiedExpressions::LipFunnelUpperLeft)
                + w(d, UnifiedExpressions::LipFunnelLowerRight)
                + w(d, UnifiedExpressions::LipFunnelLowerLeft))
                / 4.0
        })));

        parameters.push(Box::new(EParam::simple("v2/LipPuckerUpper", |d| {
            (w(d, UnifiedExpressions::LipPuckerUpperRight)
                + w(d, UnifiedExpressions::LipPuckerUpperLeft))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/LipPuckerLower", |d| {
            (w(d, UnifiedExpressions::LipPuckerLowerRight)
                + w(d, UnifiedExpressions::LipPuckerLowerLeft))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/LipPuckerRight", |d| {
            (w(d, UnifiedExpressions::LipPuckerUpperRight)
                + w(d, UnifiedExpressions::LipPuckerLowerRight))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/LipPuckerLeft", |d| {
            (w(d, UnifiedExpressions::LipPuckerUpperLeft)
                + w(d, UnifiedExpressions::LipPuckerLowerLeft))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/LipPucker", |d| {
            (w(d, UnifiedExpressions::LipPuckerUpperRight)
                + w(d, UnifiedExpressions::LipPuckerUpperLeft)
                + w(d, UnifiedExpressions::LipPuckerLowerRight)
                + w(d, UnifiedExpressions::LipPuckerLowerLeft))
                / 4.0
        })));

        // Lip Suck/Funnel compacted
        parameters.push(Box::new(EParam::simple("v2/LipSuckFunnelUpper", |d| {
            (w(d, UnifiedExpressions::LipSuckUpperRight)
                + w(d, UnifiedExpressions::LipSuckUpperLeft))
                / 2.0
                - (w(d, UnifiedExpressions::LipFunnelUpperRight)
                    + w(d, UnifiedExpressions::LipFunnelUpperLeft))
                    / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/LipSuckFunnelLower", |d| {
            (w(d, UnifiedExpressions::LipSuckLowerRight)
                + w(d, UnifiedExpressions::LipSuckLowerLeft))
                / 2.0
                - (w(d, UnifiedExpressions::LipFunnelLowerRight)
                    + w(d, UnifiedExpressions::LipFunnelLowerLeft))
                    / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/LipSuckFunnelLowerLeft", |d| {
            w(d, UnifiedExpressions::LipSuckLowerLeft)
                - w(d, UnifiedExpressions::LipFunnelLowerLeft)
        })));
        parameters.push(Box::new(EParam::simple(
            "v2/LipSuckFunnelLowerRight",
            |d| {
                w(d, UnifiedExpressions::LipSuckLowerRight)
                    - w(d, UnifiedExpressions::LipFunnelLowerRight)
            },
        )));
        parameters.push(Box::new(EParam::simple("v2/LipSuckFunnelUpperLeft", |d| {
            w(d, UnifiedExpressions::LipSuckUpperLeft)
                - w(d, UnifiedExpressions::LipFunnelUpperLeft)
        })));
        parameters.push(Box::new(EParam::simple(
            "v2/LipSuckFunnelUpperRight",
            |d| {
                w(d, UnifiedExpressions::LipSuckUpperRight)
                    - w(d, UnifiedExpressions::LipFunnelUpperRight)
            },
        )));

        // ===== Mouth Combined =====
        parameters.push(Box::new(EParam::simple("v2/MouthUpperUp", |d| {
            w(d, UnifiedExpressions::MouthUpperUpRight) * 0.5
                + w(d, UnifiedExpressions::MouthUpperUpLeft) * 0.5
        })));
        parameters.push(Box::new(EParam::simple("v2/MouthLowerDown", |d| {
            w(d, UnifiedExpressions::MouthLowerDownRight) * 0.5
                + w(d, UnifiedExpressions::MouthLowerDownLeft) * 0.5
        })));
        parameters.push(Box::new(EParam::simple("v2/MouthOpen", |d| {
            w(d, UnifiedExpressions::MouthUpperUpRight) * 0.25
                + w(d, UnifiedExpressions::MouthUpperUpLeft) * 0.25
                + w(d, UnifiedExpressions::MouthLowerDownRight) * 0.25
                + w(d, UnifiedExpressions::MouthLowerDownLeft) * 0.25
        })));

        parameters.push(Box::new(EParam::simple("v2/MouthStretch", |d| {
            (w(d, UnifiedExpressions::MouthStretchRight)
                + w(d, UnifiedExpressions::MouthStretchLeft))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/MouthTightener", |d| {
            (w(d, UnifiedExpressions::MouthTightenerRight)
                + w(d, UnifiedExpressions::MouthTightenerLeft))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/MouthPress", |d| {
            (w(d, UnifiedExpressions::MouthPressRight) + w(d, UnifiedExpressions::MouthPressLeft))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/MouthDimple", |d| {
            (w(d, UnifiedExpressions::MouthDimpleRight) + w(d, UnifiedExpressions::MouthDimpleLeft))
                / 2.0
        })));
        parameters.push(Box::new(EParam::simple("v2/NoseSneer", |d| {
            (w(d, UnifiedExpressions::NoseSneerRight) + w(d, UnifiedExpressions::NoseSneerLeft))
                / 2.0
        })));

        // Mouth compacted
        parameters.push(Box::new(EParam::simple("v2/MouthTightenerStretch", |d| {
            (w(d, UnifiedExpressions::MouthTightenerRight)
                + w(d, UnifiedExpressions::MouthTightenerLeft))
                / 2.0
                - (w(d, UnifiedExpressions::MouthStretchRight)
                    + w(d, UnifiedExpressions::MouthStretchLeft))
                    / 2.0
        })));
        parameters.push(Box::new(EParam::simple(
            "v2/MouthTightenerStretchLeft",
            |d| {
                w(d, UnifiedExpressions::MouthTightenerLeft)
                    - w(d, UnifiedExpressions::MouthStretchLeft)
            },
        )));
        parameters.push(Box::new(EParam::simple(
            "v2/MouthTightenerStretchRight",
            |d| {
                w(d, UnifiedExpressions::MouthTightenerRight)
                    - w(d, UnifiedExpressions::MouthStretchRight)
            },
        )));

        // ===== Lip Corners Combined =====
        parameters.push(Box::new(EParam::simple("v2/MouthCornerYLeft", |d| {
            w(d, UnifiedExpressions::MouthCornerSlantLeft)
                - w(d, UnifiedExpressions::MouthFrownLeft)
        })));
        parameters.push(Box::new(EParam::simple("v2/MouthCornerYRight", |d| {
            w(d, UnifiedExpressions::MouthCornerSlantRight)
                - w(d, UnifiedExpressions::MouthFrownRight)
        })));
        parameters.push(Box::new(EParam::simple("v2/MouthCornerY", |d| {
            (w(d, UnifiedExpressions::MouthCornerSlantLeft)
                - w(d, UnifiedExpressions::MouthFrownLeft)
                + w(d, UnifiedExpressions::MouthCornerSlantRight)
                - w(d, UnifiedExpressions::MouthFrownRight))
                * 0.5
        })));

        // ===== SmileFrown =====
        fn mouth_smile_right(d: &UnifiedTrackingData) -> f32 {
            w(d, UnifiedExpressions::MouthCornerPullRight) * 0.8
                + w(d, UnifiedExpressions::MouthCornerSlantRight) * 0.2
        }
        fn mouth_smile_left(d: &UnifiedTrackingData) -> f32 {
            w(d, UnifiedExpressions::MouthCornerPullLeft) * 0.8
                + w(d, UnifiedExpressions::MouthCornerSlantLeft) * 0.2
        }
        fn mouth_sad_right(d: &UnifiedTrackingData) -> f32 {
            w(d, UnifiedExpressions::MouthFrownRight)
                .max(w(d, UnifiedExpressions::MouthStretchRight))
        }
        fn mouth_sad_left(d: &UnifiedTrackingData) -> f32 {
            w(d, UnifiedExpressions::MouthFrownLeft).max(w(d, UnifiedExpressions::MouthStretchLeft))
        }

        parameters.push(Box::new(EParam::simple("v2/SmileFrownRight", |d| {
            mouth_smile_right(d) - w(d, UnifiedExpressions::MouthFrownRight)
        })));
        parameters.push(Box::new(EParam::simple("v2/SmileFrownLeft", |d| {
            mouth_smile_left(d) - w(d, UnifiedExpressions::MouthFrownLeft)
        })));
        parameters.push(Box::new(EParam::simple("v2/SmileFrown", |d| {
            mouth_smile_right(d) * 0.5 + mouth_smile_left(d) * 0.5
                - w(d, UnifiedExpressions::MouthFrownRight) * 0.5
                - w(d, UnifiedExpressions::MouthFrownLeft) * 0.5
        })));

        // SmileSad
        parameters.push(Box::new(EParam::simple("v2/SmileSadRight", |d| {
            mouth_smile_right(d) - mouth_sad_right(d)
        })));
        parameters.push(Box::new(EParam::simple("v2/SmileSadLeft", |d| {
            mouth_smile_left(d) - mouth_sad_left(d)
        })));
        parameters.push(Box::new(EParam::simple("v2/SmileSad", |d| {
            (mouth_smile_left(d) + mouth_smile_right(d)) / 2.0
                - (mouth_sad_left(d) + mouth_sad_right(d)) / 2.0
        })));

        // ===== Tongue Combined =====
        parameters.push(Box::new(EParam::simple("v2/TongueX", |d| {
            w(d, UnifiedExpressions::TongueRight) - w(d, UnifiedExpressions::TongueLeft)
        })));
        parameters.push(Box::new(EParam::simple("v2/TongueY", |d| {
            w(d, UnifiedExpressions::TongueUp) - w(d, UnifiedExpressions::TongueDown)
        })));
        parameters.push(Box::new(EParam::simple("v2/TongueArchY", |d| {
            w(d, UnifiedExpressions::TongueCurlUp) - w(d, UnifiedExpressions::TongueBendDown)
        })));
        parameters.push(Box::new(EParam::simple("v2/TongueShape", |d| {
            w(d, UnifiedExpressions::TongueFlat) - w(d, UnifiedExpressions::TongueSquish)
        })));

        // ===== All Base Expressions (v2/{ExpressionName}) =====
        // Generate EParam for each UnifiedExpression
        for i in 0..UnifiedExpressions::Max as usize {
            if let Ok(expr) = UnifiedExpressions::try_from(i) {
                if expr != UnifiedExpressions::Max {
                    let name = format!("v2/{:?}", expr);
                    // Capture i for the closure
                    let idx = i;
                    parameters.push(Box::new(EParam::expression(&name, move |d| {
                        d.shapes[idx].weight
                    })));
                }
            }
        }

        Self { parameters }
    }

    /// Reset all parameters based on new avatar's parameter list
    pub fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) {
        let mut relevant_count = 0usize;

        for param in self.parameters.iter_mut() {
            if param.reset(avatar_params, param_types) {
                relevant_count += 1;
            }
        }

        // Count actual addresses for detailed logging
        // Note: This is approximate since we can't easily introspect Box<dyn Parameter>
        log::info!(
            "Parameter Registry: {} parameters marked relevant to avatar",
            relevant_count
        );
        log::debug!(
            "Avatar has {} total params ({} bool, {} float, {} int)",
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
                .count()
        );
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
