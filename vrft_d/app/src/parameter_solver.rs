use crate::shape_legacy;
use common::{UnifiedExpressions, UnifiedTrackingData};

pub struct ParameterSolver;

impl ParameterSolver {
    pub fn solve(data: &UnifiedTrackingData) -> Vec<(&'static str, f32)> {
        let mut params = Vec::with_capacity(200);
        let s = &data.shapes;

        let w = |expr: UnifiedExpressions| s[expr as usize].weight;

        for (i, shape) in s.iter().enumerate().take(UnifiedExpressions::Max as usize) {
            if let Some(name) = Self::get_expression_name(i) {
                params.push((name, shape.weight));
            }
        }

        params.push(("v2/Head/Yaw", data.head.head_yaw));
        params.push(("v2/Head/Pitch", data.head.head_pitch));
        params.push(("v2/Head/Roll", data.head.head_roll));
        params.push(("v2/Head/PosX", data.head.head_pos_x));
        params.push(("v2/Head/PosY", data.head.head_pos_y));
        params.push(("v2/Head/PosZ", data.head.head_pos_z));

        let brow_up_right = w(UnifiedExpressions::BrowOuterUpRight) * 0.6
            + w(UnifiedExpressions::BrowInnerUpRight) * 0.4;
        let brow_up_left = w(UnifiedExpressions::BrowOuterUpLeft) * 0.6
            + w(UnifiedExpressions::BrowInnerUpLeft) * 0.4;

        let brow_down_right = w(UnifiedExpressions::BrowLowererRight) * 0.75
            + w(UnifiedExpressions::BrowPinchRight) * 0.25;
        let brow_down_left = w(UnifiedExpressions::BrowLowererLeft) * 0.75
            + w(UnifiedExpressions::BrowPinchLeft) * 0.25;

        let mouth_smile_right = w(UnifiedExpressions::MouthCornerPullRight) * 0.8
            + w(UnifiedExpressions::MouthCornerSlantRight) * 0.2;
        let mouth_smile_left = w(UnifiedExpressions::MouthCornerPullLeft) * 0.8
            + w(UnifiedExpressions::MouthCornerSlantLeft) * 0.2;

        let mouth_sad_right =
            if w(UnifiedExpressions::MouthFrownRight) > w(UnifiedExpressions::MouthStretchRight) {
                w(UnifiedExpressions::MouthFrownRight)
            } else {
                w(UnifiedExpressions::MouthStretchRight)
            };
        let mouth_sad_left =
            if w(UnifiedExpressions::MouthFrownLeft) > w(UnifiedExpressions::MouthStretchLeft) {
                w(UnifiedExpressions::MouthFrownLeft)
            } else {
                w(UnifiedExpressions::MouthStretchLeft)
            };

        params.push(("v2/BrowUpRight", brow_up_right));
        params.push(("v2/BrowUpLeft", brow_up_left));
        params.push(("v2/BrowDownRight", brow_down_right));
        params.push(("v2/BrowDownLeft", brow_down_left));
        params.push(("v2/MouthSmileRight", mouth_smile_right));
        params.push(("v2/MouthSmileLeft", mouth_smile_left));
        params.push(("v2/MouthSadRight", mouth_sad_right));
        params.push(("v2/MouthSadLeft", mouth_sad_left));

        params.push((
            "v2/EyesClosedAmount",
            1.0 - (data.eye.left.openness + data.eye.right.openness) / 2.0,
        ));

        params.push((
            "v2/PupilDiameterLeft",
            data.eye.left.pupil_diameter_mm * 0.1,
        ));
        params.push((
            "v2/PupilDiameterRight",
            data.eye.right.pupil_diameter_mm * 0.1,
        ));
        params.push((
            "v2/PupilDiameter",
            (data.eye.left.pupil_diameter_mm + data.eye.right.pupil_diameter_mm) * 0.05,
        ));
        params.push((
            "v2/PupilDilation",
            (data.eye.left.pupil_diameter_mm + data.eye.right.pupil_diameter_mm) / 2.0,
        ));

        params.push(("v2/EyeOpenLeft", data.eye.left.openness));
        params.push(("v2/EyeOpenRight", data.eye.right.openness));
        params.push((
            "v2/EyeOpen",
            (data.eye.left.openness + data.eye.right.openness) / 2.0,
        ));
        params.push(("v2/EyeClosedLeft", 1.0 - data.eye.left.openness));
        params.push(("v2/EyeClosedRight", 1.0 - data.eye.right.openness));
        params.push((
            "v2/EyeClosed",
            1.0 - (data.eye.left.openness + data.eye.right.openness) / 2.0,
        ));

        let eye_wide_left = w(UnifiedExpressions::EyeWideLeft);
        let eye_wide_right = w(UnifiedExpressions::EyeWideRight);
        params.push((
            "v2/EyeWide",
            if eye_wide_left > eye_wide_right {
                eye_wide_left
            } else {
                eye_wide_right
            },
        ));

        let eye_lid_left = data.eye.left.openness * 0.75 + eye_wide_left * 0.25;
        let eye_lid_right = data.eye.right.openness * 0.75 + eye_wide_right * 0.25;
        params.push(("v2/EyeLidLeft", eye_lid_left));
        params.push(("v2/EyeLidRight", eye_lid_right));
        params.push(("v2/EyeLid", (eye_lid_left + eye_lid_right) / 2.0));

        let eye_squint_left = w(UnifiedExpressions::EyeSquintLeft);
        let eye_squint_right = w(UnifiedExpressions::EyeSquintRight);
        let eye_squint = if eye_squint_left > eye_squint_right {
            eye_squint_left
        } else {
            eye_squint_right
        };
        params.push(("v2/EyeSquint", eye_squint));
        params.push(("v2/EyesSquint", eye_squint));

        params.push(("v2/BrowUp", (brow_up_right + brow_up_left) * 0.5));
        params.push(("v2/BrowDown", (brow_down_right + brow_down_left) * 0.5));
        params.push((
            "v2/BrowInnerUp",
            (w(UnifiedExpressions::BrowInnerUpLeft) + w(UnifiedExpressions::BrowInnerUpRight))
                / 2.0,
        ));
        params.push((
            "v2/BrowOuterUp",
            (w(UnifiedExpressions::BrowOuterUpLeft) + w(UnifiedExpressions::BrowOuterUpRight))
                / 2.0,
        ));

        params.push((
            "v2/BrowExpressionRight",
            (w(UnifiedExpressions::BrowInnerUpRight) * 0.5
                + w(UnifiedExpressions::BrowOuterUpRight) * 0.5)
                .min(1.0)
                - brow_down_right,
        ));
        params.push((
            "v2/BrowExpressionLeft",
            (w(UnifiedExpressions::BrowInnerUpLeft) * 0.5
                + w(UnifiedExpressions::BrowOuterUpLeft) * 0.5)
                .min(1.0)
                - brow_down_left,
        ));

        let brow_exp_right = (w(UnifiedExpressions::BrowInnerUpRight)
            + w(UnifiedExpressions::BrowOuterUpRight))
            * 0.5;
        let brow_exp_left =
            (w(UnifiedExpressions::BrowInnerUpLeft) + w(UnifiedExpressions::BrowOuterUpLeft)) * 0.5;
        params.push((
            "v2/BrowExpression",
            (brow_exp_right.min(1.0) - brow_down_right + brow_exp_left.min(1.0) - brow_down_left)
                * 0.5,
        ));

        params.push((
            "v2/JawX",
            w(UnifiedExpressions::JawRight) - w(UnifiedExpressions::JawLeft),
        ));
        params.push((
            "v2/JawZ",
            w(UnifiedExpressions::JawForward) - w(UnifiedExpressions::JawBackward),
        ));

        params.push((
            "v2/CheekSquint",
            (w(UnifiedExpressions::CheekSquintLeft) + w(UnifiedExpressions::CheekSquintRight))
                / 2.0,
        ));
        params.push((
            "v2/CheekPuffSuckLeft",
            w(UnifiedExpressions::CheekPuffLeft) - w(UnifiedExpressions::CheekSuckLeft),
        ));
        params.push((
            "v2/CheekPuffSuckRight",
            w(UnifiedExpressions::CheekPuffRight) - w(UnifiedExpressions::CheekSuckRight),
        ));
        params.push((
            "v2/CheekPuffSuck",
            (w(UnifiedExpressions::CheekPuffRight) + w(UnifiedExpressions::CheekPuffLeft)) / 2.0
                - (w(UnifiedExpressions::CheekSuckRight) + w(UnifiedExpressions::CheekSuckLeft))
                / 2.0,
        ));
        params.push((
            "v2/CheekSuck",
            (w(UnifiedExpressions::CheekSuckLeft) + w(UnifiedExpressions::CheekSuckRight)) / 2.0,
        ));

        params.push((
            "v2/MouthUpperX",
            w(UnifiedExpressions::MouthUpperRight) - w(UnifiedExpressions::MouthUpperLeft),
        ));
        params.push((
            "v2/MouthLowerX",
            w(UnifiedExpressions::MouthLowerRight) - w(UnifiedExpressions::MouthLowerLeft),
        ));

        let mouth_right =
            (w(UnifiedExpressions::MouthUpperRight) + w(UnifiedExpressions::MouthLowerRight)) / 2.0;
        let mouth_left =
            (w(UnifiedExpressions::MouthUpperLeft) + w(UnifiedExpressions::MouthLowerLeft)) / 2.0;
        let mouth_x = mouth_right - mouth_left;
        params.push(("v2/MouthX", mouth_x));

        params.push((
            "v2/LipSuckUpper",
            (w(UnifiedExpressions::LipSuckUpperRight) + w(UnifiedExpressions::LipSuckUpperLeft))
                / 2.0,
        ));
        params.push((
            "v2/LipSuckLower",
            (w(UnifiedExpressions::LipSuckLowerRight) + w(UnifiedExpressions::LipSuckLowerLeft))
                / 2.0,
        ));
        params.push((
            "v2/LipSuck",
            (w(UnifiedExpressions::LipSuckUpperRight)
                + w(UnifiedExpressions::LipSuckUpperLeft)
                + w(UnifiedExpressions::LipSuckLowerRight)
                + w(UnifiedExpressions::LipSuckLowerLeft))
                / 4.0,
        ));

        params.push((
            "v2/LipFunnelUpper",
            (w(UnifiedExpressions::LipFunnelUpperRight)
                + w(UnifiedExpressions::LipFunnelUpperLeft))
                / 2.0,
        ));
        params.push((
            "v2/LipFunnelLower",
            (w(UnifiedExpressions::LipFunnelLowerRight)
                + w(UnifiedExpressions::LipFunnelLowerLeft))
                / 2.0,
        ));
        params.push((
            "v2/LipFunnel",
            (w(UnifiedExpressions::LipFunnelUpperRight)
                + w(UnifiedExpressions::LipFunnelUpperLeft)
                + w(UnifiedExpressions::LipFunnelLowerRight)
                + w(UnifiedExpressions::LipFunnelLowerLeft))
                / 4.0,
        ));

        params.push((
            "v2/LipPuckerUpper",
            (w(UnifiedExpressions::LipPuckerUpperRight)
                + w(UnifiedExpressions::LipPuckerUpperLeft))
                / 2.0,
        ));
        params.push((
            "v2/LipPuckerLower",
            (w(UnifiedExpressions::LipPuckerLowerRight)
                + w(UnifiedExpressions::LipPuckerLowerLeft))
                / 2.0,
        ));
        params.push((
            "v2/LipPuckerRight",
            (w(UnifiedExpressions::LipPuckerUpperRight)
                + w(UnifiedExpressions::LipPuckerLowerRight))
                / 2.0,
        ));
        params.push((
            "v2/LipPuckerLeft",
            (w(UnifiedExpressions::LipPuckerUpperLeft) + w(UnifiedExpressions::LipPuckerLowerLeft))
                / 2.0,
        ));
        params.push((
            "v2/LipPucker",
            (w(UnifiedExpressions::LipPuckerUpperRight)
                + w(UnifiedExpressions::LipPuckerUpperLeft)
                + w(UnifiedExpressions::LipPuckerLowerRight)
                + w(UnifiedExpressions::LipPuckerLowerLeft))
                / 4.0,
        ));

        params.push((
            "v2/LipSuckFunnelUpper",
            (w(UnifiedExpressions::LipSuckUpperRight) + w(UnifiedExpressions::LipSuckUpperLeft))
                / 2.0
                - (w(UnifiedExpressions::LipFunnelUpperRight)
                + w(UnifiedExpressions::LipFunnelUpperLeft))
                / 2.0,
        ));
        params.push((
            "v2/LipSuckFunnelLower",
            (w(UnifiedExpressions::LipSuckLowerRight) + w(UnifiedExpressions::LipSuckLowerLeft))
                / 2.0
                - (w(UnifiedExpressions::LipFunnelLowerRight)
                + w(UnifiedExpressions::LipFunnelLowerLeft))
                / 2.0,
        ));
        params.push((
            "v2/LipSuckFunnelLowerLeft",
            w(UnifiedExpressions::LipSuckLowerLeft) - w(UnifiedExpressions::LipFunnelLowerLeft),
        ));
        params.push((
            "v2/LipSuckFunnelLowerRight",
            w(UnifiedExpressions::LipSuckLowerRight) - w(UnifiedExpressions::LipFunnelLowerRight),
        ));
        params.push((
            "v2/LipSuckFunnelUpperLeft",
            w(UnifiedExpressions::LipSuckUpperLeft) - w(UnifiedExpressions::LipFunnelUpperLeft),
        ));
        params.push((
            "v2/LipSuckFunnelUpperRight",
            w(UnifiedExpressions::LipSuckUpperRight) - w(UnifiedExpressions::LipFunnelUpperRight),
        ));

        params.push((
            "v2/MouthUpperUp",
            w(UnifiedExpressions::MouthUpperUpRight) * 0.5
                + w(UnifiedExpressions::MouthUpperUpLeft) * 0.5,
        ));
        params.push((
            "v2/MouthLowerDown",
            w(UnifiedExpressions::MouthLowerDownRight) * 0.5
                + w(UnifiedExpressions::MouthLowerDownLeft) * 0.5,
        ));
        params.push((
            "v2/MouthOpen",
            w(UnifiedExpressions::MouthUpperUpRight) * 0.25
                + w(UnifiedExpressions::MouthUpperUpLeft) * 0.25
                + w(UnifiedExpressions::MouthLowerDownRight) * 0.25
                + w(UnifiedExpressions::MouthLowerDownLeft) * 0.25,
        ));

        params.push((
            "v2/MouthStretch",
            (w(UnifiedExpressions::MouthStretchRight) + w(UnifiedExpressions::MouthStretchLeft))
                / 2.0,
        ));
        params.push((
            "v2/MouthTightener",
            (w(UnifiedExpressions::MouthTightenerRight)
                + w(UnifiedExpressions::MouthTightenerLeft))
                / 2.0,
        ));
        params.push((
            "v2/MouthPress",
            (w(UnifiedExpressions::MouthPressRight) + w(UnifiedExpressions::MouthPressLeft)) / 2.0,
        ));
        params.push((
            "v2/MouthDimple",
            (w(UnifiedExpressions::MouthDimpleRight) + w(UnifiedExpressions::MouthDimpleLeft))
                / 2.0,
        ));
        params.push((
            "v2/NoseSneer",
            (w(UnifiedExpressions::NoseSneerRight) + w(UnifiedExpressions::NoseSneerLeft)) / 2.0,
        ));

        params.push((
            "v2/MouthTightenerStretch",
            (w(UnifiedExpressions::MouthTightenerRight)
                + w(UnifiedExpressions::MouthTightenerLeft))
                / 2.0
                - (w(UnifiedExpressions::MouthStretchRight)
                + w(UnifiedExpressions::MouthStretchLeft))
                / 2.0,
        ));
        params.push((
            "v2/MouthTightenerStretchLeft",
            w(UnifiedExpressions::MouthTightenerLeft) - w(UnifiedExpressions::MouthStretchLeft),
        ));
        params.push((
            "v2/MouthTightenerStretchRight",
            w(UnifiedExpressions::MouthTightenerRight) - w(UnifiedExpressions::MouthStretchRight),
        ));

        params.push((
            "v2/MouthCornerYLeft",
            w(UnifiedExpressions::MouthCornerSlantLeft) - w(UnifiedExpressions::MouthFrownLeft),
        ));
        params.push((
            "v2/MouthCornerYRight",
            w(UnifiedExpressions::MouthCornerSlantRight) - w(UnifiedExpressions::MouthFrownRight),
        ));
        params.push((
            "v2/MouthCornerY",
            (w(UnifiedExpressions::MouthCornerSlantLeft) - w(UnifiedExpressions::MouthFrownLeft)
                + w(UnifiedExpressions::MouthCornerSlantRight)
                - w(UnifiedExpressions::MouthFrownRight))
                * 0.5,
        ));

        params.push((
            "v2/SmileFrownRight",
            mouth_smile_right - w(UnifiedExpressions::MouthFrownRight),
        ));
        params.push((
            "v2/SmileFrownLeft",
            mouth_smile_left - w(UnifiedExpressions::MouthFrownLeft),
        ));
        params.push((
            "v2/SmileFrown",
            mouth_smile_right * 0.5 + mouth_smile_left * 0.5
                - w(UnifiedExpressions::MouthFrownRight) * 0.5
                - w(UnifiedExpressions::MouthFrownLeft) * 0.5,
        ));

        params.push(("v2/SmileSadRight", mouth_smile_right - mouth_sad_right));
        params.push(("v2/SmileSadLeft", mouth_smile_left - mouth_sad_left));
        params.push((
            "v2/SmileSad",
            (mouth_smile_left + mouth_smile_right) / 2.0 - (mouth_sad_left + mouth_sad_right) / 2.0,
        ));

        params.push((
            "v2/TongueX",
            w(UnifiedExpressions::TongueRight) - w(UnifiedExpressions::TongueLeft),
        ));
        params.push((
            "v2/TongueY",
            w(UnifiedExpressions::TongueUp) - w(UnifiedExpressions::TongueDown),
        ));
        params.push((
            "v2/TongueArchY",
            w(UnifiedExpressions::TongueCurlUp) - w(UnifiedExpressions::TongueBendDown),
        ));
        params.push((
            "v2/TongueShape",
            w(UnifiedExpressions::TongueFlat) - w(UnifiedExpressions::TongueSquish),
        ));

        params.push(("EyeTrackingActive", 1.0));
        params.push(("ExpressionTrackingActive", 1.0));
        params.push(("LipTrackingActive", 1.0));

        params.extend(shape_legacy::get_v1_parameters(data));
        params.extend(shape_legacy::get_v1_eye_parameters(data));
        params.extend(shape_legacy::get_v1_sranipal_lip_parameters(data));

        params
    }

