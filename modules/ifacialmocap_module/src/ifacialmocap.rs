use anyhow::Result;
use std::collections::HashMap;
use std::net::UdpSocket;
use vrft_api::{ModuleLogger, TrackingModule, UnifiedExpressions, UnifiedTrackingData};

const DEFAULT_PORT: u16 = 49983;
const HANDSHAKE: &[u8] = b"iFacialMocap_sahuasouryya9218sauhuiayeta91555dy3719|sendDataVersion=v2";

pub struct IFacialMocapModule {
    socket: Option<UdpSocket>,
    logger: Option<ModuleLogger>,
    buf: Vec<u8>,
    connected: bool,
}

impl IFacialMocapModule {
    pub fn new() -> Self {
        Self {
            socket: None,
            logger: None,
            buf: vec![0u8; 4096],
            connected: false,
        }
    }

    #[cfg(test)]
    fn bind_test_socket(&mut self) -> std::net::SocketAddr {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let addr = socket.local_addr().unwrap();
        self.socket = Some(socket);
        addr
    }

    fn parse_packet(&self, packet: &[u8], data: &mut UnifiedTrackingData) {
        let text = match std::str::from_utf8(packet) {
            Ok(t) => t,
            Err(_) => return,
        };

        let parts: Vec<&str> = text.split('|').collect();
        if parts.len() < 3 {
            return;
        }

        let mut blendshapes = HashMap::new();
        let mut head = [0.0f32; 6];
        let mut left_eye = [0.0f32; 3];
        let mut right_eye = [0.0f32; 3];

        for entry in &parts[1..parts.len() - 1] {
            if entry.contains('#') {
                let mut split = entry.splitn(2, '#');
                let name = split.next().unwrap_or("");
                let values_str = split.next().unwrap_or("");
                let values: Vec<f32> = values_str
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();

                match name {
                    "=head" if values.len() >= 6 => {
                        head.copy_from_slice(&values[..6]);
                    }
                    "rightEye" if values.len() >= 3 => {
                        right_eye.copy_from_slice(&values[..3]);
                    }
                    "leftEye" if values.len() >= 3 => {
                        left_eye.copy_from_slice(&values[..3]);
                    }
                    _ => {}
                }
            } else {
                let (name, value_str) = if entry.contains('&') {
                    entry.split_once('&').unwrap_or((entry, "0"))
                } else {
                    entry.split_once('-').unwrap_or((entry, "0"))
                };
                if let Ok(int_val) = value_str.parse::<i32>() {
                    blendshapes.insert(name, int_val as f32 / 100.0);
                }
            }
        }

        self.apply_eye_gaze(&left_eye, &right_eye, data);
        self.apply_eye_openness(&blendshapes, data);
        self.apply_shapes(&blendshapes, data);
        self.apply_head(&head, data);
    }

    fn apply_eye_gaze(
        &self,
        left_eye: &[f32; 3],
        right_eye: &[f32; 3],
        data: &mut UnifiedTrackingData,
    ) {
        // iFacialMocap swaps L/R (mirror mode)
        data.eye.left.gaze.x = (right_eye[1] / 90.0).tan();
        data.eye.left.gaze.y = -(right_eye[0] / 90.0).tan();
        data.eye.right.gaze.x = (left_eye[1] / 90.0).tan();
        data.eye.right.gaze.y = -(left_eye[0] / 90.0).tan();

        data.eye.left.pupil_diameter_mm = 5.0;
        data.eye.right.pupil_diameter_mm = 5.0;
        data.eye.min_dilation = 0.0;
        data.eye.max_dilation = 10.0;
    }

    fn apply_eye_openness(&self, bs: &HashMap<&str, f32>, data: &mut UnifiedTrackingData) {
        // L/R swapped: ARKit _R -> Unified Left
        let blink_r = *bs.get("eyeBlink_R").unwrap_or(&0.0);
        let squint_r = *bs.get("eyeSquint_R").unwrap_or(&0.0);
        data.eye.left.openness = 1.0 - (blink_r + blink_r * squint_r).clamp(0.0, 1.0);

        let blink_l = *bs.get("eyeBlink_L").unwrap_or(&0.0);
        let squint_l = *bs.get("eyeSquint_L").unwrap_or(&0.0);
        data.eye.right.openness = 1.0 - (blink_l + blink_l * squint_l).clamp(0.0, 1.0);
    }

