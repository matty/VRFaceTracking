use crate::ffi::{
    EyeData_v2, EyeExpression, LipData_v2, LipShapeV2, SingleEyeDataValidity, Vector3, VerboseData,
};
use api::{UnifiedExpressions, UnifiedTrackingData};

fn flip_x_coordinates(v: Vector3) -> Vector3 {
    Vector3 {
        x: -v.x,
        y: v.y,
        z: v.z,
    }
}

// might not be needed
#[allow(dead_code)]
fn get_convergence_angle_offset(external: &VerboseData) -> Vector3 {
    let left_gaze = flip_x_coordinates(external.left.gaze_direction_normalized);
    let right_gaze = flip_x_coordinates(external.right.gaze_direction_normalized);

    let left_comp = std::f32::consts::FRAC_PI_2 + left_gaze.x.asin();
    let right_comp = std::f32::consts::FRAC_PI_2 - right_gaze.x.asin();

    let dyn_ipd_mm = external.left.gaze_origin_mm.x - external.right.gaze_origin_mm.x;

    if left_comp + right_comp >= std::f32::consts::PI {
        return Vector3::default();
    }

    let right_side_mm =
        right_comp.sin() * dyn_ipd_mm / (std::f32::consts::PI - left_comp - right_comp).sin();
    let left_side_mm =
        left_comp.sin() * dyn_ipd_mm / (std::f32::consts::PI - right_comp - left_comp).sin();

    let convergence_distance_mm = (left_side_mm / 2.0) + (right_side_mm / 2.0);

    if external
        .combined
        .eye_data
        .get_validity(SingleEyeDataValidity::GazeDirectionValidity)
    {
        let x = ((dyn_ipd_mm / 2.0) / convergence_distance_mm).atan();
        return Vector3 { x, y: 0.0, z: 0.0 };
    }

    Vector3::default()
}

pub fn update_eye(data: &mut UnifiedTrackingData, eye_data: &EyeData_v2) {
    let verbose = &eye_data.verbose_data;
    let expression = &eye_data.expression_data;

    update_eye_parameters(data, verbose);
    update_eye_expressions(data, expression);
}

fn update_eye_parameters(data: &mut UnifiedTrackingData, external: &VerboseData) {
    if external
        .left
        .get_validity(SingleEyeDataValidity::EyeOpennessValidity)
    {
        data.eye.left.openness = external.left.eye_openness;
    }
    if external
        .right
        .get_validity(SingleEyeDataValidity::EyeOpennessValidity)
    {
        data.eye.right.openness = external.right.eye_openness;
    }

    if external
        .left
        .get_validity(SingleEyeDataValidity::PupilDiameterValidity)
    {
        data.eye.left.pupil_diameter_mm = external.left.pupil_diameter_mm;
    }
    if external
        .right
        .get_validity(SingleEyeDataValidity::PupilDiameterValidity)
    {
        data.eye.right.pupil_diameter_mm = external.right.pupil_diameter_mm;
    }

    // Gaze Mapping
    // let convergence_offset = get_convergence_angle_offset(external);

    if external
        .left
        .get_validity(SingleEyeDataValidity::GazeDirectionValidity)
    {
        let gaze = flip_x_coordinates(external.left.gaze_direction_normalized);
        data.eye.left.gaze = glam::Vec3::new(gaze.x, gaze.y, gaze.z);
        // data.eye.left.gaze.x += convergence_offset.x;
    }

    if external
        .right
        .get_validity(SingleEyeDataValidity::GazeDirectionValidity)
    {
        let gaze = flip_x_coordinates(external.right.gaze_direction_normalized);
        data.eye.right.gaze = glam::Vec3::new(gaze.x, gaze.y, gaze.z);
        // data.eye.right.gaze.x -= convergence_offset.x;
    }
}

