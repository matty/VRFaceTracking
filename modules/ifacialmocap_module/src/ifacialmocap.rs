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

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(IFacialMocapModule::new())
}
