//! Generates all UnifiedExpressions as individual v2 parameters.
//! Each expression enum value becomes a v2/{ExpressionName} parameter.

use super::eparam::EParam;
use super::Parameter;
use common::UnifiedExpressions;

/// Creates all UnifiedExpressions base parameters as v2/{ExpressionName} params.
pub fn create_unified_expression_params() -> Vec<Box<dyn Parameter>> {
    // All UnifiedExpressions enum values in order
    let expressions = [
        // Eye Expressions
        (UnifiedExpressions::EyeSquintRight, "EyeSquintRight"),
        (UnifiedExpressions::EyeSquintLeft, "EyeSquintLeft"),
        (UnifiedExpressions::EyeWideRight, "EyeWideRight"),
        (UnifiedExpressions::EyeWideLeft, "EyeWideLeft"),
        // Eyebrow Expressions
        (UnifiedExpressions::BrowPinchRight, "BrowPinchRight"),
        (UnifiedExpressions::BrowPinchLeft, "BrowPinchLeft"),
        (UnifiedExpressions::BrowLowererRight, "BrowLowererRight"),
        (UnifiedExpressions::BrowLowererLeft, "BrowLowererLeft"),
        (UnifiedExpressions::BrowInnerUpRight, "BrowInnerUpRight"),
        (UnifiedExpressions::BrowInnerUpLeft, "BrowInnerUpLeft"),
        (UnifiedExpressions::BrowOuterUpRight, "BrowOuterUpRight"),
        (UnifiedExpressions::BrowOuterUpLeft, "BrowOuterUpLeft"),
        // Nose Expressions
        (UnifiedExpressions::NasalDilationRight, "NasalDilationRight"),
        (UnifiedExpressions::NasalDilationLeft, "NasalDilationLeft"),
        (
            UnifiedExpressions::NasalConstrictRight,
            "NasalConstrictRight",
        ),
        (UnifiedExpressions::NasalConstrictLeft, "NasalConstrictLeft"),
        // Cheek Expressions
        (UnifiedExpressions::CheekSquintRight, "CheekSquintRight"),
        (UnifiedExpressions::CheekSquintLeft, "CheekSquintLeft"),
        (UnifiedExpressions::CheekPuffRight, "CheekPuffRight"),
        (UnifiedExpressions::CheekPuffLeft, "CheekPuffLeft"),
        (UnifiedExpressions::CheekSuckRight, "CheekSuckRight"),
        (UnifiedExpressions::CheekSuckLeft, "CheekSuckLeft"),
        // Jaw Expressions
        (UnifiedExpressions::JawOpen, "JawOpen"),
        (UnifiedExpressions::JawRight, "JawRight"),
        (UnifiedExpressions::JawLeft, "JawLeft"),
        (UnifiedExpressions::JawForward, "JawForward"),
        (UnifiedExpressions::JawBackward, "JawBackward"),
        (UnifiedExpressions::JawClench, "JawClench"),
        (UnifiedExpressions::JawMandibleRaise, "JawMandibleRaise"),
        (UnifiedExpressions::MouthClosed, "MouthClosed"),
        // Lip Suck/Funnel/Pucker Expressions
        (UnifiedExpressions::LipSuckUpperRight, "LipSuckUpperRight"),
        (UnifiedExpressions::LipSuckUpperLeft, "LipSuckUpperLeft"),
        (UnifiedExpressions::LipSuckLowerRight, "LipSuckLowerRight"),
        (UnifiedExpressions::LipSuckLowerLeft, "LipSuckLowerLeft"),
        (UnifiedExpressions::LipSuckCornerRight, "LipSuckCornerRight"),
        (UnifiedExpressions::LipSuckCornerLeft, "LipSuckCornerLeft"),
        (
            UnifiedExpressions::LipFunnelUpperRight,
            "LipFunnelUpperRight",
        ),
        (UnifiedExpressions::LipFunnelUpperLeft, "LipFunnelUpperLeft"),
        (
            UnifiedExpressions::LipFunnelLowerRight,
            "LipFunnelLowerRight",
        ),
        (UnifiedExpressions::LipFunnelLowerLeft, "LipFunnelLowerLeft"),
        (
            UnifiedExpressions::LipPuckerUpperRight,
            "LipPuckerUpperRight",
        ),
        (UnifiedExpressions::LipPuckerUpperLeft, "LipPuckerUpperLeft"),
        (
            UnifiedExpressions::LipPuckerLowerRight,
            "LipPuckerLowerRight",
        ),
        (UnifiedExpressions::LipPuckerLowerLeft, "LipPuckerLowerLeft"),
        // Mouth Expressions
        (UnifiedExpressions::MouthUpperUpRight, "MouthUpperUpRight"),
        (UnifiedExpressions::MouthUpperUpLeft, "MouthUpperUpLeft"),
        (
            UnifiedExpressions::MouthUpperDeepenRight,
            "MouthUpperDeepenRight",
        ),
        (
            UnifiedExpressions::MouthUpperDeepenLeft,
            "MouthUpperDeepenLeft",
        ),
        (UnifiedExpressions::NoseSneerRight, "NoseSneerRight"),
        (UnifiedExpressions::NoseSneerLeft, "NoseSneerLeft"),
        (
            UnifiedExpressions::MouthLowerDownRight,
            "MouthLowerDownRight",
        ),
        (UnifiedExpressions::MouthLowerDownLeft, "MouthLowerDownLeft"),
        (UnifiedExpressions::MouthUpperRight, "MouthUpperRight"),
        (UnifiedExpressions::MouthUpperLeft, "MouthUpperLeft"),
        (UnifiedExpressions::MouthLowerRight, "MouthLowerRight"),
        (UnifiedExpressions::MouthLowerLeft, "MouthLowerLeft"),
        (
            UnifiedExpressions::MouthCornerPullRight,
            "MouthCornerPullRight",
        ),
        (
            UnifiedExpressions::MouthCornerPullLeft,
            "MouthCornerPullLeft",
        ),
        (
            UnifiedExpressions::MouthCornerSlantRight,
            "MouthCornerSlantRight",
        ),
        (
            UnifiedExpressions::MouthCornerSlantLeft,
            "MouthCornerSlantLeft",
        ),
        (UnifiedExpressions::MouthFrownRight, "MouthFrownRight"),
        (UnifiedExpressions::MouthFrownLeft, "MouthFrownLeft"),
        (UnifiedExpressions::MouthStretchRight, "MouthStretchRight"),
        (UnifiedExpressions::MouthStretchLeft, "MouthStretchLeft"),
        (UnifiedExpressions::MouthDimpleRight, "MouthDimpleRight"),
        (UnifiedExpressions::MouthDimpleLeft, "MouthDimpleLeft"),
        (UnifiedExpressions::MouthRaiserUpper, "MouthRaiserUpper"),
        (UnifiedExpressions::MouthRaiserLower, "MouthRaiserLower"),
        (UnifiedExpressions::MouthPressRight, "MouthPressRight"),
        (UnifiedExpressions::MouthPressLeft, "MouthPressLeft"),
        (
            UnifiedExpressions::MouthTightenerRight,
            "MouthTightenerRight",
        ),
        (UnifiedExpressions::MouthTightenerLeft, "MouthTightenerLeft"),
        // Tongue Expressions
        (UnifiedExpressions::TongueOut, "TongueOut"),
        (UnifiedExpressions::TongueUp, "TongueUp"),
        (UnifiedExpressions::TongueDown, "TongueDown"),
        (UnifiedExpressions::TongueRight, "TongueRight"),
        (UnifiedExpressions::TongueLeft, "TongueLeft"),
        (UnifiedExpressions::TongueRoll, "TongueRoll"),
        (UnifiedExpressions::TongueBendDown, "TongueBendDown"),
        (UnifiedExpressions::TongueCurlUp, "TongueCurlUp"),
        (UnifiedExpressions::TongueSquish, "TongueSquish"),
        (UnifiedExpressions::TongueFlat, "TongueFlat"),
        (UnifiedExpressions::TongueTwistRight, "TongueTwistRight"),
        (UnifiedExpressions::TongueTwistLeft, "TongueTwistLeft"),
        // Throat/Neck Expressions
        (UnifiedExpressions::SoftPalateClose, "SoftPalateClose"),
        (UnifiedExpressions::ThroatSwallow, "ThroatSwallow"),
        (UnifiedExpressions::NeckFlexRight, "NeckFlexRight"),
        (UnifiedExpressions::NeckFlexLeft, "NeckFlexLeft"),
    ];

    expressions
        .into_iter()
        .map(|(expr, name)| {
            let param_name = format!("v2/{}", name);
            let expr_index = expr as usize;
            Box::new(EParam::simple(&param_name, move |d| {
                d.shapes[expr_index].weight
            })) as Box<dyn Parameter>
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_all_unified_expression_params() {
        let params = create_unified_expression_params();
        assert!(
            params.len() >= 70,
            "Should have at least 70 expression params, got {}",
            params.len()
        );
    }
}
