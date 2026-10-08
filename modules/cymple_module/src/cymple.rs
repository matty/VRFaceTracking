use anyhow::Result;
use std::net::UdpSocket;
use vrft_api::{ModuleLogger, TrackingModule, UnifiedExpressions, UnifiedTrackingData};

const DEFAULT_PORT: u16 = 22999;
const PACKET_SIZE: usize = 292;
const HEADER_SIZE: usize = 12;
const NUM_FLOATS: usize = 70;
const MSG_PREFIX: i32 = -3; // 0xFFFFFFFD
const FLAG_MOUTH: u32 = 0x01;
const FLAG_EYE: u32 = 0x02;

pub struct CympleModule {
    socket: Option<UdpSocket>,
    logger: Option<ModuleLogger>,
    buf: Vec<u8>,
}

impl CympleModule {
    pub fn new() -> Self {
        Self {
            socket: None,
            logger: None,
            buf: vec![0u8; 512],
        }
    }

    fn parse_packet(&self, packet: &[u8], data: &mut UnifiedTrackingData) {
        if packet.len() != PACKET_SIZE {
            return;
        }

        let prefix = i32::from_le_bytes([packet[0], packet[1], packet[2], packet[3]]);
        if prefix != MSG_PREFIX {
            return;
        }

        let flags = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);

        let msg_type = u16::from_le_bytes([packet[8], packet[9]]);
        if msg_type != 0 {
            return;
        }

        let payload_len = u16::from_le_bytes([packet[10], packet[11]]);
        if payload_len != 280 {
            return;
        }

        let mut floats = [0.0f32; NUM_FLOATS];
        for (i, slot) in floats.iter_mut().enumerate() {
            let offset = HEADER_SIZE + i * 4;
            let val = f32::from_le_bytes([
                packet[offset],
                packet[offset + 1],
                packet[offset + 2],
                packet[offset + 3],
            ]);
            if !val.is_finite() {
                return;
            }
            *slot = val;
        }

        for val in &mut floats[..4] {
            *val = val.clamp(-1.0, 1.0);
        }
        for val in &mut floats[4..] {
            *val = val.clamp(0.0, 1.0);
        }

