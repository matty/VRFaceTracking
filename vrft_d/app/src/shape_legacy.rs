use common::{UnifiedExpressions, UnifiedTrackingData};

fn get_shape_weight(data: &UnifiedTrackingData, expr: UnifiedExpressions) -> f32 {
    data.shapes[expr as usize].weight
}

fn calculate_bipolar_weight(
    data: &UnifiedTrackingData,
    pos: UnifiedExpressions,
    neg: UnifiedExpressions,
) -> f32 {
    get_shape_weight(data, pos) - get_shape_weight(data, neg)
}

fn calculate_composite_bipolar_weight(
    data: &UnifiedTrackingData,
    pos: &[UnifiedExpressions],
    neg: &[UnifiedExpressions],
    use_max: bool,
) -> f32 {
    if !use_max {
        let pos_sum: f32 = pos.iter().map(|&e| get_shape_weight(data, e)).sum();
        let neg_sum: f32 = neg.iter().map(|&e| -get_shape_weight(data, e)).sum();
        (pos_sum / pos.len() as f32) + (neg_sum / neg.len() as f32)
    } else {
        let pos_max = pos
            .iter()
            .map(|&e| get_shape_weight(data, e))
            .fold(0.0f32, |a, b| a.max(b));
        let neg_max = neg
            .iter()
            .map(|&e| get_shape_weight(data, e))
            .fold(0.0f32, |a, b| a.max(b));
        pos_max - neg_max
    }
}

