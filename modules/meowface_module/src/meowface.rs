use anyhow::Result;
use serde::Deserialize;
use std::net::UdpSocket;
use vrft_api::{ModuleLogger, TrackingModule, UnifiedExpressions, UnifiedTrackingData};

const DEFAULT_PORT: u16 = 12345;
const DEG_TO_RAD: f32 = std::f32::consts::PI / 180.0;

pub struct MeowFaceModule {
    socket: Option<UdpSocket>,
    logger: Option<ModuleLogger>,
    buf: Vec<u8>,
}

impl MeowFaceModule {
    pub fn new() -> Self {
        Self {
            socket: None,
            logger: None,
            buf: vec![0u8; 4096],
        }
    }

    fn parse_packet(&self, packet: &[u8], data: &mut UnifiedTrackingData) {
        let Ok(mf) = serde_json::from_slice::<MeowFaceData>(packet) else {
            return;
        };

        if !mf.face_found {
            return;
        }

        self.apply_eyes(&mf, data);
        self.apply_shapes(&mf.blend_shapes, data);
    }

    fn apply_eyes(&self, mf: &MeowFaceData, data: &mut UnifiedTrackingData) {
        data.eye.left.gaze.x = mf.eye_left.y * DEG_TO_RAD;
        data.eye.left.gaze.y = -mf.eye_left.x * DEG_TO_RAD;
        data.eye.right.gaze.x = mf.eye_right.y * DEG_TO_RAD;
        data.eye.right.gaze.y = -mf.eye_right.x * DEG_TO_RAD;

        let blink_l = get_shape(&mf.blend_shapes, "eyeBlinkLeft");
        let blink_r = get_shape(&mf.blend_shapes, "eyeBlinkRight");
        let squint_l = get_shape(&mf.blend_shapes, "eyeSquintLeft");
        let squint_r = get_shape(&mf.blend_shapes, "eyeSquintRight");

        data.eye.left.openness =
            1.0 - (blink_l + blink_l.powf(0.33) * squint_l.powf(1.25)).min(1.0);
        data.eye.right.openness =
            1.0 - (blink_r + blink_r.powf(0.33) * squint_r.powf(1.25)).min(1.0);

        data.eye.left.pupil_diameter_mm = 5.0;
        data.eye.right.pupil_diameter_mm = 5.0;
        data.eye.min_dilation = 0.0;
        data.eye.max_dilation = 10.0;
    }