        if flags & FLAG_EYE != 0 {
            self.apply_eye(&floats, data);
        }
        if flags & FLAG_MOUTH != 0 {
            self.apply_face(&floats, data);
        }
    }

    fn apply_eye(&self, f: &[f32; NUM_FLOATS], data: &mut UnifiedTrackingData) {
        use UnifiedExpressions::*;

        data.eye.left.gaze.y = f[0]; // EyePitch_L
        data.eye.right.gaze.y = f[1]; // EyePitch_R
        data.eye.left.gaze.x = f[2]; // EyeYaw_L
        data.eye.right.gaze.x = f[3]; // EyeYaw_R

        data.eye.left.pupil_diameter_mm = f[4];
        data.eye.right.pupil_diameter_mm = f[5];
        data.eye.min_dilation = 0.0;
        data.eye.max_dilation = 1.0;

        data.eye.left.openness = 1.0 - f[6]; // EyeLidCloseLeft
        data.eye.right.openness = 1.0 - f[7]; // EyeLidCloseRight

        set(data, EyeSquintLeft, f[8]);
        set(data, EyeSquintRight, f[9]);
        set(data, EyeWideLeft, f[10]);
        set(data, EyeWideRight, f[11]);

        // Brow
        set(data, BrowLowererLeft, f[12]);
        set(data, BrowLowererRight, f[13]);
        set(data, BrowInnerUpLeft, f[14]);
        set(data, BrowInnerUpRight, f[15]);
        set(data, BrowOuterUpLeft, f[16]);
        set(data, BrowOuterUpRight, f[17]);
        set(data, BrowPinchLeft, f[18]);
        set(data, BrowPinchRight, f[19]);
    }

    fn apply_face(&self, f: &[f32; NUM_FLOATS], data: &mut UnifiedTrackingData) {
        use UnifiedExpressions::*;

        // Nose
        set(data, NoseSneerLeft, f[20]);
        set(data, NoseSneerRight, f[21]);

        // Cheek
        set(data, CheekSquintLeft, f[22]);
        set(data, CheekSquintRight, f[23]);
        set(data, CheekPuffLeft, f[24]);
        set(data, CheekPuffRight, f[25]);
        set(data, CheekSuckLeft, f[26]);
        set(data, CheekSuckRight, f[27]);

        // Jaw
        set(data, JawLeft, f[28]);
        set(data, JawRight, f[29]);
        set(data, JawForward, f[30]);
        set(data, JawOpen, f[31]);
        set(data, MouthClosed, f[32]);

        // Mouth shift
        set(data, MouthUpperLeft, f[33]);
        set(data, MouthUpperRight, f[34]);
        set(data, MouthLowerLeft, f[35]);
        set(data, MouthLowerRight, f[36]);

        // Lip suck
        set(data, LipSuckUpperLeft, f[37]);
        set(data, LipSuckUpperRight, f[38]);
        set(data, LipSuckLowerLeft, f[39]);
        set(data, LipSuckLowerRight, f[40]);

        // Lip funnel
        set(data, LipFunnelUpperLeft, f[41]);
        set(data, LipFunnelUpperRight, f[42]);
        set(data, LipFunnelLowerLeft, f[43]);
        set(data, LipFunnelLowerRight, f[44]);

        // Pucker (single value -> both sides)
        set(data, LipPuckerUpperLeft, f[45]);
        set(data, LipPuckerUpperRight, f[45]);
        set(data, LipPuckerLowerLeft, f[46]);
        set(data, LipPuckerLowerRight, f[46]);

        // Lip raise -> UpperUp + UpperDeepen
        set(data, MouthUpperUpLeft, f[47]);
        set(data, MouthUpperDeepenLeft, f[47]);
        set(data, MouthUpperUpRight, f[48]);
        set(data, MouthUpperDeepenRight, f[48]);
        set(data, MouthLowerDownLeft, f[49]);
        set(data, MouthLowerDownRight, f[50]);

        // Smile -> CornerPull + CornerSlant
        set(data, MouthCornerPullLeft, f[51]);
        set(data, MouthCornerSlantLeft, f[51]);
        set(data, MouthCornerPullRight, f[52]);
        set(data, MouthCornerSlantRight, f[52]);

        set(data, MouthDimpleLeft, f[53]);
        set(data, MouthDimpleRight, f[54]);
        set(data, MouthFrownLeft, f[55]);
        set(data, MouthFrownRight, f[56]);
        set(data, MouthStretchLeft, f[57]);
        set(data, MouthStretchRight, f[58]);

        set(data, MouthRaiserUpper, f[59]);
        set(data, MouthRaiserLower, f[60]);

        // Tongue
        set(data, TongueOut, f[61]);
        set(data, TongueLeft, f[62]);
        set(data, TongueRight, f[63]);
        set(data, TongueUp, f[64]);
        set(data, TongueDown, f[65]);
        set(data, TongueCurlUp, f[66]);
        set(data, TongueBendDown, f[67]);
        set(data, TongueFlat, f[68]);
        set(data, TongueRoll, f[69]);
    }
}

fn set(data: &mut UnifiedTrackingData, expr: UnifiedExpressions, value: f32) {
    data.shapes[expr as usize].weight = value;
}

impl TrackingModule for CympleModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing Cymple Module");
        let socket = UdpSocket::bind(format!("0.0.0.0:{DEFAULT_PORT}"))?;
        socket.set_nonblocking(true)?;
        self.socket = Some(socket);
        logger.info(&format!(
            "Listening for Cymple face tracking on port {DEFAULT_PORT}"
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
            logger.info("Cymple Module unloaded");
        }
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(CympleModule::new())
}
