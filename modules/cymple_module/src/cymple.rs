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

    #[cfg(test)]
    fn bind_test_socket(&mut self) -> std::net::SocketAddr {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let addr = socket.local_addr().unwrap();
        self.socket = Some(socket);
        addr
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

#[cfg(test)]
mod tests {
    use super::*;
    use vrft_api::UnifiedExpressions::*;

    fn make_packet(flags: u32, floats: &[f32; 70]) -> Vec<u8> {
        let mut buf = vec![0u8; PACKET_SIZE];
        // Prefix: 0xFFFFFFFD as little-endian i32 (== -3)
        buf[0..4].copy_from_slice(&MSG_PREFIX.to_le_bytes());
        // Flags: big-endian u32
        buf[4..8].copy_from_slice(&flags.to_be_bytes());
        // msg_type: 0 as little-endian u16
        buf[8..10].copy_from_slice(&0u16.to_le_bytes());
        // payload_len: 280 as little-endian u16
        buf[10..12].copy_from_slice(&280u16.to_le_bytes());
        // 70 little-endian f32 floats
        for (i, &val) in floats.iter().enumerate() {
            let offset = HEADER_SIZE + i * 4;
            buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
        }
        buf
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    fn w(data: &UnifiedTrackingData, expr: UnifiedExpressions) -> f32 {
        data.shapes[expr as usize].weight
    }

    #[test]
    fn eye_flag_applies_gaze_openness_pupil_and_expressions() {
        let module = CympleModule::new();
        let mut data = UnifiedTrackingData::default();
        let mut f = [0.0f32; 70];
        f[0] = 0.3; // left gaze y
        f[1] = -0.4; // right gaze y
        f[2] = 0.5; // left gaze x
        f[3] = -0.6; // right gaze x
        f[4] = 0.7; // left pupil diameter
        f[5] = 0.8; // right pupil diameter
        f[6] = 0.2; // left lid close -> openness = 0.8
        f[7] = 0.9; // right lid close -> openness = 0.1
        f[8] = 0.11; // EyeSquintLeft
        f[9] = 0.12; // EyeSquintRight
        f[10] = 0.13; // EyeWideLeft
        f[11] = 0.14; // EyeWideRight
        f[12] = 0.15; // BrowLowererLeft
        f[13] = 0.16; // BrowLowererRight
        f[14] = 0.17; // BrowInnerUpLeft
        f[15] = 0.18; // BrowInnerUpRight
        f[16] = 0.19; // BrowOuterUpLeft
        f[17] = 0.21; // BrowOuterUpRight
        f[18] = 0.22; // BrowPinchLeft
        f[19] = 0.23; // BrowPinchRight

        let packet = make_packet(FLAG_EYE, &f);
        module.parse_packet(&packet, &mut data);

        assert!(approx(data.eye.left.gaze.y, 0.3));
        assert!(approx(data.eye.right.gaze.y, -0.4));
        assert!(approx(data.eye.left.gaze.x, 0.5));
        assert!(approx(data.eye.right.gaze.x, -0.6));
        assert!(approx(data.eye.left.pupil_diameter_mm, 0.7));
        assert!(approx(data.eye.right.pupil_diameter_mm, 0.8));
        assert!(approx(data.eye.left.openness, 0.8));
        assert!(approx(data.eye.right.openness, 0.1));
        assert!(approx(data.eye.min_dilation, 0.0));
        assert!(approx(data.eye.max_dilation, 1.0));

        assert!(approx(w(&data, EyeSquintLeft), 0.11));
        assert!(approx(w(&data, EyeSquintRight), 0.12));
        assert!(approx(w(&data, EyeWideLeft), 0.13));
        assert!(approx(w(&data, EyeWideRight), 0.14));
        assert!(approx(w(&data, BrowLowererLeft), 0.15));
        assert!(approx(w(&data, BrowLowererRight), 0.16));
        assert!(approx(w(&data, BrowInnerUpLeft), 0.17));
        assert!(approx(w(&data, BrowInnerUpRight), 0.18));
        assert!(approx(w(&data, BrowOuterUpLeft), 0.19));
        assert!(approx(w(&data, BrowOuterUpRight), 0.21));
        assert!(approx(w(&data, BrowPinchLeft), 0.22));
        assert!(approx(w(&data, BrowPinchRight), 0.23));

        // Mouth expressions should be untouched (eye flag only).
        assert!(approx(w(&data, JawOpen), 0.0));
        assert!(approx(w(&data, TongueOut), 0.0));
    }

    #[test]
    fn mouth_flag_applies_face_expressions() {
        let module = CympleModule::new();
        let mut data = UnifiedTrackingData::default();
        let mut f = [0.0f32; 70];
        f[20] = 0.10; // NoseSneerLeft
        f[21] = 0.11; // NoseSneerRight
        f[22] = 0.12; // CheekSquintLeft
        f[23] = 0.13; // CheekSquintRight
        f[24] = 0.14; // CheekPuffLeft
        f[25] = 0.15; // CheekPuffRight
        f[26] = 0.16; // CheekSuckLeft
        f[27] = 0.17; // CheekSuckRight
        f[28] = 0.18; // JawLeft
        f[29] = 0.19; // JawRight
        f[30] = 0.20; // JawForward
        f[31] = 0.80; // JawOpen
        f[32] = 0.50; // MouthClosed
        f[33] = 0.21; // MouthUpperLeft
        f[34] = 0.22; // MouthUpperRight
        f[35] = 0.23; // MouthLowerLeft
        f[36] = 0.24; // MouthLowerRight
        f[37] = 0.25; // LipSuckUpperLeft
        f[38] = 0.26; // LipSuckUpperRight
        f[39] = 0.27; // LipSuckLowerLeft
        f[40] = 0.28; // LipSuckLowerRight
        f[41] = 0.29; // LipFunnelUpperLeft
        f[42] = 0.30; // LipFunnelUpperRight
        f[43] = 0.31; // LipFunnelLowerLeft
        f[44] = 0.32; // LipFunnelLowerRight
        f[45] = 0.33; // LipPuckerUpperLeft + LipPuckerUpperRight
        f[46] = 0.34; // LipPuckerLowerLeft + LipPuckerLowerRight
        f[47] = 0.35; // MouthUpperUpLeft + MouthUpperDeepenLeft
        f[48] = 0.36; // MouthUpperUpRight + MouthUpperDeepenRight
        f[49] = 0.37; // MouthLowerDownLeft
        f[50] = 0.38; // MouthLowerDownRight
        f[51] = 0.39; // MouthCornerPullLeft + MouthCornerSlantLeft
        f[52] = 0.40; // MouthCornerPullRight + MouthCornerSlantRight
        f[53] = 0.41; // MouthDimpleLeft
        f[54] = 0.42; // MouthDimpleRight
        f[55] = 0.43; // MouthFrownLeft
        f[56] = 0.44; // MouthFrownRight
        f[57] = 0.45; // MouthStretchLeft
        f[58] = 0.46; // MouthStretchRight
        f[59] = 0.47; // MouthRaiserUpper
        f[60] = 0.48; // MouthRaiserLower
        f[61] = 0.50; // TongueOut
        f[62] = 0.51; // TongueLeft
        f[63] = 0.52; // TongueRight
        f[64] = 0.53; // TongueUp
        f[65] = 0.54; // TongueDown
        f[66] = 0.55; // TongueCurlUp
        f[67] = 0.56; // TongueBendDown
        f[68] = 0.57; // TongueFlat
        f[69] = 0.58; // TongueRoll

        let packet = make_packet(FLAG_MOUTH, &f);
        module.parse_packet(&packet, &mut data);

        // Nose
        assert!(approx(w(&data, NoseSneerLeft), 0.10));
        assert!(approx(w(&data, NoseSneerRight), 0.11));
        // Cheek
        assert!(approx(w(&data, CheekSquintLeft), 0.12));
        assert!(approx(w(&data, CheekSquintRight), 0.13));
        assert!(approx(w(&data, CheekPuffLeft), 0.14));
        assert!(approx(w(&data, CheekPuffRight), 0.15));
        assert!(approx(w(&data, CheekSuckLeft), 0.16));
        assert!(approx(w(&data, CheekSuckRight), 0.17));
        // Jaw
        assert!(approx(w(&data, JawLeft), 0.18));
        assert!(approx(w(&data, JawRight), 0.19));
        assert!(approx(w(&data, JawForward), 0.20));
        assert!(approx(w(&data, JawOpen), 0.80));
        assert!(approx(w(&data, MouthClosed), 0.50));
        // Mouth shift
        assert!(approx(w(&data, MouthUpperLeft), 0.21));
        assert!(approx(w(&data, MouthUpperRight), 0.22));
        assert!(approx(w(&data, MouthLowerLeft), 0.23));
        assert!(approx(w(&data, MouthLowerRight), 0.24));
        // Lip suck
        assert!(approx(w(&data, LipSuckUpperLeft), 0.25));
        assert!(approx(w(&data, LipSuckUpperRight), 0.26));
        assert!(approx(w(&data, LipSuckLowerLeft), 0.27));
        assert!(approx(w(&data, LipSuckLowerRight), 0.28));
        // Lip funnel
        assert!(approx(w(&data, LipFunnelUpperLeft), 0.29));
        assert!(approx(w(&data, LipFunnelUpperRight), 0.30));
        assert!(approx(w(&data, LipFunnelLowerLeft), 0.31));
        assert!(approx(w(&data, LipFunnelLowerRight), 0.32));
        // Pucker (single value -> both sides)
        assert!(approx(w(&data, LipPuckerUpperLeft), 0.33));
        assert!(approx(w(&data, LipPuckerUpperRight), 0.33));
        assert!(approx(w(&data, LipPuckerLowerLeft), 0.34));
        assert!(approx(w(&data, LipPuckerLowerRight), 0.34));
        // Lip raise -> UpperUp + UpperDeepen
        assert!(approx(w(&data, MouthUpperUpLeft), 0.35));
        assert!(approx(w(&data, MouthUpperDeepenLeft), 0.35));
        assert!(approx(w(&data, MouthUpperUpRight), 0.36));
        assert!(approx(w(&data, MouthUpperDeepenRight), 0.36));
        assert!(approx(w(&data, MouthLowerDownLeft), 0.37));
        assert!(approx(w(&data, MouthLowerDownRight), 0.38));
        // Smile -> CornerPull + CornerSlant
        assert!(approx(w(&data, MouthCornerPullLeft), 0.39));
        assert!(approx(w(&data, MouthCornerSlantLeft), 0.39));
        assert!(approx(w(&data, MouthCornerPullRight), 0.40));
        assert!(approx(w(&data, MouthCornerSlantRight), 0.40));
        // Dimple, frown, stretch
        assert!(approx(w(&data, MouthDimpleLeft), 0.41));
        assert!(approx(w(&data, MouthDimpleRight), 0.42));
        assert!(approx(w(&data, MouthFrownLeft), 0.43));
        assert!(approx(w(&data, MouthFrownRight), 0.44));
        assert!(approx(w(&data, MouthStretchLeft), 0.45));
        assert!(approx(w(&data, MouthStretchRight), 0.46));
        // Raiser
        assert!(approx(w(&data, MouthRaiserUpper), 0.47));
        assert!(approx(w(&data, MouthRaiserLower), 0.48));
        // Tongue
        assert!(approx(w(&data, TongueOut), 0.50));
        assert!(approx(w(&data, TongueLeft), 0.51));
        assert!(approx(w(&data, TongueRight), 0.52));
        assert!(approx(w(&data, TongueUp), 0.53));
        assert!(approx(w(&data, TongueDown), 0.54));
        assert!(approx(w(&data, TongueCurlUp), 0.55));
        assert!(approx(w(&data, TongueBendDown), 0.56));
        assert!(approx(w(&data, TongueFlat), 0.57));
        assert!(approx(w(&data, TongueRoll), 0.58));

        // Eye data should be untouched (mouth flag only).
        assert!(approx(data.eye.left.gaze.x, 0.0));
        assert!(approx(data.eye.left.openness, 0.0));
    }

    #[test]
    fn both_flags_apply_eye_and_face() {
        let module = CympleModule::new();
        let mut data = UnifiedTrackingData::default();
        let mut f = [0.0f32; 70];
        f[0] = 0.1; // left gaze y
        f[2] = -0.2; // left gaze x
        f[6] = 0.3; // left lid close -> openness = 0.7
        f[31] = 0.9; // JawOpen
        f[61] = 0.4; // TongueOut

        let packet = make_packet(FLAG_MOUTH | FLAG_EYE, &f);
        module.parse_packet(&packet, &mut data);

        // Eye data applied
        assert!(approx(data.eye.left.gaze.y, 0.1));
        assert!(approx(data.eye.left.gaze.x, -0.2));
        assert!(approx(data.eye.left.openness, 0.7));
        // Face data applied
        assert!(approx(w(&data, JawOpen), 0.9));
        assert!(approx(w(&data, TongueOut), 0.4));
    }

    #[test]
    fn wrong_packet_size_is_ignored() {
        let module = CympleModule::new();
        let mut data = UnifiedTrackingData::default();
        let f = [0.5f32; 70];

        let full = make_packet(FLAG_MOUTH | FLAG_EYE, &f);
        // Too short
        module.parse_packet(&full[..291], &mut data);
        assert!(approx(w(&data, JawOpen), 0.0));
        // Too long
        let mut long = full.clone();
        long.push(0);
        module.parse_packet(&long, &mut data);
        assert!(approx(w(&data, JawOpen), 0.0));
    }

    #[test]
    fn wrong_prefix_is_ignored() {
        let module = CympleModule::new();
        let mut data = UnifiedTrackingData::default();
        let mut f = [0.0f32; 70];
        f[31] = 0.9;

        let mut packet = make_packet(FLAG_MOUTH, &f);
        // Corrupt prefix to 0x00000000
        packet[0..4].copy_from_slice(&0i32.to_le_bytes());
        module.parse_packet(&packet, &mut data);
        assert!(approx(w(&data, JawOpen), 0.0));
    }

    #[test]
    fn wrong_msg_type_is_ignored() {
        let module = CympleModule::new();
        let mut data = UnifiedTrackingData::default();
        let mut f = [0.0f32; 70];
        f[31] = 0.9;

        let mut packet = make_packet(FLAG_MOUTH, &f);
        // Set msg_type to 1
        packet[8..10].copy_from_slice(&1u16.to_le_bytes());
        module.parse_packet(&packet, &mut data);
        assert!(approx(w(&data, JawOpen), 0.0));
    }

    #[test]
    fn non_finite_float_rejects_packet() {
        let module = CympleModule::new();
        let mut data = UnifiedTrackingData::default();
        let mut f = [0.5f32; 70];
        f[10] = f32::NAN;

        let packet = make_packet(FLAG_MOUTH | FLAG_EYE, &f);
        module.parse_packet(&packet, &mut data);
        // Nothing should have been applied.
        assert!(approx(w(&data, JawOpen), 0.0));
        assert!(approx(data.eye.left.gaze.y, 0.0));

        // Also test infinity.
        let mut f2 = [0.5f32; 70];
        f2[0] = f32::INFINITY;
        let packet2 = make_packet(FLAG_EYE, &f2);
        module.parse_packet(&packet2, &mut data);
        assert!(approx(data.eye.left.gaze.y, 0.0));
    }

    #[test]
    fn gaze_values_clamped_to_negative_one_to_one() {
        let module = CympleModule::new();
        let mut data = UnifiedTrackingData::default();
        let mut f = [0.0f32; 70];
        f[0] = 2.0; // should clamp to 1.0
        f[1] = -3.0; // should clamp to -1.0
        f[2] = 1.5; // should clamp to 1.0
        f[3] = -0.5; // within range, stays

        let packet = make_packet(FLAG_EYE, &f);
        module.parse_packet(&packet, &mut data);

        assert!(approx(data.eye.left.gaze.y, 1.0));
        assert!(approx(data.eye.right.gaze.y, -1.0));
        assert!(approx(data.eye.left.gaze.x, 1.0));
        assert!(approx(data.eye.right.gaze.x, -0.5));
    }

    #[test]
    fn body_values_clamped_to_zero_to_one() {
        let module = CympleModule::new();
        let mut data = UnifiedTrackingData::default();
        let mut f = [0.0f32; 70];
        f[4] = -0.5; // pupil_diameter: clamped to 0.0
        f[5] = 1.5; // pupil_diameter: clamped to 1.0
        f[6] = -0.1; // lid close: clamped to 0.0 -> openness = 1.0
        f[8] = 2.0; // EyeSquintLeft: clamped to 1.0
        f[31] = -0.3; // JawOpen: clamped to 0.0
        f[61] = 1.7; // TongueOut: clamped to 1.0

        let packet = make_packet(FLAG_MOUTH | FLAG_EYE, &f);
        module.parse_packet(&packet, &mut data);

        assert!(approx(data.eye.left.pupil_diameter_mm, 0.0));
        assert!(approx(data.eye.right.pupil_diameter_mm, 1.0));
        assert!(approx(data.eye.left.openness, 1.0));
        assert!(approx(w(&data, EyeSquintLeft), 1.0));
        assert!(approx(w(&data, JawOpen), 0.0));
        assert!(approx(w(&data, TongueOut), 1.0));
    }

    #[test]
    fn udp_round_trip_binary_packet() {
        let mut module = CympleModule::new();
        let addr = module.bind_test_socket();

        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();

        let mut f = [0.0f32; 70];
        f[0] = 0.3; // left gaze y
        f[1] = -0.4; // right gaze y
        f[2] = 0.5; // left gaze x
        f[3] = -0.6; // right gaze x
        f[4] = 0.7; // left pupil diameter
        f[5] = 0.8; // right pupil diameter
        f[6] = 0.2; // left lid close -> openness = 0.8
        f[7] = 0.9; // right lid close -> openness = 0.1
        f[8] = 0.11; // EyeSquintLeft
        f[31] = 0.75; // JawOpen
        f[61] = 0.42; // TongueOut

        let packet = make_packet(FLAG_MOUTH | FLAG_EYE, &f);
        sender.send_to(&packet, addr).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));

        let mut data = UnifiedTrackingData::default();
        module.update(&mut data).unwrap();

        // Eye gaze
        assert!(approx(data.eye.left.gaze.y, 0.3));
        assert!(approx(data.eye.right.gaze.y, -0.4));
        assert!(approx(data.eye.left.gaze.x, 0.5));
        assert!(approx(data.eye.right.gaze.x, -0.6));
        // Pupil diameter
        assert!(approx(data.eye.left.pupil_diameter_mm, 0.7));
        assert!(approx(data.eye.right.pupil_diameter_mm, 0.8));
        // Openness
        assert!(approx(data.eye.left.openness, 0.8));
        assert!(approx(data.eye.right.openness, 0.1));
        // Expressions
        assert!(approx(w(&data, EyeSquintLeft), 0.11));
        assert!(approx(w(&data, JawOpen), 0.75));
        assert!(approx(w(&data, TongueOut), 0.42));
    }

    #[test]
    fn udp_undersized_packet_ignored() {
        let mut module = CympleModule::new();
        let addr = module.bind_test_socket();

        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();

        // Send a packet that is too small (100 bytes instead of 292).
        let short_packet = vec![0u8; 100];
        sender.send_to(&short_packet, addr).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));

        let mut data = UnifiedTrackingData::default();
        module.update(&mut data).unwrap();

        // Everything should remain at defaults.
        assert!(approx(data.eye.left.gaze.x, 0.0));
        assert!(approx(data.eye.left.gaze.y, 0.0));
        assert!(approx(data.eye.left.openness, 0.0));
        assert!(approx(w(&data, JawOpen), 0.0));
        assert!(approx(w(&data, TongueOut), 0.0));
    }

    #[test]
    fn udp_no_data_no_error() {
        let mut module = CympleModule::new();
        let _addr = module.bind_test_socket();

        // Don't send anything; just call update on an empty socket.
        let mut data = UnifiedTrackingData::default();
        module.update(&mut data).unwrap();

        // No error, and all data remains at defaults.
        assert!(approx(data.eye.left.gaze.x, 0.0));
        assert!(approx(data.eye.left.gaze.y, 0.0));
        assert!(approx(data.eye.left.openness, 0.0));
        assert!(approx(w(&data, JawOpen), 0.0));
        assert!(approx(w(&data, TongueOut), 0.0));
    }
}
