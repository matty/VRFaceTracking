//! Client for the daemon's local HTTP API, served by `vrft_d` on port 27275.
//!
//! The wire types mirror the daemon's `/status` response. Every field has a
//! default, so an older or newer daemon still parses and the app shows what it
//! can instead of failing.
use anyhow::{bail, Context as _, Result};
use serde::Deserialize;
use std::time::Duration;

pub const DEFAULT_ADDRESS: &str = "127.0.0.1:27275";

/// Longest the app waits for one response. The daemon is on this machine, so
/// anything slower means it is stuck or gone.
const TIMEOUT: Duration = Duration::from_millis(800);

pub struct DaemonClient {
    agent: ureq::Agent,
    address: String,
}

impl DaemonClient {
    pub fn new(address: impl Into<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent,
            address: address.into(),
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn status(&self) -> Result<Status> {
        let mut response = self
            .agent
            .get(format!("http://{}/status", self.address))
            .call()
            .with_context(|| format!("Can't reach VRFT at {}", self.address))?;
        if !response.status().is_success() {
            bail!("VRFT answered {}", response.status());
        }
        response
            .body_mut()
            .read_json()
            .context("VRFT sent a status this app doesn't understand")
    }

    /// The latest frame from `camera`, or `None` before the first one arrives
    /// or when it is still `shown`, whose pixels then aren't downloaded.
    pub fn frame(&self, camera: Camera, shown: Option<u64>) -> Result<Option<Frame>> {
        let path = match camera {
            Camera::Mouth => "frame",
            Camera::Eyes => "eye-frame",
        };
        let mut response = self
            .agent
            .get(format!("http://{}/{path}", self.address))
            .call()
            .with_context(|| format!("Can't reach VRFT at {}", self.address))?;
        if response.status() == 204 {
            return Ok(None);
        }
        if !response.status().is_success() {
            bail!("VRFT answered {}", response.status());
        }
        let sequence = response
            .headers()
            .get("x-frame-sequence")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
            .context("Camera frame has no sequence number")?;
        if shown == Some(sequence) {
            return Ok(None);
        }
        let pixels = response.body_mut().read_to_vec()?;
        Frame::new(sequence, pixels).map(Some)
    }

    /// Changes the given Quest Pro settings and returns all of them.
    pub fn update_settings(&self, patch: serde_json::Value) -> Result<Settings> {
        self.post("settings", Some(patch))
    }

    /// Takes the last second of gaze as looking straight ahead.
    pub fn recenter_eyes(&self) -> Result<Settings> {
        self.post("eye/recenter", None)
    }

    pub fn clear_eye_recenter(&self) -> Result<Settings> {
        self.post("eye/recenter/clear", None)
    }

    /// Asks the daemon to shut down, as Ctrl-C in its console would.
    pub fn shutdown(&self) -> Result<()> {
        let response = self
            .agent
            .post(format!("http://{}/shutdown", self.address))
            .send_json(serde_json::json!({ "requested_by": "the desktop app" }))
            .with_context(|| format!("Can't reach VRFT at {}", self.address))?;
        match response.status().as_u16() {
            200..=299 => Ok(()),
            404 => {
                bail!("This vrft_d can't be stopped from the app. Update it, or close its window.")
            }
            status => bail!("VRFT answered {status}"),
        }
    }

    fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let request = self.agent.post(format!("http://{}/{path}", self.address));
        let mut response = match body {
            Some(body) => request.send_json(body),
            None => request.send_empty(),
        }
        .with_context(|| format!("Can't reach VRFT at {}", self.address))?;
        if !response.status().is_success() {
            // The daemon explains refusals in plain text.
            let reason = response.body_mut().read_to_string().unwrap_or_default();
            if reason.is_empty() {
                bail!("VRFT answered {}", response.status());
            }
            bail!(reason);
        }
        response
            .body_mut()
            .read_json()
            .context("VRFT sent a reply this app doesn't understand")
    }
}

/// The two stereo camera pairs the daemon serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Camera {
    /// The lower-face pair, streamed at the headset's camera frame rate.
    Mouth,
    /// The eye pair, sent as occasional snapshots.
    Eyes,
}

