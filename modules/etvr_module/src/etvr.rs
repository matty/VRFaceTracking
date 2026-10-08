use anyhow::Result;
use std::net::UdpSocket;
use vrft_api::{ModuleLogger, TrackingModule, UnifiedExpressions, UnifiedTrackingData};

const DEFAULT_PORT: u16 = 8889;

pub struct EtvrModule {
    socket: Option<UdpSocket>,
    logger: Option<ModuleLogger>,
    buf: Vec<u8>,
    is_v2: bool,
    is_dual_eye: bool,
}

impl EtvrModule {
    pub fn new() -> Self {
        Self {
            socket: None,
            logger: None,
            buf: vec![0u8; 4096],
            is_v2: false,
            is_dual_eye: false,
        }
    }
}

fn apply_message(
    msg: rosc::OscMessage,
    is_v2: &mut bool,
    is_dual_eye: &mut bool,
    data: &mut UnifiedTrackingData,
) {
    let value = match msg.args.first() {
        Some(rosc::OscType::Float(v)) => *v,
        _ => return,
    };

    let addr = msg.addr.as_str();

    if addr.contains("/v2/") {
        *is_v2 = true;
    }

    let param = addr.rsplit('/').next().unwrap_or("");

    if *is_v2 {
        apply_v2(param, value, is_dual_eye, data);
    } else {
        apply_v1(param, value, data);
    }
}

fn apply_v1(param: &str, value: f32, data: &mut UnifiedTrackingData) {
    match param {
        "LeftEyeX" => data.eye.left.gaze.x = value,
        "RightEyeX" => data.eye.right.gaze.x = value,
        "EyesY" => {
            data.eye.left.gaze.y = value;
            data.eye.right.gaze.y = value;
        }
        "LeftEyeLidExpandedSqueeze" | "RightEyeLidExpandedSqueeze" => {
            let openness = value.clamp(0.0, 1.0);
            if param.starts_with("Left") {
                data.eye.left.openness = openness;
            } else {
                data.eye.right.openness = openness;
            }
        }
        "EyesDilation" => {
            data.eye.left.pupil_diameter_mm = value;
            data.eye.right.pupil_diameter_mm = value;
        }
        _ => {}
    }
}

fn apply_v2(param: &str, value: f32, is_dual_eye: &mut bool, data: &mut UnifiedTrackingData) {
    use UnifiedExpressions::*;

    match param {
        "EyeX" => {
            *is_dual_eye = false;
            data.eye.left.gaze.x = value;
            data.eye.right.gaze.x = value;
        }
        "EyeY" => {
            *is_dual_eye = false;
            data.eye.left.gaze.y = value;
            data.eye.right.gaze.y = value;
        }
        "EyeLeftX" => {
            *is_dual_eye = true;
            data.eye.left.gaze.x = value;
        }
        "EyeLeftY" => data.eye.left.gaze.y = value,
        "EyeRightX" => {
            *is_dual_eye = true;
            data.eye.right.gaze.x = value;
        }
        "EyeRightY" => data.eye.right.gaze.y = value,
        "EyeLid" => {
            let o = value.clamp(0.0, 1.0);
            data.eye.left.openness = o;
            data.eye.right.openness = o;
        }
        "EyeLidLeft" => data.eye.left.openness = value.clamp(0.0, 1.0),
        "EyeLidRight" => data.eye.right.openness = value.clamp(0.0, 1.0),
        "PupilDilation" => {
            data.eye.left.pupil_diameter_mm = value;
            data.eye.right.pupil_diameter_mm = value;
        }
        "EyeSquint" => {
            data.shapes[EyeSquintLeft as usize].weight = value;
            data.shapes[EyeSquintRight as usize].weight = value;
        }
        "EyeSquintLeft" => data.shapes[EyeSquintLeft as usize].weight = value,
        "EyeSquintRight" => data.shapes[EyeSquintRight as usize].weight = value,
        "CheekSquintLeft" => data.shapes[CheekSquintLeft as usize].weight = value,
        "CheekSquintRight" => data.shapes[CheekSquintRight as usize].weight = value,
        "BrowExpression" => apply_brow(value, data, true, true),
        "BrowExpressionLeft" => apply_brow(value, data, true, false),
        "BrowExpressionRight" => apply_brow(value, data, false, true),
        _ => {}
    }
}

