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
