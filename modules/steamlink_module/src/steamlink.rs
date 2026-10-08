use anyhow::Result;
use std::net::UdpSocket;
use vrft_api::{ModuleLogger, TrackingModule, UnifiedExpressions, UnifiedTrackingData};

const DEFAULT_PORT: u16 = 9015;

pub struct SteamLinkModule {
    socket: Option<UdpSocket>,
    logger: Option<ModuleLogger>,
    buf: Vec<u8>,
}

impl SteamLinkModule {
    pub fn new() -> Self {
        Self {
            socket: None,
            logger: None,
            buf: vec![0u8; 8192],
        }
    }

    fn parse_osc_and_apply(&self, packet: &[u8], data: &mut UnifiedTrackingData) -> Result<()> {
        let (_, msg) = rosc::decoder::decode_udp(packet)?;
        match msg {
            rosc::OscPacket::Message(msg) => {
                self.apply_message(&msg, data);
            }
            rosc::OscPacket::Bundle(bundle) => {
                for content in &bundle.content {
                    if let rosc::OscPacket::Message(msg) = content {
                        self.apply_message(msg, data);
                    }
                }
            }
        }
        Ok(())
    }

    fn apply_message(&self, msg: &rosc::OscMessage, data: &mut UnifiedTrackingData) {
        let addr = msg.addr.as_str();

        if addr == "/sl/eyeTrackedGazePoint" {
            if msg.args.len() >= 3 {
                let x = osc_float(&msg.args[0]).unwrap_or(0.0);
                let y = osc_float(&msg.args[1]).unwrap_or(0.0);
                let z = osc_float(&msg.args[2]).unwrap_or(0.0);
                if z.abs() > f32::EPSILON {
                    let gaze_x = (x / -z).atan();
                    let gaze_y = (y / -z).atan();
                    data.eye.left.gaze.x = gaze_x;
                    data.eye.left.gaze.y = gaze_y;
                    data.eye.right.gaze.x = gaze_x;
                    data.eye.right.gaze.y = gaze_y;
                }
            }
            return;
        }

        let value = match msg.args.first() {
            Some(rosc::OscType::Float(v)) => *v,
            Some(rosc::OscType::Bool(v)) => {
                if *v {
                    1.0
                } else {
                    0.0
                }
            }
            _ => return,
        };

        if addr == "/sl/xrfb/facew/EyesClosedL" {
            let squint = data.shapes[UnifiedExpressions::EyeSquintLeft as usize].weight;
            data.eye.left.openness = (1.0 - (value + value * squint).clamp(0.0, 1.0)).max(0.0);
            return;
        }
        if addr == "/sl/xrfb/facew/EyesClosedR" {
            let squint = data.shapes[UnifiedExpressions::EyeSquintRight as usize].weight;
            data.eye.right.openness = (1.0 - (value + value * squint).clamp(0.0, 1.0)).max(0.0);
            return;
        }

        if let Some(name) = addr.strip_prefix("/sl/xrfb/facew/") {
            if let Some(targets) = face_to_unified(name) {
                for &expr in targets {
                    data.shapes[expr as usize].weight = value;
                }
            }
        }
    }
}

fn osc_float(arg: &rosc::OscType) -> Option<f32> {
    match arg {
        rosc::OscType::Float(v) => Some(*v),
        _ => None,
    }
}

