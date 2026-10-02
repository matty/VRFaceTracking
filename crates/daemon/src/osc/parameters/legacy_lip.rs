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

impl SRanipalLipShape {
    /// Every shape except the `Max` sentinel, in declaration order
    pub const ALL: [Self; Self::Max as usize] = [
        Self::JawRight,
        Self::JawLeft,
        Self::JawForward,
        Self::JawOpen,
        Self::MouthApeShape,
        Self::MouthUpperRight,
        Self::MouthUpperLeft,
        Self::MouthLowerRight,
        Self::MouthLowerLeft,
        Self::MouthUpperOverturn,
        Self::MouthLowerOverturn,
        Self::MouthPout,
        Self::MouthSmileRight,
        Self::MouthSmileLeft,
        Self::MouthSadRight,
        Self::MouthSadLeft,
        Self::CheekPuffRight,
        Self::CheekPuffLeft,
        Self::CheekSuck,
        Self::MouthUpperUpRight,
        Self::MouthUpperUpLeft,
        Self::MouthLowerDownRight,
        Self::MouthLowerDownLeft,
        Self::MouthUpperInside,
        Self::MouthLowerInside,
        Self::MouthLowerOverlay,
        Self::TongueLongStep1,
        Self::TongueLongStep2,
        Self::TongueDown,
        Self::TongueUp,
        Self::TongueRight,
        Self::TongueLeft,
        Self::TongueRoll,
        Self::TongueUpLeftMorph,
        Self::TongueUpRightMorph,
        Self::TongueDownLeftMorph,
        Self::TongueDownRightMorph,
    ];
}

