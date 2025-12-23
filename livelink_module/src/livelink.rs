use anyhow::Result;
use api::{ModuleLogger, TrackingModule, UnifiedExpressions, UnifiedTrackingData};

use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

const PORT: u16 = 11111;
const MAX_FPS: u32 = 200;
const MIN_FRAME_DURATION: Duration = Duration::from_micros(1_000_000 / MAX_FPS as u64);

// The names of each ARKit blendshape
const LIVELINK_NAMES: &[&str] = &[
    "EyeBlinkLeft",
    "EyeLookDownLeft",
    "EyeLookInLeft",
    "EyeLookOutLeft",
    "EyeLookUpLeft",
    "EyeSquintLeft",
    "EyeWideLeft",
    "EyeBlinkRight",
    "EyeLookDownRight",
    "EyeLookInRight",
    "EyeLookOutRight",
    "EyeLookUpRight",
    "EyeSquintRight",
    "EyeWideRight",
    "JawForward",
    "JawLeft",
    "JawRight",
    "JawOpen",
    "MouthClose",
    "MouthFunnel",
    "MouthPucker",
    "MouthLeft",
    "MouthRight",
    "MouthSmileLeft",
    "MouthSmileRight",
    "MouthFrownLeft",
    "MouthFrownRight",
    "MouthDimpleLeft",
    "MouthDimpleRight",
    "MouthStretchLeft",
    "MouthStretchRight",
    "MouthRollLower",
    "MouthRollUpper",
    "MouthShrugLower",
    "MouthShrugUpper",
    "MouthPressLeft",
    "MouthPressRight",
    "MouthLowerDownLeft",
    "MouthLowerDownRight",
    "MouthUpperUpLeft",
    "MouthUpperUpRight",
    "BrowDownLeft",
    "BrowDownRight",
    "BrowInnerUp",
    "BrowOuterUpLeft",
    "BrowOuterUpRight",
    "CheekPuff",
    "CheekSquintLeft",
    "CheekSquintRight",
    "NoseSneerLeft",
    "NoseSneerRight",
    "TongueOut",
    "HeadYaw",
    "HeadPitch",
    "HeadRoll",
    "EyeYawLeft",    // LeftEyeYaw
    "EyePitchLeft",  // LeftEyePitch
    "EyeRollLeft",   // LeftEyeRoll
    "EyeYawRight",   // RightEyeYaw
    "EyePitchRight", // RightEyePitch
    "EyeRollRight",  // RightEyeRoll
];

#[derive(Default)]
struct LiveLinkTrackingDataEye {
    eye_blink: f32,
    eye_look_down: f32,
    eye_look_in: f32,
    eye_look_out: f32,
    eye_look_up: f32,
    eye_squint: f32,
    eye_wide: f32,
    eye_pitch: f32,
    eye_yaw: f32,
    eye_roll: f32,
}

#[derive(Default)]
struct LiveLinkTrackingDataLips {
    jaw_forward: f32,
    jaw_left: f32,
    jaw_right: f32,
    jaw_open: f32,
    mouth_close: f32,
    mouth_funnel: f32,
    mouth_pucker: f32,
    mouth_left: f32,
    mouth_right: f32,
    mouth_smile_left: f32,
    mouth_smile_right: f32,
    mouth_frown_left: f32,
    mouth_frown_right: f32,
    mouth_dimple_left: f32,
    mouth_dimple_right: f32,
    mouth_stretch_left: f32,
    mouth_stretch_right: f32,
    mouth_roll_lower: f32,
    mouth_roll_upper: f32,
    mouth_shrug_lower: f32,
    mouth_shrug_upper: f32,
    mouth_press_left: f32,
    mouth_press_right: f32,
    mouth_lower_down_left: f32,
    mouth_lower_down_right: f32,
    mouth_upper_up_left: f32,
    mouth_upper_up_right: f32,
    cheek_puff: f32,
    cheek_squint_left: f32,
    cheek_squint_right: f32,
    nose_sneer_left: f32,
    nose_sneer_right: f32,
    tongue_out: f32,
}

#[derive(Default)]
struct LiveLinkTrackingDataBrow {
    brow_down_left: f32,
    brow_down_right: f32,
    brow_inner_up: f32,
    brow_outer_up_left: f32,
    brow_outer_up_right: f32,
}

