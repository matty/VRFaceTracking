use anyhow::Result;
use std::net::UdpSocket;
use vrft_api::{ModuleLogger, TrackingModule, UnifiedExpressions, UnifiedTrackingData};

const DEFAULT_PORT: u16 = 8888;

pub struct BabbleModule {
    socket: Option<UdpSocket>,
    logger: Option<ModuleLogger>,
    buf: Vec<u8>,
}

impl BabbleModule {
    pub fn new() -> Self {
        Self {
            socket: None,
            logger: None,
            buf: vec![0u8; 4096],
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

    fn parse_osc_and_apply(&self, packet: &[u8], data: &mut UnifiedTrackingData) -> Result<()> {
        let (_, msg) = rosc::decoder::decode_udp(packet)?;
        match msg {
            rosc::OscPacket::Message(msg) => {
                apply_message(&msg, data);
            }
            rosc::OscPacket::Bundle(bundle) => {
                for content in &bundle.content {
                    if let rosc::OscPacket::Message(msg) = content {
                        apply_message(msg, data);
                    }
                }
            }
        }
        Ok(())
    }
}

fn apply_message(msg: &rosc::OscMessage, data: &mut UnifiedTrackingData) {
    let value = match msg.args.first() {
        Some(rosc::OscType::Float(v)) => *v,
        _ => return,
    };

    let addr = msg.addr.as_str();

    use UnifiedExpressions::*;
    match addr {
        "/cheekPuffLeft" => set(data, CheekPuffLeft, value),
        "/cheekPuffRight" => set(data, CheekPuffRight, value),
        "/cheekSuckLeft" => set(data, CheekSuckLeft, value),
        "/cheekSuckRight" => set(data, CheekSuckRight, value),
        "/jawOpen" => set(data, JawOpen, value),
        "/jawForward" => set(data, JawForward, value),
        "/jawLeft" => set(data, JawLeft, value),
        "/jawRight" => set(data, JawRight, value),
        "/noseSneerLeft" => set(data, NoseSneerLeft, value),
        "/noseSneerRight" => set(data, NoseSneerRight, value),
        "/mouthFunnel" => {
            let v = value * 4.0;
            set(data, LipFunnelLowerLeft, v);
            set(data, LipFunnelLowerRight, v);
            set(data, LipFunnelUpperLeft, v);
            set(data, LipFunnelUpperRight, v);
        }
        "/mouthPucker" => {
            let v = value * 4.0;
            set(data, LipPuckerLowerLeft, v);
            set(data, LipPuckerLowerRight, v);
            set(data, LipPuckerUpperLeft, v);
            set(data, LipPuckerUpperRight, v);
        }
        "/mouthLeft" => {
            let v = value * 2.0;
            set(data, MouthUpperLeft, v);
            set(data, MouthLowerLeft, v);
        }
        "/mouthRight" => {
            let v = value * 2.0;
            set(data, MouthUpperRight, v);
            set(data, MouthLowerRight, v);
        }
        "/mouthRollUpper" => {
            set(data, LipSuckUpperLeft, value);
            set(data, LipSuckUpperRight, value);
        }
        "/mouthRollLower" => {
            set(data, LipSuckLowerLeft, value);
            set(data, LipSuckLowerRight, value);
        }
        "/mouthShrugUpper" => set(data, MouthRaiserUpper, value),
        "/mouthShrugLower" => set(data, MouthRaiserLower, value),
        "/mouthClose" => set(data, MouthClosed, value),
        "/mouthSmileLeft" => set(data, MouthCornerPullLeft, value),
        "/mouthSmileRight" => set(data, MouthCornerPullRight, value),
        "/mouthFrownLeft" => set(data, MouthFrownLeft, value),
        "/mouthFrownRight" => set(data, MouthFrownRight, value),
        "/mouthDimpleLeft" => set(data, MouthDimpleLeft, value),
        "/mouthDimpleRight" => set(data, MouthDimpleRight, value),
        "/mouthUpperUpLeft" => set(data, MouthUpperUpLeft, value),
        "/mouthUpperUpRight" => set(data, MouthUpperUpRight, value),
        "/mouthLowerDownLeft" => set(data, MouthLowerDownLeft, value),
        "/mouthLowerDownRight" => set(data, MouthLowerDownRight, value),
        "/mouthPressLeft" => set(data, MouthPressLeft, value),
        "/mouthPressRight" => set(data, MouthPressRight, value),
        "/mouthStretchLeft" => set(data, MouthStretchLeft, value),
        "/mouthStretchRight" => set(data, MouthStretchRight, value),
        "/tongueOut" => set(data, TongueOut, value),
        "/tongueUp" => set(data, TongueUp, value),
        "/tongueDown" => set(data, TongueDown, value),
        "/tongueLeft" => set(data, TongueLeft, value),
        "/tongueRight" => set(data, TongueRight, value),
        "/tongueRoll" => set(data, TongueRoll, value),
        "/tongueBendDown" => set(data, TongueBendDown, value),
        "/tongueCurlUp" => set(data, TongueCurlUp, value),
        "/tongueSquish" => set(data, TongueSquish, value),
        "/tongueFlat" => set(data, TongueFlat, value),
        "/tongueTwistLeft" => set(data, TongueTwistLeft, value),
        "/tongueTwistRight" => set(data, TongueTwistRight, value),
        _ => {}
    }
}

fn set(data: &mut UnifiedTrackingData, expr: UnifiedExpressions, value: f32) {
    data.shapes[expr as usize].weight = value;
}

impl TrackingModule for BabbleModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing Babble Module");
        let socket = UdpSocket::bind(format!("127.0.0.1:{DEFAULT_PORT}"))?;
        socket.set_nonblocking(true)?;
        self.socket = Some(socket);
        logger.info(&format!("Listening for Babble OSC on port {DEFAULT_PORT}"));
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
                    let _ = self.parse_osc_and_apply(&self.buf[..n], data);
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
            logger.info("Babble Module unloaded");
        }
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(BabbleModule::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrft_api::UnifiedExpressions::*;

    fn osc_msg(addr: &str, value: f32) -> rosc::OscMessage {
        rosc::OscMessage {
            addr: String::from(addr),
            args: vec![rosc::OscType::Float(value)],
        }
    }

    fn weight(data: &UnifiedTrackingData, expr: UnifiedExpressions) -> f32 {
        data.shapes[expr as usize].weight
    }

    #[test]
    fn jaw_open_one_to_one() {
        let mut data = UnifiedTrackingData::default();
        apply_message(&osc_msg("/jawOpen", 0.75), &mut data);
        assert!((weight(&data, JawOpen) - 0.75).abs() < 1e-5);
    }

    #[test]
    fn mouth_funnel_multiplier_fan_out() {
        let mut data = UnifiedTrackingData::default();
        apply_message(&osc_msg("/mouthFunnel", 0.2), &mut data);
        let expected = 0.2 * 4.0;
        assert!((weight(&data, LipFunnelLowerLeft) - expected).abs() < 1e-5);
        assert!((weight(&data, LipFunnelLowerRight) - expected).abs() < 1e-5);
        assert!((weight(&data, LipFunnelUpperLeft) - expected).abs() < 1e-5);
        assert!((weight(&data, LipFunnelUpperRight) - expected).abs() < 1e-5);
    }

    #[test]
    fn mouth_pucker_multiplier_fan_out() {
        let mut data = UnifiedTrackingData::default();
        apply_message(&osc_msg("/mouthPucker", 0.15), &mut data);
        let expected = 0.15 * 4.0;
        assert!((weight(&data, LipPuckerLowerLeft) - expected).abs() < 1e-5);
        assert!((weight(&data, LipPuckerLowerRight) - expected).abs() < 1e-5);
        assert!((weight(&data, LipPuckerUpperLeft) - expected).abs() < 1e-5);
        assert!((weight(&data, LipPuckerUpperRight) - expected).abs() < 1e-5);
    }

    #[test]
    fn mouth_left_multiplier() {
        let mut data = UnifiedTrackingData::default();
        apply_message(&osc_msg("/mouthLeft", 0.3), &mut data);
        let expected = 0.3 * 2.0;
        assert!((weight(&data, MouthUpperLeft) - expected).abs() < 1e-5);
        assert!((weight(&data, MouthLowerLeft) - expected).abs() < 1e-5);
    }

    #[test]
    fn mouth_right_multiplier() {
        let mut data = UnifiedTrackingData::default();
        apply_message(&osc_msg("/mouthRight", 0.4), &mut data);
        let expected = 0.4 * 2.0;
        assert!((weight(&data, MouthUpperRight) - expected).abs() < 1e-5);
        assert!((weight(&data, MouthLowerRight) - expected).abs() < 1e-5);
    }

    #[test]
    fn mouth_roll_upper_to_lip_suck() {
        let mut data = UnifiedTrackingData::default();
        apply_message(&osc_msg("/mouthRollUpper", 0.6), &mut data);
        assert!((weight(&data, LipSuckUpperLeft) - 0.6).abs() < 1e-5);
        assert!((weight(&data, LipSuckUpperRight) - 0.6).abs() < 1e-5);
    }

    #[test]
    fn tongue_out() {
        let mut data = UnifiedTrackingData::default();
        apply_message(&osc_msg("/tongueOut", 0.9), &mut data);
        assert!((weight(&data, TongueOut) - 0.9).abs() < 1e-5);
    }

    #[test]
    fn unknown_address_no_change() {
        let mut data = UnifiedTrackingData::default();
        let before = data.clone();
        apply_message(&osc_msg("/unknownParam", 1.0), &mut data);
        assert_eq!(data, before);
    }

    #[test]
    fn non_float_arg_ignored() {
        let mut data = UnifiedTrackingData::default();
        let before = data.clone();
        let msg = rosc::OscMessage {
            addr: String::from("/jawOpen"),
            args: vec![rosc::OscType::Int(1)],
        };
        apply_message(&msg, &mut data);
        assert_eq!(data, before);
    }

    #[test]
    fn udp_round_trip_osc_message() {
        let mut module = BabbleModule::new();
        let addr = module.bind_test_socket();

        let msg = rosc::OscMessage {
            addr: String::from("/jawOpen"),
            args: vec![rosc::OscType::Float(0.75)],
        };
        let packet = rosc::encoder::encode(&rosc::OscPacket::Message(msg)).unwrap();

        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sender.send_to(&packet, addr).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));