    fn apply_shapes(&self, bs: &HashMap<&str, f32>, data: &mut UnifiedTrackingData) {
        use UnifiedExpressions::*;

        let g = |name: &str| *bs.get(name).unwrap_or(&0.0);

        // Eye shapes (L/R swapped)
        set(data, EyeSquintLeft, g("eyeSquint_R"));
        set(data, EyeSquintRight, g("eyeSquint_L"));
        set(data, EyeWideLeft, g("eyeWide_R"));
        set(data, EyeWideRight, g("eyeWide_L"));

        // Brows (L/R swapped)
        set(data, BrowInnerUpLeft, g("browInnerUp"));
        set(data, BrowInnerUpRight, g("browInnerUp"));
        let brow_down_r = g("browDown_R");
        set(data, BrowLowererLeft, brow_down_r);
        set(data, BrowPinchLeft, brow_down_r);
        set(data, BrowOuterUpLeft, g("browOuterUp_R"));
        let brow_down_l = g("browDown_L");
        set(data, BrowLowererRight, brow_down_l);
        set(data, BrowPinchRight, brow_down_l);
        set(data, BrowOuterUpRight, g("browOuterUp_L"));

        // Nose (swapped)
        set(data, NoseSneerLeft, g("noseSneer_R"));
        set(data, NoseSneerRight, g("noseSneer_L"));

        // Cheek
        let cheek_puff = g("cheekPuff");
        set(data, CheekPuffLeft, cheek_puff);
        set(data, CheekPuffRight, cheek_puff);
        set(data, CheekSquintLeft, g("cheekSquint_R"));
        set(data, CheekSquintRight, g("cheekSquint_L"));

        // Jaw (swapped)
        set(data, JawLeft, g("jawRight"));
        set(data, JawRight, g("jawLeft"));
        set(data, JawOpen, g("jawOpen"));
        set(data, JawForward, g("jawForward"));
        set(data, MouthClosed, g("mouthClose"));

        // Lips
        let pucker = g("mouthPucker");
        set(data, LipPuckerUpperLeft, pucker);
        set(data, LipPuckerUpperRight, pucker);
        set(data, LipPuckerLowerLeft, pucker);
        set(data, LipPuckerLowerRight, pucker);

        let funnel = g("mouthFunnel");
        set(data, LipFunnelUpperLeft, funnel);
        set(data, LipFunnelUpperRight, funnel);
        set(data, LipFunnelLowerLeft, funnel);
        set(data, LipFunnelLowerRight, funnel);

        // Lip suck from roll
        let roll_upper = g("mouthRollUpper");
        let upper_up = g("mouthUpperUp_R").max(g("mouthUpperUp_L"));
        let clamped = roll_upper.min(1.0 - upper_up.powf(1.0 / 6.0));
        set(data, LipSuckUpperLeft, clamped);
        set(data, LipSuckUpperRight, clamped);
        let roll_lower = g("mouthRollLower");
        set(data, LipSuckLowerLeft, roll_lower);
        set(data, LipSuckLowerRight, roll_lower);

        // Mouth (swapped)
        set(data, MouthRaiserLower, g("mouthShrugLower"));
        set(data, MouthRaiserUpper, g("mouthShrugUpper"));

        let mouth_r = g("mouthRight");
        set(data, MouthUpperLeft, mouth_r);
        set(data, MouthLowerLeft, mouth_r);
        let mouth_l = g("mouthLeft");
        set(data, MouthUpperRight, mouth_l);
        set(data, MouthLowerRight, mouth_l);

        set(data, MouthUpperUpLeft, g("mouthUpperUp_R"));
        set(data, MouthUpperUpRight, g("mouthUpperUp_L"));
        set(data, MouthLowerDownLeft, g("mouthLowerDown_R"));
        set(data, MouthLowerDownRight, g("mouthLowerDown_L"));
        set(data, MouthCornerPullLeft, g("mouthSmile_R"));
        set(data, MouthCornerPullRight, g("mouthSmile_L"));
        set(data, MouthDimpleLeft, g("mouthDimple_R"));
        set(data, MouthDimpleRight, g("mouthDimple_L"));
        set(data, MouthFrownLeft, g("mouthFrown_R"));
        set(data, MouthFrownRight, g("mouthFrown_L"));
        set(data, MouthPressLeft, g("mouthPress_R"));
        set(data, MouthPressRight, g("mouthPress_L"));
        set(data, MouthStretchLeft, g("mouthStretch_R"));
        set(data, MouthStretchRight, g("mouthStretch_L"));

        set(data, TongueOut, g("tongueOut"));
    }