fn update_eye_expressions(data: &mut UnifiedTrackingData, external: &EyeExpression) {
    let s = &mut data.shapes;

    s[UnifiedExpressions::EyeWideLeft as usize].weight = external.left.eye_wide;
    s[UnifiedExpressions::EyeWideRight as usize].weight = external.right.eye_wide;

    s[UnifiedExpressions::EyeSquintLeft as usize].weight = external.left.eye_squeeze;
    s[UnifiedExpressions::EyeSquintRight as usize].weight = external.right.eye_squeeze;

    // Emulator expressions
    s[UnifiedExpressions::BrowInnerUpLeft as usize].weight = external.left.eye_wide;
    s[UnifiedExpressions::BrowOuterUpLeft as usize].weight = external.left.eye_wide;

    s[UnifiedExpressions::BrowInnerUpRight as usize].weight = external.right.eye_wide;
    s[UnifiedExpressions::BrowOuterUpRight as usize].weight = external.right.eye_wide;

    s[UnifiedExpressions::BrowPinchLeft as usize].weight = external.left.eye_squeeze;
    s[UnifiedExpressions::BrowLowererLeft as usize].weight = external.left.eye_squeeze;

    s[UnifiedExpressions::BrowPinchRight as usize].weight = external.right.eye_squeeze;
    s[UnifiedExpressions::BrowLowererRight as usize].weight = external.right.eye_squeeze;
}