/// Width and height of the side-by-side stereo mouth camera image.
pub const FRAME_WIDTH: u32 = 800;
pub const FRAME_HEIGHT: u32 = 400;

/// One 8-bit grayscale stereo frame.
pub struct Frame {
    pub sequence: u64,
    pub pixels: Vec<u8>,
}

impl Frame {
    pub fn new(sequence: u64, pixels: Vec<u8>) -> Result<Self> {
        let expected = (FRAME_WIDTH * FRAME_HEIGHT) as usize;
        if pixels.len() != expected {
            bail!(
                "Camera frame has {} bytes; expected {expected}",
                pixels.len()
            );
        }
        Ok(Self { sequence, pixels })
    }

    /// The frame as BGRA, the pixel order GPUI images use.
    pub fn to_bgra(&self) -> Vec<u8> {
        let mut bgra = Vec::with_capacity(self.pixels.len() * 4);
        for &gray in &self.pixels {
            bgra.extend_from_slice(&[gray, gray, gray, u8::MAX]);
        }
        bgra
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Status {
    /// The daemon's own description of the headset connection.
    pub status: String,
    /// Address of the headset stream, once connected.
    pub source: Option<String>,
    pub sequence: Option<u64>,
    pub frame_age_ms: Option<u64>,
    pub headset: Option<Headset>,
    /// Set while the headset app speaks a stream protocol the daemon can't read.
    pub headset_mismatch: Option<HeadsetMismatch>,
    pub model_error: Option<String>,
    pub model: Option<TongueModel>,
    pub output: Option<TongueOutput>,
    pub eye_frame_sequence: Option<u64>,
    pub eye_frame_age_ms: Option<u64>,
    pub eyes: Eyes,
    /// Yaw and pitch in degrees that VRFT sends for its left and right eye,
    /// while independent gaze is live.
    pub eye_output_deg: Option<[[f32; 2]; 2]>,
    pub settings: Settings,
    /// Missing from daemons built before the desktop app existed.
    pub daemon: Option<DaemonReport>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Headset {
    pub apk_version: Option<String>,
    pub camera_fps: Option<u32>,
    pub eye: Option<HeadsetEye>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HeadsetMismatch {
    pub apk_version: Option<String>,
    pub update: Update,
}

/// Which side needs updating before the daemon can use the headset app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Update {
    #[default]
    Vrft,
    HeadsetApp,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HeadsetEye {
    pub enabled: bool,
    pub state: String,
    pub message: String,
    pub model_patched: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TongueModel {
    pub sequence: u64,
    pub age_ms: u64,
    pub fresh: bool,
    pub inference_ms: f32,
    pub skipped_frames: u64,
    /// Visibility, extension, horizontal, vertical, then six shape heads.
    pub values: [f32; 10],
    pub threshold: f32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TongueOutput {
    /// `enhanced model` or `tracking module`.
    pub source: String,
    pub native_tongue_out: f32,
    pub visible: Option<bool>,
    /// TongueOut, Up, Down, Left, Right, then seven shapes.
    pub values: [f32; 12],
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Eyes {
    /// Path of the calibration file in use.
    pub calibration: String,
    pub calibration_error: Option<String>,
    pub convergence_calibrated: bool,
    pub rate_hz: f32,
    pub fresh: bool,
    pub sample: Option<EyeSample>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EyeSample {
    pub engine_profile: u32,
    /// Whether the headset runs the per-eye model rather than Meta's stock one.
    pub model_patched: bool,
}

/// The daemon's Quest Pro settings, saved in `.local/quest-pro-settings.json`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub eye_gaze: bool,
    pub eye_swap_output: bool,
    pub eye_invert_yaw: bool,
    pub eye_offsets: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DaemonReport {
    pub version: String,
    pub mode: RunMode,
    pub module: Option<ModuleStatus>,
    pub output: Option<OutputTarget>,
    pub tracking_frames: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Normal,
    CameraPreviewOnly,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ModuleStatus {
    pub name: String,
    pub runtime: Option<String>,
    pub loaded: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct OutputTarget {
    pub mode: String,
    pub address: String,
    pub port: u16,
    pub max_fps: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_status() {
        let json = r#"{
            "status": "Streaming from 192.168.1.20:27274",
            "source": "192.168.1.20:27274",
            "sequence": 812,
            "width": 800, "height": 400,
            "frame_age_ms": 31,
            "headset": {"apk_version": "0.2", "camera_fps": 24, "eye_preview_fps": 2,
                        "eye": {"enabled": true, "state": "running", "message": "", "engine_profile": 2,
                                "model_patched": true, "restore_pending": false}},
            "model_error": null,
            "model": {"sequence": 811, "age_ms": 40, "fresh": true, "inference_ms": 6.5,
                      "skipped_frames": 3, "raw_values": [0,0,0,0,0,0,0,0,0,0],
                      "values": [0.9,0.8,0.1,-0.2,0,0,0,0,0,0], "camera_weight": 0.7, "threshold": 0.5},
            "output": {"source": "enhanced model", "native_tongue_out": 0.4,
                       "fused_visibility": 0.8, "visible": true, "values": [0.9,0,0.2,0,0.1,0,0,0,0,0,0,0]},
            "eyes": {"calibration": "x.json", "calibration_error": null, "convergence_calibrated": false,
                     "rate_hz": 88.0, "age_ms": 5, "fresh": true, "dropped_invalid": 0,
                     "sample": {"sequence": 5, "engine_profile": 2, "model_patched": true,
                                "tag0_deg": [0, 0], "tag1_deg": [0, 0], "left_deg": [1, 2], "right_deg": [3, 4]}},
            "eye_output_deg": [[1.5, -2.0], [-1.0, -2.0]],
            "settings": {"tongue_smoothing": 55, "tongue_visibility": "weighted", "eye_gaze": true,
                         "eye_swap_output": true, "eye_invert_yaw": false,
                         "eye_offsets": {"left_deg": [0.5, 0.0], "right_deg": [0.0, 0.0]}},
            "daemon": {"version": "0.1.0", "mode": "normal",
                       "module": {"name": "vd_module.dll", "runtime": "native", "loaded": true, "error": null},
                       "output": {"mode": "VRChat", "address": "127.0.0.1", "port": 9000, "max_fps": 60.0},
                       "tracking_frames": 5120}
        }"#;
        let status: Status = serde_json::from_str(json).unwrap();
        assert_eq!(status.frame_age_ms, Some(31));
        assert_eq!(status.headset.unwrap().camera_fps, Some(24));
        assert!(status.model.unwrap().fresh);
        assert_eq!(status.output.unwrap().values[0], 0.9);
        let daemon = status.daemon.unwrap();
        assert_eq!(daemon.mode, RunMode::Normal);
        assert_eq!(daemon.module.unwrap().runtime.as_deref(), Some("native"));
        assert_eq!(daemon.output.unwrap().port, 9000);
        assert!(status.eye_output_deg.is_some());
        assert!(status.settings.eye_gaze && status.settings.eye_offsets.is_some());
        assert!(status.eyes.sample.unwrap().model_patched);
    }

    #[test]
    fn parses_an_older_daemon_without_the_daemon_report() {
        let status: Status = serde_json::from_str(r#"{"status": "Looking for the headset"}"#)
            .expect("missing fields fall back to defaults");
        assert!(status.daemon.is_none());
        assert!(status.source.is_none());
        assert!(!status.eyes.fresh);
    }

    #[test]
    fn parses_a_headset_protocol_mismatch() {
        let status: Status = serde_json::from_str(
            r#"{"headset_mismatch": {"protocol": 4, "apk_version": "2027.1.0", "update": "vrft"}}"#,
        )
        .unwrap();
        let mismatch = status.headset_mismatch.unwrap();
        assert_eq!(mismatch.update, Update::Vrft);
        assert_eq!(mismatch.apk_version.as_deref(), Some("2027.1.0"));
    }

    #[test]
    fn frame_rejects_the_wrong_size_and_expands_to_bgra() {
        assert!(Frame::new(1, vec![0; 10]).is_err());
        let mut pixels = vec![0; (FRAME_WIDTH * FRAME_HEIGHT) as usize];
        pixels[0] = 200;
        let frame = Frame::new(7, pixels).unwrap();
        let bgra = frame.to_bgra();
        assert_eq!(bgra.len(), frame.pixels.len() * 4);
        assert_eq!(&bgra[..4], &[200, 200, 200, 255]);
    }
}