fn apply_brow(value: f32, data: &mut UnifiedTrackingData, left: bool, right: bool) {
    use UnifiedExpressions::*;

    if value < 0.5 {
        let weight = (0.5 - value) * 2.0;
        if left {
            data.shapes[BrowLowererLeft as usize].weight = weight;
            data.shapes[BrowPinchLeft as usize].weight = weight;
            data.shapes[BrowOuterUpLeft as usize].weight = 0.0;
            data.shapes[BrowInnerUpLeft as usize].weight = 0.0;
        }
        if right {
            data.shapes[BrowLowererRight as usize].weight = weight;
            data.shapes[BrowPinchRight as usize].weight = weight;
            data.shapes[BrowOuterUpRight as usize].weight = 0.0;
            data.shapes[BrowInnerUpRight as usize].weight = 0.0;
        }
    } else {
        let weight = (value - 0.5) * 2.0;
        if left {
            data.shapes[BrowOuterUpLeft as usize].weight = weight;
            data.shapes[BrowInnerUpLeft as usize].weight = weight;
            data.shapes[BrowLowererLeft as usize].weight = 0.0;
            data.shapes[BrowPinchLeft as usize].weight = 0.0;
        }
        if right {
            data.shapes[BrowOuterUpRight as usize].weight = weight;
            data.shapes[BrowInnerUpRight as usize].weight = weight;
            data.shapes[BrowLowererRight as usize].weight = 0.0;
            data.shapes[BrowPinchRight as usize].weight = 0.0;
        }
    }
}

