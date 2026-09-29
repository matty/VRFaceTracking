//! Independent per-eye gaze from the Quest Pro's native visual-axis detector.
//!
//! The companion APK swaps Meta's eye model for one that publishes each eye's
//! own prediction, traces the detector's per-eye visual-axis vectors and
//! streams them as `QPGAZE1` packets. This module converts them to angles,
//! applies a per-eye calibration and independent filters, and overrides the
//! tracking module's gaze while samples are fresh. Ported from
//! Qpro-Enhanced-FT's `independent_visual_axis_runtime.py` and
//! `eye_signal_filter.py`.
use crate::settings::{EyeOffsets, QuestProSettings, SettingsStore};
use log::{info, warn};
use serde::Deserialize;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use vrft_api::UnifiedTrackingData;
pub use vrft_quest_pro_protocol::{EyeSample, EyeStatus};

pub const GAZE_PACKET_BYTES: usize = 64;
pub const GAZE_FRESH_FOR: Duration = Duration::from_millis(250);
const RECENTER_WINDOW: Duration = Duration::from_millis(1000);
const RECENTER_MIN_SAMPLES: usize = 30;
const FILTER_RESET_GAP_S: f64 = 0.5;
const FLAG_TAG0_VALID: u32 = 1;
const FLAG_TAG1_VALID: u32 = 2;
const FLAG_MODEL_PATCHED: u32 = 4;
const BUNDLED_CALIBRATION: &str = "models/quest-pro/qpro-independent-visual-axis-v2.json";
const PERSONAL_CALIBRATION: &str = ".local/eye-calibration.json";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GazePacket {
    pub sequence: u64,
    pub kernel_ns: u64,
    pub flags: u32,
    pub engine_profile: u32,
    pub tag0: [f32; 3],
    pub tag1: [f32; 3],
}

impl GazePacket {
    pub fn parse(bytes: &[u8; GAZE_PACKET_BYTES]) -> Result<Self, String> {
        let u32_at =
            |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let u64_at =
            |offset: usize| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let f32_at =
            |offset: usize| f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        if &bytes[..8] != b"QPGAZE1\0" || u32_at(8) != 1 || u32_at(12) != GAZE_PACKET_BYTES as u32 {
            return Err("invalid QPGAZE1 header".into());
        }
        Ok(Self {
            sequence: u64_at(16),
            kernel_ns: u64_at(24),
            flags: u32_at(32),
            engine_profile: u32_at(36),
            tag0: [f32_at(40), f32_at(44), f32_at(48)],
            tag1: [f32_at(52), f32_at(56), f32_at(60)],
        })
    }

    fn model_patched(&self) -> bool {
        self.flags & FLAG_MODEL_PATCHED != 0
    }

    fn valid(&self) -> bool {
        self.flags & (FLAG_TAG0_VALID | FLAG_TAG1_VALID) == FLAG_TAG0_VALID | FLAG_TAG1_VALID
            && self
                .tag0
                .iter()
                .chain(self.tag1.iter())
                .all(|value| value.is_finite())
    }
}

/// Detector vector to (yaw, pitch) in degrees, as the reference's
/// `vector_yaw_pitch`.
pub fn vector_yaw_pitch(vector: [f32; 3]) -> [f64; 2] {
    let [x, y, z] = vector.map(f64::from);
    [x.atan2(z).to_degrees(), (-y).atan2(x.hypot(z)).to_degrees()]
}

#[derive(Debug, Clone, Deserialize)]
struct TagMapping {
    #[serde(default = "default_left_tag")]
    physical_left: String,
    #[serde(default = "default_right_tag")]
    physical_right: String,
}

fn default_left_tag() -> String {
    "trace_tag_0".into()
}

fn default_right_tag() -> String {
    "trace_tag_1".into()
}

