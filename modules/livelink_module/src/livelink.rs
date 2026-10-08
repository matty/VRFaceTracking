use anyhow::Result;
use std::net::UdpSocket;
use vrft_api::{ModuleLogger, TrackingModule, UnifiedExpressions, UnifiedTrackingData};

const DEFAULT_PORT: u16 = 11111;
const PAYLOAD_SIZE: usize = 244;
const NUM_FLOATS: usize = 61;

pub struct LiveLinkModule {
    socket: Option<UdpSocket>,
    logger: Option<ModuleLogger>,
    buf: Vec<u8>,
}

impl LiveLinkModule {
    pub fn new() -> Self {
        Self {
            socket: None,
            logger: None,
            buf: vec![0u8; 4096],
        }
    }

    fn parse_packet(&self, packet: &[u8], data: &mut UnifiedTrackingData) {
        if packet.len() < PAYLOAD_SIZE {
            return;
        }

        let payload = &packet[packet.len() - PAYLOAD_SIZE..];
        let mut values = [0.0f32; NUM_FLOATS];
        for (i, val) in values.iter_mut().enumerate() {
            let offset = i * 4;
            let bytes = [
                payload[offset + 3],
                payload[offset + 2],
                payload[offset + 1],
                payload[offset],
            ];
            *val = f32::from_le_bytes(bytes);
        }

        self.apply_values(&values, data);
    }

