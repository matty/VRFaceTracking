use std::ffi::c_void;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Vector2 {
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Vector3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SingleEyeExpression {
    pub eye_wide: f32,
    pub eye_squeeze: f32,
    pub eye_frown: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct EyeExpression {
    pub left: SingleEyeExpression,
    pub right: SingleEyeExpression,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SingleEyeData {
    pub eye_data_validata_bit_mask: u64,
    pub gaze_origin_mm: Vector3,
    pub gaze_direction_normalized: Vector3,
    pub pupil_diameter_mm: f32,
    pub eye_openness: f32,
    pub pupil_position_in_sensor_area: Vector2,
}

impl SingleEyeData {
    pub fn get_validity(&self, validity: SingleEyeDataValidity) -> bool {
        (self.eye_data_validata_bit_mask & (1 << validity as i32)) > 0
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CombinedEyeData {
    pub eye_data: SingleEyeData,
    pub convergence_distance_validity: u8,
    pub convergence_distance_mm: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct TrackingImprovements {
    pub count: i32,
    pub items: [i32; 10], // unsafe fixed int (10)
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VerboseData {
    pub left: SingleEyeData,
    pub right: SingleEyeData,
    pub combined: CombinedEyeData,
    pub tracking_improvements: TrackingImprovements,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct EyeData_v2 {
    pub no_user: u8, // bool in C++ is often 1 byte, but should check alignment.
    pub frame_sequence: i32,
    pub timestamp: i32,
    pub verbose_data: VerboseData,
    pub expression_data: EyeExpression,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PredictionData_v2 {
    pub blend_shape_weight: [f32; 60],
}

impl Default for PredictionData_v2 {
    fn default() -> Self {
        Self {
            blend_shape_weight: [0.0; 60],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LipData_v2 {
    pub frame: i32,
    pub time: i32,
    pub image: *mut c_void, // IntPtr
    pub prediction_data: PredictionData_v2,
}

impl Default for LipData_v2 {
    fn default() -> Self {
        Self {
            frame: 0,
            time: 0,
            image: std::ptr::null_mut(),
            prediction_data: PredictionData_v2::default(),
        }
    }
}

#[repr(i32)]
#[derive(Debug, Clone, Copy)]
pub enum Error {
    Work = 0,
    FoxipSo = 1000, // placeholder, check actual value if needed
                    // add others?
}

#[repr(i32)]
#[derive(Debug, Clone, Copy)]
pub enum AnipalType {
    EyeV2 = 2, // ANIPAL_TYPE_EYE_V2
    LipV2 = 3, // ANIPAL_TYPE_LIP_V2
}

#[repr(i32)]
#[derive(Debug, Clone, Copy)]
pub enum SingleEyeDataValidity {
    GazeOriginValidity = 0,
    GazeDirectionValidity = 1,
    PupilDiameterValidity = 2,
    EyeOpennessValidity = 3,
    PupilPositionInSensorAreaValidity = 4,
}

// Lip Shapes Enum
#[repr(usize)]
#[derive(Debug, Clone, Copy)]
pub enum LipShapeV2 {
    JawRight = 0,
    JawLeft = 1,
    JawForward = 2,
    JawOpen = 3,
    MouthApeShape = 4,
    MouthUpperRight = 5,
    MouthUpperLeft = 6,
    MouthLowerRight = 7,
    MouthLowerLeft = 8,
    MouthUpperOverturn = 9,
    MouthLowerOverturn = 10,
    MouthPout = 11,
    MouthSmileRight = 12,
    MouthSmileLeft = 13,
    MouthSadRight = 14,
    MouthSadLeft = 15,
    CheekPuffRight = 16,
    CheekPuffLeft = 17,
    CheekSuck = 18,
    MouthUpperUpRight = 19,
    MouthUpperUpLeft = 20,
    MouthLowerDownRight = 21,
    MouthLowerDownLeft = 22,
    MouthUpperInside = 23,
    MouthLowerInside = 24,
    MouthLowerOverlay = 25,
    TongueLongStep1 = 26,
    TongueLongStep2 = 32,
    TongueDown = 30,
    TongueUp = 29,
    TongueRight = 28,
    TongueLeft = 27,
    TongueRoll = 31,
    TongueUpLeftMorph = 34,
    TongueUpRightMorph = 33,
    TongueDownLeftMorph = 36,
    TongueDownRightMorph = 35,
}
