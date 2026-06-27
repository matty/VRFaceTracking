//! Legacy SRanipal lip tracking parameters for backwards compatibility.
//!
//! Includes both direct SRanipal shapes and merged/combined shapes.

use super::eparam::EParam;
use super::Parameter;
use vrft_common::{UnifiedExpressions, UnifiedTrackingData};

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
        SRanipalLipShape::JawOpen => (w(data, UnifiedExpressions::JawOpen)
            - w(data, UnifiedExpressions::MouthClosed))
        .clamp(0.0, 1.0),
        SRanipalLipShape::MouthApeShape => w(data, UnifiedExpressions::MouthClosed),
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
        SRanipalLipShape::MouthSmileRight => (w(data, UnifiedExpressions::MouthCornerPullRight)
            * 0.8
            + w(data, UnifiedExpressions::MouthCornerSlantRight) * 0.2)
            .max(w(data, UnifiedExpressions::MouthDimpleRight)),
        SRanipalLipShape::MouthSmileLeft => (w(data, UnifiedExpressions::MouthCornerPullLeft)
            * 0.8
            + w(data, UnifiedExpressions::MouthCornerSlantLeft) * 0.2)
            .max(w(data, UnifiedExpressions::MouthDimpleLeft)),
        SRanipalLipShape::MouthSadRight => {
            let bilateral_frown = (w(data, UnifiedExpressions::MouthFrownRight)
                + w(data, UnifiedExpressions::MouthFrownLeft))
                / 2.0;
            let smile_right = get_sranipal_shape(SRanipalLipShape::MouthSmileRight, data);
            (bilateral_frown.max(w(data, UnifiedExpressions::MouthStretchRight)) - smile_right)
                .max(0.0)
        }
        SRanipalLipShape::MouthSadLeft => {
            let bilateral_frown = (w(data, UnifiedExpressions::MouthFrownRight)
                + w(data, UnifiedExpressions::MouthFrownLeft))
                / 2.0;
            let smile_left = get_sranipal_shape(SRanipalLipShape::MouthSmileLeft, data);
            (bilateral_frown.max(w(data, UnifiedExpressions::MouthStretchLeft)) - smile_left)
                .max(0.0)
        }
        SRanipalLipShape::CheekPuffRight => w(data, UnifiedExpressions::CheekPuffRight),
        SRanipalLipShape::CheekPuffLeft => w(data, UnifiedExpressions::CheekPuffLeft),
        SRanipalLipShape::CheekSuck => {
            (w(data, UnifiedExpressions::CheekSuckLeft)
                + w(data, UnifiedExpressions::CheekSuckRight))
                / 2.0
        }
        SRanipalLipShape::MouthUpperUpRight => (w(data, UnifiedExpressions::MouthUpperUpRight)
            + (1.0 - w(data, UnifiedExpressions::LipPuckerUpperRight))
                * w(data, UnifiedExpressions::LipFunnelUpperRight))
        .max(0.0),
        SRanipalLipShape::MouthUpperUpLeft => (w(data, UnifiedExpressions::MouthUpperUpLeft)
            + (1.0 - w(data, UnifiedExpressions::LipPuckerUpperLeft))
                * w(data, UnifiedExpressions::LipFunnelUpperLeft))
        .max(0.0),
        SRanipalLipShape::MouthLowerDownRight => (w(data, UnifiedExpressions::MouthLowerDownRight)
            + (1.0 - w(data, UnifiedExpressions::LipPuckerLowerRight))
                * w(data, UnifiedExpressions::LipFunnelLowerRight))
        .max(0.0),
        SRanipalLipShape::MouthLowerDownLeft => (w(data, UnifiedExpressions::MouthLowerDownLeft)
            + (1.0 - w(data, UnifiedExpressions::LipPuckerLowerLeft))
                * w(data, UnifiedExpressions::LipFunnelLowerLeft))
        .max(0.0),
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
        SRanipalLipShape::TongueLongStep1 => {
            (w(data, UnifiedExpressions::TongueOut) * 2.0).min(1.0)
        }
        SRanipalLipShape::TongueLongStep2 => {
            (w(data, UnifiedExpressions::TongueOut) * 2.0 - 1.0).clamp(0.0, 1.0)
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
        params.push(Box::new(EParam::expression(name, move |d| {
            // Safe to convert since we iterate 0..37 and SRanipalLipShape has 38 values
            let shape = unsafe { std::mem::transmute::<usize, SRanipalLipShape>(shape_idx) };
            get_sranipal_shape(shape, d)
        })));
    }

    // Basic Merged Shapes
    params.push(Box::new(EParam::expression("JawX", |d| {
        pos_neg_shape(d, SRanipalLipShape::JawRight, SRanipalLipShape::JawLeft)
    })));
    params.push(Box::new(EParam::expression("MouthUpper", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperRight,
            SRanipalLipShape::MouthUpperLeft,
        )
    })));
    params.push(Box::new(EParam::expression("MouthLower", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthLowerRight,
            SRanipalLipShape::MouthLowerLeft,
        )
    })));
    params.push(Box::new(EParam::expression("MouthX", |d| {
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
    params.push(Box::new(EParam::expression("SmileSadRight", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileRight,
            SRanipalLipShape::MouthSadRight,
        )
    })));
    params.push(Box::new(EParam::expression("SmileSadLeft", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileLeft,
            SRanipalLipShape::MouthSadLeft,
        )
    })));
    params.push(Box::new(EParam::expression("SmileSad", |d| {
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
    params.push(Box::new(EParam::expression("TongueY", |d| {
        pos_neg_shape(d, SRanipalLipShape::TongueUp, SRanipalLipShape::TongueDown)
    })));
    params.push(Box::new(EParam::expression("TongueX", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::TongueRight,
            SRanipalLipShape::TongueLeft,
        )
    })));
    params.push(Box::new(EParam::expression("PuffSuckRight", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::CheekPuffRight,
            SRanipalLipShape::CheekSuck,
        )
    })));
    params.push(Box::new(EParam::expression("PuffSuckLeft", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::CheekPuffLeft,
            SRanipalLipShape::CheekSuck,
        )
    })));
    params.push(Box::new(EParam::expression("PuffSuck", |d| {
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
    params.push(Box::new(EParam::expression("JawOpenApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::JawOpen,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(EParam::expression("JawOpenPuff", |d| {
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
    params.push(Box::new(EParam::expression("JawOpenPuffRight", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::JawOpen,
            SRanipalLipShape::CheekPuffRight,
        )
    })));
    params.push(Box::new(EParam::expression("JawOpenPuffLeft", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::JawOpen,
            SRanipalLipShape::CheekPuffLeft,
        )
    })));
    params.push(Box::new(EParam::expression("JawOpenSuck", |d| {
        pos_neg_shape(d, SRanipalLipShape::JawOpen, SRanipalLipShape::CheekSuck)
    })));
    params.push(Box::new(EParam::expression("JawOpenForward", |d| {
        pos_neg_shape(d, SRanipalLipShape::JawOpen, SRanipalLipShape::JawForward)
    })));
    params.push(Box::new(EParam::expression("JawOpenOverlay", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::JawOpen,
            SRanipalLipShape::MouthLowerOverlay,
        )
    })));

    // MouthUpperUp Right Based
    params.push(Box::new(EParam::expression(
        "MouthUpperUpRightUpperInside",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthUpperUpRight,
                SRanipalLipShape::MouthUpperInside,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthUpperUpRightPuffRight",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthUpperUpRight,
                SRanipalLipShape::CheekPuffRight,
            )
        },
    )));
    params.push(Box::new(EParam::expression("MouthUpperUpRightApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpRight,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(EParam::expression("MouthUpperUpRightPout", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpRight,
            SRanipalLipShape::MouthPout,
        )
    })));
    params.push(Box::new(EParam::expression(
        "MouthUpperUpRightOverlay",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthUpperUpRight,
                SRanipalLipShape::MouthLowerOverlay,
            )
        },
    )));
    params.push(Box::new(EParam::expression("MouthUpperUpRightSuck", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpRight,
            SRanipalLipShape::CheekSuck,
        )
    })));

    // MouthUpperUp Left Based
    params.push(Box::new(EParam::expression(
        "MouthUpperUpLeftUpperInside",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthUpperInside,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthUpperUpLeftPuffLeft",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::CheekPuffLeft,
            )
        },
    )));
    params.push(Box::new(EParam::expression("MouthUpperUpLeftApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpLeft,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(EParam::expression("MouthUpperUpLeftPout", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpLeft,
            SRanipalLipShape::MouthPout,
        )
    })));
    params.push(Box::new(EParam::expression(
        "MouthUpperUpLeftOverlay",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthUpperUpLeft,
                SRanipalLipShape::MouthLowerOverlay,
            )
        },
    )));
    params.push(Box::new(EParam::expression("MouthUpperUpLeftSuck", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthUpperUpLeft,
            SRanipalLipShape::CheekSuck,
        )
    })));

    // MouthUpperUp Combined
    params.push(Box::new(EParam::expression(
        "MouthUpperUpUpperInside",
        |d| {
            pos_neg_avg_shape(
                d,
                &[
                    SRanipalLipShape::MouthUpperUpLeft,
                    SRanipalLipShape::MouthUpperUpRight,
                ],
                &[SRanipalLipShape::MouthUpperInside],
                false,
            )
        },
    )));
    params.push(Box::new(EParam::expression("MouthUpperUpInside", |d| {
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
    params.push(Box::new(EParam::expression("MouthUpperUpPuff", |d| {
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
    params.push(Box::new(EParam::expression("MouthUpperUpPuffLeft", |d| {
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
    params.push(Box::new(EParam::expression("MouthUpperUpPuffRight", |d| {
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
    params.push(Box::new(EParam::expression("MouthUpperUpApe", |d| {
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
    params.push(Box::new(EParam::expression("MouthUpperUpPout", |d| {
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
    params.push(Box::new(EParam::expression("MouthUpperUpOverlay", |d| {
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
    params.push(Box::new(EParam::expression("MouthUpperUpSuck", |d| {
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
    params.push(Box::new(EParam::expression(
        "MouthLowerDownRightLowerInside",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownRight,
                SRanipalLipShape::MouthLowerInside,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthLowerDownRightPuffRight",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownRight,
                SRanipalLipShape::CheekPuffRight,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthLowerDownRightApe",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownRight,
                SRanipalLipShape::MouthApeShape,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthLowerDownRightPout",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownRight,
                SRanipalLipShape::MouthPout,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthLowerDownRightOverlay",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownRight,
                SRanipalLipShape::MouthLowerOverlay,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthLowerDownRightSuck",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownRight,
                SRanipalLipShape::CheekSuck,
            )
        },
    )));

    // MouthLowerDown Left Based
    params.push(Box::new(EParam::expression(
        "MouthLowerDownLeftLowerInside",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerInside,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthLowerDownLeftPuffLeft",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::CheekPuffLeft,
            )
        },
    )));
    params.push(Box::new(EParam::expression("MouthLowerDownLeftApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthLowerDownLeft,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(EParam::expression(
        "MouthLowerDownLeftPout",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthPout,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthLowerDownLeftOverlay",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::MouthLowerOverlay,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthLowerDownLeftSuck",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerDownLeft,
                SRanipalLipShape::CheekSuck,
            )
        },
    )));

    // MouthLowerDown Combined
    params.push(Box::new(EParam::expression(
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
    params.push(Box::new(EParam::expression("MouthLowerDownInside", |d| {
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
    params.push(Box::new(EParam::expression("MouthLowerDownPuff", |d| {
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
    params.push(Box::new(EParam::expression(
        "MouthLowerDownPuffLeft",
        |d| {
            pos_neg_avg_shape(
                d,
                &[
                    SRanipalLipShape::MouthLowerDownLeft,
                    SRanipalLipShape::MouthLowerDownRight,
                ],
                &[SRanipalLipShape::CheekPuffLeft],
                false,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthLowerDownPuffRight",
        |d| {
            pos_neg_avg_shape(
                d,
                &[
                    SRanipalLipShape::MouthLowerDownLeft,
                    SRanipalLipShape::MouthLowerDownRight,
                ],
                &[SRanipalLipShape::CheekPuffRight],
                false,
            )
        },
    )));
    params.push(Box::new(EParam::expression("MouthLowerDownApe", |d| {
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
    params.push(Box::new(EParam::expression("MouthLowerDownPout", |d| {
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
    params.push(Box::new(EParam::expression("MouthLowerDownOverlay", |d| {
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
    params.push(Box::new(EParam::expression("MouthLowerDownSuck", |d| {
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
    params.push(Box::new(EParam::expression(
        "MouthUpperInsideOverturn",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthUpperInside,
                SRanipalLipShape::MouthUpperOverturn,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "MouthLowerInsideOverturn",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthLowerInside,
                SRanipalLipShape::MouthLowerOverturn,
            )
        },
    )));

    // Smile Right Based
    params.push(Box::new(EParam::expression(
        "SmileRightUpperOverturn",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthSmileRight,
                SRanipalLipShape::MouthUpperOverturn,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "SmileRightLowerOverturn",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthSmileRight,
                SRanipalLipShape::MouthLowerOverturn,
            )
        },
    )));
    params.push(Box::new(EParam::expression("SmileRightOverturn", |d| {
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
    params.push(Box::new(EParam::expression("SmileRightApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileRight,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(EParam::expression("SmileRightOverlay", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileRight,
            SRanipalLipShape::MouthLowerOverlay,
        )
    })));
    params.push(Box::new(EParam::expression("SmileRightPout", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileRight,
            SRanipalLipShape::MouthPout,
        )
    })));

    // Smile Left Based
    params.push(Box::new(EParam::expression(
        "SmileLeftUpperOverturn",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthSmileLeft,
                SRanipalLipShape::MouthUpperOverturn,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "SmileLeftLowerOverturn",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::MouthSmileLeft,
                SRanipalLipShape::MouthLowerOverturn,
            )
        },
    )));
    params.push(Box::new(EParam::expression("SmileLeftOverturn", |d| {
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
    params.push(Box::new(EParam::expression("SmileLeftApe", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileLeft,
            SRanipalLipShape::MouthApeShape,
        )
    })));
    params.push(Box::new(EParam::expression("SmileLeftOverlay", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileLeft,
            SRanipalLipShape::MouthLowerOverlay,
        )
    })));
    params.push(Box::new(EParam::expression("SmileLeftPout", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::MouthSmileLeft,
            SRanipalLipShape::MouthPout,
        )
    })));

    // Smile Combined
    params.push(Box::new(EParam::expression("SmileUpperOverturn", |d| {
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
    params.push(Box::new(EParam::expression("SmileLowerOverturn", |d| {
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
    params.push(Box::new(EParam::expression("SmileOverturn", |d| {
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
    params.push(Box::new(EParam::expression("SmileApe", |d| {
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
    params.push(Box::new(EParam::expression("SmileOverlay", |d| {
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
    params.push(Box::new(EParam::expression("SmilePout", |d| {
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
    params.push(Box::new(EParam::expression(
        "PuffRightUpperOverturn",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::CheekPuffRight,
                SRanipalLipShape::MouthUpperOverturn,
            )
        },
    )));
    params.push(Box::new(EParam::expression(
        "PuffRightLowerOverturn",
        |d| {
            pos_neg_shape(
                d,
                SRanipalLipShape::CheekPuffRight,
                SRanipalLipShape::MouthLowerOverturn,
            )
        },
    )));
    params.push(Box::new(EParam::expression("PuffRightOverturn", |d| {
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
    params.push(Box::new(EParam::expression("PuffLeftUpperOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::CheekPuffLeft,
            SRanipalLipShape::MouthUpperOverturn,
        )
    })));
    params.push(Box::new(EParam::expression("PuffLeftLowerOverturn", |d| {
        pos_neg_shape(
            d,
            SRanipalLipShape::CheekPuffLeft,
            SRanipalLipShape::MouthLowerOverturn,
        )
    })));
    params.push(Box::new(EParam::expression("PuffLeftOverturn", |d| {
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
    params.push(Box::new(EParam::expression("PuffUpperOverturn", |d| {
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
    params.push(Box::new(EParam::expression("PuffLowerOverturn", |d| {
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
    params.push(Box::new(EParam::expression("PuffOverturn", |d| {
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
    params.push(Box::new(EParam::expression("TongueSteps", |d| {
        let step1 = get_sranipal_shape(SRanipalLipShape::TongueLongStep1, d);
        let step2 = get_sranipal_shape(SRanipalLipShape::TongueLongStep2, d);
        (step1 + step2) - 1.0
    })));

    params
}