impl Default for TagMapping {
    fn default() -> Self {
        Self {
            physical_left: default_left_tag(),
            physical_right: default_right_tag(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct EyeMapping {
    coefficients: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, Deserialize)]
struct QualityGate {
    #[serde(default)]
    gaze_pass: bool,
    #[serde(default)]
    convergence_pass: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct CalibrationFile {
    format: String,
    #[serde(default)]
    detector_tag_mapping: TagMapping,
    left: EyeMapping,
    right: EyeMapping,
    quality_gate: QualityGate,
}

/// A per-eye affine map from detector yaw/pitch to target yaw/pitch, in the
/// reference's `qpro-independent-personalized-visual-axis-v1/v2` format.
#[derive(Debug, Clone)]
pub struct Calibration {
    left_from_tag1: bool,
    left: [[f64; 2]; 3],
    right: [[f64; 2]; 3],
    pub source: String,
    pub convergence_pass: bool,
}

impl Calibration {
    pub fn from_json(bytes: &[u8], source: String) -> Result<Self, String> {
        let file: CalibrationFile = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid calibration: {error}"))?;
        if !matches!(
            file.format.as_str(),
            "qpro-independent-personalized-visual-axis-v1"
                | "qpro-independent-personalized-visual-axis-v2"
        ) {
            return Err(format!("unsupported calibration format {}", file.format));
        }
        if !file.quality_gate.gaze_pass {
            return Err("the calibration did not pass its absolute-gaze gate".into());
        }
        let left_from_tag1 = match (
            file.detector_tag_mapping.physical_left.as_str(),
            file.detector_tag_mapping.physical_right.as_str(),
        ) {
            ("trace_tag_0", "trace_tag_1") => false,
            ("trace_tag_1", "trace_tag_0") => true,
            _ => return Err("unsupported detector tag mapping".into()),
        };
        Ok(Self {
            left_from_tag1,
            left: coefficients(&file.left, "left")?,
            right: coefficients(&file.right, "right")?,
            source,
            convergence_pass: file.quality_gate.convergence_pass,
        })
    }

    /// Physical (left, right) target angles in degrees for one packet.
    fn map(&self, packet: &GazePacket) -> ([f64; 2], [f64; 2], [f64; 2], [f64; 2]) {
        let tag0 = vector_yaw_pitch(packet.tag0);
        let tag1 = vector_yaw_pitch(packet.tag1);
        let (left_raw, right_raw) = if self.left_from_tag1 {
            (tag1, tag0)
        } else {
            (tag0, tag1)
        };
        (
            affine(&self.left, left_raw),
            affine(&self.right, right_raw),
            tag0,
            tag1,
        )
    }
}

fn coefficients(mapping: &EyeMapping, eye: &str) -> Result<[[f64; 2]; 3], String> {
    let rows = &mapping.coefficients;
    if rows.len() != 3
        || rows
            .iter()
            .any(|row| row.len() != 2 || row.iter().any(|value| !value.is_finite()))
    {
        return Err(format!("the {eye} calibration coefficients are invalid"));
    }
    Ok([
        [rows[0][0], rows[0][1]],
        [rows[1][0], rows[1][1]],
        [rows[2][0], rows[2][1]],
    ])
}

fn affine(coefficients: &[[f64; 2]; 3], [yaw, pitch]: [f64; 2]) -> [f64; 2] {
    [0, 1].map(|output| {
        coefficients[0][output] + yaw * coefficients[1][output] + pitch * coefficients[2][output]
    })
}

/// Loads `$VRFT_EYE_CALIBRATION`, then `.local/eye-calibration.json`, then
/// the bundled developer demonstration profile.
pub fn load_calibration(root: &Path) -> Result<Calibration, String> {
    let explicit = std::env::var_os("VRFT_EYE_CALIBRATION").map(PathBuf::from);
    let path = explicit.clone().unwrap_or_else(|| {
        let personal = root.join(PERSONAL_CALIBRATION);
        if personal.is_file() {
            personal
        } else {
            root.join(BUNDLED_CALIBRATION)
        }
    });
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("eye calibration {} unavailable: {error}", path.display()))?;
    Calibration::from_json(&bytes, path.display().to_string())
}

/// Timestamp-aware One Euro filter for a yaw/pitch pair.
#[derive(Debug, Clone)]
struct OneEuro {
    min_cutoff_hz: f64,
    beta: f64,
    derivative_cutoff_hz: f64,
    value: Option<[f64; 2]>,
    derivative: [f64; 2],
    time_s: f64,
}

fn smoothing_alpha(cutoff_hz: f64, elapsed_s: f64) -> f64 {
    let rate = 2.0 * std::f64::consts::PI * cutoff_hz * elapsed_s;
    rate / (rate + 1.0)
}

impl OneEuro {
    fn new(min_cutoff_hz: f64, beta: f64, derivative_cutoff_hz: f64) -> Self {
        Self {
            min_cutoff_hz,
            beta,
            derivative_cutoff_hz,
            value: None,
            derivative: [0.0; 2],
            time_s: 0.0,
        }
    }

    fn update(&mut self, current: [f64; 2], time_s: f64) -> [f64; 2] {
        let Some(mut value) = self.value else {
            self.value = Some(current);
            self.time_s = time_s;
            return current;
        };
        let elapsed = time_s - self.time_s;
        self.time_s = time_s;
        if !elapsed.is_finite() || elapsed <= 1e-6 {
            return value;
        }
        let elapsed = elapsed.min(0.25);
        let derivative_alpha = smoothing_alpha(self.derivative_cutoff_hz, elapsed);
        for axis in 0..2 {
            let raw_derivative = (current[axis] - value[axis]) / elapsed;
            self.derivative[axis] += derivative_alpha * (raw_derivative - self.derivative[axis]);
            let cutoff = self.min_cutoff_hz + self.beta * self.derivative[axis].abs();
            value[axis] += smoothing_alpha(cutoff, elapsed) * (current[axis] - value[axis]);
        }
        self.value = Some(value);
        value
    }
}

/// Three-sample median followed by a One Euro filter, one per eye so a
/// disturbance in one eye cannot drag the other.
#[derive(Debug, Clone)]
struct EyeFilter {
    history: VecDeque<[f64; 2]>,
    euro: OneEuro,
}

impl EyeFilter {
    fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(3),
            euro: OneEuro::new(4.0, 0.15, 1.5),
        }
    }

    fn update(&mut self, value: [f64; 2], time_s: f64) -> [f64; 2] {
        if self.history.len() == 3 {
            self.history.pop_front();
        }
        self.history.push_back(value);
        let median = [0, 1].map(|axis| {
            let mut values: Vec<f64> = self.history.iter().map(|sample| sample[axis]).collect();
            values.sort_by(f64::total_cmp);
            if values.len() % 2 == 1 {
                values[values.len() / 2]
            } else {
                (values[values.len() / 2 - 1] + values[values.len() / 2]) / 2.0
            }
        });
        self.euro.update(median, time_s)
    }
}

#[derive(Default)]
struct EyeShared {
    /// The latest sample, and when it arrived.
    latest: Option<(Instant, EyeSample)>,
    /// Calibrated but not yet recentered or filtered, for recentering.
    recent: VecDeque<(Instant, [f64; 2], [f64; 2])>,
    rate_hz: f32,
    dropped_invalid: u64,
}

#[derive(Clone)]
pub struct EyeState {
    shared: Arc<RwLock<EyeShared>>,
    calibration: Result<Arc<Calibration>, String>,
}

impl EyeState {
    pub fn new(calibration: Result<Calibration, String>) -> Self {
        match &calibration {
            Ok(calibration) => info!("Quest Pro eyes: calibration {}", calibration.source),
            Err(error) => warn!("Quest Pro eyes: independent gaze disabled: {error}"),
        }
        Self {
            shared: Arc::new(RwLock::new(EyeShared::default())),
            calibration: calibration.map(Arc::new),
        }
    }