#[derive(Default)]
struct LiveLinkTrackingDataStruct {
    left_eye: LiveLinkTrackingDataEye,
    right_eye: LiveLinkTrackingDataEye,
    lips: LiveLinkTrackingDataLips,
    brow: LiveLinkTrackingDataBrow,
}

impl LiveLinkTrackingDataStruct {
    fn process_data(&mut self, values: &HashMap<&str, f32>) {
        // Helper macro to get value or default to 0.0
        macro_rules! get {
            ($key:expr) => {
                *values.get($key).unwrap_or(&0.0)
            };
        }

        // Left Eye
        self.left_eye.eye_blink = get!("EyeBlinkLeft");
        self.left_eye.eye_look_down = get!("EyeLookDownLeft");
        self.left_eye.eye_look_in = get!("EyeLookInLeft");
        self.left_eye.eye_look_out = get!("EyeLookOutLeft");
        self.left_eye.eye_look_up = get!("EyeLookUpLeft");
        self.left_eye.eye_squint = get!("EyeSquintLeft");
        self.left_eye.eye_wide = get!("EyeWideLeft");
        self.left_eye.eye_pitch = get!("EyePitchLeft");
        self.left_eye.eye_yaw = get!("EyeYawLeft");
        self.left_eye.eye_roll = get!("EyeRollLeft");

        // Right Eye
        self.right_eye.eye_blink = get!("EyeBlinkRight");
        self.right_eye.eye_look_down = get!("EyeLookDownRight");
        self.right_eye.eye_look_in = get!("EyeLookInRight");
        self.right_eye.eye_look_out = get!("EyeLookOutRight");
        self.right_eye.eye_look_up = get!("EyeLookUpRight");
        self.right_eye.eye_squint = get!("EyeSquintRight");
        self.right_eye.eye_wide = get!("EyeWideRight");
        self.right_eye.eye_pitch = get!("EyePitchRight");
        self.right_eye.eye_yaw = get!("EyeYawRight");
        self.right_eye.eye_roll = get!("EyeRollRight");

        // Lips
        self.lips.jaw_forward = get!("JawForward");
        self.lips.jaw_left = get!("JawLeft");
        self.lips.jaw_right = get!("JawRight");
        self.lips.jaw_open = get!("JawOpen");
        self.lips.mouth_close = get!("MouthClose");
        self.lips.mouth_funnel = get!("MouthFunnel");
        self.lips.mouth_pucker = get!("MouthPucker");
        self.lips.mouth_left = get!("MouthLeft");
        self.lips.mouth_right = get!("MouthRight");
        self.lips.mouth_smile_left = get!("MouthSmileLeft");
        self.lips.mouth_smile_right = get!("MouthSmileRight");
        self.lips.mouth_frown_left = get!("MouthFrownLeft");
        self.lips.mouth_frown_right = get!("MouthFrownRight");
        self.lips.mouth_dimple_left = get!("MouthDimpleLeft");
        self.lips.mouth_dimple_right = get!("MouthDimpleRight");
        self.lips.mouth_stretch_left = get!("MouthStretchLeft");
        self.lips.mouth_stretch_right = get!("MouthStretchRight");
        self.lips.mouth_roll_lower = get!("MouthRollLower");
        self.lips.mouth_roll_upper = get!("MouthRollUpper");
        self.lips.mouth_shrug_lower = get!("MouthShrugLower");
        self.lips.mouth_shrug_upper = get!("MouthShrugUpper");
        self.lips.mouth_press_left = get!("MouthPressLeft");
        self.lips.mouth_press_right = get!("MouthPressRight");
        self.lips.mouth_lower_down_left = get!("MouthLowerDownLeft");
        self.lips.mouth_lower_down_right = get!("MouthLowerDownRight");
        self.lips.mouth_upper_up_left = get!("MouthUpperUpLeft");
        self.lips.mouth_upper_up_right = get!("MouthUpperUpRight");
        self.lips.cheek_puff = get!("CheekPuff");
        self.lips.cheek_squint_left = get!("CheekSquintLeft");
        self.lips.cheek_squint_right = get!("CheekSquintRight");
        self.lips.nose_sneer_left = get!("NoseSneerLeft");
        self.lips.nose_sneer_right = get!("NoseSneerRight");
        self.lips.tongue_out = get!("TongueOut");

        // Brow
        self.brow.brow_down_left = get!("BrowDownLeft");
        self.brow.brow_down_right = get!("BrowDownRight");
        self.brow.brow_inner_up = get!("BrowInnerUp");
        self.brow.brow_outer_up_left = get!("BrowOuterUpLeft");
        self.brow.brow_outer_up_right = get!("BrowOuterUpRight");
    }
}

