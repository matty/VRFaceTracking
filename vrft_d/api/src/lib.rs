mod proxy;
pub use proxy::ProxyModule;

use anyhow::Result;
use glam::Vec3;
use serde::{Deserialize, Serialize};

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct UnifiedSingleEyeData {
    pub gaze: Vec3,
    pub pupil_diameter_mm: f32,
    pub openness: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UnifiedEyeData {
    pub left: UnifiedSingleEyeData,
    pub right: UnifiedSingleEyeData,
    pub max_dilation: f32,
    pub min_dilation: f32,
    pub left_diameter: f32,
    pub right_diameter: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnifiedExpressionShape {
    pub weight: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct UnifiedHeadData {
    pub head_yaw: f32,
    pub head_pitch: f32,
    pub head_roll: f32,
    pub head_pos_x: f32,
    pub head_pos_y: f32,
    pub head_pos_z: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnifiedTrackingData {
    pub eye: UnifiedEyeData,
    pub shapes: Vec<UnifiedExpressionShape>,
    pub head: UnifiedHeadData,
}

impl Default for UnifiedTrackingData {
    fn default() -> Self {
        Self {
            eye: UnifiedEyeData::default(),
            shapes: vec![UnifiedExpressionShape::default(); UnifiedExpressions::Max as usize],
            head: UnifiedHeadData::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum UnifiedExpressions {
    // Eye Gaze Expressions (Unused in shapes)

    // Eye Expressions
    EyeSquintRight = 0,
    EyeSquintLeft,
    EyeWideRight,
    EyeWideLeft,

    // Eyebrow Expressions
    BrowPinchRight,
    BrowPinchLeft,
    BrowLowererRight,
    BrowLowererLeft,
    BrowInnerUpRight,
    BrowInnerUpLeft,
    BrowOuterUpRight,
    BrowOuterUpLeft,

    // Nose Expressions
    NasalDilationRight,
    NasalDilationLeft,
    NasalConstrictRight,
    NasalConstrictLeft,

    // Cheek Expressions
    CheekSquintRight,
    CheekSquintLeft,
    CheekPuffRight,
    CheekPuffLeft,
    CheekSuckRight,
    CheekSuckLeft,

    // Jaw Exclusive Expressions
    JawOpen,
    JawRight,
    JawLeft,
    JawForward,
    JawBackward,
    JawClench,
    JawMandibleRaise,
    MouthClosed,

    // Lip Expressions
    LipSuckUpperRight,
    LipSuckUpperLeft,
    LipSuckLowerRight,
    LipSuckLowerLeft,
    LipSuckCornerRight,
    LipSuckCornerLeft,
    LipFunnelUpperRight,
    LipFunnelUpperLeft,
    LipFunnelLowerRight,
    LipFunnelLowerLeft,
    LipPuckerUpperRight,
    LipPuckerUpperLeft,
    LipPuckerLowerRight,
    LipPuckerLowerLeft,

    // Upper lip raiser group
    MouthUpperUpRight,
    MouthUpperUpLeft,
    MouthUpperDeepenRight,
    MouthUpperDeepenLeft,
    NoseSneerRight,
    NoseSneerLeft,

    // Lower lip depressor group
    MouthLowerDownRight,
    MouthLowerDownLeft,

    // Mouth Direction group
    MouthUpperRight,
    MouthUpperLeft,
    MouthLowerRight,
    MouthLowerLeft,

    // Smile group
    MouthCornerPullRight,
    MouthCornerPullLeft,
    MouthCornerSlantRight,
    MouthCornerSlantLeft,

    // Sad group
    MouthFrownRight,
    MouthFrownLeft,
    MouthStretchRight,
    MouthStretchLeft,
    MouthDimpleRight,
    MouthDimpleLeft,
    MouthRaiserUpper,
    MouthRaiserLower,
    MouthPressRight,
    MouthPressLeft,
    MouthTightenerRight,
    MouthTightenerLeft,

    // Tongue Expressions
    TongueOut,
    TongueUp,
    TongueDown,
    TongueRight,
    TongueLeft,
    TongueRoll,
    TongueBendDown,
    TongueCurlUp,
    TongueSquish,
    TongueFlat,
    TongueTwistRight,
    TongueTwistLeft,

    // Throat/Neck Expressions
    SoftPalateClose,
    ThroatSwallow,
    NeckFlexRight,
    NeckFlexLeft,

    Max,
}

impl TryFrom<usize> for UnifiedExpressions {
    type Error = ();

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value >= Self::Max as usize {
            return Err(());
        }
        // We've verified the value is in range [0, Max)
        // and the enum is repr(usize) with contiguous discriminants starting from 0
        Ok(unsafe { std::mem::transmute::<usize, UnifiedExpressions>(value) })
    }
}

/// log level for module logging
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

/// logger callback for modules
pub type LogCallback = extern "C" fn(level: LogLevel, target: *const i8, message: *const i8);

/// Logger interface for modules
pub struct ModuleLogger {
    callback: LogCallback,
    module_name: String,
}

impl ModuleLogger {
    pub fn new(callback: LogCallback, module_name: String) -> Self {
        Self {
            callback,
            module_name,
        }
    }

    pub fn error(&self, message: &str) {
        self.log(LogLevel::Error, message);
    }

    pub fn warn(&self, message: &str) {
        self.log(LogLevel::Warn, message);
    }

    pub fn info(&self, message: &str) {
        self.log(LogLevel::Info, message);
    }

    pub fn debug(&self, message: &str) {
        self.log(LogLevel::Debug, message);
    }

    pub fn trace(&self, message: &str) {
        self.log(LogLevel::Trace, message);
    }

    fn log(&self, level: LogLevel, message: &str) {
        let target = std::ffi::CString::new(self.module_name.as_str()).unwrap();
        let msg = std::ffi::CString::new(message).unwrap();
        (self.callback)(level, target.as_ptr(), msg.as_ptr());
    }
}

pub trait TrackingModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()>;
    fn update(&mut self, data: &mut UnifiedTrackingData) -> Result<()>;
    fn unload(&mut self);
}