pub fn update_lip(data: &mut UnifiedTrackingData, lip_data: &LipData_v2) {
    let weights = &lip_data.prediction_data.blend_shape_weight;
    let s = &mut data.shapes;

    macro_rules! w {
        ($enum_val:expr) => {
            weights[$enum_val as usize]
        };
    }

    // Direct Jaw
    s[UnifiedExpressions::JawOpen as usize].weight =
        w!(LipShapeV2::JawOpen) + w!(LipShapeV2::MouthApeShape);
    s[UnifiedExpressions::JawLeft as usize].weight = w!(LipShapeV2::JawLeft);
    s[UnifiedExpressions::JawRight as usize].weight = w!(LipShapeV2::JawRight);
    s[UnifiedExpressions::JawForward as usize].weight = w!(LipShapeV2::JawForward);
    s[UnifiedExpressions::MouthClosed as usize].weight = w!(LipShapeV2::MouthApeShape);

    // Direct Mouth and Lip
    s[UnifiedExpressions::MouthUpperUpRight as usize].weight =
        w!(LipShapeV2::MouthUpperUpRight) - w!(LipShapeV2::MouthUpperOverturn);
    s[UnifiedExpressions::MouthUpperDeepenRight as usize].weight =
        w!(LipShapeV2::MouthUpperUpRight) - w!(LipShapeV2::MouthUpperOverturn);
    s[UnifiedExpressions::MouthUpperUpLeft as usize].weight =
        w!(LipShapeV2::MouthUpperUpLeft) - w!(LipShapeV2::MouthUpperOverturn);
    s[UnifiedExpressions::MouthUpperDeepenLeft as usize].weight =
        w!(LipShapeV2::MouthUpperUpLeft) - w!(LipShapeV2::MouthUpperOverturn);

    s[UnifiedExpressions::MouthLowerDownLeft as usize].weight =
        w!(LipShapeV2::MouthLowerDownLeft) - w!(LipShapeV2::MouthLowerOverturn);
    s[UnifiedExpressions::MouthLowerDownRight as usize].weight =
        w!(LipShapeV2::MouthLowerDownRight) - w!(LipShapeV2::MouthLowerOverturn);

    let pout = w!(LipShapeV2::MouthPout);
    s[UnifiedExpressions::LipPuckerUpperLeft as usize].weight = pout;
    s[UnifiedExpressions::LipPuckerLowerLeft as usize].weight = pout;
    s[UnifiedExpressions::LipPuckerUpperRight as usize].weight = pout;
    s[UnifiedExpressions::LipPuckerLowerRight as usize].weight = pout;

    let overturn = w!(LipShapeV2::MouthUpperOverturn);
    s[UnifiedExpressions::LipFunnelUpperLeft as usize].weight = overturn;
    s[UnifiedExpressions::LipFunnelUpperRight as usize].weight = overturn;
    s[UnifiedExpressions::LipFunnelLowerLeft as usize].weight = overturn;
    s[UnifiedExpressions::LipFunnelLowerRight as usize].weight = overturn;

    s[UnifiedExpressions::LipSuckUpperLeft as usize].weight = w!(LipShapeV2::MouthUpperInside);
    s[UnifiedExpressions::LipSuckUpperRight as usize].weight = w!(LipShapeV2::MouthUpperInside);
    s[UnifiedExpressions::LipSuckLowerLeft as usize].weight = w!(LipShapeV2::MouthLowerInside);
    s[UnifiedExpressions::LipSuckLowerRight as usize].weight = w!(LipShapeV2::MouthLowerInside);

    s[UnifiedExpressions::MouthUpperLeft as usize].weight = w!(LipShapeV2::MouthUpperLeft);
    s[UnifiedExpressions::MouthUpperRight as usize].weight = w!(LipShapeV2::MouthUpperRight);
    s[UnifiedExpressions::MouthLowerLeft as usize].weight = w!(LipShapeV2::MouthLowerLeft);
    s[UnifiedExpressions::MouthLowerRight as usize].weight = w!(LipShapeV2::MouthLowerRight);

    s[UnifiedExpressions::MouthCornerPullLeft as usize].weight = w!(LipShapeV2::MouthSmileLeft);
    s[UnifiedExpressions::MouthCornerPullRight as usize].weight = w!(LipShapeV2::MouthSmileRight);
    s[UnifiedExpressions::MouthCornerSlantLeft as usize].weight = w!(LipShapeV2::MouthSmileLeft);
    s[UnifiedExpressions::MouthCornerSlantRight as usize].weight = w!(LipShapeV2::MouthSmileRight);
    s[UnifiedExpressions::MouthFrownLeft as usize].weight = w!(LipShapeV2::MouthSadLeft);
    s[UnifiedExpressions::MouthFrownRight as usize].weight = w!(LipShapeV2::MouthSadRight);

    s[UnifiedExpressions::MouthRaiserUpper as usize].weight =
        w!(LipShapeV2::MouthLowerOverlay) - w!(LipShapeV2::MouthUpperInside);
    s[UnifiedExpressions::MouthRaiserLower as usize].weight = w!(LipShapeV2::MouthLowerOverlay);

    // Direct Cheek
    s[UnifiedExpressions::CheekPuffLeft as usize].weight = w!(LipShapeV2::CheekPuffLeft);
    s[UnifiedExpressions::CheekPuffRight as usize].weight = w!(LipShapeV2::CheekPuffRight);
    s[UnifiedExpressions::CheekSuckLeft as usize].weight = w!(LipShapeV2::CheekSuck);
    s[UnifiedExpressions::CheekSuckRight as usize].weight = w!(LipShapeV2::CheekSuck);

    // Direct Tongue
    s[UnifiedExpressions::TongueOut as usize].weight =
        (w!(LipShapeV2::TongueLongStep1) + w!(LipShapeV2::TongueLongStep2)) / 2.0;
    s[UnifiedExpressions::TongueUp as usize].weight = w!(LipShapeV2::TongueUp);
    s[UnifiedExpressions::TongueDown as usize].weight = w!(LipShapeV2::TongueDown);
    s[UnifiedExpressions::TongueLeft as usize].weight = w!(LipShapeV2::TongueLeft);
    s[UnifiedExpressions::TongueRight as usize].weight = w!(LipShapeV2::TongueRight);
    s[UnifiedExpressions::TongueRoll as usize].weight = w!(LipShapeV2::TongueRoll);

    // Emulated Unified Mapping
    s[UnifiedExpressions::CheekSquintLeft as usize].weight = w!(LipShapeV2::MouthSmileLeft);
    s[UnifiedExpressions::CheekSquintRight as usize].weight = w!(LipShapeV2::MouthSmileRight);

    s[UnifiedExpressions::MouthDimpleLeft as usize].weight = w!(LipShapeV2::MouthSmileLeft);
    s[UnifiedExpressions::MouthDimpleRight as usize].weight = w!(LipShapeV2::MouthSmileRight);

    s[UnifiedExpressions::MouthStretchLeft as usize].weight = w!(LipShapeV2::MouthSadRight);
    s[UnifiedExpressions::MouthStretchRight as usize].weight = w!(LipShapeV2::MouthSadRight);
}
