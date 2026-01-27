//! Legacy SRanipal lip tracking parameters for backwards compatibility.
//!
//! Includes both direct SRanipal shapes and merged/combined shapes.

use super::base_param::FloatParam;
use super::Parameter;
use common::{UnifiedExpressions, UnifiedTrackingData};

/// SRanipal Lip Shape v2 enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum SRanipalLipShape {
    JawRight = 0,
    JawLeft,
    JawForward,
    JawOpen,
    MouthApeShape,
    MouthUpperRight,
    MouthUpperLeft,
    MouthLowerRight,
    MouthLowerLeft,
    MouthUpperOverturn,
    MouthLowerOverturn,
    MouthPout,
    MouthSmileRight,
    MouthSmileLeft,
    MouthSadRight,
    MouthSadLeft,
    CheekPuffRight,
    CheekPuffLeft,
    CheekSuck,
    MouthUpperUpRight,
    MouthUpperUpLeft,
    MouthLowerDownRight,
    MouthLowerDownLeft,
    MouthUpperInside,
    MouthLowerInside,
    MouthLowerOverlay,
    TongueLongStep1,
    TongueLongStep2,
    TongueDown,
    TongueUp,
    TongueRight,
    TongueLeft,
    TongueRoll,
    TongueUpLeftMorph,
    TongueUpRightMorph,
    TongueDownLeftMorph,
    TongueDownRightMorph,
    Max,
}

// Helper to get shape weight from UnifiedTrackingData
fn w(data: &UnifiedTrackingData, expr: UnifiedExpressions) -> f32 {
    data.shapes[expr as usize].weight
}