    pub fn get_expression_name(idx: usize) -> Option<&'static str> {
        if idx >= UnifiedExpressions::Max as usize {
            return None;
        }

        let expr: UnifiedExpressions = unsafe { std::mem::transmute(idx) };

        match expr {
            UnifiedExpressions::EyeSquintRight => Some("v2/EyeSquintRight"),
            UnifiedExpressions::EyeSquintLeft => Some("v2/EyeSquintLeft"),
            UnifiedExpressions::EyeWideRight => Some("v2/EyeWideRight"),
            UnifiedExpressions::EyeWideLeft => Some("v2/EyeWideLeft"),
            UnifiedExpressions::BrowPinchRight => Some("v2/BrowPinchRight"),
            UnifiedExpressions::BrowPinchLeft => Some("v2/BrowPinchLeft"),
            UnifiedExpressions::BrowLowererRight => Some("v2/BrowLowererRight"),
            UnifiedExpressions::BrowLowererLeft => Some("v2/BrowLowererLeft"),
            UnifiedExpressions::BrowInnerUpRight => Some("v2/BrowInnerUpRight"),
            UnifiedExpressions::BrowInnerUpLeft => Some("v2/BrowInnerUpLeft"),
            UnifiedExpressions::BrowOuterUpRight => Some("v2/BrowOuterUpRight"),
            UnifiedExpressions::BrowOuterUpLeft => Some("v2/BrowOuterUpLeft"),
            UnifiedExpressions::NasalDilationRight => Some("v2/NasalDilationRight"),
            UnifiedExpressions::NasalDilationLeft => Some("v2/NasalDilationLeft"),
            UnifiedExpressions::NasalConstrictRight => Some("v2/NasalConstrictRight"),
            UnifiedExpressions::NasalConstrictLeft => Some("v2/NasalConstrictLeft"),
            UnifiedExpressions::CheekSquintRight => Some("v2/CheekSquintRight"),
            UnifiedExpressions::CheekSquintLeft => Some("v2/CheekSquintLeft"),
            UnifiedExpressions::CheekPuffRight => Some("v2/CheekPuffRight"),
            UnifiedExpressions::CheekPuffLeft => Some("v2/CheekPuffLeft"),
            UnifiedExpressions::CheekSuckRight => Some("v2/CheekSuckRight"),
            UnifiedExpressions::CheekSuckLeft => Some("v2/CheekSuckLeft"),
            UnifiedExpressions::JawOpen => Some("v2/JawOpen"),
            UnifiedExpressions::JawRight => Some("v2/JawRight"),
            UnifiedExpressions::JawLeft => Some("v2/JawLeft"),
            UnifiedExpressions::JawForward => Some("v2/JawForward"),
            UnifiedExpressions::JawBackward => Some("v2/JawBackward"),
            UnifiedExpressions::JawClench => Some("v2/JawClench"),
            UnifiedExpressions::JawMandibleRaise => Some("v2/JawMandibleRaise"),
            UnifiedExpressions::MouthClosed => Some("v2/MouthClosed"),
            UnifiedExpressions::LipSuckUpperRight => Some("v2/LipSuckUpperRight"),
            UnifiedExpressions::LipSuckUpperLeft => Some("v2/LipSuckUpperLeft"),
            UnifiedExpressions::LipSuckLowerRight => Some("v2/LipSuckLowerRight"),
            UnifiedExpressions::LipSuckLowerLeft => Some("v2/LipSuckLowerLeft"),
            UnifiedExpressions::LipSuckCornerRight => Some("v2/LipSuckCornerRight"),
            UnifiedExpressions::LipSuckCornerLeft => Some("v2/LipSuckCornerLeft"),
            UnifiedExpressions::LipFunnelUpperRight => Some("v2/LipFunnelUpperRight"),
            UnifiedExpressions::LipFunnelUpperLeft => Some("v2/LipFunnelUpperLeft"),
            UnifiedExpressions::LipFunnelLowerRight => Some("v2/LipFunnelLowerRight"),
            UnifiedExpressions::LipFunnelLowerLeft => Some("v2/LipFunnelLowerLeft"),
            UnifiedExpressions::LipPuckerUpperRight => Some("v2/LipPuckerUpperRight"),
            UnifiedExpressions::LipPuckerUpperLeft => Some("v2/LipPuckerUpperLeft"),
            UnifiedExpressions::LipPuckerLowerRight => Some("v2/LipPuckerLowerRight"),
            UnifiedExpressions::LipPuckerLowerLeft => Some("v2/LipPuckerLowerLeft"),
            UnifiedExpressions::MouthUpperUpRight => Some("v2/MouthUpperUpRight"),
            UnifiedExpressions::MouthUpperUpLeft => Some("v2/MouthUpperUpLeft"),
            UnifiedExpressions::MouthUpperDeepenRight => Some("v2/MouthUpperDeepenRight"),
            UnifiedExpressions::MouthUpperDeepenLeft => Some("v2/MouthUpperDeepenLeft"),
            UnifiedExpressions::NoseSneerRight => Some("v2/NoseSneerRight"),
            UnifiedExpressions::NoseSneerLeft => Some("v2/NoseSneerLeft"),
            UnifiedExpressions::MouthLowerDownRight => Some("v2/MouthLowerDownRight"),
            UnifiedExpressions::MouthLowerDownLeft => Some("v2/MouthLowerDownLeft"),
            UnifiedExpressions::MouthUpperRight => Some("v2/MouthUpperRight"),
            UnifiedExpressions::MouthUpperLeft => Some("v2/MouthUpperLeft"),
            UnifiedExpressions::MouthLowerRight => Some("v2/MouthLowerRight"),
            UnifiedExpressions::MouthLowerLeft => Some("v2/MouthLowerLeft"),
            UnifiedExpressions::MouthCornerPullRight => Some("v2/MouthCornerPullRight"),
            UnifiedExpressions::MouthCornerPullLeft => Some("v2/MouthCornerPullLeft"),
            UnifiedExpressions::MouthCornerSlantRight => Some("v2/MouthCornerSlantRight"),
            UnifiedExpressions::MouthCornerSlantLeft => Some("v2/MouthCornerSlantLeft"),
            UnifiedExpressions::MouthFrownRight => Some("v2/MouthFrownRight"),
            UnifiedExpressions::MouthFrownLeft => Some("v2/MouthFrownLeft"),
            UnifiedExpressions::MouthStretchRight => Some("v2/MouthStretchRight"),
            UnifiedExpressions::MouthStretchLeft => Some("v2/MouthStretchLeft"),
            UnifiedExpressions::MouthDimpleRight => Some("v2/MouthDimpleRight"),
            UnifiedExpressions::MouthDimpleLeft => Some("v2/MouthDimpleLeft"),
            UnifiedExpressions::MouthRaiserUpper => Some("v2/MouthRaiserUpper"),
            UnifiedExpressions::MouthRaiserLower => Some("v2/MouthRaiserLower"),
            UnifiedExpressions::MouthPressRight => Some("v2/MouthPressRight"),
            UnifiedExpressions::MouthPressLeft => Some("v2/MouthPressLeft"),
            UnifiedExpressions::MouthTightenerRight => Some("v2/MouthTightenerRight"),
            UnifiedExpressions::MouthTightenerLeft => Some("v2/MouthTightenerLeft"),
            UnifiedExpressions::TongueOut => Some("v2/TongueOut"),
            UnifiedExpressions::TongueUp => Some("v2/TongueUp"),
            UnifiedExpressions::TongueDown => Some("v2/TongueDown"),
            UnifiedExpressions::TongueRight => Some("v2/TongueRight"),
            UnifiedExpressions::TongueLeft => Some("v2/TongueLeft"),
            UnifiedExpressions::TongueRoll => Some("v2/TongueRoll"),
            UnifiedExpressions::TongueBendDown => Some("v2/TongueBendDown"),
            UnifiedExpressions::TongueCurlUp => Some("v2/TongueCurlUp"),
            UnifiedExpressions::TongueSquish => Some("v2/TongueSquish"),
            UnifiedExpressions::TongueFlat => Some("v2/TongueFlat"),
            UnifiedExpressions::TongueTwistRight => Some("v2/TongueTwistRight"),
            UnifiedExpressions::TongueTwistLeft => Some("v2/TongueTwistLeft"),
            UnifiedExpressions::SoftPalateClose => Some("v2/SoftPalateClose"),
            UnifiedExpressions::ThroatSwallow => Some("v2/ThroatSwallow"),
            UnifiedExpressions::NeckFlexRight => Some("v2/NeckFlexRight"),
            UnifiedExpressions::NeckFlexLeft => Some("v2/NeckFlexLeft"),
            _ => None,
        }
    }
}