impl TrackingModule for EtvrModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing EyeTrackVR Module");
        let socket = UdpSocket::bind(format!("0.0.0.0:{DEFAULT_PORT}"))?;
        socket.set_nonblocking(true)?;
        self.socket = Some(socket);
        logger.info(&format!("Listening for ETVR OSC on port {DEFAULT_PORT}"));
        self.logger = Some(logger);
        Ok(())
    }

    fn update(&mut self, data: &mut UnifiedTrackingData) -> Result<()> {
        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not initialized"))?;

        let mut received = false;
        loop {
            match socket.recv(&mut self.buf) {
                Ok(n) => {
                    if let Ok((_, msg)) = rosc::decoder::decode_udp(&self.buf[..n]) {
                        match msg {
                            rosc::OscPacket::Message(msg) => {
                                apply_message(msg, &mut self.is_v2, &mut self.is_dual_eye, data);
                            }
                            rosc::OscPacket::Bundle(bundle) => {
                                for content in bundle.content {
                                    if let rosc::OscPacket::Message(msg) = content {
                                        apply_message(
                                            msg,
                                            &mut self.is_v2,
                                            &mut self.is_dual_eye,
                                            data,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    received = true;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }

        if !received {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        Ok(())
    }

    fn unload(&mut self) {
        self.socket.take();
        if let Some(logger) = &self.logger {
            logger.info("EyeTrackVR Module unloaded");
        }
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(EtvrModule::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{OscMessage, OscType};
    use vrft_api::UnifiedExpressions::*;

    fn msg(addr: &str, value: f32) -> OscMessage {
        OscMessage {
            addr: addr.to_string(),
            args: vec![OscType::Float(value)],
        }
    }

    fn apply(addr: &str, value: f32) -> (UnifiedTrackingData, bool, bool) {
        let mut data = UnifiedTrackingData::default();
        let mut is_v2 = false;
        let mut is_dual_eye = false;
        apply_message(msg(addr, value), &mut is_v2, &mut is_dual_eye, &mut data);
        (data, is_v2, is_dual_eye)
    }

    fn apply_v2_msg(addr: &str, value: f32) -> (UnifiedTrackingData, bool) {
        let mut data = UnifiedTrackingData::default();
        let mut is_v2 = true;
        let mut is_dual_eye = false;
        apply_message(msg(addr, value), &mut is_v2, &mut is_dual_eye, &mut data);
        (data, is_dual_eye)
    }

    // V1 eye gaze
    #[test]
    fn v1_left_eye_x() {
        let (data, ..) = apply("/etvr/LeftEyeX", 0.7);
        assert_eq!(data.eye.left.gaze.x, 0.7);
    }

    #[test]
    fn v1_right_eye_x() {
        let (data, ..) = apply("/etvr/RightEyeX", -0.3);
        assert_eq!(data.eye.right.gaze.x, -0.3);
    }

    #[test]
    fn v1_eyes_y_sets_both() {
        let (data, ..) = apply("/etvr/EyesY", 0.5);
        assert_eq!(data.eye.left.gaze.y, 0.5);
        assert_eq!(data.eye.right.gaze.y, 0.5);
    }

    // V1 lid openness with clamp
    #[test]
    fn v1_lid_clamps_above_one() {
        let (data, ..) = apply("/etvr/LeftEyeLidExpandedSqueeze", 1.5);
        assert_eq!(data.eye.left.openness, 1.0);
    }

    #[test]
    fn v1_lid_clamps_below_zero() {
        let (data, ..) = apply("/etvr/RightEyeLidExpandedSqueeze", -0.5);
        assert_eq!(data.eye.right.openness, 0.0);
    }

    #[test]
    fn v1_lid_normal_value() {
        let (data, ..) = apply("/etvr/LeftEyeLidExpandedSqueeze", 0.6);
        assert!((data.eye.left.openness - 0.6).abs() < 1e-6);
    }

    // V2 auto-detection
    #[test]
    fn v2_autodetect_sets_is_v2() {
        let (_, is_v2, _) = apply("/etvr/v2/EyeX", 0.1);
        assert!(is_v2);
    }

    #[test]
    fn v1_addr_does_not_set_v2() {
        let (_, is_v2, _) = apply("/etvr/LeftEyeX", 0.1);
        assert!(!is_v2);
    }

    // V2 single-eye mode
    #[test]
    fn v2_eye_x_sets_both_eyes() {
        let (data, is_dual_eye) = apply_v2_msg("/etvr/v2/EyeX", 0.4);
        assert_eq!(data.eye.left.gaze.x, 0.4);
        assert_eq!(data.eye.right.gaze.x, 0.4);
        assert!(!is_dual_eye);
    }

    #[test]
    fn v2_eye_y_sets_both_eyes() {
        let (data, is_dual_eye) = apply_v2_msg("/etvr/v2/EyeY", -0.2);
        assert_eq!(data.eye.left.gaze.y, -0.2);
        assert_eq!(data.eye.right.gaze.y, -0.2);
        assert!(!is_dual_eye);
    }

    // V2 dual-eye mode
    #[test]
    fn v2_eye_left_x_sets_dual_eye() {
        let (data, is_dual_eye) = apply_v2_msg("/etvr/v2/EyeLeftX", 0.3);
        assert_eq!(data.eye.left.gaze.x, 0.3);
        assert!(is_dual_eye);
    }

    #[test]
    fn v2_eye_right_x_sets_dual_eye() {
        let (data, is_dual_eye) = apply_v2_msg("/etvr/v2/EyeRightX", -0.6);
        assert_eq!(data.eye.right.gaze.x, -0.6);
        assert!(is_dual_eye);
    }

    #[test]
    fn v2_eye_left_y() {
        let (data, _) = apply_v2_msg("/etvr/v2/EyeLeftY", 0.8);
        assert_eq!(data.eye.left.gaze.y, 0.8);
    }

    #[test]
    fn v2_eye_right_y() {
        let (data, _) = apply_v2_msg("/etvr/v2/EyeRightY", -0.1);
        assert_eq!(data.eye.right.gaze.y, -0.1);
    }

    // V2 EyeLid
    #[test]
    fn v2_eyelid_sets_both() {
        let (data, _) = apply_v2_msg("/etvr/v2/EyeLid", 0.7);
        assert!((data.eye.left.openness - 0.7).abs() < 1e-6);
        assert!((data.eye.right.openness - 0.7).abs() < 1e-6);
    }

    #[test]
    fn v2_eyelid_left_only() {
        let (data, _) = apply_v2_msg("/etvr/v2/EyeLidLeft", 0.3);
        assert!((data.eye.left.openness - 0.3).abs() < 1e-6);
        assert_eq!(data.eye.right.openness, 0.0);
    }

    #[test]
    fn v2_eyelid_right_only() {
        let (data, _) = apply_v2_msg("/etvr/v2/EyeLidRight", 0.9);
        assert!((data.eye.right.openness - 0.9).abs() < 1e-6);
        assert_eq!(data.eye.left.openness, 0.0);
    }

    #[test]
    fn v2_eyelid_clamps() {
        let (data, _) = apply_v2_msg("/etvr/v2/EyeLid", 2.0);
        assert_eq!(data.eye.left.openness, 1.0);
        assert_eq!(data.eye.right.openness, 1.0);
    }

    // BrowExpression
    #[test]
    fn brow_at_025_lowerer() {
        let (data, _) = apply_v2_msg("/etvr/v2/BrowExpression", 0.25);
        // lowerer = (0.5 - 0.25) * 2.0 = 0.5
        assert!((data.shapes[BrowLowererLeft as usize].weight - 0.5).abs() < 1e-6);
        assert!((data.shapes[BrowLowererRight as usize].weight - 0.5).abs() < 1e-6);
        assert!((data.shapes[BrowPinchLeft as usize].weight - 0.5).abs() < 1e-6);
        assert!((data.shapes[BrowPinchRight as usize].weight - 0.5).abs() < 1e-6);
        assert_eq!(data.shapes[BrowOuterUpLeft as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowInnerUpLeft as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowOuterUpRight as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowInnerUpRight as usize].weight, 0.0);
    }

    #[test]
    fn brow_at_075_raise() {
        let (data, _) = apply_v2_msg("/etvr/v2/BrowExpression", 0.75);
        // raise = (0.75 - 0.5) * 2.0 = 0.5
        assert!((data.shapes[BrowOuterUpLeft as usize].weight - 0.5).abs() < 1e-6);
        assert!((data.shapes[BrowInnerUpLeft as usize].weight - 0.5).abs() < 1e-6);
        assert!((data.shapes[BrowOuterUpRight as usize].weight - 0.5).abs() < 1e-6);
        assert!((data.shapes[BrowInnerUpRight as usize].weight - 0.5).abs() < 1e-6);
        assert_eq!(data.shapes[BrowLowererLeft as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowPinchLeft as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowLowererRight as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowPinchRight as usize].weight, 0.0);
    }

    #[test]
    fn brow_at_05_neutral() {
        let (data, _) = apply_v2_msg("/etvr/v2/BrowExpression", 0.5);
        // 0.5 >= 0.5 -> raise branch, weight = (0.5 - 0.5) * 2.0 = 0.0
        assert_eq!(data.shapes[BrowOuterUpLeft as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowInnerUpLeft as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowLowererLeft as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowPinchLeft as usize].weight, 0.0);
    }

    // BrowExpressionLeft only affects left side
    #[test]
    fn brow_expression_left_only() {
        let (data, _) = apply_v2_msg("/etvr/v2/BrowExpressionLeft", 0.25);
        // Left side: lowerer = 0.5
        assert!((data.shapes[BrowLowererLeft as usize].weight - 0.5).abs() < 1e-6);
        assert!((data.shapes[BrowPinchLeft as usize].weight - 0.5).abs() < 1e-6);
        // Right side: untouched (default 0.0)
        assert_eq!(data.shapes[BrowLowererRight as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowPinchRight as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowOuterUpRight as usize].weight, 0.0);
        assert_eq!(data.shapes[BrowInnerUpRight as usize].weight, 0.0);
    }
}