/// Maps SRanipal lip shapes to Unified Expressions
fn get_sranipal_shape(shape: SRanipalLipShape, data: &UnifiedTrackingData) -> f32 {
    match shape {
        SRanipalLipShape::JawRight => w(data, UnifiedExpressions::JawRight),
        SRanipalLipShape::JawLeft => w(data, UnifiedExpressions::JawLeft),
        SRanipalLipShape::JawForward => w(data, UnifiedExpressions::JawForward),
        SRanipalLipShape::JawOpen => w(data, UnifiedExpressions::JawOpen),
        SRanipalLipShape::MouthApeShape => {
            // Ape shape: Jaw open without upper lip movement
            w(data, UnifiedExpressions::JawOpen)
                * (1.0
                    - (w(data, UnifiedExpressions::MouthUpperUpLeft)
                        + w(data, UnifiedExpressions::MouthUpperUpRight))
                        / 2.0)
        }
        SRanipalLipShape::MouthUpperRight => w(data, UnifiedExpressions::MouthUpperRight),
        SRanipalLipShape::MouthUpperLeft => w(data, UnifiedExpressions::MouthUpperLeft),
        SRanipalLipShape::MouthLowerRight => w(data, UnifiedExpressions::MouthLowerRight),
        SRanipalLipShape::MouthLowerLeft => w(data, UnifiedExpressions::MouthLowerLeft),
        SRanipalLipShape::MouthUpperOverturn => {
            (w(data, UnifiedExpressions::LipFunnelUpperLeft)
                + w(data, UnifiedExpressions::LipFunnelUpperRight))
                / 2.0
        }
        SRanipalLipShape::MouthLowerOverturn => {
            (w(data, UnifiedExpressions::LipFunnelLowerLeft)
                + w(data, UnifiedExpressions::LipFunnelLowerRight))
                / 2.0
        }
        SRanipalLipShape::MouthPout => {
            (w(data, UnifiedExpressions::LipPuckerUpperLeft)
                + w(data, UnifiedExpressions::LipPuckerUpperRight)
                + w(data, UnifiedExpressions::LipPuckerLowerLeft)
                + w(data, UnifiedExpressions::LipPuckerLowerRight))
                / 4.0
        }
        SRanipalLipShape::MouthSmileRight => {
            w(data, UnifiedExpressions::MouthCornerPullRight) * 0.8
                + w(data, UnifiedExpressions::MouthCornerSlantRight) * 0.2
        }
        SRanipalLipShape::MouthSmileLeft => {
            w(data, UnifiedExpressions::MouthCornerPullLeft) * 0.8
                + w(data, UnifiedExpressions::MouthCornerSlantLeft) * 0.2
        }
        SRanipalLipShape::MouthSadRight => w(data, UnifiedExpressions::MouthFrownRight)
            .max(w(data, UnifiedExpressions::MouthStretchRight)),
        SRanipalLipShape::MouthSadLeft => w(data, UnifiedExpressions::MouthFrownLeft)
            .max(w(data, UnifiedExpressions::MouthStretchLeft)),
        SRanipalLipShape::CheekPuffRight => w(data, UnifiedExpressions::CheekPuffRight),
        SRanipalLipShape::CheekPuffLeft => w(data, UnifiedExpressions::CheekPuffLeft),
        SRanipalLipShape::CheekSuck => {
            (w(data, UnifiedExpressions::CheekSuckLeft)
                + w(data, UnifiedExpressions::CheekSuckRight))
                / 2.0
        }
        SRanipalLipShape::MouthUpperUpRight => w(data, UnifiedExpressions::MouthUpperUpRight),
        SRanipalLipShape::MouthUpperUpLeft => w(data, UnifiedExpressions::MouthUpperUpLeft),
        SRanipalLipShape::MouthLowerDownRight => w(data, UnifiedExpressions::MouthLowerDownRight),
        SRanipalLipShape::MouthLowerDownLeft => w(data, UnifiedExpressions::MouthLowerDownLeft),
        SRanipalLipShape::MouthUpperInside => {
            (w(data, UnifiedExpressions::LipSuckUpperLeft)
                + w(data, UnifiedExpressions::LipSuckUpperRight))
                / 2.0
        }
        SRanipalLipShape::MouthLowerInside => {
            (w(data, UnifiedExpressions::LipSuckLowerLeft)
                + w(data, UnifiedExpressions::LipSuckLowerRight))
                / 2.0
        }
        SRanipalLipShape::MouthLowerOverlay => w(data, UnifiedExpressions::MouthRaiserLower),
        SRanipalLipShape::TongueLongStep1 => w(data, UnifiedExpressions::TongueOut),
        SRanipalLipShape::TongueLongStep2 => {
            w(data, UnifiedExpressions::TongueOut) * w(data, UnifiedExpressions::TongueOut)
        }
        SRanipalLipShape::TongueDown => w(data, UnifiedExpressions::TongueDown),
        SRanipalLipShape::TongueUp => w(data, UnifiedExpressions::TongueUp),
        SRanipalLipShape::TongueRight => w(data, UnifiedExpressions::TongueRight),
        SRanipalLipShape::TongueLeft => w(data, UnifiedExpressions::TongueLeft),
        SRanipalLipShape::TongueRoll => w(data, UnifiedExpressions::TongueRoll),
        SRanipalLipShape::TongueUpLeftMorph => {
            w(data, UnifiedExpressions::TongueUp) * (1.0 - w(data, UnifiedExpressions::TongueRight))
        }
        SRanipalLipShape::TongueUpRightMorph => {
            w(data, UnifiedExpressions::TongueUp) * (1.0 - w(data, UnifiedExpressions::TongueLeft))
        }
        SRanipalLipShape::TongueDownLeftMorph => {
            w(data, UnifiedExpressions::TongueDown)
                * (1.0 - w(data, UnifiedExpressions::TongueRight))
        }
        SRanipalLipShape::TongueDownRightMorph => {
            w(data, UnifiedExpressions::TongueDown)
                * (1.0 - w(data, UnifiedExpressions::TongueLeft))
        }
        SRanipalLipShape::Max => 0.0,
    }
}

/// Helper for positive-negative shape blending
fn pos_neg_shape(
    data: &UnifiedTrackingData,
    positive: SRanipalLipShape,
    negative: SRanipalLipShape,
) -> f32 {
    get_sranipal_shape(positive, data) - get_sranipal_shape(negative, data)
}

/// Helper for averaged positive-negative shape blending
fn pos_neg_avg_shape(
    data: &UnifiedTrackingData,
    positives: &[SRanipalLipShape],
    negatives: &[SRanipalLipShape],
    use_max: bool,
) -> f32 {
    if use_max {
        let pos_max = positives
            .iter()
            .map(|s| get_sranipal_shape(*s, data))
            .fold(0.0_f32, |a, b| a.max(b));
        let neg_max = negatives
            .iter()
            .map(|s| get_sranipal_shape(*s, data))
            .fold(0.0_f32, |a, b| a.max(b));
        pos_max - neg_max
    } else {
        let pos_avg = if positives.is_empty() {
            0.0
        } else {
            positives
                .iter()
                .map(|s| get_sranipal_shape(*s, data))
                .sum::<f32>()
                / positives.len() as f32
        };
        let neg_avg = if negatives.is_empty() {
            0.0
        } else {
            negatives
                .iter()
                .map(|s| get_sranipal_shape(*s, data))
                .sum::<f32>()
                / negatives.len() as f32
        };
        pos_avg - neg_avg
    }
}