/// Maps SRanipal lip shapes to Unified Expressions
fn get_sranipal_shape(shape: SRanipalLipShape, data: &UnifiedTrackingData) -> f32 {
    match shape {
        SRanipalLipShape::JawRight => data.weight(UnifiedExpressions::JawRight),
        SRanipalLipShape::JawLeft => data.weight(UnifiedExpressions::JawLeft),
        SRanipalLipShape::JawForward => data.weight(UnifiedExpressions::JawForward),
        SRanipalLipShape::JawOpen => (data.weight(UnifiedExpressions::JawOpen)
            - data.weight(UnifiedExpressions::MouthClosed))
        .clamp(0.0, 1.0),
        SRanipalLipShape::MouthApeShape => data.weight(UnifiedExpressions::MouthClosed),
        SRanipalLipShape::MouthUpperRight => data.weight(UnifiedExpressions::MouthUpperRight),
        SRanipalLipShape::MouthUpperLeft => data.weight(UnifiedExpressions::MouthUpperLeft),
        SRanipalLipShape::MouthLowerRight => data.weight(UnifiedExpressions::MouthLowerRight),
        SRanipalLipShape::MouthLowerLeft => data.weight(UnifiedExpressions::MouthLowerLeft),
        SRanipalLipShape::MouthUpperOverturn => {
            (data.weight(UnifiedExpressions::LipFunnelUpperLeft)
                + data.weight(UnifiedExpressions::LipFunnelUpperRight))
                / 2.0
        }
        SRanipalLipShape::MouthLowerOverturn => {
            (data.weight(UnifiedExpressions::LipFunnelLowerLeft)
                + data.weight(UnifiedExpressions::LipFunnelLowerRight))
                / 2.0
        }
        SRanipalLipShape::MouthPout => {
            (data.weight(UnifiedExpressions::LipPuckerUpperLeft)
                + data.weight(UnifiedExpressions::LipPuckerUpperRight)
                + data.weight(UnifiedExpressions::LipPuckerLowerLeft)
                + data.weight(UnifiedExpressions::LipPuckerLowerRight))
                / 4.0
        }
        SRanipalLipShape::MouthSmileRight => {
            (data.weight(UnifiedExpressions::MouthCornerPullRight) * 0.8
                + data.weight(UnifiedExpressions::MouthCornerSlantRight) * 0.2)
                .max(data.weight(UnifiedExpressions::MouthDimpleRight))
        }
        SRanipalLipShape::MouthSmileLeft => (data.weight(UnifiedExpressions::MouthCornerPullLeft)
            * 0.8
            + data.weight(UnifiedExpressions::MouthCornerSlantLeft) * 0.2)
            .max(data.weight(UnifiedExpressions::MouthDimpleLeft)),
        SRanipalLipShape::MouthSadRight => {
            let bilateral_frown = (data.weight(UnifiedExpressions::MouthFrownRight)
                + data.weight(UnifiedExpressions::MouthFrownLeft))
                / 2.0;
            let smile_right = get_sranipal_shape(SRanipalLipShape::MouthSmileRight, data);
            (bilateral_frown.max(data.weight(UnifiedExpressions::MouthStretchRight)) - smile_right)
                .max(0.0)
        }
        SRanipalLipShape::MouthSadLeft => {
            let bilateral_frown = (data.weight(UnifiedExpressions::MouthFrownRight)
                + data.weight(UnifiedExpressions::MouthFrownLeft))
                / 2.0;
            let smile_left = get_sranipal_shape(SRanipalLipShape::MouthSmileLeft, data);
            (bilateral_frown.max(data.weight(UnifiedExpressions::MouthStretchLeft)) - smile_left)
                .max(0.0)
        }
        SRanipalLipShape::CheekPuffRight => data.weight(UnifiedExpressions::CheekPuffRight),
        SRanipalLipShape::CheekPuffLeft => data.weight(UnifiedExpressions::CheekPuffLeft),
        SRanipalLipShape::CheekSuck => {
            (data.weight(UnifiedExpressions::CheekSuckLeft)
                + data.weight(UnifiedExpressions::CheekSuckRight))
                / 2.0
        }
        SRanipalLipShape::MouthUpperUpRight => (data.weight(UnifiedExpressions::MouthUpperUpRight)
            + (1.0 - data.weight(UnifiedExpressions::LipPuckerUpperRight))
                * data.weight(UnifiedExpressions::LipFunnelUpperRight))
        .max(0.0),
        SRanipalLipShape::MouthUpperUpLeft => (data.weight(UnifiedExpressions::MouthUpperUpLeft)
            + (1.0 - data.weight(UnifiedExpressions::LipPuckerUpperLeft))
                * data.weight(UnifiedExpressions::LipFunnelUpperLeft))
        .max(0.0),
        SRanipalLipShape::MouthLowerDownRight => (data
            .weight(UnifiedExpressions::MouthLowerDownRight)
            + (1.0 - data.weight(UnifiedExpressions::LipPuckerLowerRight))
                * data.weight(UnifiedExpressions::LipFunnelLowerRight))
        .max(0.0),
        SRanipalLipShape::MouthLowerDownLeft => (data
            .weight(UnifiedExpressions::MouthLowerDownLeft)
            + (1.0 - data.weight(UnifiedExpressions::LipPuckerLowerLeft))
                * data.weight(UnifiedExpressions::LipFunnelLowerLeft))
        .max(0.0),
        SRanipalLipShape::MouthUpperInside => {
            (data.weight(UnifiedExpressions::LipSuckUpperLeft)
                + data.weight(UnifiedExpressions::LipSuckUpperRight))
                / 2.0
        }
        SRanipalLipShape::MouthLowerInside => {
            (data.weight(UnifiedExpressions::LipSuckLowerLeft)
                + data.weight(UnifiedExpressions::LipSuckLowerRight))
                / 2.0
        }
        SRanipalLipShape::MouthLowerOverlay => data.weight(UnifiedExpressions::MouthRaiserLower),
        SRanipalLipShape::TongueLongStep1 => {
            (data.weight(UnifiedExpressions::TongueOut) * 2.0).min(1.0)
        }
        SRanipalLipShape::TongueLongStep2 => {
            (data.weight(UnifiedExpressions::TongueOut) * 2.0 - 1.0).clamp(0.0, 1.0)
        }
        SRanipalLipShape::TongueDown => data.weight(UnifiedExpressions::TongueDown),
        SRanipalLipShape::TongueUp => data.weight(UnifiedExpressions::TongueUp),
        SRanipalLipShape::TongueRight => data.weight(UnifiedExpressions::TongueRight),
        SRanipalLipShape::TongueLeft => data.weight(UnifiedExpressions::TongueLeft),
        SRanipalLipShape::TongueRoll => data.weight(UnifiedExpressions::TongueRoll),
        SRanipalLipShape::TongueUpLeftMorph => {
            data.weight(UnifiedExpressions::TongueUp)
                * (1.0 - data.weight(UnifiedExpressions::TongueRight))
        }
        SRanipalLipShape::TongueUpRightMorph => {
            data.weight(UnifiedExpressions::TongueUp)
                * (1.0 - data.weight(UnifiedExpressions::TongueLeft))
        }
        SRanipalLipShape::TongueDownLeftMorph => {
            data.weight(UnifiedExpressions::TongueDown)
                * (1.0 - data.weight(UnifiedExpressions::TongueRight))
        }
        SRanipalLipShape::TongueDownRightMorph => {
            data.weight(UnifiedExpressions::TongueDown)
                * (1.0 - data.weight(UnifiedExpressions::TongueLeft))
        }
        SRanipalLipShape::Max => 0.0,
    }
}