    pub fn processor(&self, settings: SettingsStore) -> EyeProcessor {
        EyeProcessor {
            state: self.clone(),
            settings,
            left: EyeFilter::new(),
            right: EyeFilter::new(),
            last_time_s: None,
            rate_started: Instant::now(),
            rate_count: 0,
        }
    }

    pub fn latest_fresh(&self) -> Option<EyeSample> {
        self.shared
            .read()
            .unwrap()
            .latest
            .clone()
            .filter(|(received_at, _)| received_at.elapsed() <= GAZE_FRESH_FOR)
            .map(|(_, sample)| sample)
    }

    pub fn status(&self) -> EyeStatus {
        let shared = self.shared.read().unwrap();
        let age = shared
            .latest
            .as_ref()
            .map(|(received_at, _)| received_at.elapsed());
        let (calibration, calibration_error, convergence_calibrated) = match &self.calibration {
            Ok(calibration) => (
                calibration.source.clone(),
                None,
                calibration.convergence_pass,
            ),
            Err(error) => (String::new(), Some(error.clone()), false),
        };
        EyeStatus {
            calibration,
            calibration_error,
            convergence_calibrated,
            rate_hz: shared.rate_hz,
            age_ms: age.map(|age| age.as_millis() as u64),
            fresh: age.is_some_and(|age| age <= GAZE_FRESH_FOR),
            dropped_invalid: shared.dropped_invalid,
            sample: shared.latest.as_ref().map(|(_, sample)| sample.clone()),
            // The camera snapshots' pupils are added where they are measured.
            pupils: Default::default(),
        }
    }

