// Thanks to "VRCFaceTracking" for the initial implementation
// https://github.com/guygodin/VirtualDesktop.VRCFaceTracking

use anyhow::Result;
use api::{ModuleLogger, TrackingModule, UnifiedExpressions, UnifiedTrackingData};
use glam::{Quat, Vec2};
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{OpenEventW, WaitForSingleObject, EVENT_ALL_ACCESS};

const BODY_STATE_MAP_NAME: &str = "VirtualDesktop.BodyState";
const BODY_STATE_EVENT_NAME: &str = "VirtualDesktop.BodyStateEvent";
const ENABLED_EYE_SMOOTHING: bool = false;
const ENABLED_CHEEK_CROSSTALK_REDUCTION: bool = false;
const SMOOTHING_FACTOR: f32 = 0.5;

/// Extracts pitch/yaw Euler angles from a quaternion orientation.
/// Returns (pitch, yaw) in radians.
fn quaternion_to_pitch_yaw(q: Quat) -> (f32, f32) {
    let (x, y, z, w) = (q.x, q.y, q.z, q.w);
    let magnitude = (x * x + y * y + z * z + w * w).sqrt();

    // Guard against zero/near-zero magnitude (malformed quaternion)
    if magnitude < 0.0001 {
        return (0.0, 0.0);
    }

    let xm = x / magnitude;
    let ym = y / magnitude;
    let zm = z / magnitude;
    let wm = w / magnitude;

    let pitch = (2.0 * (xm * zm - wm * ym)).asin();
    let yaw = (2.0 * (ym * zm + wm * xm)).atan2(wm * wm - xm * xm - ym * ym + zm * zm);

    (pitch, yaw)
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Vector3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Quaternion {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Pose {
    pub orientation: Quaternion,
    pub position: Vector3,
}

#[repr(C)]
pub struct FaceState {
    pub face_is_valid: u8,
    pub is_eye_following_blendshapes_valid: u8,
    // Padding to align to 4 bytes
    pub _padding1: [u8; 2],
    pub expression_weights: [f32; 70],
    pub expression_confidences: [f32; 2],
    pub left_eye_is_valid: u8,
    pub right_eye_is_valid: u8,
    // Padding to align to 4 bytes
    pub _padding2: [u8; 2],
    pub left_eye_pose: Pose,
    pub right_eye_pose: Pose,
    pub left_eye_confidence: f32,
    pub right_eye_confidence: f32,
}

struct EyeSmoothingState {
    left_rot: Quat,
    right_rot: Quat,
    initialized: bool,
}

impl EyeSmoothingState {
    fn new() -> Self {
        Self {
            left_rot: Quat::IDENTITY,
            right_rot: Quat::IDENTITY,
            initialized: false,
        }
    }
}

pub struct VirtualDesktopModule {
    mapping_handle: HANDLE,
    event_handle: HANDLE,
    face_state_ptr: *const FaceState,
    logger: Option<ModuleLogger>,
    eye_smoothing: EyeSmoothingState,
    last_valid_frame_time: std::time::Instant,
}

impl VirtualDesktopModule {
    pub fn new() -> Self {
        Self {
            mapping_handle: HANDLE(std::ptr::null_mut()),
            event_handle: HANDLE(std::ptr::null_mut()),
            face_state_ptr: std::ptr::null(),
            logger: None,
            eye_smoothing: EyeSmoothingState::new(),
            last_valid_frame_time: std::time::Instant::now(),
        }
    }

    fn is_connected(&self) -> bool {
        !self.mapping_handle.is_invalid()
            && !self.event_handle.is_invalid()
            && !self.face_state_ptr.is_null()
    }

    fn disconnect(&mut self) {
        use windows::Win32::System::Memory::UnmapViewOfFile;
        unsafe {
            if !self.event_handle.is_invalid() {
                let _ = CloseHandle(self.event_handle);
                self.event_handle = HANDLE(std::ptr::null_mut());
            }
            if !self.face_state_ptr.is_null() {
                let _ =
                    UnmapViewOfFile(windows::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: self.face_state_ptr as *mut std::ffi::c_void,
                    });
                self.face_state_ptr = std::ptr::null();
            }
            if !self.mapping_handle.is_invalid() {
                let _ = CloseHandle(self.mapping_handle);
                self.mapping_handle = HANDLE(std::ptr::null_mut());
            }
        }
        if let Some(logger) = &self.logger {
            logger.info("Virtual Desktop Disconnected.");
        }
    }

    fn connect(&mut self) -> Result<()> {
        use windows::core::PCWSTR;
        use windows::Win32::System::Memory::{
            MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE,
            MEMORY_MAPPED_VIEW_ADDRESS,
        };

        let map_name_wide: Vec<u16> = BODY_STATE_MAP_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let event_name_wide: Vec<u16> = BODY_STATE_EVENT_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        unsafe {
            // Open file mapping
            let mapping_handle = match OpenFileMappingW(
                (FILE_MAP_READ | FILE_MAP_WRITE).0,
                false,
                PCWSTR(map_name_wide.as_ptr()),
            ) {
                Ok(handle) if !handle.is_invalid() => handle,
                _ => return Err(anyhow::anyhow!("Failed to open file mapping")),
            };

            // Map view of file
            let ptr = MapViewOfFile(
                mapping_handle,
                FILE_MAP_READ | FILE_MAP_WRITE,
                0,
                0,
                std::mem::size_of::<FaceState>(),
            );

            if ptr.Value.is_null() {
                let _ = CloseHandle(mapping_handle);
                return Err(anyhow::anyhow!("Failed to map view of file"));
            }

            // Open event
            let event_handle =
                match OpenEventW(EVENT_ALL_ACCESS, false, PCWSTR(event_name_wide.as_ptr())) {
                    Ok(event) if !event.is_invalid() => event,
                    _ => {
                        // Clean up on failure
                        let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: ptr.Value });
                        let _ = CloseHandle(mapping_handle);
                        return Err(anyhow::anyhow!("Failed to open event"));
                    }
                };

            // Success - store all handles
            self.mapping_handle = mapping_handle;
            self.face_state_ptr = ptr.Value as *const FaceState;
            self.event_handle = event_handle;
            self.last_valid_frame_time = std::time::Instant::now();

            if let Some(logger) = &self.logger {
                logger.info("Virtual Desktop Connected!");
            }

            Ok(())
        }
    }

    fn update_eye_data(&mut self, data: &mut UnifiedTrackingData, face_state: &FaceState) {
        let expressions = &face_state.expression_weights;

        let eye_openness_scale = 1.0; // Scale factor for eye openness to match VD's expected range

        if face_state.left_eye_is_valid != 0 {
            // Eye Openness
            let left_openness = (1.0
                - (expressions[12] + expressions[4] * expressions[28]).clamp(0.0, 1.0))
                * eye_openness_scale;
            data.eye.left.openness = left_openness;

            // Gaze: extract pitch/yaw from quaternion orientation
            let mut left_quat = Quat::from_xyzw(
                face_state.left_eye_pose.orientation.x,
                face_state.left_eye_pose.orientation.y,
                face_state.left_eye_pose.orientation.z,
                face_state.left_eye_pose.orientation.w,
            );

            if ENABLED_EYE_SMOOTHING {
                if !self.eye_smoothing.initialized {
                    self.eye_smoothing.left_rot = left_quat;
                } else {
                    left_quat = self
                        .eye_smoothing
                        .left_rot
                        .slerp(left_quat, SMOOTHING_FACTOR);
                    self.eye_smoothing.left_rot = left_quat;
                }
            }

            let (pitch, yaw) = quaternion_to_pitch_yaw(left_quat);
            data.eye.left.gaze = Vec2::new(pitch, yaw);

            data.eye.left.pupil_diameter_mm = 5.0;
        } else {
            data.eye.left.openness = 0.5;
            data.eye.left.pupil_diameter_mm = 2.0;
            data.eye.left.gaze = glam::Vec2::ZERO;
        }

        if face_state.right_eye_is_valid != 0 {
            // Eye Openness
            let right_openness = (1.0
                - (expressions[13] + expressions[5] * expressions[29]).clamp(0.0, 1.0))
                * eye_openness_scale;
            data.eye.right.openness = right_openness;

            // Gaze: extract pitch/yaw Euler angles from quaternion orientation
            let mut right_quat = Quat::from_xyzw(
                face_state.right_eye_pose.orientation.x,
                face_state.right_eye_pose.orientation.y,
                face_state.right_eye_pose.orientation.z,
                face_state.right_eye_pose.orientation.w,
            );

            if ENABLED_EYE_SMOOTHING {
                if !self.eye_smoothing.initialized {
                    self.eye_smoothing.right_rot = right_quat;
                    self.eye_smoothing.initialized = true;
                } else {
                    right_quat = self
                        .eye_smoothing
                        .right_rot
                        .slerp(right_quat, SMOOTHING_FACTOR);
                    self.eye_smoothing.right_rot = right_quat;
                }
            }

            let (pitch, yaw) = quaternion_to_pitch_yaw(right_quat);
            data.eye.right.gaze = Vec2::new(pitch, yaw);

            data.eye.right.pupil_diameter_mm = 5.0;
        } else {
            data.eye.right.openness = 0.5;
            data.eye.right.pupil_diameter_mm = 2.0;
            data.eye.right.gaze = glam::Vec2::ZERO;
        }

        // Pupil dilation normalization bounds for downstream consumers
        data.eye.min_dilation = 0.0;
        data.eye.max_dilation = 10.0;
    }

    fn update_eye_expressions(&self, data: &mut UnifiedTrackingData, face_state: &FaceState) {
        let w = &face_state.expression_weights;
        let s = &mut data.shapes;

        macro_rules! map_idx {
            ($unified:ident, $idx:expr) => {
                s[UnifiedExpressions::$unified as usize].weight = w[$idx];
            };
        }

        // Eye Expressions
        map_idx!(EyeWideLeft, 59); // UpperLidRaiserL
        map_idx!(EyeWideRight, 60); // UpperLidRaiserR
        map_idx!(EyeSquintLeft, 28); // LidTightenerL
        map_idx!(EyeSquintRight, 29); // LidTightenerR

        // Brow Expressions
        map_idx!(BrowInnerUpLeft, 22); // InnerBrowRaiserL
        map_idx!(BrowInnerUpRight, 23); // InnerBrowRaiserR
        map_idx!(BrowOuterUpLeft, 57); // OuterBrowRaiserL
        map_idx!(BrowOuterUpRight, 58); // OuterBrowRaiserR
        map_idx!(BrowPinchLeft, 0); // BrowLowererL
        map_idx!(BrowLowererLeft, 0); // BrowLowererL
        map_idx!(BrowPinchRight, 1); // BrowLowererR
        map_idx!(BrowLowererRight, 1); // BrowLowererR
    }

    fn update_mouth_expressions(&self, data: &mut UnifiedTrackingData, face_state: &FaceState) {
        let w = &face_state.expression_weights;
        let s = &mut data.shapes;

        macro_rules! map_idx {
            ($unified:ident, $idx:expr) => {
                s[UnifiedExpressions::$unified as usize].weight = w[$idx];
            };
        }
        macro_rules! map_val {
            ($unified:ident, $val:expr) => {
                s[UnifiedExpressions::$unified as usize].weight = $val;
            };
        }

        // Jaw Expressions
        map_idx!(JawOpen, 24); // JawDrop
        map_idx!(JawLeft, 25); // JawSidewaysLeft
        map_idx!(JawRight, 26); // JawSidewaysRight
        map_idx!(JawForward, 27); // JawThrust

        // Mouth Expressions
        map_idx!(MouthClosed, 50); // LipsToward
        map_idx!(MouthUpperLeft, 53); // MouthLeft
        map_idx!(MouthLowerLeft, 53); // MouthLeft
        map_idx!(MouthUpperRight, 54); // MouthRight
        map_idx!(MouthLowerRight, 54); // MouthRight

        map_idx!(MouthCornerPullLeft, 32); // LipCornerPullerL
        map_idx!(MouthCornerSlantLeft, 32); // LipCornerPullerL
        map_idx!(MouthCornerPullRight, 33); // LipCornerPullerR
        map_idx!(MouthCornerSlantRight, 33); // LipCornerPullerR

        map_idx!(MouthFrownLeft, 30); // LipCornerDepressorL
        map_idx!(MouthFrownRight, 31); // LipCornerDepressorR

        map_idx!(MouthLowerDownLeft, 51); // LowerLipDepressorL
        map_idx!(MouthLowerDownRight, 52); // LowerLipDepressorR

        // Upper Lip Up Workaround
        map_val!(MouthUpperUpLeft, (w[61] - w[55]).max(0.0));
        map_val!(MouthUpperDeepenLeft, (w[61] - w[55]).max(0.0));
        map_val!(MouthUpperUpRight, (w[62] - w[56]).max(0.0));
        map_val!(MouthUpperDeepenRight, (w[62] - w[56]).max(0.0));

        map_idx!(MouthRaiserUpper, 9); // ChinRaiserT
        map_idx!(MouthRaiserLower, 8); // ChinRaiserB

        map_idx!(MouthDimpleLeft, 10); // DimplerL
        map_idx!(MouthDimpleRight, 11); // DimplerR

        map_idx!(MouthTightenerLeft, 48); // LipTightenerL
        map_idx!(MouthTightenerRight, 49); // LipTightenerR

        map_idx!(MouthPressLeft, 38); // LipPressorL
        map_idx!(MouthPressRight, 39); // LipPressorR

        map_idx!(MouthStretchLeft, 42); // LipStretcherL
        map_idx!(MouthStretchRight, 43); // LipStretcherR

        // Lip Expressions
        map_idx!(LipPuckerUpperRight, 41); // LipPuckerR
        map_idx!(LipPuckerLowerRight, 41); // LipPuckerR
        map_idx!(LipPuckerUpperLeft, 40); // LipPuckerL
        map_idx!(LipPuckerLowerLeft, 40); // LipPuckerL

        map_idx!(LipFunnelUpperLeft, 35); // LipFunnelerLt
        map_idx!(LipFunnelUpperRight, 37); // LipFunnelerRt
        map_idx!(LipFunnelLowerLeft, 34); // LipFunnelerLb
        map_idx!(LipFunnelLowerRight, 36); // LipFunnelerRb

        // Lip Suck
        map_val!(LipSuckUpperLeft, (1.0 - w[61].powf(1.0 / 6.0)).min(w[45]));
        map_val!(LipSuckUpperRight, (1.0 - w[62].powf(1.0 / 6.0)).min(w[47]));
        map_idx!(LipSuckLowerLeft, 44); // LipSuckLb
        map_idx!(LipSuckLowerRight, 46); // LipSuckRb

        // Cheek Expressions
        let mut puff_l = w[2];
        let mut puff_r = w[3];

        // Optional crosstalk reduction: suppress weaker side when asymmetric
        if ENABLED_CHEEK_CROSSTALK_REDUCTION {
            if puff_l > puff_r + 0.1 && puff_r < 0.4 {
                puff_r = 0.0;
            } else if puff_r > puff_l + 0.1 && puff_l < 0.4 {
                puff_l = 0.0;
            }
        }

        s[UnifiedExpressions::CheekPuffLeft as usize].weight = puff_l;
        s[UnifiedExpressions::CheekPuffRight as usize].weight = puff_r;

        map_idx!(CheekSuckLeft, 6); // CheekSuckL
        map_idx!(CheekSuckRight, 7); // CheekSuckR
        map_idx!(CheekSquintLeft, 4); // CheekRaiserL
        map_idx!(CheekSquintRight, 5); // CheekRaiserR

        // Nose Expressions
        map_idx!(NoseSneerLeft, 55); // NoseWrinklerL
        map_idx!(NoseSneerRight, 56); // NoseWrinklerR

        // Tongue Expressions
        map_idx!(TongueOut, 68); // TongueOut
        map_idx!(TongueCurlUp, 64); // TongueTipAlveolar
    }
}