    fn apply_values(&self, v: &[f32; NUM_FLOATS], data: &mut UnifiedTrackingData) {
        use UnifiedExpressions::*;

        // Eye openness: 1 - clamp(blink + blink * squint, 0, 1)
        let blink_l = v[0];
        let squint_l = v[5];
        let blink_r = v[7];
        let squint_r = v[12];
        data.eye.left.openness = 1.0 - (blink_l + blink_l * squint_l).clamp(0.0, 1.0);
        data.eye.right.openness = 1.0 - (blink_r + blink_r * squint_r).clamp(0.0, 1.0);

        // Eye gaze from indices 55-60
        data.eye.left.gaze.x = v[55]; // EyeYawLeft
        data.eye.left.gaze.y = -v[56]; // -EyePitchLeft
        data.eye.right.gaze.x = v[58]; // EyeYawRight
        data.eye.right.gaze.y = -v[59]; // -EyePitchRight

        data.eye.left.pupil_diameter_mm = 5.0;
        data.eye.right.pupil_diameter_mm = 5.0;
        data.eye.min_dilation = 0.0;
        data.eye.max_dilation = 10.0;

        // Eye expressions
        set(data, EyeSquintLeft, squint_l);
        set(data, EyeSquintRight, squint_r);
        set(data, EyeWideLeft, v[6]);
        set(data, EyeWideRight, v[13]);

        // Jaw
        set(data, JawForward, v[14]);
        set(data, JawLeft, v[15]);
        set(data, JawRight, v[16]);
        set(data, JawOpen, v[17]);

        // Mouth
        set(data, MouthClosed, v[18]);

        // Lip funnel (single value -> 4 shapes)
        let funnel = v[19];
        set(data, LipFunnelUpperLeft, funnel);
        set(data, LipFunnelUpperRight, funnel);
        set(data, LipFunnelLowerLeft, funnel);
        set(data, LipFunnelLowerRight, funnel);

        // Lip pucker (single value -> 4 shapes)
        let pucker = v[20];
        set(data, LipPuckerUpperLeft, pucker);
        set(data, LipPuckerUpperRight, pucker);
        set(data, LipPuckerLowerLeft, pucker);
        set(data, LipPuckerLowerRight, pucker);

        // Mouth direction
        let mouth_l = v[21];
        set(data, MouthUpperLeft, mouth_l);
        set(data, MouthLowerLeft, mouth_l);
        let mouth_r = v[22];
        set(data, MouthUpperRight, mouth_r);
        set(data, MouthLowerRight, mouth_r);

        // Smile -> corner pull + slant
        let smile_l = v[23];
        set(data, MouthCornerPullLeft, smile_l);
        set(data, MouthCornerSlantLeft, smile_l);
        let smile_r = v[24];
        set(data, MouthCornerPullRight, smile_r);
        set(data, MouthCornerSlantRight, smile_r);

        set(data, MouthFrownLeft, v[25]);
        set(data, MouthFrownRight, v[26]);
        set(data, MouthDimpleLeft, v[27]);
        set(data, MouthDimpleRight, v[28]);
        set(data, MouthStretchLeft, v[29]);
        set(data, MouthStretchRight, v[30]);

        // Lip suck
        let roll_lower = v[31];
        set(data, LipSuckLowerLeft, roll_lower);
        set(data, LipSuckLowerRight, roll_lower);

        // Lip suck upper with correction
        let roll_upper = v[32];
        let upper_up_l = v[39];
        let clamped_l = roll_upper.min(1.0 - upper_up_l.powf(1.0 / 6.0));
        set(data, LipSuckUpperLeft, clamped_l);
        let upper_up_r = v[40];
        let clamped_r = roll_upper.min(1.0 - upper_up_r.powf(1.0 / 6.0));
        set(data, LipSuckUpperRight, clamped_r);

        set(data, MouthRaiserLower, v[33]);
        set(data, MouthRaiserUpper, v[34]);
        set(data, MouthPressLeft, v[35]);
        set(data, MouthPressRight, v[36]);
        set(data, MouthLowerDownLeft, v[37]);
        set(data, MouthLowerDownRight, v[38]);

        // Upper lip raise -> both UpperUp and UpperDeepen
        set(data, MouthUpperUpLeft, upper_up_l);
        set(data, MouthUpperDeepenLeft, upper_up_l);
        set(data, MouthUpperUpRight, upper_up_r);
        set(data, MouthUpperDeepenRight, upper_up_r);

        // Brow
        let brow_down_l = v[41];
        set(data, BrowPinchLeft, brow_down_l);
        set(data, BrowLowererLeft, brow_down_l);
        let brow_down_r = v[42];
        set(data, BrowPinchRight, brow_down_r);
        set(data, BrowLowererRight, brow_down_r);
        let brow_inner = v[43];
        set(data, BrowInnerUpLeft, brow_inner);
        set(data, BrowInnerUpRight, brow_inner);
        set(data, BrowOuterUpLeft, v[44]);
        set(data, BrowOuterUpRight, v[45]);

        // Cheek puff (single value -> both)
        let cheek_puff = v[46];
        set(data, CheekPuffLeft, cheek_puff);
        set(data, CheekPuffRight, cheek_puff);

        set(data, CheekSquintLeft, v[47]);
        set(data, CheekSquintRight, v[48]);
        set(data, NoseSneerLeft, v[49]);
        set(data, NoseSneerRight, v[50]);
        set(data, TongueOut, v[51]);

        // Head pose (negated)
        data.head.head_yaw = -v[52];
        data.head.head_pitch = -v[53];
        data.head.head_roll = -v[54];
    }
}

fn set(data: &mut UnifiedTrackingData, expr: UnifiedExpressions, value: f32) {
    data.shapes[expr as usize].weight = value;
}

impl TrackingModule for LiveLinkModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing LiveLink Module");
        let socket = UdpSocket::bind(format!("0.0.0.0:{DEFAULT_PORT}"))?;
        socket.set_nonblocking(true)?;
        self.socket = Some(socket);
        logger.info(&format!(
            "Listening for LiveLink Face on port {DEFAULT_PORT}"
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
            match socket.recv(&mut self.buf) {
                Ok(n) => {
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
        if let Some(logger) = &self.logger {
            logger.info("LiveLink Module unloaded");
        }
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(LiveLinkModule::new())
}