    /// Offsets that make the last second of gaze read straight ahead.
    pub fn recenter_offsets(&self) -> Result<EyeOffsets, String> {
        let shared = self.shared.read().unwrap();
        let window: Vec<_> = shared
            .recent
            .iter()
            .filter(|(at, _, _)| at.elapsed() <= RECENTER_WINDOW)
            .collect();
        if window.len() < RECENTER_MIN_SAMPLES {
            return Err(format!(
                "Need a live eye stream: only {} samples in the last second",
                window.len()
            ));
        }
        let median = |values: Vec<f64>| {
            let mut values = values;
            values.sort_by(f64::total_cmp);
            values[values.len() / 2]
        };
        let axis = |left: bool, axis: usize| {
            median(
                window
                    .iter()
                    .map(|(_, l, r)| if left { l[axis] } else { r[axis] })
                    .collect(),
            )
        };
        let offsets = EyeOffsets {
            left_deg: [axis(true, 0), axis(true, 1)],
            right_deg: [axis(false, 0), axis(false, 1)],
        };
        if offsets
            .left_deg
            .iter()
            .chain(offsets.right_deg.iter())
            .any(|value| value.abs() > 30.0)
        {
            return Err(
                "Gaze is more than 30 degrees off centre; look straight ahead and retry".into(),
            );
        }
        Ok(offsets)
    }
}

/// Owned by the camera receive thread; turns packets into [`EyeSample`]s.
pub struct EyeProcessor {
    state: EyeState,
    settings: SettingsStore,
    left: EyeFilter,
    right: EyeFilter,
    last_time_s: Option<f64>,
    rate_started: Instant,
    rate_count: u32,
}

impl EyeProcessor {
    pub fn process(&mut self, packet: &GazePacket, received_at: Instant) {
        let Ok(calibration) = self.state.calibration.clone() else {
            return;
        };
        if !packet.valid() {
            self.state.shared.write().unwrap().dropped_invalid += 1;
            return;
        }
        let time_s = packet.kernel_ns as f64 / 1e9;
        if self
            .last_time_s
            .is_none_or(|last| !(0.0..=FILTER_RESET_GAP_S).contains(&(time_s - last)))
        {
            self.left = EyeFilter::new();
            self.right = EyeFilter::new();
        }
        self.last_time_s = Some(time_s);
        let (left_calibrated, right_calibrated, tag0_deg, tag1_deg) = calibration.map(packet);
        let settings = self.settings.get();
        let offsets = settings.eye_offsets;
        let recentered = |value: [f64; 2], offset: Option<[f64; 2]>| {
            let offset = offset.unwrap_or([0.0; 2]);
            [value[0] - offset[0], value[1] - offset[1]]
        };
        let left_deg = self.left.update(
            recentered(left_calibrated, offsets.map(|o| o.left_deg)),
            time_s,
        );
        let right_deg = self.right.update(
            recentered(right_calibrated, offsets.map(|o| o.right_deg)),
            time_s,
        );
        self.rate_count += 1;
        let mut shared = self.state.shared.write().unwrap();
        let elapsed = self.rate_started.elapsed();
        if elapsed >= Duration::from_secs(1) {
            shared.rate_hz = self.rate_count as f32 / elapsed.as_secs_f32();
            self.rate_count = 0;
            self.rate_started = Instant::now();
        }
        shared
            .recent
            .push_back((received_at, left_calibrated, right_calibrated));
        while shared
            .recent
            .front()
            .is_some_and(|(at, _, _)| at.elapsed() > RECENTER_WINDOW)
        {
            shared.recent.pop_front();
        }
        shared.latest = Some((
            received_at,
            EyeSample {
                sequence: packet.sequence,
                engine_profile: packet.engine_profile,
                model_patched: packet.model_patched(),
                tag0_deg,
                tag1_deg,
                left_deg,
                right_deg,
            },
        ));
    }
}

/// VRFT (left, right) gaze vectors in radians for one sample, following the
/// reference's publication rule: calibrated yaw and pitch go straight to x and
/// y, and by default the physical eyes cross over to the opposite channel.
pub fn output_gaze(sample: &EyeSample, settings: &QuestProSettings) -> ([f32; 2], [f32; 2]) {
    let convert = |[yaw, pitch]: [f64; 2]| {
        let yaw = if settings.eye_invert_yaw { -yaw } else { yaw };
        [yaw.to_radians() as f32, pitch.to_radians() as f32]
    };
    let left = convert(sample.left_deg);
    let right = convert(sample.right_deg);
    if settings.eye_swap_output {
        (right, left)
    } else {
        (left, right)
    }
}

pub struct EyeOverlay {
    state: EyeState,
    settings: SettingsStore,
    active: bool,
}

impl EyeOverlay {
    pub fn new(state: EyeState, settings: SettingsStore) -> Self {
        Self {
            state,
            settings,
            active: false,
        }
    }

