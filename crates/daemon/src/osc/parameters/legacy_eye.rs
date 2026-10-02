//! Legacy eye tracking parameters for backwards compatibility with older avatars.

use super::base_param::BoolParam;
use super::binary_param::BinaryBaseParameter;
use super::eparam::EParam;
use super::Parameter;
use vrft_common::{UnifiedExpressions, UnifiedTrackingData};

#[derive(Debug, Clone, Copy)]
enum Side {
    Left,
    Right,
}

impl Side {
    fn openness(self, data: &UnifiedTrackingData) -> f32 {
        match self {
            Side::Left => data.eye.left.openness,
            Side::Right => data.eye.right.openness,
        }
    }

    fn wide(self, data: &UnifiedTrackingData) -> f32 {
        data.weight(match self {
            Side::Left => UnifiedExpressions::EyeWideLeft,
            Side::Right => UnifiedExpressions::EyeWideRight,
        })
    }

    fn squint(self, data: &UnifiedTrackingData) -> f32 {
        data.weight(match self {
            Side::Left => UnifiedExpressions::EyeSquintLeft,
            Side::Right => UnifiedExpressions::EyeSquintRight,
        })
    }
}

/// Eyelid value expanded past fully open: openness drives the first 0.8,
/// EyeWide the remaining 0.2
fn lid(data: &UnifiedTrackingData, side: Side) -> f32 {
    side.wide(data) * 0.2 + side.openness(data) * 0.8
}

/// Calculate the squeeze factor for eye lid calculations
fn squeeze(data: &UnifiedTrackingData, side: Side) -> f32 {
    // Openness is clamped first: a negative value would make powf NaN
    (1.0 - side.openness(data).clamp(0.0, 1.0).powf(0.15)) * side.squint(data)
}

/// [`lid`] extended below closed by the squeeze factor
fn lid_squeeze(data: &UnifiedTrackingData, side: Side) -> f32 {
    lid(data, side) - squeeze(data, side)
}

/// Average of both eyes' [`lid_squeeze`]
fn combined_lid_squeeze(data: &UnifiedTrackingData) -> f32 {
    ((Side::Left.wide(data) + Side::Right.wide(data)) * 0.2
        + (Side::Left.openness(data) + Side::Right.openness(data)) * 0.8
        - squeeze(data, Side::Left)
        - squeeze(data, Side::Right))
        * 0.5
}

/// How a combined parameter merges its Left and Right values
#[derive(Clone, Copy)]
enum Combine {
    Avg,
    Max,
}

/// Combined, Left and Right parameter names, each side's shapes (averaged),
/// and how the combined parameter merges the two sides
type Triplet = (
    &'static str,
    &'static str,
    &'static str,
    &'static [UnifiedExpressions],
    &'static [UnifiedExpressions],
    Combine,
);

/// Quest Pro Legacy Brow Parameters
#[rustfmt::skip]
const BROW_TRIPLETS: &[Triplet] = {
    use UnifiedExpressions::*;
    &[
        ("BrowsInnerUp", "BrowInnerUpLeft", "BrowInnerUpRight", &[BrowInnerUpLeft], &[BrowInnerUpRight], Combine::Max),
        ("BrowsOuterUp", "BrowOuterUpLeft", "BrowOuterUpRight", &[BrowOuterUpLeft], &[BrowOuterUpRight], Combine::Max),
        ("BrowsDown", "BrowDownLeft", "BrowDownRight", &[BrowPinchLeft, BrowLowererLeft], &[BrowPinchRight, BrowLowererRight], Combine::Max),
    ]
};

/// Quest Pro Legacy Face Parameters with Left/Right halves
#[rustfmt::skip]
const FACE_TRIPLETS: &[Triplet] = {
    use UnifiedExpressions::*;
    &[
        ("EyesSquint", "EyeSquintLeft", "EyeSquintRight", &[EyeSquintLeft], &[EyeSquintRight], Combine::Avg),
        ("CheeksSquint", "CheekSquintLeft", "CheekSquintRight", &[CheekSquintLeft], &[CheekSquintRight], Combine::Avg),
        ("MouthDimple", "MouthDimpleLeft", "MouthDimpleRight", &[MouthDimpleLeft], &[MouthDimpleRight], Combine::Avg),
        ("MouthPress", "MouthPressLeft", "MouthPressRight", &[MouthPressLeft], &[MouthPressRight], Combine::Avg),
        ("MouthStretch", "MouthStretchLeft", "MouthStretchRight", &[MouthStretchLeft], &[MouthStretchRight], Combine::Avg),
        ("MouthTightener", "MouthTightenerLeft", "MouthTightenerRight", &[MouthTightenerLeft], &[MouthTightenerRight], Combine::Avg),
        ("NoseSneer", "NoseSneerLeft", "NoseSneerRight", &[NoseSneerLeft], &[NoseSneerRight], Combine::Avg),
    ]
};

