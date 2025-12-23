use crate::data::PicoBlendShape;
use api::{UnifiedExpressions, UnifiedTrackingData};

/// Maps Pico blend shape weights to UnifiedTrackingData
pub fn update_face_data(data: &mut UnifiedTrackingData, weights: &[f32; 72]) {
    let s = &mut data.shapes;

    macro_rules! w {
        ($enum_val:expr) => {
            weights[$enum_val as usize]
        };
    }

    // Eye Expressions
    s[UnifiedExpressions::EyeWideLeft as usize].weight = w!(PicoBlendShape::EyeWideL);
    s[UnifiedExpressions::EyeWideRight as usize].weight = w!(PicoBlendShape::EyeWideR);
    s[UnifiedExpressions::EyeSquintLeft as usize].weight = w!(PicoBlendShape::EyeSquintL);
    s[UnifiedExpressions::EyeSquintRight as usize].weight = w!(PicoBlendShape::EyeSquintR);

    // Eyebrow Expressions
    s[UnifiedExpressions::BrowInnerUpLeft as usize].weight = w!(PicoBlendShape::BrowInnerUp);
    s[UnifiedExpressions::BrowInnerUpRight as usize].weight = w!(PicoBlendShape::BrowInnerUp);
    s[UnifiedExpressions::BrowOuterUpLeft as usize].weight = w!(PicoBlendShape::BrowOuterUpL);
    s[UnifiedExpressions::BrowOuterUpRight as usize].weight = w!(PicoBlendShape::BrowOuterUpR);
    s[UnifiedExpressions::BrowLowererLeft as usize].weight = w!(PicoBlendShape::BrowDownL);
    s[UnifiedExpressions::BrowLowererRight as usize].weight = w!(PicoBlendShape::BrowDownR);

    // Nose Expressions
    s[UnifiedExpressions::NoseSneerLeft as usize].weight = w!(PicoBlendShape::NoseSneerL);
    s[UnifiedExpressions::NoseSneerRight as usize].weight = w!(PicoBlendShape::NoseSneerR);

    // Cheek Expressions
    s[UnifiedExpressions::CheekSquintLeft as usize].weight = w!(PicoBlendShape::CheekSquintL);
    s[UnifiedExpressions::CheekSquintRight as usize].weight = w!(PicoBlendShape::CheekSquintR);
    s[UnifiedExpressions::CheekPuffLeft as usize].weight = w!(PicoBlendShape::CheekPuff);
    s[UnifiedExpressions::CheekPuffRight as usize].weight = w!(PicoBlendShape::CheekPuff);

    // Jaw Expressions
    s[UnifiedExpressions::JawOpen as usize].weight = w!(PicoBlendShape::JawOpen);
    s[UnifiedExpressions::JawLeft as usize].weight = w!(PicoBlendShape::JawLeft);
    s[UnifiedExpressions::JawRight as usize].weight = w!(PicoBlendShape::JawRight);
    s[UnifiedExpressions::JawForward as usize].weight = w!(PicoBlendShape::JawForward);
    s[UnifiedExpressions::MouthClosed as usize].weight = w!(PicoBlendShape::MouthClose);

    // Lip Funnel and Pucker
    s[UnifiedExpressions::LipFunnelUpperLeft as usize].weight = w!(PicoBlendShape::MouthFunnel);
    s[UnifiedExpressions::LipFunnelUpperRight as usize].weight = w!(PicoBlendShape::MouthFunnel);
    s[UnifiedExpressions::LipFunnelLowerLeft as usize].weight = w!(PicoBlendShape::MouthFunnel);
    s[UnifiedExpressions::LipFunnelLowerRight as usize].weight = w!(PicoBlendShape::MouthFunnel);

    s[UnifiedExpressions::LipPuckerUpperLeft as usize].weight = w!(PicoBlendShape::MouthPucker);
    s[UnifiedExpressions::LipPuckerUpperRight as usize].weight = w!(PicoBlendShape::MouthPucker);
    s[UnifiedExpressions::LipPuckerLowerLeft as usize].weight = w!(PicoBlendShape::MouthPucker);
    s[UnifiedExpressions::LipPuckerLowerRight as usize].weight = w!(PicoBlendShape::MouthPucker);

    // Lip Roll and Shrug
    s[UnifiedExpressions::MouthRaiserUpper as usize].weight = w!(PicoBlendShape::MouthRollUpper);
    s[UnifiedExpressions::MouthRaiserLower as usize].weight = w!(PicoBlendShape::MouthRollLower);

    // Upper Mouth
    s[UnifiedExpressions::MouthUpperUpLeft as usize].weight = w!(PicoBlendShape::MouthUpperUpL);
    s[UnifiedExpressions::MouthUpperUpRight as usize].weight = w!(PicoBlendShape::MouthUpperUpR);

    // Lower Mouth
    s[UnifiedExpressions::MouthLowerDownLeft as usize].weight = w!(PicoBlendShape::MouthLowerDownL);
    s[UnifiedExpressions::MouthLowerDownRight as usize].weight =
        w!(PicoBlendShape::MouthLowerDownR);

    // Mouth Direction
    s[UnifiedExpressions::MouthUpperLeft as usize].weight = w!(PicoBlendShape::MouthLeft);
    s[UnifiedExpressions::MouthUpperRight as usize].weight = w!(PicoBlendShape::MouthRight);
    s[UnifiedExpressions::MouthLowerLeft as usize].weight = w!(PicoBlendShape::MouthLeft);
    s[UnifiedExpressions::MouthLowerRight as usize].weight = w!(PicoBlendShape::MouthRight);

    // Smile Expressions
    s[UnifiedExpressions::MouthCornerPullLeft as usize].weight = w!(PicoBlendShape::MouthSmileL);
    s[UnifiedExpressions::MouthCornerPullRight as usize].weight = w!(PicoBlendShape::MouthSmileR);
    s[UnifiedExpressions::MouthCornerSlantLeft as usize].weight = w!(PicoBlendShape::MouthSmileL);
    s[UnifiedExpressions::MouthCornerSlantRight as usize].weight = w!(PicoBlendShape::MouthSmileR);

    // Frown Expressions
    s[UnifiedExpressions::MouthFrownLeft as usize].weight = w!(PicoBlendShape::MouthFrownL);
    s[UnifiedExpressions::MouthFrownRight as usize].weight = w!(PicoBlendShape::MouthFrownR);

    // Stretch Expressions
    s[UnifiedExpressions::MouthStretchLeft as usize].weight = w!(PicoBlendShape::MouthStretchL);
    s[UnifiedExpressions::MouthStretchRight as usize].weight = w!(PicoBlendShape::MouthStretchR);

    // Dimple Expressions
    s[UnifiedExpressions::MouthDimpleLeft as usize].weight = w!(PicoBlendShape::MouthDimpleL);
    s[UnifiedExpressions::MouthDimpleRight as usize].weight = w!(PicoBlendShape::MouthDimpleR);

    // Press Expressions
    s[UnifiedExpressions::MouthPressLeft as usize].weight = w!(PicoBlendShape::MouthPressL);
    s[UnifiedExpressions::MouthPressRight as usize].weight = w!(PicoBlendShape::MouthPressR);

    // Tongue
    s[UnifiedExpressions::TongueOut as usize].weight = w!(PicoBlendShape::TongueOut);
}