    fn apply_shapes(&self, shapes: &[MeowShape], data: &mut UnifiedTrackingData) {
        use UnifiedExpressions::*;

        let g = |name: &str| get_shape(shapes, name);

        // Eye expressions
        set(data, EyeSquintLeft, g("eyeSquintLeft"));
        set(data, EyeSquintRight, g("eyeSquintRight"));
        set(data, EyeWideLeft, g("eyeWideLeft"));
        set(data, EyeWideRight, g("eyeWideRight"));

        // Brows
        let brow_down_l = g("browDownLeft");
        set(data, BrowPinchLeft, brow_down_l);
        set(data, BrowLowererLeft, brow_down_l);
        let brow_down_r = g("browDownRight");
        set(data, BrowPinchRight, brow_down_r);
        set(data, BrowLowererRight, brow_down_r);
        set(data, BrowInnerUpLeft, g("browInnerUpLeft"));
        set(data, BrowInnerUpRight, g("browInnerUpRight"));
        set(data, BrowOuterUpLeft, g("browOuterUpLeft"));
        set(data, BrowOuterUpRight, g("browOuterUpRight"));

        // Jaw
        set(data, JawOpen, g("jawOpen"));
        set(data, JawLeft, g("jawLeft"));
        set(data, JawRight, g("jawRight"));

        // Mouth direction
        let mouth_r = g("mouthRight");
        set(data, MouthUpperRight, mouth_r);
        set(data, MouthLowerRight, mouth_r);
        let mouth_l = g("mouthLeft");
        set(data, MouthUpperLeft, mouth_l);
        set(data, MouthLowerLeft, mouth_l);

        // Upper lip
        let upper_up_r = g("mouthUpperUpRight");
        set(data, MouthUpperUpRight, upper_up_r);
        set(data, MouthUpperDeepenRight, upper_up_r);
        let upper_up_l = g("mouthUpperUpLeft");
        set(data, MouthUpperUpLeft, upper_up_l);
        set(data, MouthUpperDeepenLeft, upper_up_l);

        set(data, MouthLowerDownRight, g("mouthLowerDownRight"));
        set(data, MouthLowerDownLeft, g("mouthLowerDownLeft"));

        // Pucker (single value -> 4)
        let pucker = g("mouthPucker");
        set(data, LipPuckerUpperRight, pucker);
        set(data, LipPuckerUpperLeft, pucker);
        set(data, LipPuckerLowerRight, pucker);
        set(data, LipPuckerLowerLeft, pucker);

        set(data, NoseSneerRight, g("noseSneerRight"));
        set(data, NoseSneerLeft, g("noseSneerLeft"));

        // Smile -> corner pull + slant + cheek squint
        let smile_r = g("mouthSmileRight");
        set(data, MouthCornerPullRight, smile_r);
        set(data, MouthCornerSlantRight, smile_r);
        set(data, CheekSquintRight, smile_r);
        let smile_l = g("mouthSmileLeft");
        set(data, MouthCornerPullLeft, smile_l);
        set(data, MouthCornerSlantLeft, smile_l);
        set(data, CheekSquintLeft, smile_l);

        set(data, MouthFrownRight, g("mouthFrownRight"));
        set(data, MouthFrownLeft, g("mouthFrownLeft"));

        // Funnel (single value -> 4)
        let funnel = g("mouthFunnel");
        set(data, LipFunnelLowerLeft, funnel);
        set(data, LipFunnelLowerRight, funnel);
        set(data, LipFunnelUpperLeft, funnel);
        set(data, LipFunnelUpperRight, funnel);

        // Roll -> lip suck
        let roll_upper = g("mouthRollUpper");
        set(data, LipSuckUpperRight, roll_upper);
        set(data, LipSuckUpperLeft, roll_upper);
        let roll_lower = g("mouthRollLower");
        set(data, LipSuckLowerRight, roll_lower);
        set(data, LipSuckLowerLeft, roll_lower);

        // Shrug -> raiser
        let shrug_upper = g("mouthShrugUpper");
        set(data, MouthRaiserUpper, shrug_upper);
        set(data, MouthRaiserLower, shrug_upper);

        set(data, TongueOut, g("tongueOut"));

        // Simulated expressions
        set(data, MouthDimpleRight, smile_r * 0.5);
        set(data, MouthDimpleLeft, smile_l * 0.5);
        let frown_r = g("mouthFrownRight");
        let frown_l = g("mouthFrownLeft");
        set(data, MouthStretchRight, frown_r * 0.5);
        set(data, MouthStretchLeft, frown_l * 0.5);
    }
}

fn get_shape(shapes: &[MeowShape], name: &str) -> f32 {
    shapes
        .iter()
        .find(|s| s.k.eq_ignore_ascii_case(name))
        .map_or(0.0, |s| s.v)
}

fn set(data: &mut UnifiedTrackingData, expr: UnifiedExpressions, value: f32) {
    data.shapes[expr as usize].weight = value;
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct MeowFaceData {
    #[allow(dead_code)]
    timestamp: Option<i64>,
    #[allow(dead_code)]
    hotkey: Option<i32>,
    face_found: bool,
    #[serde(default)]
    eye_left: MeowVector,
    #[serde(default)]
    eye_right: MeowVector,
    #[serde(default, rename = "BlendShapes")]
    blend_shapes: Vec<MeowShape>,
}

#[derive(Deserialize, Default)]
struct MeowVector {
    x: f32,
    y: f32,
    #[allow(dead_code)]
    z: f32,
}

#[derive(Deserialize)]
struct MeowShape {
    k: String,
    v: f32,
}

impl TrackingModule for MeowFaceModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing MeowFace Module");
        let socket = UdpSocket::bind(format!("0.0.0.0:{DEFAULT_PORT}"))?;
        socket.set_nonblocking(true)?;
        self.socket = Some(socket);
        logger.info(&format!("Listening for MeowFace on port {DEFAULT_PORT}"));
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
            logger.info("MeowFace Module unloaded");
        }
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(MeowFaceModule::new())
}