        let mut data = UnifiedTrackingData::default();
        module.update(&mut data).unwrap();

        assert!((weight(&data, JawOpen) - 0.75).abs() < 1e-5);
    }

    #[test]
    fn udp_round_trip_osc_bundle() {
        let mut module = BabbleModule::new();
        let addr = module.bind_test_socket();

        let bundle = rosc::OscBundle {
            timetag: rosc::OscTime {
                seconds: 0,
                fractional: 0,
            },
            content: vec![
                rosc::OscPacket::Message(rosc::OscMessage {
                    addr: String::from("/jawOpen"),
                    args: vec![rosc::OscType::Float(0.5)],
                }),
                rosc::OscPacket::Message(rosc::OscMessage {
                    addr: String::from("/tongueOut"),
                    args: vec![rosc::OscType::Float(0.8)],
                }),
            ],
        };
        let packet = rosc::encoder::encode(&rosc::OscPacket::Bundle(bundle)).unwrap();

        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sender.send_to(&packet, addr).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));

        let mut data = UnifiedTrackingData::default();
        module.update(&mut data).unwrap();

        assert!((weight(&data, JawOpen) - 0.5).abs() < 1e-5);
        assert!((weight(&data, TongueOut) - 0.8).abs() < 1e-5);
    }

    #[test]
    fn udp_no_data_no_error() {
        let mut module = BabbleModule::new();
        module.bind_test_socket();

        let mut data = UnifiedTrackingData::default();
        let before = data.clone();

        module.update(&mut data).unwrap();

        assert_eq!(data, before);
    }
}