pub struct LiveLinkModule {
    socket: Option<UdpSocket>,
    latest_data: LiveLinkTrackingDataStruct,
    last_update_time: Option<Instant>,
    logger: Option<ModuleLogger>,
}

impl LiveLinkModule {
    pub fn new() -> Self {
        Self {
            socket: None,
            latest_data: LiveLinkTrackingDataStruct::default(),
            last_update_time: None,
            logger: None,
        }
    }

    fn eye_calc(eye_blink: f32, eye_squint: f32) -> f32 {
        (0.05 + eye_blink).powf(6.0) + eye_squint
    }

    #[allow(dead_code)]
    fn ape_calc(jaw_open: f32, mouth_close: f32) -> f32 {
        (0.05 + jaw_open) * (0.05 + mouth_close).powf(2.0)
    }

    fn update_eye(&self, data: &mut UnifiedTrackingData) {
        // Convert Yaw/Pitch to Vector3
        // x = sin(yaw) * cos(pitch)
        // y = sin(pitch)
        // z = cos(yaw) * cos(pitch)

        // Left Eye
        let yaw = self.latest_data.left_eye.eye_yaw.to_radians();
        let pitch = -self.latest_data.left_eye.eye_pitch.to_radians();
        let x = yaw.sin() * pitch.cos();
        let y = pitch.sin();
        let z = yaw.cos() * pitch.cos();
        data.eye.left.gaze = glam::Vec3::new(x, y, z);
        data.eye.left.openness = 1.0
            - Self::eye_calc(
                self.latest_data.left_eye.eye_blink,
                self.latest_data.left_eye.eye_squint,
            );
        data.shapes[UnifiedExpressions::EyeWideLeft as usize].weight =
            self.latest_data.left_eye.eye_wide;

        // Right Eye
        let yaw = self.latest_data.right_eye.eye_yaw.to_radians();
        let pitch = -self.latest_data.right_eye.eye_pitch.to_radians();
        let x = yaw.sin() * pitch.cos();
        let y = pitch.sin();
        let z = yaw.cos() * pitch.cos();
        data.eye.right.gaze = glam::Vec3::new(x, y, z);
        data.eye.right.openness = 1.0
            - Self::eye_calc(
                self.latest_data.right_eye.eye_blink,
                self.latest_data.right_eye.eye_squint,
            );
        data.shapes[UnifiedExpressions::EyeWideRight as usize].weight =
            self.latest_data.right_eye.eye_wide;
    }