    /// Whether independent gaze is on and fresh samples are arriving.
    pub fn is_live(&self) -> bool {
        self.settings.get().eye_gaze && self.state.latest_fresh().is_some()
    }

    pub fn apply(&mut self, data: &mut UnifiedTrackingData) {
        let settings = self.settings.get();
        let sample = settings
            .eye_gaze
            .then(|| self.state.latest_fresh())
            .flatten();
        let Some(sample) = sample else {
            if self.active {
                info!("Quest Pro eyes: independent gaze stale or off; using tracking module gaze");
                self.active = false;
            }
            return;
        };
        if !self.active {
            info!(
                "Quest Pro eyes: independent gaze active (engine profile {}, model patched: {})",
                sample.engine_profile, sample.model_patched
            );
            self.active = true;
        }
        let (left, right) = output_gaze(&sample, &settings);
        data.eye.left.gaze.x = left[0];
        data.eye.left.gaze.y = left[1];
        data.eye.right.gaze.x = right[0];
        data.eye.right.gaze.y = right[1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE_V2: &str =
        include_str!("../../../../models/quest-pro/qpro-independent-visual-axis-v2.json");

    fn packet_bytes(
        sequence: u64,
        kernel_ns: u64,
        flags: u32,
        tag0: [f32; 3],
        tag1: [f32; 3],
    ) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[..8].copy_from_slice(b"QPGAZE1\0");
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&64u32.to_le_bytes());
        bytes[16..24].copy_from_slice(&sequence.to_le_bytes());
        bytes[24..32].copy_from_slice(&kernel_ns.to_le_bytes());
        bytes[32..36].copy_from_slice(&flags.to_le_bytes());
        bytes[36..40].copy_from_slice(&2u32.to_le_bytes());
        for (index, value) in tag0.iter().chain(tag1.iter()).enumerate() {
            bytes[40 + index * 4..44 + index * 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn direction(yaw_deg: f64, pitch_deg: f64) -> [f32; 3] {
        let (yaw, pitch) = (yaw_deg.to_radians(), pitch_deg.to_radians());
        [
            (yaw.sin() * pitch.cos()) as f32,
            (-pitch.sin()) as f32,
            (yaw.cos() * pitch.cos()) as f32,
        ]
    }

    fn identity_calibration() -> Calibration {
        Calibration::from_json(
            br#"{"format":"qpro-independent-personalized-visual-axis-v2",
                 "detector_tag_mapping":{"physical_left":"trace_tag_1","physical_right":"trace_tag_0"},
                 "left":{"coefficients":[[0,0],[1,0],[0,1]]},
                 "right":{"coefficients":[[0,0],[1,0],[0,1]]},
                 "quality_gate":{"gaze_pass":true,"convergence_pass":false}}"#,
            "test".into(),
        )
        .unwrap()
    }

    fn processor(calibration: Calibration, settings: QuestProSettings) -> (EyeState, EyeProcessor) {
        let state = EyeState::new(Ok(calibration));
        let processor = state.processor(SettingsStore::in_memory(settings));
        (state, processor)
    }

    #[test]
    fn packet_round_trips_and_rejects_bad_headers() {
        let bytes = packet_bytes(7, 1_500_000_000, 7, [0.1, 0.2, 0.9], [-0.1, 0.0, 1.0]);
        let packet = GazePacket::parse(&bytes).unwrap();
        assert_eq!(packet.sequence, 7);
        assert_eq!(packet.kernel_ns, 1_500_000_000);
        assert_eq!(packet.engine_profile, 2);
        assert!(packet.model_patched() && packet.valid());
        assert_eq!(packet.tag1, [-0.1, 0.0, 1.0]);
        let mut wrong = bytes;
        wrong[8] = 2;
        assert!(GazePacket::parse(&wrong).is_err());
    }

    #[test]
    fn vector_angles_match_the_reference_convention() {
        let [yaw, pitch] = vector_yaw_pitch(direction(20.0, 10.0));
        assert!((yaw - 20.0).abs() < 1e-4 && (pitch - 10.0).abs() < 1e-4);
        assert_eq!(vector_yaw_pitch([0.0, 0.0, 1.0]), [0.0, 0.0]);
    }

    #[test]
    fn reference_profile_loads_and_matches_its_python_mapping() {
        let calibration =
            Calibration::from_json(REFERENCE_V2.as_bytes(), "bundled".into()).unwrap();
        assert!(calibration.left_from_tag1);
        assert!(!calibration.convergence_pass);
        // numpy: [1, 10, -5] @ left coefficients from the reference profile.
        let [yaw, pitch] = affine(&calibration.left, [10.0, -5.0]);
        let c = calibration.left;
        assert!((yaw - (c[0][0] + 10.0 * c[1][0] - 5.0 * c[2][0])).abs() < 1e-12);
        assert!((pitch - (c[0][1] + 10.0 * c[1][1] - 5.0 * c[2][1])).abs() < 1e-12);
    }

    #[test]
    fn calibration_without_a_gaze_pass_is_refused() {
        let text = REFERENCE_V2.replace("\"gaze_pass\": true", "\"gaze_pass\": false");
        assert!(Calibration::from_json(text.as_bytes(), "x".into()).is_err());
    }

    #[test]
    fn tag_mapping_and_default_crossover_follow_the_reference() {
        let (state, mut processor) = processor(identity_calibration(), QuestProSettings::default());
        // Tag 1 is the physical left eye in the reference profile.
        let packet = GazePacket::parse(&packet_bytes(
            1,
            1_000_000_000,
            7,
            direction(-4.0, 1.0),
            direction(6.0, -2.0),
        ))
        .unwrap();
        processor.process(&packet, Instant::now());
        let sample = state.latest_fresh().unwrap();
        assert!((sample.left_deg[0] - 6.0).abs() < 1e-4, "{sample:?}");
        assert!((sample.right_deg[0] + 4.0).abs() < 1e-4, "{sample:?}");
        let (left, right) = output_gaze(&sample, &QuestProSettings::default());
        assert!(
            (left[0] - (-4.0f32).to_radians()).abs() < 1e-5,
            "VRFaceTracking left gets physical right"
        );
        assert!((right[0] - 6.0f32.to_radians()).abs() < 1e-5);
        assert!((right[1] - (-2.0f32).to_radians()).abs() < 1e-5);
        let unswapped = QuestProSettings {
            eye_swap_output: false,
            eye_invert_yaw: true,
            ..Default::default()
        };
        let (left, _) = output_gaze(&sample, &unswapped);
        assert!((left[0] - (-6.0f32).to_radians()).abs() < 1e-5);
    }

    #[test]
    fn eyes_are_filtered_independently_and_spikes_are_rejected() {
        let (state, mut processor) = processor(identity_calibration(), QuestProSettings::default());
        let mut time_ns = 1_000_000_000;
        let mut send = |tag0: [f32; 3], tag1: [f32; 3]| {
            time_ns += 11_111_111;
            let packet = GazePacket::parse(&packet_bytes(1, time_ns, 7, tag0, tag1)).unwrap();
            processor.process(&packet, Instant::now());
            state.latest_fresh().unwrap()
        };
        for _ in 0..30 {
            send(direction(0.0, 0.0), direction(0.0, 0.0));
        }
        // A single-sample spike in one eye is removed by the median.
        let sample = send(direction(25.0, 0.0), direction(0.0, 0.0));
        assert!(sample.right_deg[0].abs() < 1e-6, "{sample:?}");
        // A sustained move in one eye moves only that eye.
        let mut sample = sample;
        for _ in 0..60 {
            sample = send(direction(0.0, 0.0), direction(8.0, 0.0));
        }
        assert!((sample.left_deg[0] - 8.0).abs() < 0.2, "{sample:?}");
        assert!(sample.right_deg[0].abs() < 1e-6, "{sample:?}");
    }

    #[test]
    fn invalid_or_non_finite_packets_are_dropped() {
        let (state, mut processor) = processor(identity_calibration(), QuestProSettings::default());
        let packet =
            GazePacket::parse(&packet_bytes(1, 1, 5, [0.0, 0.0, 1.0], [0.0, 0.0, 1.0])).unwrap();
        processor.process(&packet, Instant::now());
        let packet = GazePacket::parse(&packet_bytes(
            2,
            2,
            7,
            [f32::NAN, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ))
        .unwrap();
        processor.process(&packet, Instant::now());
        assert!(state.latest_fresh().is_none());
        assert_eq!(state.status().dropped_invalid, 2);
    }

    #[test]
    fn recenter_offsets_zero_the_current_gaze() {
        let (state, mut processor) = processor(identity_calibration(), QuestProSettings::default());
        assert!(state.recenter_offsets().is_err());
        for index in 0..40 {
            let packet = GazePacket::parse(&packet_bytes(
                index,
                1_000_000_000 + index * 11_000_000,
                7,
                direction(-2.0, 1.5),
                direction(3.0, -1.0),
            ))
            .unwrap();
            processor.process(&packet, Instant::now());
        }
        let offsets = state.recenter_offsets().unwrap();
        assert!(
            (offsets.left_deg[0] - 3.0).abs() < 1e-4 && (offsets.left_deg[1] + 1.0).abs() < 1e-4
        );
        assert!(
            (offsets.right_deg[0] + 2.0).abs() < 1e-4 && (offsets.right_deg[1] - 1.5).abs() < 1e-4
        );
    }

    #[test]
    fn overlay_overrides_only_fresh_gaze() {
        let settings = SettingsStore::in_memory(QuestProSettings::default());
        let state = EyeState::new(Ok(identity_calibration()));
        let mut processor = state.processor(settings.clone());
        let mut overlay = EyeOverlay::new(state.clone(), settings);
        let mut data = UnifiedTrackingData::default();
        data.eye.left.gaze.x = 0.3;
        data.eye.left.openness = 0.8;
        overlay.apply(&mut data);
        assert_eq!(data.eye.left.gaze.x, 0.3, "no samples keeps module gaze");
        let packet = GazePacket::parse(&packet_bytes(
            1,
            1,
            7,
            direction(-5.0, 0.0),
            direction(5.0, 0.0),
        ))
        .unwrap();
        processor.process(&packet, Instant::now());
        overlay.apply(&mut data);
        assert!((data.eye.left.gaze.x - (-5.0f32).to_radians()).abs() < 1e-5);
        assert!((data.eye.right.gaze.x - 5.0f32.to_radians()).abs() < 1e-5);
        assert_eq!(
            data.eye.left.openness, 0.8,
            "openness stays from the module"
        );
        processor.process(&packet, Instant::now() - Duration::from_secs(1));
        data.eye.left.gaze.x = 0.3;
        overlay.apply(&mut data);
        assert_eq!(data.eye.left.gaze.x, 0.3, "stale samples keep module gaze");
    }
}