pub fn get_v1_parameters(data: &UnifiedTrackingData) -> Vec<(&'static str, f32)> {
    let mut params = Vec::with_capacity(100);

    let cheek_suck = (get_shape_weight(data, UnifiedExpressions::CheekSuckLeft)
        + get_shape_weight(data, UnifiedExpressions::CheekSuckRight))
        / 2.0;

    let mouth_upper_inside = (get_shape_weight(data, UnifiedExpressions::LipSuckUpperLeft)
        + get_shape_weight(data, UnifiedExpressions::LipSuckUpperRight))
        / 2.0;

    let mouth_lower_inside = (get_shape_weight(data, UnifiedExpressions::LipSuckLowerLeft)
        + get_shape_weight(data, UnifiedExpressions::LipSuckLowerRight))
        / 2.0;

    let mouth_ape_shape = get_shape_weight(data, UnifiedExpressions::MouthClosed);
    let mouth_lower_overlay = get_shape_weight(data, UnifiedExpressions::MouthRaiserLower);

    let mouth_pout = (get_shape_weight(data, UnifiedExpressions::LipPuckerUpperLeft)
        + get_shape_weight(data, UnifiedExpressions::LipPuckerUpperRight)
        + get_shape_weight(data, UnifiedExpressions::LipPuckerLowerLeft)
        + get_shape_weight(data, UnifiedExpressions::LipPuckerLowerRight))
        / 4.0;

    let mouth_upper_overturn = (get_shape_weight(data, UnifiedExpressions::LipFunnelUpperLeft)
        + get_shape_weight(data, UnifiedExpressions::LipFunnelUpperRight))
        / 2.0;

    let mouth_lower_overturn = (get_shape_weight(data, UnifiedExpressions::LipFunnelLowerLeft)
        + get_shape_weight(data, UnifiedExpressions::LipFunnelLowerRight))
        / 2.0;

    let mouth_smile_right = get_shape_weight(data, UnifiedExpressions::MouthCornerPullRight) * 0.8
        + get_shape_weight(data, UnifiedExpressions::MouthCornerSlantRight) * 0.2;
    let mouth_smile_left = get_shape_weight(data, UnifiedExpressions::MouthCornerPullLeft) * 0.8
        + get_shape_weight(data, UnifiedExpressions::MouthCornerSlantLeft) * 0.2;

    let mouth_sad_right = if get_shape_weight(data, UnifiedExpressions::MouthFrownRight)
        > get_shape_weight(data, UnifiedExpressions::MouthStretchRight)
    {
        get_shape_weight(data, UnifiedExpressions::MouthFrownRight)
    } else {
        get_shape_weight(data, UnifiedExpressions::MouthStretchRight)
    };
    let mouth_sad_left = if get_shape_weight(data, UnifiedExpressions::MouthFrownLeft)
        > get_shape_weight(data, UnifiedExpressions::MouthStretchLeft)
    {
        get_shape_weight(data, UnifiedExpressions::MouthFrownLeft)
    } else {
        get_shape_weight(data, UnifiedExpressions::MouthStretchLeft)
    };

    params.push((
        "JawX",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::JawRight,
            UnifiedExpressions::JawLeft,
        ),
    ));
    params.push((
        "JawOpen",
        get_shape_weight(data, UnifiedExpressions::JawOpen),
    ));
    params.push((
        "MouthUpper",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::MouthUpperRight,
            UnifiedExpressions::MouthUpperLeft,
        ),
    ));
    params.push((
        "MouthLower",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::MouthLowerRight,
            UnifiedExpressions::MouthLowerLeft,
        ),
    ));
    params.push((
        "MouthX",
        calculate_composite_bipolar_weight(
            data,
            &[
                UnifiedExpressions::MouthUpperRight,
                UnifiedExpressions::MouthLowerRight,
            ],
            &[
                UnifiedExpressions::MouthUpperLeft,
                UnifiedExpressions::MouthLowerLeft,
            ],
            true,
        ),
    ));

    params.push(("SmileSadRight", mouth_smile_right - mouth_sad_right));
    params.push(("SmileSadLeft", mouth_smile_left - mouth_sad_left));
    params.push((
        "SmileSad",
        (mouth_smile_left + mouth_smile_right) / 2.0 - (mouth_sad_left + mouth_sad_right) / 2.0,
    ));

    params.push((
        "TongueY",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::TongueUp,
            UnifiedExpressions::TongueDown,
        ),
    ));
    params.push((
        "TongueX",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::TongueRight,
            UnifiedExpressions::TongueLeft,
        ),
    ));

    params.push((
        "PuffSuckRight",
        get_shape_weight(data, UnifiedExpressions::CheekPuffRight) - cheek_suck,
    ));
    params.push((
        "PuffSuckLeft",
        get_shape_weight(data, UnifiedExpressions::CheekPuffLeft) - cheek_suck,
    ));
    let puff_max = get_shape_weight(data, UnifiedExpressions::CheekPuffLeft)
        .max(get_shape_weight(data, UnifiedExpressions::CheekPuffRight));
    params.push(("PuffSuck", puff_max - cheek_suck));

    params.push((
        "JawOpenApe",
        get_shape_weight(data, UnifiedExpressions::JawOpen) - mouth_ape_shape,
    ));
    params.push((
        "JawOpenPuff",
        calculate_composite_bipolar_weight(
            data,
            &[UnifiedExpressions::JawOpen],
            &[
                UnifiedExpressions::CheekPuffLeft,
                UnifiedExpressions::CheekPuffRight,
            ],
            false,
        ),
    ));
    params.push((
        "JawOpenPuffRight",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::JawOpen,
            UnifiedExpressions::CheekPuffRight,
        ),
    ));
    params.push((
        "JawOpenPuffLeft",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::JawOpen,
            UnifiedExpressions::CheekPuffLeft,
        ),
    ));
    params.push((
        "JawOpenSuck",
        get_shape_weight(data, UnifiedExpressions::JawOpen) - cheek_suck,
    ));
    params.push((
        "JawOpenForward",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::JawOpen,
            UnifiedExpressions::JawForward,
        ),
    ));
    params.push((
        "JawOpenOverlay",
        get_shape_weight(data, UnifiedExpressions::JawOpen) - mouth_lower_overlay,
    ));

    params.push((
        "MouthUpperUpRightUpperInside",
        get_shape_weight(data, UnifiedExpressions::MouthUpperUpRight) - mouth_upper_inside,
    ));
    params.push((
        "MouthUpperUpRightPuffRight",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::MouthUpperUpRight,
            UnifiedExpressions::CheekPuffRight,
        ),
    ));
    params.push((
        "MouthUpperUpRightApe",
        get_shape_weight(data, UnifiedExpressions::MouthUpperUpRight) - mouth_ape_shape,
    ));
    params.push((
        "MouthUpperUpRightPout",
        get_shape_weight(data, UnifiedExpressions::MouthUpperUpRight) - mouth_pout,
    ));
    params.push((
        "MouthUpperUpRightOverlay",
        get_shape_weight(data, UnifiedExpressions::MouthUpperUpRight) - mouth_lower_overlay,
    ));
    params.push((
        "MouthUpperUpRightSuck",
        get_shape_weight(data, UnifiedExpressions::MouthUpperUpRight) - cheek_suck,
    ));

    params.push((
        "MouthUpperUpLeftUpperInside",
        get_shape_weight(data, UnifiedExpressions::MouthUpperUpLeft) - mouth_upper_inside,
    ));
    params.push((
        "MouthUpperUpLeftPuffLeft",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::MouthUpperUpLeft,
            UnifiedExpressions::CheekPuffLeft,
        ),
    ));
    params.push((
        "MouthUpperUpLeftApe",
        get_shape_weight(data, UnifiedExpressions::MouthUpperUpLeft) - mouth_ape_shape,
    ));
    params.push((
        "MouthUpperUpLeftPout",
        get_shape_weight(data, UnifiedExpressions::MouthUpperUpLeft) - mouth_pout,
    ));
    params.push((
        "MouthUpperUpLeftOverlay",
        get_shape_weight(data, UnifiedExpressions::MouthUpperUpLeft) - mouth_lower_overlay,
    ));
    params.push((
        "MouthUpperUpLeftSuck",
        get_shape_weight(data, UnifiedExpressions::MouthUpperUpLeft) - cheek_suck,
    ));

    let mouth_upper_up_avg = (get_shape_weight(data, UnifiedExpressions::MouthUpperUpLeft)
        + get_shape_weight(data, UnifiedExpressions::MouthUpperUpRight))
        / 2.0;
    params.push((
        "MouthUpperUpUpperInside",
        mouth_upper_up_avg - mouth_upper_inside,
    ));

    let pos_max_upper_up = get_shape_weight(data, UnifiedExpressions::MouthUpperUpLeft).max(
        get_shape_weight(data, UnifiedExpressions::MouthUpperUpRight),
    );
    let neg_max_inside = mouth_upper_inside.max(mouth_lower_inside);
    params.push(("MouthUpperUpInside", pos_max_upper_up - neg_max_inside));

    params.push((
        "MouthUpperUpPuff",
        calculate_composite_bipolar_weight(
            data,
            &[
                UnifiedExpressions::MouthUpperUpLeft,
                UnifiedExpressions::MouthUpperUpRight,
            ],
            &[
                UnifiedExpressions::CheekPuffLeft,
                UnifiedExpressions::CheekPuffRight,
            ],
            false,
        ),
    ));
    params.push((
        "MouthUpperUpPuffLeft",
        calculate_composite_bipolar_weight(
            data,
            &[
                UnifiedExpressions::MouthUpperUpLeft,
                UnifiedExpressions::MouthUpperUpRight,
            ],
            &[UnifiedExpressions::CheekPuffLeft],
            false,
        ),
    ));
    params.push((
        "MouthUpperUpPuffRight",
        calculate_composite_bipolar_weight(
            data,
            &[
                UnifiedExpressions::MouthUpperUpLeft,
                UnifiedExpressions::MouthUpperUpRight,
            ],
            &[UnifiedExpressions::CheekPuffRight],
            false,
        ),
    ));
    params.push(("MouthUpperUpApe", mouth_upper_up_avg - mouth_ape_shape));
    params.push(("MouthUpperUpPout", mouth_upper_up_avg - mouth_pout));
    params.push((
        "MouthUpperUpOverlay",
        mouth_upper_up_avg - mouth_lower_overlay,
    ));
    params.push(("MouthUpperUpSuck", mouth_upper_up_avg - cheek_suck));

    params.push((
        "MouthLowerDownRightLowerInside",
        get_shape_weight(data, UnifiedExpressions::MouthLowerDownRight) - mouth_lower_inside,
    ));
    params.push((
        "MouthLowerDownRightPuffRight",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::MouthLowerDownRight,
            UnifiedExpressions::CheekPuffRight,
        ),
    ));
    params.push((
        "MouthLowerDownRightApe",
        get_shape_weight(data, UnifiedExpressions::MouthLowerDownRight) - mouth_ape_shape,
    ));
    params.push((
        "MouthLowerDownRightPout",
        get_shape_weight(data, UnifiedExpressions::MouthLowerDownRight) - mouth_pout,
    ));
    params.push((
        "MouthLowerDownRightOverlay",
        get_shape_weight(data, UnifiedExpressions::MouthLowerDownRight) - mouth_lower_overlay,
    ));
    params.push((
        "MouthLowerDownRightSuck",
        get_shape_weight(data, UnifiedExpressions::MouthLowerDownRight) - cheek_suck,
    ));

    params.push((
        "MouthLowerDownLeftLowerInside",
        get_shape_weight(data, UnifiedExpressions::MouthLowerDownLeft) - mouth_lower_inside,
    ));
    params.push((
        "MouthLowerDownLeftPuffLeft",
        calculate_bipolar_weight(
            data,
            UnifiedExpressions::MouthLowerDownLeft,
            UnifiedExpressions::CheekPuffLeft,
        ),
    ));
    params.push((
        "MouthLowerDownLeftApe",
        get_shape_weight(data, UnifiedExpressions::MouthLowerDownLeft) - mouth_ape_shape,
    ));
    params.push((
        "MouthLowerDownLeftPout",
        get_shape_weight(data, UnifiedExpressions::MouthLowerDownLeft) - mouth_pout,
    ));
    params.push((
        "MouthLowerDownLeftOverlay",
        get_shape_weight(data, UnifiedExpressions::MouthLowerDownLeft) - mouth_lower_overlay,
    ));
    params.push((
        "MouthLowerDownLeftSuck",
        get_shape_weight(data, UnifiedExpressions::MouthLowerDownLeft) - cheek_suck,
    ));

    let mouth_lower_down_avg = (get_shape_weight(data, UnifiedExpressions::MouthLowerDownLeft)
        + get_shape_weight(data, UnifiedExpressions::MouthLowerDownRight))
        / 2.0;
    params.push((
        "MouthLowerDownLowerInside",
        mouth_lower_down_avg - mouth_lower_inside,
    ));

    let pos_max_lower_down = get_shape_weight(data, UnifiedExpressions::MouthLowerDownLeft).max(
        get_shape_weight(data, UnifiedExpressions::MouthLowerDownRight),
    );
    params.push(("MouthLowerDownInside", pos_max_lower_down - neg_max_inside));

    params.push((
        "MouthLowerDownPuff",
        calculate_composite_bipolar_weight(
            data,
            &[
                UnifiedExpressions::MouthLowerDownLeft,
                UnifiedExpressions::MouthLowerDownRight,
            ],
            &[
                UnifiedExpressions::CheekPuffLeft,
                UnifiedExpressions::CheekPuffRight,
            ],
            false,
        ),
    ));
    params.push((
        "MouthLowerDownPuffLeft",
        calculate_composite_bipolar_weight(
            data,
            &[
                UnifiedExpressions::MouthLowerDownLeft,
                UnifiedExpressions::MouthLowerDownRight,
            ],
            &[UnifiedExpressions::CheekPuffLeft],
            false,
        ),
    ));
    params.push((
        "MouthLowerDownPuffRight",
        calculate_composite_bipolar_weight(
            data,
            &[
                UnifiedExpressions::MouthLowerDownLeft,
                UnifiedExpressions::MouthLowerDownRight,
            ],
            &[UnifiedExpressions::CheekPuffRight],
            false,
        ),
    ));
    params.push(("MouthLowerDownApe", mouth_lower_down_avg - mouth_ape_shape));
    params.push(("MouthLowerDownPout", mouth_lower_down_avg - mouth_pout));
    params.push((
        "MouthLowerDownOverlay",
        mouth_lower_down_avg - mouth_lower_overlay,
    ));
    params.push(("MouthLowerDownSuck", mouth_lower_down_avg - cheek_suck));

    params.push((
        "MouthUpperInsideOverturn",
        mouth_upper_inside - mouth_upper_overturn,
    ));
    params.push((
        "MouthLowerInsideOverturn",
        mouth_lower_inside - mouth_lower_overturn,
    ));

    params.push((
        "SmileRightUpperOverturn",
        mouth_smile_right - mouth_upper_overturn,
    ));
    params.push((
        "SmileRightLowerOverturn",
        mouth_smile_right - mouth_lower_overturn,
    ));
    params.push((
        "SmileRightOverturn",
        mouth_smile_right - (mouth_upper_overturn + mouth_lower_overturn) / 2.0,
    ));
    params.push(("SmileRightApe", mouth_smile_right - mouth_ape_shape));
    params.push(("SmileRightOverlay", mouth_smile_right - mouth_lower_overlay));
    params.push(("SmileRightPout", mouth_smile_right - mouth_pout));

    params.push((
        "SmileLeftUpperOverturn",
        mouth_smile_left - mouth_upper_overturn,
    ));
    params.push((
        "SmileLeftLowerOverturn",
        mouth_smile_left - mouth_lower_overturn,
    ));
    params.push((
        "SmileLeftOverturn",
        mouth_smile_left - (mouth_upper_overturn + mouth_lower_overturn) / 2.0,
    ));
    params.push(("SmileLeftApe", mouth_smile_left - mouth_ape_shape));
    params.push(("SmileLeftOverlay", mouth_smile_left - mouth_lower_overlay));
    params.push(("SmileLeftPout", mouth_smile_left - mouth_pout));

    let smile_avg = (mouth_smile_left + mouth_smile_right) / 2.0;
    params.push(("SmileUpperOverturn", smile_avg - mouth_upper_overturn));
    params.push(("SmileLowerOverturn", smile_avg - mouth_lower_overturn));
    params.push((
        "SmileOverturn",
        smile_avg - (mouth_upper_overturn + mouth_lower_overturn) / 2.0,
    ));
    params.push(("SmileApe", smile_avg - mouth_ape_shape));
    params.push(("SmileOverlay", smile_avg - mouth_lower_overlay));
    params.push(("SmilePout", smile_avg - mouth_pout));

    params.push((
        "PuffRightUpperOverturn",
        get_shape_weight(data, UnifiedExpressions::CheekPuffRight) - mouth_upper_overturn,
    ));
    params.push((
        "PuffRightLowerOverturn",
        get_shape_weight(data, UnifiedExpressions::CheekPuffRight) - mouth_lower_overturn,
    ));
    let overturn_max = mouth_upper_overturn.max(mouth_lower_overturn);
    params.push((
        "PuffRightOverturn",
        get_shape_weight(data, UnifiedExpressions::CheekPuffRight) - overturn_max,
    ));

    params.push((
        "PuffLeftUpperOverturn",
        get_shape_weight(data, UnifiedExpressions::CheekPuffLeft) - mouth_upper_overturn,
    ));
    params.push((
        "PuffLeftLowerOverturn",
        get_shape_weight(data, UnifiedExpressions::CheekPuffLeft) - mouth_lower_overturn,
    ));
    params.push((
        "PuffLeftOverturn",
        get_shape_weight(data, UnifiedExpressions::CheekPuffLeft) - overturn_max,
    ));

    let puff_avg = (get_shape_weight(data, UnifiedExpressions::CheekPuffLeft)
        + get_shape_weight(data, UnifiedExpressions::CheekPuffRight))
        / 2.0;
    params.push(("PuffUpperOverturn", puff_avg - mouth_upper_overturn));
    params.push(("PuffLowerOverturn", puff_avg - mouth_lower_overturn));
    params.push(("PuffOverturn", puff_avg - overturn_max));

    let tongue_out = get_shape_weight(data, UnifiedExpressions::TongueOut);
    let tongue_long_step1 = (tongue_out * 2.0).min(1.0);
    let tongue_long_step2 = ((tongue_out * 2.0) - 1.0).clamp(0.0, 1.0);
    params.push(("TongueSteps", (tongue_long_step1 - tongue_long_step2) - 1.0));

    params
}