    fn update_lips(&self, data: &mut UnifiedTrackingData) {
        let lips = &self.latest_data.lips;
        let s = &mut data.shapes;

        macro_rules! map {
            ($unified:ident, $val:expr) => {
                s[UnifiedExpressions::$unified as usize].weight = $val;
            };
        }

        // map!(MouthUpperOverturn, lips.mouth_shrug_upper);
        // map!(MouthLowerOverturn, lips.mouth_shrug_lower);
        // MouthPout not supported directly
        // map!(MouthPout, (lips.mouth_funnel + lips.mouth_pucker) / 2.0);
        map!(MouthCornerPullRight, lips.mouth_smile_right);
        map!(MouthCornerPullLeft, lips.mouth_smile_left);
        map!(MouthFrownRight, lips.mouth_frown_right);
        map!(MouthFrownLeft, lips.mouth_frown_left);
        map!(CheekPuffRight, lips.cheek_puff);
        map!(CheekPuffLeft, lips.cheek_puff);
        map!(CheekSuckRight, lips.cheek_puff); // Mapping puff to suck? C# mapped CheekPuff to CheekPuff. Wait, C# mapped CheekPuff to CheekPuffRight/Left.
        map!(CheekSuckLeft, lips.cheek_puff); // Wait, C# mapped CheekPuff to CheekPuffRight/Left.
                                              // C# had: { LipShape_v2.CheekSuck, 0 },
                                              // So CheekSuck was 0.
        map!(CheekSuckRight, 0.0);
        map!(CheekSuckLeft, 0.0);
        map!(MouthUpperUpRight, lips.mouth_upper_up_right);
        map!(MouthUpperUpLeft, lips.mouth_upper_up_left);
        map!(MouthLowerDownRight, lips.mouth_lower_down_right);
        map!(MouthLowerDownLeft, lips.mouth_lower_down_left);
        map!(LipSuckUpperRight, lips.mouth_roll_upper);
        map!(LipSuckUpperLeft, lips.mouth_roll_upper);
        map!(LipSuckLowerRight, lips.mouth_roll_lower);
        map!(LipSuckLowerLeft, lips.mouth_roll_lower);
        // MouthLowerOverlay not supported
        // map!(MouthLowerOverlay, 0.0);
        // TongueLongStep1/2 not supported
        // map!(TongueLongStep1, lips.tongue_out);
        map!(TongueLeft, 0.0);
        map!(TongueRight, 0.0);
        map!(TongueUp, 0.0);
        map!(TongueDown, 0.0);
        map!(TongueRoll, 0.0);
        // map!(TongueLongStep2, lips.tongue_out);
        // Tongue morphs not supported
        // map!(TongueUpRightMorph, 0.0);
        // map!(TongueUpLeftMorph, 0.0);
        // map!(TongueDownRightMorph, 0.0);
        // map!(TongueDownLeftMorph, 0.0);
    }

    fn parse_livelink_packet_v1(
        buf: &[u8],
        logger: &Option<ModuleLogger>,
    ) -> Option<HashMap<&'static str, f32>> {
        let mut offset = 1; // Skip Version (already checked)

        // Device ID Length (u16 BE) + 1 byte padding?
        // Based on dump: 0, 36, 0 -> 0x00, 0x24, 0x00.
        // So u16 BE length, then 1 byte padding.
        if offset + 3 > buf.len() {
            return None;
        }
        let did_len = u16::from_be_bytes(buf[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        offset += 1; // Skip padding

        // 3. Device ID
        if offset + did_len > buf.len() {
            return None;
        }
        // We don't use the ID, just skip it
        offset += did_len;

        // Subject Name / Metadata
        // Based on dump, there are 5 bytes of "junk" before floats start.
        // "36, 115, 43, 0, 0" -> "$s+\0\0"
        // We'll skip 5 bytes for now.
        offset += 5;

        // Blend Shapes (61 * float Little Endian)
        // Check if we have enough bytes left
        if offset + (61 * 4) > buf.len() {
            if let Some(logger) = logger {
                logger.warn(&format!(
                    "Packet truncated. Expected {} bytes for blend shapes, got {}",
                    61 * 4,
                    buf.len() - offset
                ));
            }
            return None;
        }

        let mut values = HashMap::new();

        // Process 61 floats (4 bytes each, Little Endian)
        for i in 0..61 {
            let float_bytes = &buf[offset + i * 4..offset + (i + 1) * 4];
            let val = f32::from_le_bytes(float_bytes.try_into().unwrap());

            if i < LIVELINK_NAMES.len() {
                values.insert(LIVELINK_NAMES[i], val);
            }
        }

        Some(values)
    }

    fn parse_livelink_packet(
        buf: &[u8],
        logger: &Option<ModuleLogger>,
    ) -> Option<HashMap<&'static str, f32>> {
        // Minimal size check
        if buf.len() < 1 {
            return None;
        }

        let version = buf[0];
        if version == 1 {
            return Self::parse_livelink_packet_v1(buf, logger);
        }

        // Minimal size check for V6: Version(1) + DeviceIdLen(4) + SubjectNameLen(4) + FrameTime(16) + BSCount(1) = 26 bytes minimum (empty strings)
        if buf.len() < 26 {
            if let Some(logger) = logger {
                logger.warn(&format!("Packet too small: {} bytes", buf.len()));
            }
            return None;
        }

        let mut offset = 0;

        // Version (uint8)
        offset += 1;
        if version != 6 {
            if let Some(logger) = logger {
                logger.warn(&format!(
                    "Unsupported LiveLink version: {}. Expected 6.",
                    version
                ));
            }
            return None;
        }

        // Helper to read u32 BE
        let read_u32 = |buf: &[u8], offset: &mut usize| -> Option<u32> {
            if *offset + 4 > buf.len() {
                return None;
            }
            let val = u32::from_be_bytes(buf[*offset..*offset + 4].try_into().unwrap());
            *offset += 4;
            Some(val)
        };

        // 2. Device ID
        if let Some(len) = read_u32(buf, &mut offset) {
            offset += len as usize; // Skip Device ID string
        } else {
            return None;
        }

        // 3. Subject Name
        if let Some(len) = read_u32(buf, &mut offset) {
            offset += len as usize; // Skip Subject Name string
        } else {
            return None;
        }

        // 4. Frame Time (4 * u32)
        offset += 16; // Skip FrameNumber, SubFrame, FPS, Denominator

        // Bounds check before reading BlendShape count
        if offset + 1 > buf.len() {
            return None;
        }

        // 5. Blend Shape Count (uint8)
        let bs_count = buf[offset];
        offset += 1;

        if bs_count != 61 {
            if let Some(logger) = logger {
                logger.warn(&format!(
                    "Unexpected blend shape count: {}. Expected 61.",
                    bs_count
                ));
            }
            return None;
        }

        // 6. Blend Shapes (61 * float)
        // Check if we have enough bytes left
        if offset + (61 * 4) > buf.len() {
            if let Some(logger) = logger {
                logger.warn(&format!(
                    "Packet truncated. Expected {} bytes for blend shapes, got {}",
                    61 * 4,
                    buf.len() - offset
                ));
            }
            return None;
        }

        let mut values = HashMap::new();

        // Process 61 floats (4 bytes each)
        for i in 0..61 {
            let float_bytes = &buf[offset + i * 4..offset + (i + 1) * 4];
            let val = f32::from_be_bytes(float_bytes.try_into().unwrap());

            if i < LIVELINK_NAMES.len() {
                values.insert(LIVELINK_NAMES[i], val);
            }
        }

        Some(values)
    }
}

