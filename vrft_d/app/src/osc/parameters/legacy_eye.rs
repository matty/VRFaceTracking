//! Legacy eye tracking parameters for backwards compatibility with older avatars.

use super::base_param::{BoolParam, FloatParam};
use super::binary_param::BinaryBaseParameter;
use super::Parameter;
use common::{UnifiedExpressions, UnifiedTrackingData};

// Helper to get shape weight
fn w(data: &UnifiedTrackingData, expr: UnifiedExpressions) -> f32 {
    data.shapes[expr as usize].weight
}

/// Calculate the squeeze factor for eye lid calculations
fn squeeze(data: &UnifiedTrackingData, eye_index: usize) -> f32 {
    if eye_index == 0 {
        // Left eye
        (1.0 - data.eye.left.openness.powf(0.15)) * w(data, UnifiedExpressions::EyeSquintLeft)
    } else {
        // Right eye
        (1.0 - data.eye.right.openness.powf(0.15)) * w(data, UnifiedExpressions::EyeSquintRight)
    }
}

/// Creates all legacy eye tracking parameters
pub fn create_legacy_eye_parameters() -> Vec<Box<dyn Parameter>> {
    let mut params: Vec<Box<dyn Parameter>> = Vec::new();

    // XY Eye Params (split into X/Y since OSC doesn't support Vector2)
    params.push(Box::new(FloatParam::new("EyesX", |d| {
        (d.eye.left.gaze.x + d.eye.right.gaze.x) / 2.0
    })));
    params.push(Box::new(FloatParam::new("EyesY", |d| {
        (d.eye.left.gaze.y + d.eye.right.gaze.y) / 2.0
    })));
    params.push(Box::new(FloatParam::new("LeftEyeX", |d| d.eye.left.gaze.x)));
    params.push(Box::new(FloatParam::new("LeftEyeY", |d| d.eye.left.gaze.y)));
    params.push(Box::new(FloatParam::new("RightEyeX", |d| {
        d.eye.right.gaze.x
    })));
    params.push(Box::new(FloatParam::new("RightEyeY", |d| {
        d.eye.right.gaze.y
    })));

    // Eye Widen
    params.push(Box::new(FloatParam::new("LeftEyeWiden", |d| {
        w(d, UnifiedExpressions::EyeWideLeft)
    })));
    params.push(Box::new(FloatParam::new("RightEyeWiden", |d| {
        w(d, UnifiedExpressions::EyeWideRight)
    })));
    params.push(Box::new(FloatParam::new("EyeWiden", |d| {
        (w(d, UnifiedExpressions::EyeWideLeft) + w(d, UnifiedExpressions::EyeWideRight)) / 2.0
    })));

    // Eye Squeeze
    params.push(Box::new(FloatParam::new("LeftEyeSqueeze", |d| {
        w(d, UnifiedExpressions::EyeSquintLeft)
    })));
    params.push(Box::new(FloatParam::new("RightEyeSqueeze", |d| {
        w(d, UnifiedExpressions::EyeSquintRight)
    })));
    params.push(Box::new(FloatParam::new("EyesSqueeze", |d| {
        (w(d, UnifiedExpressions::EyeSquintLeft) + w(d, UnifiedExpressions::EyeSquintRight)) / 2.0
    })));

    // Eye Dilation
    params.push(Box::new(FloatParam::new("EyesDilation", |d| {
        (d.eye.left.pupil_diameter_mm + d.eye.right.pupil_diameter_mm) / 2.0
    })));
    params.push(Box::new(FloatParam::new("EyesPupilDiameter", |d| {
        (d.eye.left.pupil_diameter_mm + d.eye.right.pupil_diameter_mm) * 0.5
    })));

    // Eye Lid (Simple)
    params.push(Box::new(FloatParam::new("LeftEyeLid", |d| {
        d.eye.left.openness
    })));
    params.push(Box::new(FloatParam::new("RightEyeLid", |d| {
        d.eye.right.openness
    })));
    params.push(Box::new(FloatParam::new("CombinedEyeLid", |d| {
        (d.eye.left.openness + d.eye.right.openness) / 2.0
    })));

    // Eye Lid Expanded (Float)
    params.push(Box::new(FloatParam::new("LeftEyeLidExpanded", |d| {
        w(d, UnifiedExpressions::EyeWideLeft) * 0.2 + d.eye.left.openness * 0.8
    })));
    params.push(Box::new(FloatParam::new("RightEyeLidExpanded", |d| {
        w(d, UnifiedExpressions::EyeWideRight) * 0.2 + d.eye.right.openness * 0.8
    })));
    params.push(Box::new(FloatParam::new("EyeLidExpanded", |d| {
        (w(d, UnifiedExpressions::EyeWideLeft) + w(d, UnifiedExpressions::EyeWideRight)) * 0.1
            + (d.eye.left.openness + d.eye.right.openness) * 0.4
    })));

    // Eye Lid Expanded Squeeze (Float)
    params.push(Box::new(FloatParam::new(
        "LeftEyeLidExpandedSqueeze",
        |d| w(d, UnifiedExpressions::EyeWideLeft) * 0.2 + d.eye.left.openness * 0.8 - squeeze(d, 0),
    )));
    params.push(Box::new(FloatParam::new(
        "RightEyeLidExpandedSqueeze",
        |d| {
            w(d, UnifiedExpressions::EyeWideRight) * 0.2 + d.eye.right.openness * 0.8
                - squeeze(d, 1)
        },
    )));
    params.push(Box::new(FloatParam::new("EyeLidExpandedSqueeze", |d| {
        ((w(d, UnifiedExpressions::EyeWideLeft) + w(d, UnifiedExpressions::EyeWideRight)) * 0.2
            + (d.eye.left.openness + d.eye.right.openness) * 0.8
            - squeeze(d, 0)
            - squeeze(d, 1))
            * 0.5
    })));

    // Eye Lid Expanded Binary
    // Uses conditional selection based on combined eyelid value:
    // If eyelid > 0.8 → return wide value; else → return openness
    params.push(Box::new(BinaryBaseParameter::new(
        "LeftEyeLidExpanded",
        |d| {
            let eyelid = w(d, UnifiedExpressions::EyeWideLeft) * 0.2 + d.eye.left.openness * 0.8;
            if eyelid > 0.8 {
                w(d, UnifiedExpressions::EyeWideLeft)
            } else {
                d.eye.left.openness
            }
        },
    )));
    params.push(Box::new(BinaryBaseParameter::new(
        "RightEyeLidExpanded",
        |d| {
            let eyelid = w(d, UnifiedExpressions::EyeWideRight) * 0.2 + d.eye.right.openness * 0.8;
            if eyelid > 0.8 {
                w(d, UnifiedExpressions::EyeWideRight)
            } else {
                d.eye.right.openness
            }
        },
    )));
    params.push(Box::new(BinaryBaseParameter::new(
        "CombinedEyeLidExpanded",
        |d| {
            let avg_wide = (w(d, UnifiedExpressions::EyeWideLeft)
                + w(d, UnifiedExpressions::EyeWideRight))
                / 2.0;
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
    params.push(Box::new(BinaryBaseParameter::new(
        "LeftEyeLidExpandedSqueeze",
        |d| {
            let eyelid = w(d, UnifiedExpressions::EyeWideLeft) * 0.2 + d.eye.left.openness * 0.8
                - squeeze(d, 0);
            if eyelid > 0.8 {
                w(d, UnifiedExpressions::EyeWideLeft)
            } else if eyelid >= 0.0 {
                d.eye.left.openness
            } else {
                squeeze(d, 0)
            }
        },
    )));
    params.push(Box::new(BinaryBaseParameter::new(
        "RightEyeLidExpandedSqueeze",
        |d| {
            let eyelid = w(d, UnifiedExpressions::EyeWideRight) * 0.2 + d.eye.right.openness * 0.8
                - squeeze(d, 1);
            if eyelid > 0.8 {
                w(d, UnifiedExpressions::EyeWideRight)
            } else if eyelid >= 0.0 {
                d.eye.right.openness
            } else {
                squeeze(d, 1)
            }
        },
    )));
    params.push(Box::new(BinaryBaseParameter::new(
        "CombinedEyeLidExpandedSqueeze",
        |d| {
            let eyelid = ((w(d, UnifiedExpressions::EyeWideLeft)
                + w(d, UnifiedExpressions::EyeWideRight))
                * 0.2
                + (d.eye.left.openness + d.eye.right.openness) * 0.8
                - squeeze(d, 0)
                - squeeze(d, 1))
                * 0.5;
            if eyelid > 0.8 {
                (w(d, UnifiedExpressions::EyeWideRight) + w(d, UnifiedExpressions::EyeWideLeft))
                    * 0.5
            } else if eyelid >= 0.0 {
                (d.eye.left.openness + d.eye.right.openness) / 2.0
            } else {
                (squeeze(d, 0) + squeeze(d, 1)) * 0.5
            }
        },
    )));

    // Eye Toggle Parameters (Bool)
    params.push(Box::new(BoolParam::new("LeftEyeWidenToggle", |d| {
        w(d, UnifiedExpressions::EyeWideLeft) * 0.2 + d.eye.left.openness * 0.8 > 0.8
    })));
    params.push(Box::new(BoolParam::new("RightEyeWidenToggle", |d| {
        w(d, UnifiedExpressions::EyeWideRight) * 0.2 + d.eye.right.openness * 0.8 > 0.8
    })));
    params.push(Box::new(BoolParam::new("EyesWidenToggle", |d| {
        (w(d, UnifiedExpressions::EyeWideRight) * 0.2
            + d.eye.right.openness * 0.8
            + w(d, UnifiedExpressions::EyeWideLeft) * 0.2
            + d.eye.left.openness * 0.8)
            / 2.0
            > 0.8
    })));

    params.push(Box::new(BoolParam::new("LeftEyeSqueezeToggle", |d| {
        w(d, UnifiedExpressions::EyeWideLeft) * 0.2 + d.eye.left.openness * 0.8 - squeeze(d, 0)
            < 0.0
    })));
    params.push(Box::new(BoolParam::new("RightEyeSqueezeToggle", |d| {
        w(d, UnifiedExpressions::EyeWideRight) * 0.2 + d.eye.right.openness * 0.8 - squeeze(d, 1)
            < 0.0
    })));
    params.push(Box::new(BoolParam::new("EyesSqueezeToggle", |d| {
        (w(d, UnifiedExpressions::EyeWideRight) * 0.2 + d.eye.right.openness * 0.8 - squeeze(d, 1)
            + w(d, UnifiedExpressions::EyeWideLeft) * 0.2
            + d.eye.left.openness * 0.8
            - squeeze(d, 0))
            / 2.0
            < 0.0
    })));

    // Quest Pro Legacy Brow Parameters
    params.push(Box::new(FloatParam::new("BrowsInnerUp", |d| {
        w(d, UnifiedExpressions::BrowInnerUpLeft).max(w(d, UnifiedExpressions::BrowInnerUpRight))
    })));
    params.push(Box::new(FloatParam::new("BrowInnerUpLeft", |d| {
        w(d, UnifiedExpressions::BrowInnerUpLeft)
    })));
    params.push(Box::new(FloatParam::new("BrowInnerUpRight", |d| {
        w(d, UnifiedExpressions::BrowInnerUpRight)
    })));

    params.push(Box::new(FloatParam::new("BrowsOuterUp", |d| {
        w(d, UnifiedExpressions::BrowOuterUpLeft).max(w(d, UnifiedExpressions::BrowOuterUpRight))
    })));
    params.push(Box::new(FloatParam::new("BrowOuterUpLeft", |d| {
        w(d, UnifiedExpressions::BrowOuterUpLeft)
    })));
    params.push(Box::new(FloatParam::new("BrowOuterUpRight", |d| {
        w(d, UnifiedExpressions::BrowOuterUpRight)
    })));

    params.push(Box::new(FloatParam::new("BrowsDown", |d| {
        let left = (w(d, UnifiedExpressions::BrowPinchLeft)
            + w(d, UnifiedExpressions::BrowLowererLeft))
            / 2.0;
        let right = (w(d, UnifiedExpressions::BrowPinchRight)
            + w(d, UnifiedExpressions::BrowLowererRight))
            / 2.0;
        left.max(right)
    })));
    params.push(Box::new(FloatParam::new("BrowDownLeft", |d| {
        (w(d, UnifiedExpressions::BrowPinchLeft) + w(d, UnifiedExpressions::BrowLowererLeft)) / 2.0
    })));
    params.push(Box::new(FloatParam::new("BrowDownRight", |d| {
        (w(d, UnifiedExpressions::BrowPinchRight) + w(d, UnifiedExpressions::BrowLowererRight))
            / 2.0
    })));

    // Quest Pro Legacy Face Parameters
    params.push(Box::new(FloatParam::new("MouthRaiserLower", |d| {
        w(d, UnifiedExpressions::MouthRaiserLower)
    })));
    params.push(Box::new(FloatParam::new("MouthRaiserUpper", |d| {
        w(d, UnifiedExpressions::MouthRaiserUpper)
    })));

    params.push(Box::new(FloatParam::new("EyesSquint", |d| {
        (w(d, UnifiedExpressions::EyeSquintLeft) + w(d, UnifiedExpressions::EyeSquintRight)) / 2.0
    })));
    params.push(Box::new(FloatParam::new("EyeSquintLeft", |d| {
        w(d, UnifiedExpressions::EyeSquintLeft)
    })));
    params.push(Box::new(FloatParam::new("EyeSquintRight", |d| {
        w(d, UnifiedExpressions::EyeSquintRight)
    })));

    params.push(Box::new(FloatParam::new("CheeksSquint", |d| {
        (w(d, UnifiedExpressions::CheekSquintLeft) + w(d, UnifiedExpressions::CheekSquintRight))
            / 2.0
    })));
    params.push(Box::new(FloatParam::new("CheekSquintLeft", |d| {
        w(d, UnifiedExpressions::CheekSquintLeft)
    })));
    params.push(Box::new(FloatParam::new("CheekSquintRight", |d| {
        w(d, UnifiedExpressions::CheekSquintRight)
    })));

    params.push(Box::new(FloatParam::new("MouthDimple", |d| {
        (w(d, UnifiedExpressions::MouthDimpleLeft) + w(d, UnifiedExpressions::MouthDimpleRight))
            / 2.0
    })));
    params.push(Box::new(FloatParam::new("MouthDimpleLeft", |d| {
        w(d, UnifiedExpressions::MouthDimpleLeft)
    })));
    params.push(Box::new(FloatParam::new("MouthDimpleRight", |d| {
        w(d, UnifiedExpressions::MouthDimpleRight)
    })));

    params.push(Box::new(FloatParam::new("MouthPress", |d| {
        (w(d, UnifiedExpressions::MouthPressLeft) + w(d, UnifiedExpressions::MouthPressRight)) / 2.0
    })));
    params.push(Box::new(FloatParam::new("MouthPressLeft", |d| {
        w(d, UnifiedExpressions::MouthPressLeft)
    })));
    params.push(Box::new(FloatParam::new("MouthPressRight", |d| {
        w(d, UnifiedExpressions::MouthPressRight)
    })));

    params.push(Box::new(FloatParam::new("MouthStretch", |d| {
        (w(d, UnifiedExpressions::MouthStretchLeft) + w(d, UnifiedExpressions::MouthStretchRight))
            / 2.0
    })));
    params.push(Box::new(FloatParam::new("MouthStretchLeft", |d| {
        w(d, UnifiedExpressions::MouthStretchLeft)
    })));
    params.push(Box::new(FloatParam::new("MouthStretchRight", |d| {
        w(d, UnifiedExpressions::MouthStretchRight)
    })));

    params.push(Box::new(FloatParam::new("MouthTightener", |d| {
        (w(d, UnifiedExpressions::MouthTightenerLeft)
            + w(d, UnifiedExpressions::MouthTightenerRight))
            / 2.0
    })));
    params.push(Box::new(FloatParam::new("MouthTightenerLeft", |d| {
        w(d, UnifiedExpressions::MouthTightenerLeft)
    })));
    params.push(Box::new(FloatParam::new("MouthTightenerRight", |d| {
        w(d, UnifiedExpressions::MouthTightenerRight)
    })));

    params.push(Box::new(FloatParam::new("NoseSneer", |d| {
        (w(d, UnifiedExpressions::NoseSneerLeft) + w(d, UnifiedExpressions::NoseSneerRight)) / 2.0
    })));
    params.push(Box::new(FloatParam::new("NoseSneerLeft", |d| {
        w(d, UnifiedExpressions::NoseSneerLeft)
    })));
    params.push(Box::new(FloatParam::new("NoseSneerRight", |d| {
        w(d, UnifiedExpressions::NoseSneerRight)
    })));

    params
}