fn squeeze(openness: f32, squint: f32) -> f32 {
    (1.0 - openness.powf(0.15)) * squint
}

pub fn get_v1_eye_parameters(data: &UnifiedTrackingData) -> Vec<(&'static str, f32)> {
    let mut params = Vec::with_capacity(30);

    params.push((
        "LeftEyeWiden",
        get_shape_weight(data, UnifiedExpressions::EyeWideLeft),
    ));
    params.push((
        "RightEyeWiden",
        get_shape_weight(data, UnifiedExpressions::EyeWideRight),
    ));
    params.push((
        "EyeWiden",
        get_shape_weight(data, UnifiedExpressions::EyeWideLeft)
            .max(get_shape_weight(data, UnifiedExpressions::EyeWideRight)),
    ));

    params.push((
        "LeftEyeSqueeze",
        get_shape_weight(data, UnifiedExpressions::EyeSquintLeft),
    ));
    params.push((
        "RightEyeSqueeze",
        get_shape_weight(data, UnifiedExpressions::EyeSquintRight),
    ));
    params.push((
        "EyesSqueeze",
        get_shape_weight(data, UnifiedExpressions::EyeSquintLeft)
            .max(get_shape_weight(data, UnifiedExpressions::EyeSquintRight)),
    ));

    params.push(("LeftEyeLid", 1.0 - data.eye.left.openness));
    params.push(("RightEyeLid", 1.0 - data.eye.right.openness));
    params.push((
        "CombinedEyeLid",
        (1.0 - data.eye.left.openness + 1.0 - data.eye.right.openness) / 2.0,
    ));

    params.push(("EyesDilation", data.eye.max_dilation));
    params.push((
        "EyesPupilDiameter",
        (data.eye.left.pupil_diameter_mm + data.eye.right.pupil_diameter_mm) / 2.0,
    ));

    let left_squeeze = squeeze(
        data.eye.left.openness,
        get_shape_weight(data, UnifiedExpressions::EyeSquintLeft),
    );
    let right_squeeze = squeeze(
        data.eye.right.openness,
        get_shape_weight(data, UnifiedExpressions::EyeSquintRight),
    );

    params.push((
        "LeftEyeLidExpanded",
        get_shape_weight(data, UnifiedExpressions::EyeWideLeft) - left_squeeze,
    ));
    params.push((
        "RightEyeLidExpanded",
        get_shape_weight(data, UnifiedExpressions::EyeWideRight) - right_squeeze,
    ));
    params.push((
        "EyeLidExpanded",
        (get_shape_weight(data, UnifiedExpressions::EyeWideLeft)
            + get_shape_weight(data, UnifiedExpressions::EyeWideRight))
            / 2.0
            - (left_squeeze + right_squeeze) / 2.0,
    ));

    params.push((
        "EyeLidExpandedSqueeze",
        (left_squeeze + right_squeeze) / 2.0,
    ));

    params.push((
        "BrowsInnerUp",
        (get_shape_weight(data, UnifiedExpressions::BrowInnerUpLeft)
            + get_shape_weight(data, UnifiedExpressions::BrowInnerUpRight))
            / 2.0,
    ));
    params.push((
        "BrowsOuterUp",
        (get_shape_weight(data, UnifiedExpressions::BrowOuterUpLeft)
            + get_shape_weight(data, UnifiedExpressions::BrowOuterUpRight))
            / 2.0,
    ));
    params.push((
        "BrowsDown",
        (get_shape_weight(data, UnifiedExpressions::BrowLowererLeft)
            + get_shape_weight(data, UnifiedExpressions::BrowLowererRight))
            / 2.0,
    ));

    params.push((
        "MouthRaiserLower",
        get_shape_weight(data, UnifiedExpressions::MouthRaiserLower),
    ));
    params.push((
        "MouthRaiserUpper",
        get_shape_weight(data, UnifiedExpressions::MouthRaiserUpper),
    ));

    params.push((
        "EyesSquint",
        (get_shape_weight(data, UnifiedExpressions::EyeSquintLeft)
            + get_shape_weight(data, UnifiedExpressions::EyeSquintRight))
            / 2.0,
    ));
    params.push((
        "CheeksSquint",
        (get_shape_weight(data, UnifiedExpressions::CheekSquintLeft)
            + get_shape_weight(data, UnifiedExpressions::CheekSquintRight))
            / 2.0,
    ));

    params.push((
        "MouthDimple",
        (get_shape_weight(data, UnifiedExpressions::MouthDimpleLeft)
            + get_shape_weight(data, UnifiedExpressions::MouthDimpleRight))
            / 2.0,
    ));
    params.push((
        "MouthPress",
        (get_shape_weight(data, UnifiedExpressions::MouthPressLeft)
            + get_shape_weight(data, UnifiedExpressions::MouthPressRight))
            / 2.0,
    ));
    params.push((
        "MouthStretch",
        (get_shape_weight(data, UnifiedExpressions::MouthStretchLeft)
            + get_shape_weight(data, UnifiedExpressions::MouthStretchRight))
            / 2.0,
    ));
    params.push((
        "MouthTightener",
        (get_shape_weight(data, UnifiedExpressions::MouthTightenerLeft)
            + get_shape_weight(data, UnifiedExpressions::MouthTightenerRight))
            / 2.0,
    ));
    params.push((
        "NoseSneer",
        (get_shape_weight(data, UnifiedExpressions::NoseSneerLeft)
            + get_shape_weight(data, UnifiedExpressions::NoseSneerRight))
            / 2.0,
    ));

    params
}