impl Default for VirtualDesktopModule {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackingModule for VirtualDesktopModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing Virtual Desktop Module (Background Mode)");
        self.logger = Some(logger);
        // We don't block here anymore. Connection is handled in update().
        Ok(())
    }

    fn update(&mut self, data: &mut UnifiedTrackingData) -> Result<()> {
        if !self.is_connected() && self.connect().is_err() {
            // Sleep to avoid busy loop when not connected
            thread::sleep(Duration::from_secs(1));
            return Err(anyhow::anyhow!("Not connected to Virtual Desktop"));
        }

        unsafe {
            // Wait for event
            let result = WaitForSingleObject(self.event_handle, 50); // 50ms timeout
            if result.0 == 0 {
                // WAIT_OBJECT_0
                let face_state = &*self.face_state_ptr;

                let is_valid = face_state.face_is_valid != 0
                    || face_state.left_eye_is_valid != 0
                    || face_state.right_eye_is_valid != 0
                    || face_state.is_eye_following_blendshapes_valid != 0;

                if is_valid {
                    self.last_valid_frame_time = std::time::Instant::now();
                    self.update_eye_data(data, face_state);

                    if face_state.is_eye_following_blendshapes_valid != 0 {
                        self.update_eye_expressions(data, face_state);
                    }

                    if face_state.face_is_valid != 0 {
                        self.update_mouth_expressions(data, face_state);
                    }

                    // Heartbeat Logging (Throttled)
                    // let now = std::time::Instant::now();
                    // if self.last_log_time.map_or(true, |t| now.duration_since(t).as_secs() >= 1) {
                    //     info!("Virtual Desktop: Receiving valid face data...");
                    //     let w = &face_state.expression_weights;
                    //     info!("Eye Openness: L={:.3} R={:.3}", data.eye.left.openness, data.eye.right.openness);
                    //     info!("Cheek Puff: L={:.3} R={:.3}", w[2], w[3]);
                    //     info!("Raw Left: Closed={:.3} Cheek={:.3} Tight={:.3}", w[12], w[4], w[28]);
                    //     info!("Raw Right: Closed={:.3} Cheek={:.3} Tight={:.3}", w[13], w[5], w[29]);
                    //     self.last_log_time = Some(now);
                    // }

                    return Ok(());
                }
            }
        }

        // Check for timeout
        if self.last_valid_frame_time.elapsed() > Duration::from_secs(10) {
            if let Some(logger) = &self.logger {
                logger.warn("Connection timeout. No valid data for 10s. Reconnecting...");
            }
            self.disconnect();
            return Err(anyhow::anyhow!("Connection timeout"));
        }

        // Timeout or invalid face
        Err(anyhow::anyhow!("No new frame"))
    }

    fn unload(&mut self) {
        self.disconnect();
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(VirtualDesktopModule::new())
}