    fn apply_head(&self, head: &[f32; 6], data: &mut UnifiedTrackingData) {
        data.head.head_pitch = head[0] / 100.0;
        data.head.head_yaw = head[1] / 100.0;
        data.head.head_roll = head[2] / 100.0;
        data.head.head_pos_x = head[3];
        data.head.head_pos_y = head[4];
        data.head.head_pos_z = head[5];
    }
}

fn set(data: &mut UnifiedTrackingData, expr: UnifiedExpressions, value: f32) {
    data.shapes[expr as usize].weight = value;
}

impl TrackingModule for IFacialMocapModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing iFacialMocap Module");
        let socket = UdpSocket::bind(format!("0.0.0.0:{DEFAULT_PORT}"))?;
        socket.set_nonblocking(true)?;
        self.socket = Some(socket);
        logger.info(&format!(
            "Listening for iFacialMocap on port {DEFAULT_PORT}"
        ));
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
            match socket.recv_from(&mut self.buf) {
                Ok((n, addr)) => {
                    if !self.connected {
                        let reply_socket = UdpSocket::bind("0.0.0.0:0")?;
                        let _ = reply_socket.send_to(HANDSHAKE, addr);
                        self.connected = true;
                        if let Some(logger) = &self.logger {
                            logger.info(&format!("iFacialMocap connected from {addr}"));
                        }
                    }
                    self.parse_packet(&self.buf[..n], data);
                    received = true;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }

        if !received {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        Ok(())
    }

    fn unload(&mut self) {
        self.socket.take();
        self.connected = false;
        if let Some(logger) = &self.logger {
            logger.info("iFacialMocap Module unloaded");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrft_api::UnifiedExpressions::*;

    fn module() -> IFacialMocapModule {
        IFacialMocapModule::new()
    }

    fn make_packet(entries: &[&str]) -> Vec<u8> {
        let mut parts = vec!["ignored_first"];
        parts.extend_from_slice(entries);
        parts.push("ignored_last");
        parts.join("|").into_bytes()
    }

    fn shape(data: &UnifiedTrackingData, expr: UnifiedExpressions) -> f32 {
        data.shapes[expr as usize].weight
    }

    #[test]
    fn blendshapes_ampersand_separator_with_jaw_swap() {
        let m = module();
        let mut data = UnifiedTrackingData::default();
        let pkt = make_packet(&["jawOpen&75", "jawRight&50", "jawLeft&30"]);
        m.parse_packet(&pkt, &mut data);

        assert!((shape(&data, JawOpen) - 0.75).abs() < 1e-6);
        // L/R swapped: jawRight -> JawLeft, jawLeft -> JawRight
        assert!((shape(&data, JawLeft) - 0.50).abs() < 1e-6);
        assert!((shape(&data, JawRight) - 0.30).abs() < 1e-6);
    }

    #[test]
    fn blendshapes_dash_separator() {
        let m = module();
        let mut data = UnifiedTrackingData::default();
        let pkt = make_packet(&["jawOpen-60"]);
        m.parse_packet(&pkt, &mut data);

        assert!((shape(&data, JawOpen) - 0.60).abs() < 1e-6);
    }

    #[test]
    fn eye_gaze_from_pose_data_with_lr_swap() {
        let m = module();
        let mut data = UnifiedTrackingData::default();
        // rightEye values -> left gaze (swapped), leftEye -> right gaze
        let pkt = make_packet(&["rightEye#30.0,45.0,0.0", "leftEye#20.0,10.0,0.0"]);
        m.parse_packet(&pkt, &mut data);

        let expected_left_x = (45.0f32 / 90.0).tan();
        let expected_left_y = -(30.0f32 / 90.0).tan();
        assert!((data.eye.left.gaze.x - expected_left_x).abs() < 1e-6);
        assert!((data.eye.left.gaze.y - expected_left_y).abs() < 1e-6);

        let expected_right_x = (10.0f32 / 90.0).tan();
        let expected_right_y = -(20.0f32 / 90.0).tan();
        assert!((data.eye.right.gaze.x - expected_right_x).abs() < 1e-6);
        assert!((data.eye.right.gaze.y - expected_right_y).abs() < 1e-6);
    }

    #[test]
    fn eye_openness_with_blink_squint_and_lr_swap() {
        let m = module();
        let mut data = UnifiedTrackingData::default();
        // eyeBlink_R=60 (0.6), eyeSquint_R=50 (0.5) -> left openness (swapped)
        // eyeBlink_L=40 (0.4), eyeSquint_L=20 (0.2) -> right openness (swapped)
        let pkt = make_packet(&[
            "eyeBlink_R&60",
            "eyeSquint_R&50",
            "eyeBlink_L&40",
            "eyeSquint_L&20",
        ]);
        m.parse_packet(&pkt, &mut data);

        let blink_r = 0.6f32;
        let squint_r = 0.5f32;
        let expected_left = 1.0 - (blink_r + blink_r * squint_r).clamp(0.0, 1.0);
        assert!((data.eye.left.openness - expected_left).abs() < 1e-6);

        let blink_l = 0.4f32;
        let squint_l = 0.2f32;
        let expected_right = 1.0 - (blink_l + blink_l * squint_l).clamp(0.0, 1.0);
        assert!((data.eye.right.openness - expected_right).abs() < 1e-6);
    }

    #[test]
    fn head_pose_divided_by_100() {
        let m = module();
        let mut data = UnifiedTrackingData::default();
        let pkt = make_packet(&["=head#1500,2000,-500,10.0,20.0,30.0"]);
        m.parse_packet(&pkt, &mut data);

        assert!((data.head.head_pitch - 15.0).abs() < 1e-6);
        assert!((data.head.head_yaw - 20.0).abs() < 1e-6);
        assert!((data.head.head_roll - -5.0).abs() < 1e-6);
        assert!((data.head.head_pos_x - 10.0).abs() < 1e-6);
        assert!((data.head.head_pos_y - 20.0).abs() < 1e-6);
        assert!((data.head.head_pos_z - 30.0).abs() < 1e-6);
    }

    #[test]
    fn too_few_pipe_parts_no_change() {
        let m = module();
        let mut data = UnifiedTrackingData::default();
        let original = UnifiedTrackingData::default();

        // Only 2 parts: "a|b"
        m.parse_packet(b"a|b", &mut data);
        assert_eq!(data.head.head_pitch, original.head.head_pitch);
        assert_eq!(data.eye.left.openness, original.eye.left.openness);

        // Only 1 part
        m.parse_packet(b"no_pipes", &mut data);
        assert_eq!(data.head.head_pitch, original.head.head_pitch);
    }

    #[test]
    fn non_utf8_packet_no_change() {
        let m = module();
        let mut data = UnifiedTrackingData::default();
        let original = UnifiedTrackingData::default();

        let pkt: Vec<u8> = vec![0xFF, 0xFE, b'|', b'x', b'|', 0x80];
        m.parse_packet(&pkt, &mut data);
        assert_eq!(data.head.head_pitch, original.head.head_pitch);
        assert_eq!(data.eye.left.openness, original.eye.left.openness);
    }

    #[test]
    fn mouth_smile_lr_swap() {
        let m = module();
        let mut data = UnifiedTrackingData::default();
        let pkt = make_packet(&["mouthSmile_R&80", "mouthSmile_L&40"]);
        m.parse_packet(&pkt, &mut data);

        // mouthSmile_R -> MouthCornerPullLeft (swapped)
        assert!((shape(&data, MouthCornerPullLeft) - 0.80).abs() < 1e-6);
        // mouthSmile_L -> MouthCornerPullRight (swapped)
        assert!((shape(&data, MouthCornerPullRight) - 0.40).abs() < 1e-6);
    }

    #[test]
    fn udp_round_trip_face_data() {
        let mut m = module();
        let addr = m.bind_test_socket();

        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let payload = make_packet(&[
            "jawOpen&75",
            "cheekPuff&50",
            "tongueOut&90",
            "mouthClose&30",
            "=head#1000,2000,-500,1.0,2.0,3.0",
        ]);
        sender.send_to(&payload, addr).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));

        let mut data = UnifiedTrackingData::default();
        m.update(&mut data).unwrap();

        assert!((shape(&data, JawOpen) - 0.75).abs() < 1e-6);
        assert!((shape(&data, CheekPuffLeft) - 0.50).abs() < 1e-6);
        assert!((shape(&data, CheekPuffRight) - 0.50).abs() < 1e-6);
        assert!((shape(&data, TongueOut) - 0.90).abs() < 1e-6);
        assert!((shape(&data, MouthClosed) - 0.30).abs() < 1e-6);

        assert!((data.head.head_pitch - 10.0).abs() < 1e-6);
        assert!((data.head.head_yaw - 20.0).abs() < 1e-6);
        assert!((data.head.head_roll - -5.0).abs() < 1e-6);
        assert!((data.head.head_pos_x - 1.0).abs() < 1e-6);
        assert!((data.head.head_pos_y - 2.0).abs() < 1e-6);
        assert!((data.head.head_pos_z - 3.0).abs() < 1e-6);
    }

    #[test]
    fn udp_round_trip_left_right_swap() {
        let mut m = module();
        let addr = m.bind_test_socket();

        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        // ARKit _R blendshapes should map to Unified Left (and vice versa)
        let payload = make_packet(&[
            "eyeSquint_R&70",
            "eyeSquint_L&30",
            "noseSneer_R&60",
            "noseSneer_L&20",
            "mouthSmile_R&80",
            "mouthSmile_L&40",
            "browOuterUp_R&55",
            "browOuterUp_L&25",
            "rightEye#30.0,45.0,0.0",
            "leftEye#20.0,10.0,0.0",
        ]);
        sender.send_to(&payload, addr).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));

        let mut data = UnifiedTrackingData::default();
        m.update(&mut data).unwrap();

        // Face shapes: _R -> Left, _L -> Right
        assert!((shape(&data, EyeSquintLeft) - 0.70).abs() < 1e-6);
        assert!((shape(&data, EyeSquintRight) - 0.30).abs() < 1e-6);
        assert!((shape(&data, NoseSneerLeft) - 0.60).abs() < 1e-6);
        assert!((shape(&data, NoseSneerRight) - 0.20).abs() < 1e-6);
        assert!((shape(&data, MouthCornerPullLeft) - 0.80).abs() < 1e-6);
        assert!((shape(&data, MouthCornerPullRight) - 0.40).abs() < 1e-6);
        assert!((shape(&data, BrowOuterUpLeft) - 0.55).abs() < 1e-6);
        assert!((shape(&data, BrowOuterUpRight) - 0.25).abs() < 1e-6);

        // Eye gaze: rightEye -> left gaze, leftEye -> right gaze
        let expected_left_x = (45.0f32 / 90.0).tan();
        let expected_left_y = -(30.0f32 / 90.0).tan();
        assert!((data.eye.left.gaze.x - expected_left_x).abs() < 1e-6);
        assert!((data.eye.left.gaze.y - expected_left_y).abs() < 1e-6);

        let expected_right_x = (10.0f32 / 90.0).tan();
        let expected_right_y = -(20.0f32 / 90.0).tan();
        assert!((data.eye.right.gaze.x - expected_right_x).abs() < 1e-6);
        assert!((data.eye.right.gaze.y - expected_right_y).abs() < 1e-6);
    }

    #[test]
    fn udp_no_data_no_error() {
        let mut m = module();
        m.bind_test_socket();

        let mut data = UnifiedTrackingData::default();
        let original = UnifiedTrackingData::default();

        // No data sent, update should succeed with no changes
        m.update(&mut data).unwrap();

        assert_eq!(data.head.head_pitch, original.head.head_pitch);
        assert_eq!(data.head.head_yaw, original.head.head_yaw);
        assert_eq!(data.eye.left.openness, original.eye.left.openness);
        assert_eq!(data.eye.right.openness, original.eye.right.openness);
        assert_eq!(
            data.shapes[JawOpen as usize].weight,
            original.shapes[JawOpen as usize].weight
        );
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(IFacialMocapModule::new())
}