pub fn get_v1_sranipal_lip_parameters(data: &UnifiedTrackingData) -> Vec<(&'static str, f32)> {
    let mut params = Vec::with_capacity(40);

    params.push((
        "JawRight",
        get_shape_weight(data, UnifiedExpressions::JawRight),
    ));
    params.push((
        "JawLeft",
        get_shape_weight(data, UnifiedExpressions::JawLeft),
    ));
    params.push((
        "JawForward",
        get_shape_weight(data, UnifiedExpressions::JawForward),
    ));
    params.push((
        "JawOpen",
        (get_shape_weight(data, UnifiedExpressions::JawOpen)
            - get_shape_weight(data, UnifiedExpressions::MouthClosed))
        .clamp(0.0, 1.0),
    ));
    params.push((
        "MouthApeShape",
        get_shape_weight(data, UnifiedExpressions::MouthClosed),
    ));

    params.push((
        "MouthUpperRight",
        get_shape_weight(data, UnifiedExpressions::MouthUpperRight),
    ));
    params.push((
        "MouthUpperLeft",
        get_shape_weight(data, UnifiedExpressions::MouthUpperLeft),
    ));
    params.push((
        "MouthLowerRight",
        get_shape_weight(data, UnifiedExpressions::MouthLowerRight),
    ));
    params.push((
        "MouthLowerLeft",
        get_shape_weight(data, UnifiedExpressions::MouthLowerLeft),
    ));

    params.push((
        "MouthUpperOverturn",
        (get_shape_weight(data, UnifiedExpressions::LipFunnelUpperLeft)
            + get_shape_weight(data, UnifiedExpressions::LipFunnelUpperRight))
            / 2.0,
    ));
    params.push((
        "MouthLowerOverturn",
        (get_shape_weight(data, UnifiedExpressions::LipFunnelLowerLeft)
            + get_shape_weight(data, UnifiedExpressions::LipFunnelLowerRight))
            / 2.0,
    ));

    params.push((
        "MouthPout",
        (get_shape_weight(data, UnifiedExpressions::LipPuckerUpperLeft)
            + get_shape_weight(data, UnifiedExpressions::LipPuckerUpperRight)
            + get_shape_weight(data, UnifiedExpressions::LipPuckerLowerLeft)
            + get_shape_weight(data, UnifiedExpressions::LipPuckerLowerRight))
            / 4.0,
    ));

    let smile_right_simple = get_shape_weight(data, UnifiedExpressions::MouthCornerPullRight) * 0.8
        + get_shape_weight(data, UnifiedExpressions::MouthCornerSlantRight) * 0.2;
    let dimple_right = get_shape_weight(data, UnifiedExpressions::MouthDimpleRight);
    params.push((
        "MouthSmileRight",
        if smile_right_simple > dimple_right {
            smile_right_simple
        } else {
            dimple_right
        },
    ));

    let smile_left_simple = get_shape_weight(data, UnifiedExpressions::MouthCornerPullLeft) * 0.8
        + get_shape_weight(data, UnifiedExpressions::MouthCornerSlantLeft) * 0.2;
    let dimple_left = get_shape_weight(data, UnifiedExpressions::MouthDimpleLeft);
    params.push((
        "MouthSmileLeft",
        if smile_left_simple > dimple_left {
            smile_left_simple
        } else {
            dimple_left
        },
    ));

    let frown_avg = (get_shape_weight(data, UnifiedExpressions::MouthFrownRight)
        + get_shape_weight(data, UnifiedExpressions::MouthFrownLeft))
        / 2.0;
    let stretch_right = get_shape_weight(data, UnifiedExpressions::MouthStretchRight);
    let sad_base_right = if frown_avg > stretch_right {
        frown_avg
    } else {
        stretch_right
    };
    params.push((
        "MouthSadRight",
        (sad_base_right - smile_right_simple).max(0.0),
    ));

    let stretch_left = get_shape_weight(data, UnifiedExpressions::MouthStretchLeft);
    let sad_base_left = if frown_avg > stretch_left {
        frown_avg
    } else {
        stretch_left
    };
    params.push(("MouthSadLeft", (sad_base_left - smile_left_simple).max(0.0)));

    params.push((
        "CheekPuffLeft",
        get_shape_weight(data, UnifiedExpressions::CheekPuffLeft),
    ));
    params.push((
        "CheekPuffRight",
        get_shape_weight(data, UnifiedExpressions::CheekPuffRight),
    ));
    params.push((
        "CheekSuck",
        (get_shape_weight(data, UnifiedExpressions::CheekSuckLeft)
            + get_shape_weight(data, UnifiedExpressions::CheekSuckRight))
            / 2.0,
    ));

    params.push((
        "MouthUpperUpRight",
        (get_shape_weight(data, UnifiedExpressions::MouthUpperUpRight)
            + (1.0 - get_shape_weight(data, UnifiedExpressions::LipPuckerUpperRight))
                * get_shape_weight(data, UnifiedExpressions::LipFunnelUpperRight))
        .max(0.0),
    ));

    params.push((
        "MouthUpperUpLeft",
        (get_shape_weight(data, UnifiedExpressions::MouthUpperUpLeft)
            + (1.0 - get_shape_weight(data, UnifiedExpressions::LipPuckerUpperLeft))
                * get_shape_weight(data, UnifiedExpressions::LipFunnelUpperLeft))
        .max(0.0),
    ));

    params.push((
        "MouthLowerDownRight",
        (get_shape_weight(data, UnifiedExpressions::MouthLowerDownRight)
            + (1.0 - get_shape_weight(data, UnifiedExpressions::LipPuckerLowerRight))
                * get_shape_weight(data, UnifiedExpressions::LipFunnelLowerRight))
        .max(0.0),
    ));

    params.push((
        "MouthLowerDownLeft",
        (get_shape_weight(data, UnifiedExpressions::MouthLowerDownLeft)
            + (1.0 - get_shape_weight(data, UnifiedExpressions::LipPuckerLowerLeft))
                * get_shape_weight(data, UnifiedExpressions::LipFunnelLowerLeft))
        .max(0.0),
    ));

    params.push((
        "MouthUpperInside",
        ((get_shape_weight(data, UnifiedExpressions::LipSuckUpperLeft)
            + get_shape_weight(data, UnifiedExpressions::LipSuckUpperRight))
            / 2.0)
            .max(0.0),
    ));
    params.push((
        "MouthLowerInside",
        ((get_shape_weight(data, UnifiedExpressions::LipSuckLowerLeft)
            + get_shape_weight(data, UnifiedExpressions::LipSuckLowerRight))
            / 2.0)
            .max(0.0),
    ));

    params.push((
        "MouthLowerOverlay",
        get_shape_weight(data, UnifiedExpressions::MouthRaiserLower),
    ));

    params.push((
        "TongueLongStep1",
        (get_shape_weight(data, UnifiedExpressions::TongueOut) * 2.0).min(1.0),
    ));
    params.push((
        "TongueLongStep2",
        ((get_shape_weight(data, UnifiedExpressions::TongueOut) * 2.0) - 1.0).clamp(0.0, 1.0),
    ));

    params.push((
        "TongueDown",
        get_shape_weight(data, UnifiedExpressions::TongueDown),
    ));
    params.push((
        "TongueUp",
        get_shape_weight(data, UnifiedExpressions::TongueUp),
    ));
    params.push((
        "TongueRight",
        get_shape_weight(data, UnifiedExpressions::TongueRight),
    ));
    params.push((
        "TongueLeft",
        get_shape_weight(data, UnifiedExpressions::TongueLeft),
    ));
    params.push((
        "TongueRoll",
        get_shape_weight(data, UnifiedExpressions::TongueRoll),
    ));

    params
}