/// How [`combine`] pools several shapes into one value
#[derive(Clone, Copy)]
enum Pool {
    /// Average of the shapes
    Mean,
    /// Largest shape, floored at 0
    Max,
}

/// Pools `shapes` into one value
fn combine(data: &UnifiedTrackingData, shapes: &[SRanipalLipShape], pool: Pool) -> f32 {
    let values = shapes.iter().map(|s| get_sranipal_shape(*s, data));
    match pool {
        Pool::Max => values.fold(0.0_f32, |a, b| a.max(b)),
        Pool::Mean if shapes.is_empty() => 0.0,
        Pool::Mean => values.sum::<f32>() / shapes.len() as f32,
    }
}

/// Merged shapes: name, positive shapes, negative shapes and how each side is
/// pooled. The parameter is the positive pool minus the negative pool
type MergedShape = (
    &'static str,
    &'static [SRanipalLipShape],
    &'static [SRanipalLipShape],
    Pool,
);

#[rustfmt::skip]
const MERGED_SHAPES: &[MergedShape] = {
    use SRanipalLipShape::*;
    &[
        // Basic Merged Shapes
        ("JawX", &[JawRight], &[JawLeft], Pool::Mean),
        ("MouthUpper", &[MouthUpperRight], &[MouthUpperLeft], Pool::Mean),
        ("MouthLower", &[MouthLowerRight], &[MouthLowerLeft], Pool::Mean),
        ("MouthX", &[MouthUpperRight, MouthLowerRight], &[MouthUpperLeft, MouthLowerLeft], Pool::Max),
        ("SmileSadRight", &[MouthSmileRight], &[MouthSadRight], Pool::Mean),
        ("SmileSadLeft", &[MouthSmileLeft], &[MouthSadLeft], Pool::Mean),
        ("SmileSad", &[MouthSmileLeft, MouthSmileRight], &[MouthSadLeft, MouthSadRight], Pool::Mean),
        ("TongueY", &[TongueUp], &[TongueDown], Pool::Mean),
        ("TongueX", &[TongueRight], &[TongueLeft], Pool::Mean),
        ("PuffSuckRight", &[CheekPuffRight], &[CheekSuck], Pool::Mean),
        ("PuffSuckLeft", &[CheekPuffLeft], &[CheekSuck], Pool::Mean),
        ("PuffSuck", &[CheekPuffLeft, CheekPuffRight], &[CheekSuck], Pool::Max),
        // JawOpen Based
        ("JawOpenApe", &[JawOpen], &[MouthApeShape], Pool::Mean),
        ("JawOpenPuff", &[JawOpen], &[CheekPuffLeft, CheekPuffRight], Pool::Mean),
        ("JawOpenPuffRight", &[JawOpen], &[CheekPuffRight], Pool::Mean),
        ("JawOpenPuffLeft", &[JawOpen], &[CheekPuffLeft], Pool::Mean),
        ("JawOpenSuck", &[JawOpen], &[CheekSuck], Pool::Mean),
        ("JawOpenForward", &[JawOpen], &[JawForward], Pool::Mean),
        ("JawOpenOverlay", &[JawOpen], &[MouthLowerOverlay], Pool::Mean),
        // MouthUpperUp Right Based
        ("MouthUpperUpRightUpperInside", &[MouthUpperUpRight], &[MouthUpperInside], Pool::Mean),
        ("MouthUpperUpRightPuffRight", &[MouthUpperUpRight], &[CheekPuffRight], Pool::Mean),
        ("MouthUpperUpRightApe", &[MouthUpperUpRight], &[MouthApeShape], Pool::Mean),
        ("MouthUpperUpRightPout", &[MouthUpperUpRight], &[MouthPout], Pool::Mean),
        ("MouthUpperUpRightOverlay", &[MouthUpperUpRight], &[MouthLowerOverlay], Pool::Mean),
        ("MouthUpperUpRightSuck", &[MouthUpperUpRight], &[CheekSuck], Pool::Mean),
        // MouthUpperUp Left Based
        ("MouthUpperUpLeftUpperInside", &[MouthUpperUpLeft], &[MouthUpperInside], Pool::Mean),
        ("MouthUpperUpLeftPuffLeft", &[MouthUpperUpLeft], &[CheekPuffLeft], Pool::Mean),
        ("MouthUpperUpLeftApe", &[MouthUpperUpLeft], &[MouthApeShape], Pool::Mean),
        ("MouthUpperUpLeftPout", &[MouthUpperUpLeft], &[MouthPout], Pool::Mean),
        ("MouthUpperUpLeftOverlay", &[MouthUpperUpLeft], &[MouthLowerOverlay], Pool::Mean),
        ("MouthUpperUpLeftSuck", &[MouthUpperUpLeft], &[CheekSuck], Pool::Mean),
        // MouthUpperUp Combined
        ("MouthUpperUpUpperInside", &[MouthUpperUpLeft, MouthUpperUpRight], &[MouthUpperInside], Pool::Mean),
        ("MouthUpperUpInside", &[MouthUpperUpLeft, MouthUpperUpRight], &[MouthUpperInside, MouthLowerInside], Pool::Max),
        ("MouthUpperUpPuff", &[MouthUpperUpLeft, MouthUpperUpRight], &[CheekPuffLeft, CheekPuffRight], Pool::Mean),
        ("MouthUpperUpPuffLeft", &[MouthUpperUpLeft, MouthUpperUpRight], &[CheekPuffLeft], Pool::Mean),
        ("MouthUpperUpPuffRight", &[MouthUpperUpLeft, MouthUpperUpRight], &[CheekPuffRight], Pool::Mean),
        ("MouthUpperUpApe", &[MouthUpperUpLeft, MouthUpperUpRight], &[MouthApeShape], Pool::Mean),
        ("MouthUpperUpPout", &[MouthUpperUpLeft, MouthUpperUpRight], &[MouthPout], Pool::Mean),
        ("MouthUpperUpOverlay", &[MouthUpperUpLeft, MouthUpperUpRight], &[MouthLowerOverlay], Pool::Mean),
        ("MouthUpperUpSuck", &[MouthUpperUpLeft, MouthUpperUpRight], &[CheekSuck], Pool::Mean),
        // MouthLowerDown Right Based
        ("MouthLowerDownRightLowerInside", &[MouthLowerDownRight], &[MouthLowerInside], Pool::Mean),
        ("MouthLowerDownRightPuffRight", &[MouthLowerDownRight], &[CheekPuffRight], Pool::Mean),
        ("MouthLowerDownRightApe", &[MouthLowerDownRight], &[MouthApeShape], Pool::Mean),
        ("MouthLowerDownRightPout", &[MouthLowerDownRight], &[MouthPout], Pool::Mean),
        ("MouthLowerDownRightOverlay", &[MouthLowerDownRight], &[MouthLowerOverlay], Pool::Mean),
        ("MouthLowerDownRightSuck", &[MouthLowerDownRight], &[CheekSuck], Pool::Mean),
        // MouthLowerDown Left Based
        ("MouthLowerDownLeftLowerInside", &[MouthLowerDownLeft], &[MouthLowerInside], Pool::Mean),
        ("MouthLowerDownLeftPuffLeft", &[MouthLowerDownLeft], &[CheekPuffLeft], Pool::Mean),
        ("MouthLowerDownLeftApe", &[MouthLowerDownLeft], &[MouthApeShape], Pool::Mean),
        ("MouthLowerDownLeftPout", &[MouthLowerDownLeft], &[MouthPout], Pool::Mean),
        ("MouthLowerDownLeftOverlay", &[MouthLowerDownLeft], &[MouthLowerOverlay], Pool::Mean),
        ("MouthLowerDownLeftSuck", &[MouthLowerDownLeft], &[CheekSuck], Pool::Mean),
        // MouthLowerDown Combined
        ("MouthLowerDownLowerInside", &[MouthLowerDownLeft, MouthLowerDownRight], &[MouthLowerInside], Pool::Mean),
        ("MouthLowerDownInside", &[MouthLowerDownLeft, MouthLowerDownRight], &[MouthUpperInside, MouthLowerInside], Pool::Max),
        ("MouthLowerDownPuff", &[MouthLowerDownLeft, MouthLowerDownRight], &[CheekPuffLeft, CheekPuffRight], Pool::Mean),
        ("MouthLowerDownPuffLeft", &[MouthLowerDownLeft, MouthLowerDownRight], &[CheekPuffLeft], Pool::Mean),
        ("MouthLowerDownPuffRight", &[MouthLowerDownLeft, MouthLowerDownRight], &[CheekPuffRight], Pool::Mean),
        ("MouthLowerDownApe", &[MouthLowerDownLeft, MouthLowerDownRight], &[MouthApeShape], Pool::Mean),
        ("MouthLowerDownPout", &[MouthLowerDownLeft, MouthLowerDownRight], &[MouthPout], Pool::Mean),
        ("MouthLowerDownOverlay", &[MouthLowerDownLeft, MouthLowerDownRight], &[MouthLowerOverlay], Pool::Mean),
        ("MouthLowerDownSuck", &[MouthLowerDownLeft, MouthLowerDownRight], &[CheekSuck], Pool::Mean),
        // Inside/Overturn Based
        ("MouthUpperInsideOverturn", &[MouthUpperInside], &[MouthUpperOverturn], Pool::Mean),
        ("MouthLowerInsideOverturn", &[MouthLowerInside], &[MouthLowerOverturn], Pool::Mean),
        // Smile Right Based
        ("SmileRightUpperOverturn", &[MouthSmileRight], &[MouthUpperOverturn], Pool::Mean),
        ("SmileRightLowerOverturn", &[MouthSmileRight], &[MouthLowerOverturn], Pool::Mean),
        ("SmileRightOverturn", &[MouthSmileRight], &[MouthUpperOverturn, MouthLowerOverturn], Pool::Mean),
        ("SmileRightApe", &[MouthSmileRight], &[MouthApeShape], Pool::Mean),
        ("SmileRightOverlay", &[MouthSmileRight], &[MouthLowerOverlay], Pool::Mean),
        ("SmileRightPout", &[MouthSmileRight], &[MouthPout], Pool::Mean),
        // Smile Left Based
        ("SmileLeftUpperOverturn", &[MouthSmileLeft], &[MouthUpperOverturn], Pool::Mean),
        ("SmileLeftLowerOverturn", &[MouthSmileLeft], &[MouthLowerOverturn], Pool::Mean),
        ("SmileLeftOverturn", &[MouthSmileLeft], &[MouthUpperOverturn, MouthLowerOverturn], Pool::Mean),
        ("SmileLeftApe", &[MouthSmileLeft], &[MouthApeShape], Pool::Mean),
        ("SmileLeftOverlay", &[MouthSmileLeft], &[MouthLowerOverlay], Pool::Mean),
        ("SmileLeftPout", &[MouthSmileLeft], &[MouthPout], Pool::Mean),
        // Smile Combined
        ("SmileUpperOverturn", &[MouthSmileLeft, MouthSmileRight], &[MouthUpperOverturn], Pool::Mean),
        ("SmileLowerOverturn", &[MouthSmileLeft, MouthSmileRight], &[MouthLowerOverturn], Pool::Mean),
        ("SmileOverturn", &[MouthSmileLeft, MouthSmileRight], &[MouthUpperOverturn, MouthLowerOverturn], Pool::Mean),
        ("SmileApe", &[MouthSmileLeft, MouthSmileRight], &[MouthApeShape], Pool::Mean),
        ("SmileOverlay", &[MouthSmileLeft, MouthSmileRight], &[MouthLowerOverlay], Pool::Mean),
        ("SmilePout", &[MouthSmileLeft, MouthSmileRight], &[MouthPout], Pool::Mean),
        // CheekPuff Right Based
        ("PuffRightUpperOverturn", &[CheekPuffRight], &[MouthUpperOverturn], Pool::Mean),
        ("PuffRightLowerOverturn", &[CheekPuffRight], &[MouthLowerOverturn], Pool::Mean),
        ("PuffRightOverturn", &[CheekPuffRight], &[MouthUpperOverturn, MouthLowerOverturn], Pool::Max),
        // CheekPuff Left Based
        ("PuffLeftUpperOverturn", &[CheekPuffLeft], &[MouthUpperOverturn], Pool::Mean),
        ("PuffLeftLowerOverturn", &[CheekPuffLeft], &[MouthLowerOverturn], Pool::Mean),
        ("PuffLeftOverturn", &[CheekPuffLeft], &[MouthUpperOverturn, MouthLowerOverturn], Pool::Max),
        // CheekPuff Combined
        ("PuffUpperOverturn", &[CheekPuffRight, CheekPuffLeft], &[MouthUpperOverturn], Pool::Mean),
        ("PuffLowerOverturn", &[CheekPuffRight, CheekPuffLeft], &[MouthLowerOverturn], Pool::Mean),
        ("PuffOverturn", &[CheekPuffRight, CheekPuffLeft], &[MouthUpperOverturn, MouthLowerOverturn], Pool::Max),
    ]
};

/// Creates all legacy SRanipal lip shape parameters
pub fn create_legacy_lip_parameters() -> Vec<Box<dyn Parameter>> {
    let mut params: Vec<Box<dyn Parameter>> = Vec::new();

    // All SRanipal Lip Shapes (direct mappings)
    for shape in SRanipalLipShape::ALL {
        params.push(Box::new(EParam::expression(
            &format!("{shape:?}"),
            move |d| get_sranipal_shape(shape, d),
        )));
    }

    for &(name, positives, negatives, pool) in MERGED_SHAPES {
        params.push(Box::new(EParam::expression(name, move |d| {
            combine(d, positives, pool) - combine(d, negatives, pool)
        })));
    }

    // TongueSteps
    // Combines TongueLongStep1 and TongueLongStep2 into a -1 to +1 range
    params.push(Box::new(EParam::expression("TongueSteps", |d| {
        let step1 = get_sranipal_shape(SRanipalLipShape::TongueLongStep1, d);
        let step2 = get_sranipal_shape(SRanipalLipShape::TongueLongStep2, d);
        (step1 + step2) - 1.0
    })));

    params
}