fn face_to_unified(name: &str) -> Option<&'static [UnifiedExpressions]> {
    use UnifiedExpressions::*;
    let targets: &[UnifiedExpressions] = match name {
        "UpperLidRaiserL" => &[EyeWideLeft],
        "UpperLidRaiserR" => &[EyeWideRight],
        "LidTightenerL" => &[EyeSquintLeft],
        "LidTightenerR" => &[EyeSquintRight],
        "InnerBrowRaiserL" => &[BrowInnerUpLeft],
        "InnerBrowRaiserR" => &[BrowInnerUpRight],
        "OuterBrowRaiserL" => &[BrowOuterUpLeft],
        "OuterBrowRaiserR" => &[BrowOuterUpRight],
        "BrowLowererL" => &[BrowPinchLeft, BrowLowererLeft],
        "BrowLowererR" => &[BrowPinchRight, BrowLowererRight],
        "JawDrop" => &[JawOpen],
        "JawSidewaysLeft" => &[JawLeft],
        "JawSidewaysRight" => &[JawRight],
        "JawThrust" => &[JawForward],
        "MouthLeft" => &[MouthLowerLeft, MouthUpperLeft],
        "MouthRight" => &[MouthLowerRight, MouthUpperRight],
        "ChinRaiserT" => &[MouthRaiserUpper],
        "ChinRaiserB" => &[MouthRaiserLower],
        "DimplerL" => &[MouthDimpleLeft],
        "DimplerR" => &[MouthDimpleRight],
        "LipsToward" => &[MouthClosed],
        "LipCornerPullerL" => &[MouthCornerPullLeft, MouthCornerSlantLeft],
        "LipCornerPullerR" => &[MouthCornerPullRight, MouthCornerSlantRight],
        "LipCornerDepressorL" => &[MouthFrownLeft],
        "LipCornerDepressorR" => &[MouthFrownRight],
        "LowerLipDepressorL" => &[MouthLowerDownLeft],
        "LowerLipDepressorR" => &[MouthLowerDownRight],
        "UpperLipRaiserL" => &[MouthUpperUpLeft],
        "UpperLipRaiserR" => &[MouthUpperUpRight],
        "LipTightenerL" => &[MouthTightenerLeft],
        "LipTightenerR" => &[MouthTightenerRight],
        "LipPressorL" => &[MouthPressLeft],
        "LipPressorR" => &[MouthPressRight],
        "LipStretcherL" => &[MouthStretchLeft],
        "LipStretcherR" => &[MouthStretchRight],
        "LipPuckerL" => &[LipPuckerLowerLeft, LipPuckerUpperLeft],
        "LipPuckerR" => &[LipPuckerLowerRight, LipPuckerUpperRight],
        "LipFunnelerLB" => &[LipFunnelLowerLeft],
        "LipFunnelerLT" => &[LipFunnelUpperLeft],
        "LipFunnelerRB" => &[LipFunnelLowerRight],
        "LipFunnelerRT" => &[LipFunnelUpperRight],
        "LipSuckLB" => &[LipSuckLowerLeft],
        "LipSuckLT" => &[LipSuckUpperLeft],
        "LipSuckRB" => &[LipSuckLowerRight],
        "LipSuckRT" => &[LipSuckUpperRight],
        "CheekPuffL" => &[CheekPuffLeft],
        "CheekPuffR" => &[CheekPuffRight],
        "CheekSuckL" => &[CheekSuckLeft],
        "CheekSuckR" => &[CheekSuckRight],
        "CheekRaiserL" => &[CheekSquintLeft],
        "CheekRaiserR" => &[CheekSquintRight],
        "NoseWrinklerL" => &[NoseSneerLeft],
        "NoseWrinklerR" => &[NoseSneerRight],
        "TongueOut" => &[TongueOut],
        "TongueRetreat" => &[TongueBendDown],
        "TongueTipAlveolar" => &[TongueCurlUp],
        _ => return None,
    };
    Some(targets)
}

impl TrackingModule for SteamLinkModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing SteamLink Module");
        let socket = UdpSocket::bind(format!("127.0.0.1:{DEFAULT_PORT}"))?;
        socket.set_nonblocking(true)?;
        self.socket = Some(socket);
        logger.info(&format!(
            "Listening for SteamLink OSC on port {DEFAULT_PORT}"
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
                    let _ = self.parse_osc_and_apply(&self.buf[..n], data);
                    received = true;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }

        if !received {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        Ok(())
    }

    fn unload(&mut self) {
        self.socket.take();
        if let Some(logger) = &self.logger {
            logger.info("SteamLink Module unloaded");
        }
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(SteamLinkModule::new())
}