impl TrackingModule for LiveLinkModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing LiveLink Module");
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", PORT))?;
        socket.set_nonblocking(true)?;
        self.socket = Some(socket);
        logger.info(&format!("Ready and listening on UDP port {}", PORT));
        self.logger = Some(logger);
        Ok(())
    }

    fn update(&mut self, data: &mut UnifiedTrackingData) -> Result<()> {
        // Check if enough time has elapsed since last update (FPS limiting)
        let now = Instant::now();
        if let Some(last_time) = self.last_update_time {
            let elapsed = now.duration_since(last_time);
            if elapsed < MIN_FRAME_DURATION {
                // Not enough time has passed, skip this update
                return Ok(());
            }
        }

        if let Some(socket) = &self.socket {
            let mut buf = [0u8; 1024]; // Buffer larger than expected packet
            let mut received_data = false;

            loop {
                match socket.recv_from(&mut buf) {
                    Ok((amt, _src)) => {
                        if let Some(values) = Self::parse_livelink_packet(&buf[..amt], &self.logger)
                        {
                            self.latest_data.process_data(&values);
                            received_data = true;
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(e) => {
                        if let Some(logger) = &self.logger {
                            logger.warn(&format!("UDP receive error: {}", e));
                        }
                        break;
                    }
                }
            }

            // Only update tracking data and timestamp if we received new data
            if received_data {
                self.update_eye(data);
                self.update_lips(data);
                self.last_update_time = Some(now);
            }

            Ok(())
        } else {
            Err(anyhow::anyhow!("LiveLink socket not initialized"))
        }
    }

    fn unload(&mut self) {
        if let Some(logger) = &self.logger {
            logger.info("LiveLink shutting down");
        }
        self.socket = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eye_calc() {
        let val = LiveLinkModule::eye_calc(0.5, 0.2);
        // (0.05 + 0.5)^6 + 0.2
        // 0.55^6 + 0.2
        // 0.02768 + 0.2 = 0.22768
        assert!((val - 0.22768).abs() < 0.0001);
    }

    #[test]
    fn test_ape_calc() {
        let val = LiveLinkModule::ape_calc(0.5, 0.2);
        // (0.05 + 0.5) * (0.05 + 0.2)^2
        // 0.55 * 0.25^2
        // 0.55 * 0.0625 = 0.034375
        assert!((val - 0.034375).abs() < 0.0001);
    }

    #[test]
    fn test_process_data() {
        let mut data = LiveLinkTrackingDataStruct::default();
        let mut values = HashMap::new();
        values.insert("EyeBlinkLeft", 0.8);
        values.insert("JawOpen", 0.5);

        data.process_data(&values);

        assert_eq!(data.left_eye.eye_blink, 0.8);
        assert_eq!(data.lips.jaw_open, 0.5);
        assert_eq!(data.right_eye.eye_blink, 0.0); // Default
    }

    #[test]
    fn test_eye_gaze_calculation() {
        let mut module = LiveLinkModule::new();
        let mut data = UnifiedTrackingData::default();

        // Simulate receiving 30 degrees Yaw, 0 degrees Pitch
        // sin(30 deg) = 0.5
        // cos(30 deg) = 0.866
        module.latest_data.left_eye.eye_yaw = 30.0;
        module.latest_data.left_eye.eye_pitch = 0.0;

        module.update_eye(&mut data);

        let gaze = data.eye.left.gaze;

        // Expected: x = sin(30) * cos(0) = 0.5 * 1 = 0.5
        //           y = sin(0) = 0
        //           z = cos(30) * cos(0) = 0.866 * 1 = 0.866

        assert!(
            (gaze.x - 0.5).abs() < 0.001,
            "X component mismatch: expected 0.5, got {}",
            gaze.x
        );
        assert!(
            (gaze.y - 0.0).abs() < 0.001,
            "Y component mismatch: expected 0.0, got {}",
            gaze.y
        );
        assert!(
            (gaze.z - 0.866).abs() < 0.001,
            "Z component mismatch: expected 0.866, got {}",
            gaze.z
        );
    }

    #[test]
    fn test_parse_livelink_packet_v1() {
        // Data from user dump
        let mut buf = vec![
            1, 0, 36, 0, 53, 57, 55, 70, 67, 68, 67, 69, 45, 53, 49, 53, 52, 45, 52, 52, 70, 50,
            45, 66, 51, 55, 56, 45, 66, 67, 67, 68, 70, 51, 52, 68, 53, 70, 68, 67, 36, 115, 43, 0,
            0,
        ];

        // Add 61 floats (Little Endian)
        // First float: 0.0116 -> 178, 233, 61, 60
        buf.extend_from_slice(&[178, 233, 61, 60]);

        // Add 60 more dummy floats
        for _ in 0..60 {
            buf.extend_from_slice(&0.0f32.to_le_bytes());
        }

        let result = LiveLinkModule::parse_livelink_packet(&buf, &None);
        assert!(result.is_some());

        let values = result.unwrap();
        assert_eq!(values.len(), 61);
        // Verify first value is approx 0.0116
        let val = *values.get("EyeBlinkLeft").unwrap();
        assert!((val - 0.0116).abs() < 0.0001);
    }

    #[test]
    fn test_parse_livelink_packet() {
        let mut buf = Vec::new();

        // 1. Version (6)
        buf.push(6);

        // 2. Device ID Length (0)
        buf.extend_from_slice(&0u32.to_be_bytes());
        // Device ID (empty)

        // 3. Subject Name Length (0)
        buf.extend_from_slice(&0u32.to_be_bytes());
        // Subject Name (empty)

        // 4. Frame Time (4 * u32)
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&60u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());

        // 5. Blend Shape Count (61)
        buf.push(61);

        // 6. Blend Shapes (61 * float)
        for i in 0..61 {
            let val = if i == 0 { 0.5f32 } else { 0.0f32 }; // Set first BS (EyeBlinkLeft) to 0.5
            buf.extend_from_slice(&val.to_be_bytes());
        }

        let result = LiveLinkModule::parse_livelink_packet(&buf, &None);
        assert!(result.is_some());

        let values = result.unwrap();
        assert_eq!(values.len(), 61);
        assert_eq!(*values.get("EyeBlinkLeft").unwrap(), 0.5);
    }
}
