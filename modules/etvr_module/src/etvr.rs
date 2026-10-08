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