/// Creates all legacy SRanipal lip shape parameters
pub fn create_legacy_lip_parameters() -> Vec<Box<dyn Parameter>> {
    let mut params: Vec<Box<dyn Parameter>> = Vec::new();

    // All SRanipal Lip Shapes (direct mappings)
    let sranipal_shapes = [
        "JawRight",
        "JawLeft",
        "JawForward",
        "JawOpen",
        "MouthApeShape",
        "MouthUpperRight",
        "MouthUpperLeft",
        "MouthLowerRight",
        "MouthLowerLeft",
        "MouthUpperOverturn",
        "MouthLowerOverturn",
        "MouthPout",
        "MouthSmileRight",
        "MouthSmileLeft",
        "MouthSadRight",
        "MouthSadLeft",
        "CheekPuffRight",
        "CheekPuffLeft",
        "CheekSuck",
        "MouthUpperUpRight",
        "MouthUpperUpLeft",
        "MouthLowerDownRight",
        "MouthLowerDownLeft",
        "MouthUpperInside",
        "MouthLowerInside",
        "MouthLowerOverlay",
        "TongueLongStep1",
        "TongueLongStep2",
        "TongueDown",
        "TongueUp",
        "TongueRight",
        "TongueLeft",
        "TongueRoll",
        "TongueUpLeftMorph",
        "TongueUpRightMorph",
        "TongueDownLeftMorph",
        "TongueDownRightMorph",
    ];

    for (i, name) in sranipal_shapes.iter().enumerate() {
        let shape_idx = i;
        params.push(Box::new(FloatParam::new(name, move |d| {
            // Safe to convert since we iterate 0..37 and SRanipalLipShape has 38 values
            let shape = unsafe { std::mem::transmute::<usize, SRanipalLipShape>(shape_idx) };
            get_sranipal_shape(shape, d)
        })));
    }

    // Basic Merged Shapes
    params.push(Box::new(FloatParam::new("JawX", |d| {
        pos_neg_shape(d, SRanipalLipShape::JawRight, SRanipalLipShape::JawLeft)
    })));
    params.push(Box::new(FloatParam::new("MouthUpper", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperRight,
            SRanipalLipShape::MouthUpperLeft,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthLower", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthLowerRight,
            SRanipalLipShape::MouthLowerLeft,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthX", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthUpperRight,
                SRanipalLipShape::MouthLowerRight,
            ],
            &[
                SRanipalLipShape::MouthUpperLeft,
                SRanipalLipShape::MouthLowerLeft,
            ],
            true,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileSadRight", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileRight,
            SRanipalLipShape::MouthSadRight,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileSadLeft", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileLeft,
            SRanipalLipShape::MouthSadLeft,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileSad", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthSmileLeft,
                SRanipalLipShape::MouthSmileRight,
            ],
            &[
                SRanipalLipShape::MouthSadLeft,
                SRanipalLipShape::MouthSadRight,
            ],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("TongueY", |d| {
        pos_neg_shape(d, SRanipalLipShape::TongueUp, SRanipalLipShape::TongueDown)
    })));
    params.push(Box::new(FloatParam::new("TongueX", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::TongueRight,
            SRanipalLipShape::TongueLeft,
        )
    })));
    params.push(Box::new(FloatParam::new("PuffSuckRight", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::CheekPuffRight,
            SRanipalLipShape::CheekSuck,
        )
    })));
    params.push(Box::new(FloatParam::new("PuffSuckLeft", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::CheekPuffLeft,
            SRanipalLipShape::CheekSuck,
        )
    })));
    params.push(Box::new(FloatParam::new("PuffSuck", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::CheekPuffLeft,
                SRanipalLipShape::CheekPuffRight,
            ],
            &[SRanipalLipShape::CheekSuck],
            true,
        )
    })));

    // JawOpen Based
    params.push(Box::new(FloatParam::new("JawOpenApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::JawOpen,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(FloatParam::new("JawOpenPuff", |d| {
        pos_neg_avg_shape(
            d,
            &[SRanipalLipShape::JawOpen],
            &[
                SRanipalLipShape::CheekPuffLeft,
                SRanipalLipShape::CheekPuffRight,
            ],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("JawOpenPuffRight", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::JawOpen,
            SRanipalLipShape::CheekPuffRight,
        )
    })));
    params.push(Box::new(FloatParam::new("JawOpenPuffLeft", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::JawOpen,
            SRanipalLipShape::CheekPuffLeft,
        )
    })));
    params.push(Box::new(FloatParam::new("JawOpenSuck", |d| {
        pos_neg_shape(d, SRanipalLipShape::JawOpen, SRanipalLipShape::CheekSuck)
    })));
    params.push(Box::new(FloatParam::new("JawOpenForward", |d| {
        pos_neg_shape(d, SRanipalLipShape::JawOpen, SRanipalLipShape::JawForward)
    })));
    params.push(Box::new(FloatParam::new("JawOpenOverlay", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::JawOpen,
            SRanipalLipShape::MouthLowerOverlay,
        )
    })));

    // MouthUpperUp Right Based
    params.push(Box::new(FloatParam::new(
        "MouthUpperUpRightUpperInside",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthUpperUpRight,
                SRanipalLipShape::MouthUpperInside,
            )
        },
    )));
    params.push(Box::new(FloatParam::new(
        "MouthUpperUpRightPuffRight",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthUpperUpRight,
                SRanipalLipShape::CheekPuffRight,
            )
        },
    )));
    params.push(Box::new(FloatParam::new("MouthUpperUpRightApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpRight,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpRightPout", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpRight,
            SRanipalLipShape::MouthPout,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpRightOverlay", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpRight,
            SRanipalLipShape::MouthLowerOverlay,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpRightSuck", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpRight,
            SRanipalLipShape::CheekSuck,
        )
    })));

    // MouthUpperUp Left Based
    params.push(Box::new(FloatParam::new(
        "MouthUpperUpLeftUpperInside",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthUpperInside,
            )
        },
    )));
    params.push(Box::new(FloatParam::new("MouthUpperUpLeftPuffLeft", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpLeft,
            SRanipalLipShape::CheekPuffLeft,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpLeftApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpLeft,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpLeftPout", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpLeft,
            SRanipalLipShape::MouthPout,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpLeftOverlay", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpLeft,
            SRanipalLipShape::MouthLowerOverlay,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpLeftSuck", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpLeft,
            SRanipalLipShape::CheekSuck,
        )
    })));

    // MouthUpperUp Combined
    params.push(Box::new(FloatParam::new("MouthUpperUpUpperInside", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthUpperUpRight,
            ],
            &[SRanipalLipShape::MouthUpperInside],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpInside", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthUpperUpRight,
            ],
            &[
                SRanipalLipShape::MouthUpperInside,
                SRanipalLipShape::MouthLowerInside,
            ],
            true,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpPuff", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthUpperUpRight,
            ],
            &[
                SRanipalLipShape::CheekPuffLeft,
                SRanipalLipShape::CheekPuffRight,
            ],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpPuffLeft", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthUpperUpRight,
            ],
            &[SRanipalLipShape::CheekPuffLeft],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpPuffRight", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthUpperUpRight,
            ],
            &[SRanipalLipShape::CheekPuffRight],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpApe", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthUpperUpRight,
            ],
            &[SRanipalLipShape::MouthApeShape],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpPout", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthUpperUpRight,
            ],
            &[SRanipalLipShape::MouthPout],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpOverlay", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthUpperUpRight,
            ],
            &[SRanipalLipShape::MouthLowerOverlay],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthUpperUpSuck", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthUpperUpRight,
            ],
            &[SRanipalLipShape::CheekSuck],
            false,
        )
    })));

    // MouthLowerDown Right Based
    params.push(Box::new(FloatParam::new(
        "MouthLowerDownRightLowerInside",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownRight,
                SRanipalLipShape::MouthLowerInside,
            )
        },
    )));
    params.push(Box::new(FloatParam::new(
        "MouthLowerDownRightPuffRight",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownRight,
                SRanipalLipShape::CheekPuffRight,
            )
        },
    )));
    params.push(Box::new(FloatParam::new("MouthLowerDownRightApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthLowerDownRight,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthLowerDownRightPout", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthLowerDownRight,
            SRanipalLipShape::MouthPout,
        )
    })));
    params.push(Box::new(FloatParam::new(
        "MouthLowerDownRightOverlay",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownRight,
                SRanipalLipShape::MouthLowerOverlay,
            )
        },
    )));
    params.push(Box::new(FloatParam::new("MouthLowerDownRightSuck", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthLowerDownRight,
            SRanipalLipShape::CheekSuck,
        )
    })));

    // MouthLowerDown Left Based
    params.push(Box::new(FloatParam::new(
        "MouthLowerDownLeftLowerInside",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerInside,
            )
        },
    )));
    params.push(Box::new(FloatParam::new(
        "MouthLowerDownLeftPuffLeft",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::CheekPuffLeft,
            )
        },
    )));
    params.push(Box::new(FloatParam::new("MouthLowerDownLeftApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthLowerDownLeft,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthLowerDownLeftPout", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthLowerDownLeft,
            SRanipalLipShape::MouthPout,
        )
    })));
    params.push(Box::new(FloatParam::new(
        "MouthLowerDownLeftOverlay",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerOverlay,
            )
        },
    )));
    params.push(Box::new(FloatParam::new("MouthLowerDownLeftSuck", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthLowerDownLeft,
            SRanipalLipShape::CheekSuck,
        )
    })));

    // MouthLowerDown Combined
    params.push(Box::new(FloatParam::new(
        "MouthLowerDownLowerInside",
        |d| {
            pos_neg_avg_shape(
                d,
                &[
                    SRanipalLipShape::MouthLowerDownLeft,
                    SRanipalLipShape::MouthLowerDownRight,
                ],
                &[SRanipalLipShape::MouthLowerInside],
                false,
            )
        },
    )));
    params.push(Box::new(FloatParam::new("MouthLowerDownInside", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerDownRight,
            ],
            &[
                SRanipalLipShape::MouthUpperInside,
                SRanipalLipShape::MouthLowerInside,
            ],
            true,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthLowerDownPuff", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerDownRight,
            ],
            &[
                SRanipalLipShape::CheekPuffLeft,
                SRanipalLipShape::CheekPuffRight,
            ],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthLowerDownPuffLeft", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerDownRight,
            ],
            &[SRanipalLipShape::CheekPuffLeft],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthLowerDownPuffRight", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerDownRight,
            ],
            &[SRanipalLipShape::CheekPuffRight],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthLowerDownApe", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerDownRight,
            ],
            &[SRanipalLipShape::MouthApeShape],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthLowerDownPout", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerDownRight,
            ],
            &[SRanipalLipShape::MouthPout],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthLowerDownOverlay", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerDownRight,
            ],
            &[SRanipalLipShape::MouthLowerOverlay],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthLowerDownSuck", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerDownRight,
            ],
            &[SRanipalLipShape::CheekSuck],
            false,
        )
    })));

    // Inside/Overturn Based
    params.push(Box::new(FloatParam::new("MouthUpperInsideOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperInside,
            SRanipalLipShape::MouthUpperOverturn,
        )
    })));
    params.push(Box::new(FloatParam::new("MouthLowerInsideOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthLowerInside,
            SRanipalLipShape::MouthLowerOverturn,
        )
    })));

    // Smile Right Based
    params.push(Box::new(FloatParam::new("SmileRightUpperOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileRight,
            SRanipalLipShape::MouthUpperOverturn,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileRightLowerOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileRight,
            SRanipalLipShape::MouthLowerOverturn,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileRightOverturn", |d| {
        pos_neg_avg_shape(
            d,
            &[SRanipalLipShape::MouthSmileRight],
            &[
                SRanipalLipShape::MouthUpperOverturn,
                SRanipalLipShape::MouthLowerOverturn,
            ],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileRightApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileRight,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileRightOverlay", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileRight,
            SRanipalLipShape::MouthLowerOverlay,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileRightPout", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileRight,
            SRanipalLipShape::MouthPout,
        )
    })));

    // Smile Left Based
    params.push(Box::new(FloatParam::new("SmileLeftUpperOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileLeft,
            SRanipalLipShape::MouthUpperOverturn,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileLeftLowerOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileLeft,
            SRanipalLipShape::MouthLowerOverturn,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileLeftOverturn", |d| {
        pos_neg_avg_shape(
            d,
            &[SRanipalLipShape::MouthSmileLeft],
            &[
                SRanipalLipShape::MouthUpperOverturn,
                SRanipalLipShape::MouthLowerOverturn,
            ],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileLeftApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileLeft,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileLeftOverlay", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileLeft,
            SRanipalLipShape::MouthLowerOverlay,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileLeftPout", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileLeft,
            SRanipalLipShape::MouthPout,
        )
    })));

    // Smile Combined
    params.push(Box::new(FloatParam::new("SmileUpperOverturn", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthSmileLeft,
                SRanipalLipShape::MouthSmileRight,
            ],
            &[SRanipalLipShape::MouthUpperOverturn],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileLowerOverturn", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthSmileLeft,
                SRanipalLipShape::MouthSmileRight,
            ],
            &[SRanipalLipShape::MouthLowerOverturn],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileOverturn", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthSmileLeft,
                SRanipalLipShape::MouthSmileRight,
            ],
            &[
                SRanipalLipShape::MouthUpperOverturn,
                SRanipalLipShape::MouthLowerOverturn,
            ],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileApe", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthSmileLeft,
                SRanipalLipShape::MouthSmileRight,
            ],
            &[SRanipalLipShape::MouthApeShape],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("SmileOverlay", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthSmileLeft,
                SRanipalLipShape::MouthSmileRight,
            ],
            &[SRanipalLipShape::MouthLowerOverlay],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("SmilePout", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::MouthSmileLeft,
                SRanipalLipShape::MouthSmileRight,
            ],
            &[SRanipalLipShape::MouthPout],
            false,
        )
    })));

    // CheekPuff Right Based
    params.push(Box::new(FloatParam::new("PuffRightUpperOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::CheekPuffRight,
            SRanipalLipShape::MouthUpperOverturn,
        )
    })));
    params.push(Box::new(FloatParam::new("PuffRightLowerOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::CheekPuffRight,
            SRanipalLipShape::MouthLowerOverturn,
        )
    })));
    params.push(Box::new(FloatParam::new("PuffRightOverturn", |d| {
        pos_neg_avg_shape(
            d,
            &[SRanipalLipShape::CheekPuffRight],
            &[
                SRanipalLipShape::MouthUpperOverturn,
                SRanipalLipShape::MouthLowerOverturn,
            ],
            true,
        )
    })));

    // CheekPuff Left Based
    params.push(Box::new(FloatParam::new("PuffLeftUpperOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::CheekPuffLeft,
            SRanipalLipShape::MouthUpperOverturn,
        )
    })));
    params.push(Box::new(FloatParam::new("PuffLeftLowerOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::CheekPuffLeft,
            SRanipalLipShape::MouthLowerOverturn,
        )
    })));
    params.push(Box::new(FloatParam::new("PuffLeftOverturn", |d| {
        pos_neg_avg_shape(
            d,
            &[SRanipalLipShape::CheekPuffLeft],
            &[
                SRanipalLipShape::MouthUpperOverturn,
                SRanipalLipShape::MouthLowerOverturn,
            ],
            true,
        )
    })));

    // CheekPuff Combined
    params.push(Box::new(FloatParam::new("PuffUpperOverturn", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::CheekPuffRight,
                SRanipalLipShape::CheekPuffLeft,
            ],
            &[SRanipalLipShape::MouthUpperOverturn],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("PuffLowerOverturn", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::CheekPuffRight,
                SRanipalLipShape::CheekPuffLeft,
            ],
            &[SRanipalLipShape::MouthLowerOverturn],
            false,
        )
    })));
    params.push(Box::new(FloatParam::new("PuffOverturn", |d| {
        pos_neg_avg_shape(
            d,
            &[
                SRanipalLipShape::CheekPuffRight,
                SRanipalLipShape::CheekPuffLeft,
            ],
            &[
                SRanipalLipShape::MouthUpperOverturn,
                SRanipalLipShape::MouthLowerOverturn,
            ],
            true,
        )
    })));

    // TongueSteps
    // Combines TongueLongStep1 and TongueLongStep2 into a -1 to +1 range
    params.push(Box::new(FloatParam::new("TongueSteps", |d| {
        let step1 = get_sranipal_shape(SRanipalLipShape::TongueLongStep1, d);
        let step2 = get_sranipal_shape(SRanipalLipShape::TongueLongStep2, d);
        (step1 - step2) - 1.0
    })));

    params
}