/// Average weight of `shapes`
fn mean(data: &UnifiedTrackingData, shapes: &[UnifiedExpressions]) -> f32 {
    shapes.iter().map(|&e| data.weight(e)).sum::<f32>() / shapes.len() as f32
}

/// Pushes the combined, Left and Right parameter of each triplet, in that order
fn push_triplets(params: &mut Vec<Box<dyn Parameter>>, triplets: &[Triplet]) {
    for &(combined, left_name, right_name, left, right, combine) in triplets {
        params.push(Box::new(EParam::simple(combined, move |d| {
            let (l, r) = (mean(d, left), mean(d, right));
            match combine {
                Combine::Avg => (l + r) / 2.0,
                Combine::Max => l.max(r),
            }
        })));
        params.push(Box::new(EParam::simple(left_name, move |d| mean(d, left))));
        params.push(Box::new(EParam::simple(right_name, move |d| {
            mean(d, right)
        })));
    }
}

/// Creates all legacy eye tracking parameters
pub fn create_legacy_eye_parameters() -> Vec<Box<dyn Parameter>> {
    let mut params: Vec<Box<dyn Parameter>> = Vec::new();

    // XY Eye Params (split into X/Y since OSC doesn't support Vector2)
    params.push(Box::new(EParam::simple("EyesX", |d| {
        (d.eye.left.gaze.x + d.eye.right.gaze.x) / 2.0
    })));
    params.push(Box::new(EParam::simple("EyesY", |d| {
        (d.eye.left.gaze.y + d.eye.right.gaze.y) / 2.0
    })));
    params.push(Box::new(EParam::simple("LeftEyeX", |d| d.eye.left.gaze.x)));
    params.push(Box::new(EParam::simple("LeftEyeY", |d| d.eye.left.gaze.y)));
    params.push(Box::new(EParam::simple("RightEyeX", |d| {
        d.eye.right.gaze.x
    })));
    params.push(Box::new(EParam::simple("RightEyeY", |d| {
        d.eye.right.gaze.y
    })));

    // Eye Widen
    params.push(Box::new(EParam::simple("LeftEyeWiden", |d| {
        Side::Left.wide(d)
    })));
    params.push(Box::new(EParam::simple("RightEyeWiden", |d| {
        Side::Right.wide(d)
    })));
    params.push(Box::new(EParam::simple("EyeWiden", |d| {
        (Side::Left.wide(d) + Side::Right.wide(d)) / 2.0
    })));

    // Eye Squeeze
    params.push(Box::new(EParam::simple("LeftEyeSqueeze", |d| {
        Side::Left.squint(d)
    })));
    params.push(Box::new(EParam::simple("RightEyeSqueeze", |d| {
        Side::Right.squint(d)
    })));
    params.push(Box::new(EParam::simple("EyesSqueeze", |d| {
        (Side::Left.squint(d) + Side::Right.squint(d)) / 2.0
    })));

    // Eye Dilation
    params.push(Box::new(EParam::simple("EyesDilation", |d| {
        d.eye.dilation()
    })));
    params.push(Box::new(EParam::simple("EyesPupilDiameter", |d| {
        (d.eye.left.pupil_diameter_mm + d.eye.right.pupil_diameter_mm) * 0.5
    })));

    // Eye Lid (Simple)
    params.push(Box::new(EParam::simple("LeftEyeLid", |d| {
        d.eye.left.openness
    })));
    params.push(Box::new(EParam::simple("RightEyeLid", |d| {
        d.eye.right.openness
    })));
    params.push(Box::new(EParam::simple("CombinedEyeLid", |d| {
        (d.eye.left.openness + d.eye.right.openness) / 2.0
    })));

    // Eye Lid Expanded (Float + Bool, binary handled separately below)
    params.push(Box::new(EParam::new(
        "LeftEyeLidExpanded",
        |d| lid(d, Side::Left),
        0.5,
        true,
    )));
    params.push(Box::new(EParam::new(
        "RightEyeLidExpanded",
        |d| lid(d, Side::Right),
        0.5,
        true,
    )));
    params.push(Box::new(EParam::new(
        "EyeLidExpanded",
        |d| {
            (Side::Left.wide(d) + Side::Right.wide(d)) * 0.1
                + (d.eye.left.openness + d.eye.right.openness) * 0.4
        },
        0.5,
        true,
    )));

    // Eye Lid Expanded Squeeze (Float + Bool, binary handled separately below)
    params.push(Box::new(EParam::new(
        "LeftEyeLidExpandedSqueeze",
        |d| lid_squeeze(d, Side::Left),
        0.5,
        true,
    )));
    params.push(Box::new(EParam::new(
        "RightEyeLidExpandedSqueeze",
        |d| lid_squeeze(d, Side::Right),
        0.5,
        true,
    )));
    params.push(Box::new(EParam::new(
        "EyeLidExpandedSqueeze",
        combined_lid_squeeze,
        0.5,
        true,
    )));

    // Eye Lid Expanded Binary
    // Uses conditional selection based on combined eyelid value:
    // If eyelid > 0.8 → return wide value; else → return openness
    for (name, side) in [
        ("LeftEyeLidExpanded", Side::Left),
        ("RightEyeLidExpanded", Side::Right),
    ] {
        params.push(Box::new(BinaryBaseParameter::new(name, move |d| {
            if lid(d, side) > 0.8 {
                side.wide(d)
            } else {
                side.openness(d)
            }
        })));
    }
    params.push(Box::new(BinaryBaseParameter::new(
        "CombinedEyeLidExpanded",
        |d| {
            let avg_wide = (Side::Left.wide(d) + Side::Right.wide(d)) / 2.0;
            // If wide avg > 0, return wide; else return openness
            if avg_wide > 0.0 {
                avg_wide
            } else {
                (d.eye.left.openness + d.eye.right.openness) / 2.0
            }
        },
    )));

    // Eye Lid Expanded Squeeze Binary
    // Tri-state selection:
    // If eyelid > 0.8 → return wide; if eyelid >= 0 → return openness; else → return squeeze
    for (name, side) in [
        ("LeftEyeLidExpandedSqueeze", Side::Left),
        ("RightEyeLidExpandedSqueeze", Side::Right),
    ] {
        params.push(Box::new(BinaryBaseParameter::new(name, move |d| {
            let eyelid = lid_squeeze(d, side);
            if eyelid > 0.8 {
                side.wide(d)
            } else if eyelid >= 0.0 {
                side.openness(d)
            } else {
                squeeze(d, side)
            }
        })));
    }
    params.push(Box::new(BinaryBaseParameter::new(
        "CombinedEyeLidExpandedSqueeze",
        |d| {
            let eyelid = combined_lid_squeeze(d);
            if eyelid > 0.8 {
                (Side::Right.wide(d) + Side::Left.wide(d)) * 0.5
            } else if eyelid >= 0.0 {
                (d.eye.left.openness + d.eye.right.openness) / 2.0
            } else {
                (squeeze(d, Side::Left) + squeeze(d, Side::Right)) * 0.5
            }
        },
    )));

    // Eye Toggle Parameters (Bool)
    params.push(Box::new(BoolParam::new("LeftEyeWidenToggle", |d| {
        lid(d, Side::Left) > 0.8
    })));
    params.push(Box::new(BoolParam::new("RightEyeWidenToggle", |d| {
        lid(d, Side::Right) > 0.8
    })));
    params.push(Box::new(BoolParam::new("EyesWidenToggle", |d| {
        (lid(d, Side::Right) + lid(d, Side::Left)) / 2.0 > 0.8
    })));

    params.push(Box::new(BoolParam::new("LeftEyeSqueezeToggle", |d| {
        lid_squeeze(d, Side::Left) < 0.0
    })));
    params.push(Box::new(BoolParam::new("RightEyeSqueezeToggle", |d| {
        lid_squeeze(d, Side::Right) < 0.0
    })));
    params.push(Box::new(BoolParam::new("EyesSqueezeToggle", |d| {
        (lid_squeeze(d, Side::Right) + lid_squeeze(d, Side::Left)) / 2.0 < 0.0
    })));

    push_triplets(&mut params, BROW_TRIPLETS);

    // Quest Pro Legacy Face Parameters
    params.push(Box::new(EParam::simple("MouthRaiserLower", |d| {
        d.weight(UnifiedExpressions::MouthRaiserLower)
    })));
    params.push(Box::new(EParam::simple("MouthRaiserUpper", |d| {
        d.weight(UnifiedExpressions::MouthRaiserUpper)
    })));

    push_triplets(&mut params, FACE_TRIPLETS);

    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_openness_squeezes_without_nan() {
        let mut data = UnifiedTrackingData::default();
        data.eye.left.openness = -0.25;
        data.eye.right.openness = 0.0;
        *data.weight_mut(UnifiedExpressions::EyeSquintLeft).unwrap() = 1.0;
        *data.weight_mut(UnifiedExpressions::EyeSquintRight).unwrap() = 1.0;

        // A negative openness squeezes like a closed eye
        assert_eq!(squeeze(&data, Side::Left), squeeze(&data, Side::Right));
        assert!(lid_squeeze(&data, Side::Left) < 0.0);
        assert!(combined_lid_squeeze(&data).is_finite());
    }
}
