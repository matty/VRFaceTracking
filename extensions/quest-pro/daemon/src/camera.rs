use crate::capture::{CaptureManager, CaptureStatus};
use crate::eye::{output_gaze, EyeOverlay, EyeProcessor, EyeState, GazePacket, GAZE_PACKET_BYTES};
use crate::pupil::{PupilOverlay, PupilProcessor, PupilState};
use crate::settings::{QuestProSettings, SettingsPatch, SettingsStore, VisibilityMode};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{extract::State, Json, Router};
use log::{debug, info, warn};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use vrft_api::{UnifiedExpressions, UnifiedTrackingData};
use vrft_extension::FrameHook;
use vrft_quest_pro_protocol::{
    routes, CameraLayout, CaptureCommand, CaptureRequest, FaceModelStatus, FaceValue, Headset,
    Mismatch, ModelStatus, OutputStatus, PupilMark, Status, TongueSource, Update, BROW_CAMERA,
    EYE_CAMERAS, FRAME_HEIGHT, FRAME_SEQUENCE_HEADER, FRAME_WIDTH, MOUTH_CAMERAS, PUPILS_HEADER,
    STRIP_BYTES, STRIP_WIDTH, VIEW_BYTES,
};
use vrft_tongue::universal::{Enrollment, FaceModel, FACE_TARGETS};
use vrft_tongue::{Accelerator, Role, TongueModel, CHEEK_COLUMNS};

const SERVICE_TYPE: &str = "_vrftcam._tcp.local.";
/// How often a headset app that couldn't be read is tried again while mDNS
/// is quiet.
const MISMATCH_RECHECK: Duration = Duration::from_secs(30);
/// Wait before the first retry of a headset that dropped or can't be reached;
/// it doubles with each failure in a row, up to [`RETRY_MAX`].
const RETRY_FIRST: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);
/// A connection that lasted this long was healthy, so losing it starts the
/// backoff over.
const RETRY_RESET_AFTER: Duration = Duration::from_secs(30);
const WIDTH: usize = FRAME_WIDTH as usize;
const HEIGHT: usize = FRAME_HEIGHT as usize;
const FRAME_BYTES: usize = vrft_quest_pro_protocol::FRAME_BYTES;
/// Camera mask of the lower-face pair (cameras 2 and 3) used for the tongue.
const MOUTH_MASK: u32 = 0x0c;
/// Camera mask of the eye pair (cameras 0 and 1), sent as low-rate snapshots.
const EYES_MASK: u32 = 0x03;
/// Camera mask of the whole five-camera strip, which the headset app sends
/// only to a daemon whose [`hello`] asks for it.
const ALL_MASK: u32 = 0x1f;
/// Eye snapshots a second cut from five-camera frames for the pupils, until
/// the headset app says its own snapshot rate.
const DEFAULT_EYE_FPS: u32 = 5;
const STATUS_MAX_BYTES: usize = 16 * 1024;
/// Headset stream protocols this daemon reads. The headset app advertises its
/// protocol over mDNS and in the `QPSTAT1` it sends first on every connection.
/// Only a change to an existing message bumps it: new message types don't,
/// because `read_message` skips `QP` messages it doesn't know. A headset app
/// release serves many VRFT releases, so widen this on a bump rather than
/// raising the start, which would make every user update the headset app.
const PROTOCOLS: RangeInclusive<u32> = 3..=3;
/// What a headset app that predates the `protocol` field speaks.
const LEGACY_PROTOCOL: u32 = 3;
/// Largest unknown message that is skipped rather than taken as a broken stream.
const SKIP_MAX_BYTES: usize = 4 * 1024 * 1024;
const MODEL_HEADS: usize = vrft_quest_pro_protocol::MODEL_HEADS;
const _: () = assert!(vrft_tongue::TARGETS.len() == MODEL_HEADS);
const TONGUE_FRESH_FOR: Duration = Duration::from_millis(250);
const NATIVE_HISTORY: Duration = Duration::from_millis(1000);
/// A module TongueOut further than this from a camera frame isn't used for it.
const NATIVE_FRESH_FOR: Duration = Duration::from_millis(200);
/// Frame rate the reference smoothing slider was tuned at; the slider value is
/// converted to a time constant so smoothing does not depend on camera FPS.
const SMOOTHING_REFERENCE_FPS: f64 = 24.0;
const TONGUE_SHAPES: [UnifiedExpressions; 12] = [
    UnifiedExpressions::TongueOut,
    UnifiedExpressions::TongueUp,
    UnifiedExpressions::TongueDown,
    UnifiedExpressions::TongueLeft,
    UnifiedExpressions::TongueRight,
    UnifiedExpressions::TongueRoll,
    UnifiedExpressions::TongueBendDown,
    UnifiedExpressions::TongueCurlUp,
    UnifiedExpressions::TongueSquish,
    UnifiedExpressions::TongueFlat,
    UnifiedExpressions::TongueTwistLeft,
    UnifiedExpressions::TongueTwistRight,
];

#[derive(Clone)]
struct Frame {
    sequence: u64,
    /// Headset capture timestamp from the relay header, in nanoseconds.
    headset_ns: u64,
    pixels: Arc<[u8]>,
    received_at: Instant,
}

/// One message from the headset stream.
enum Message {
    Mouth(Frame),
    Eyes(Frame),
    /// All five cameras, 2000 x 400.
    Strip(Frame),
    Gaze(GazePacket),
    Status(serde_json::Value),
}

/// What to tell people about a headset app this daemon can't read.
fn mismatch_message(mismatch: &Mismatch) -> String {
    let app = match &mismatch.apk_version {
        Some(version) => format!("The headset app ({version})"),
        None => "The headset app".into(),
    };
    match mismatch.update {
        Update::Vrft => {
            format!("{app} is newer than this VRFaceTracking can read. Update VRFaceTracking")
        }
        Update::HeadsetApp => {
            format!("{app} is too old for this VRFaceTracking. Update the headset app")
        }
    }
}

fn check_protocol(protocol: u32, apk_version: Option<&str>) -> Result<(), Mismatch> {
    let update = if protocol > *PROTOCOLS.end() {
        Update::Vrft
    } else if protocol < *PROTOCOLS.start() {
        Update::HeadsetApp
    } else {
        return Ok(());
    };
    Err(Mismatch {
        protocol,
        apk_version: apk_version.map(str::to_owned),
        update,
    })
}

/// The protocol a `QPSTAT1` status declares.
fn status_protocol(status: &serde_json::Value) -> u32 {
    match status.get("protocol").and_then(serde_json::Value::as_u64) {
        // Too large to hold is still newer than anything this daemon reads.
        Some(protocol) => u32::try_from(protocol).unwrap_or(u32::MAX),
        None => LEGACY_PROTOCOL,
    }
}

/// A headset app found over mDNS.
struct Advert {
    address: SocketAddr,
    protocol: u32,
    apk_version: Option<String>,
}

impl Advert {
    fn from_service(info: &ResolvedService) -> Option<Self> {
        let ip = info
            .get_addresses()
            .iter()
            .map(|ip| ip.to_ip_addr())
            .find(IpAddr::is_ipv4)?;
        Some(Self {
            address: SocketAddr::new(ip, info.get_port()),
            protocol: advertised_protocol(
                info.get_property_val_str("protocol"),
                info.get_property_val_str("version"),
            ),
            apk_version: info.get_property_val_str("apk_version").map(str::to_owned),
        })
    }
}

/// Headset apps before `protocol` advertised the same number as `version`.
fn advertised_protocol(protocol: Option<&str>, version: Option<&str>) -> u32 {
    protocol
        .or(version)
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(LEGACY_PROTOCOL)
}

/// Why a headset connection ended.
#[derive(Debug)]
enum Disconnect {
    /// The headset didn't accept the connection.
    Unreachable(std::io::Error),
    Io(std::io::Error),
    Mismatch(Mismatch),
}

impl From<std::io::Error> for Disconnect {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Outputs of the universal face model.
const FACE_OUTPUTS: usize = FACE_TARGETS.len();

/// What VRFT sends from the universal face model besides the tongue and
/// cheek puffs, by index into `FACE_TARGETS`.
const FACE_SHAPES: [(usize, UnifiedExpressions); 11] = [
    (6, UnifiedExpressions::CheekSuckLeft),
    (7, UnifiedExpressions::CheekSuckRight),
    (8, UnifiedExpressions::JawOpen),
    (9, UnifiedExpressions::BrowInnerUpLeft),
    (10, UnifiedExpressions::BrowInnerUpRight),
    (11, UnifiedExpressions::BrowOuterUpLeft),
    (12, UnifiedExpressions::BrowOuterUpRight),
    (13, UnifiedExpressions::BrowLowererLeft),
    (14, UnifiedExpressions::BrowLowererRight),
    (15, UnifiedExpressions::BrowPinchLeft),
    (16, UnifiedExpressions::BrowPinchRight),
];

/// The universal face model's outputs for a frame, after smoothing.
#[derive(Clone)]
struct FaceReading {
    values: [f32; FACE_OUTPUTS],
    /// Which outputs it was trained for; the others are left to the module.
    enabled: [bool; FACE_OUTPUTS],
}

#[derive(Clone)]
struct TonguePrediction {
    sequence: u64,
    /// Unsmoothed model output, shown in the preview.
    raw: [f32; MODEL_HEADS],
    /// Output after the user's smoothing setting; drives VRFT.
    values: [f32; MODEL_HEADS],
    received_at: Instant,
    inference_ms: f32,
    dropped_frames: u64,
    camera_weight: f32,
    threshold: f32,
    /// Whether the model has learned cheek puffs.
    cheeks: bool,
    /// Set when the universal face model made this prediction.
    face: Option<FaceReading>,
}

type TongueState = Arc<RwLock<Option<TonguePrediction>>>;

#[derive(Clone)]
struct OutputSnapshot {
    source: TongueSource,
    /// The tracking module's TongueOut, when a module is sending frames.
    native_tongue_out: Option<f32>,
    fused_visibility: Option<f32>,
    visible: Option<bool>,
    values: [f32; 12],
    cheek_source: TongueSource,
    cheek_puffs: [f32; 2],
    face: Vec<FaceValue>,
}

type OutputState = Arc<RwLock<Option<OutputSnapshot>>>;

/// Applies enhanced Quest Pro tongue, cheek puff, pupil and independent eye
/// tracking on top of the active tracking module's output.
pub struct QuestProOverlay {
    latest: TongueState,
    output: OutputState,
    capture: CaptureManager,
    settings: SettingsStore,
    eyes: EyeOverlay,
    pupils: PupilOverlay,
    /// Module TongueOut before the daemon's smoothing, with arrival times, so
    /// visibility fusion can use the value closest to each camera frame.
    native_history: VecDeque<(Instant, f32)>,
    active: bool,
    visible_latched: bool,
    last_diagnostic: Instant,
}

impl FrameHook for QuestProOverlay {
    fn module_loaded(&mut self, loaded: bool) {
        self.capture.set_module_loaded(loaded);
    }

    /// Records the tracking module's own values before any daemon filtering.
    fn before_mutation(&mut self, data: &UnifiedTrackingData) {
        let native = data.shapes[UnifiedExpressions::TongueOut as usize]
            .weight
            .clamp(0.0, 1.0);
        self.capture.update_native(native);
        self.pupils.see_eyes(data, &self.settings.get());
        let now = Instant::now();
        self.native_history.push_back((now, native));
        while self
            .native_history
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) > NATIVE_HISTORY)
        {
            self.native_history.pop_front();
        }
    }

    fn after_mutation(&mut self, data: &mut UnifiedTrackingData) {
        self.apply(data);
    }

    /// The headset stream drives output by itself, with or without a
    /// tracking module.
    fn has_live_data(&self) -> bool {
        self.eyes.is_live()
            || self.pupils.is_live()
            || self
                .latest
                .read()
                .unwrap()
                .as_ref()
                .is_some_and(|prediction| prediction.received_at.elapsed() <= TONGUE_FRESH_FOR)
    }
}

impl QuestProOverlay {
    /// Native TongueOut observed closest to `at`, if the tracking module sent
    /// one near then. None without a module, or while it is silent.
    fn native_near(&self, at: Instant) -> Option<f32> {
        let gap = |observed: &Instant| {
            if *observed > at {
                *observed - at
            } else {
                at - *observed
            }
        };
        self.native_history
            .iter()
            .min_by_key(|(observed, _)| gap(observed))
            .filter(|(observed, _)| gap(observed) <= NATIVE_FRESH_FOR)
            .map(|(_, value)| *value)
    }

    fn apply(&mut self, data: &mut UnifiedTrackingData) {
        self.eyes.apply(data);
        let settings = self.settings.get();
        self.pupils.apply(data, &settings);
        let current = self.latest.read().unwrap().clone();
        // With the mouth model off, the headset's own tongue and cheeks go
        // out untouched.
        let fresh = current
            .filter(|prediction| prediction.received_at.elapsed() <= TONGUE_FRESH_FOR)
            .filter(|_| settings.mouth_model);
        let cheek_source = apply_cheeks(data, fresh.as_ref(), &settings);
        let cheek_puffs = CHEEK_SHAPES.map(|shape| data.shapes[shape as usize].weight);
        let face = apply_face(data, fresh.as_ref(), &settings);
        let Some(prediction) = fresh else {
            if self.active {
                if settings.mouth_model {
                    info!("Quest Pro tongue: camera or inference stale; using tracking module tongue values");
                } else {
                    info!("Quest Pro tongue: mouth model turned off; using tracking module tongue values");
                }
                self.active = false;
                self.visible_latched = false;
            }
            *self.output.write().unwrap() = Some(OutputSnapshot {
                source: TongueSource::TrackingModule,
                native_tongue_out: self.native_near(Instant::now()),
                fused_visibility: None,
                visible: None,
                values: tongue_values(data),
                cheek_source,
                cheek_puffs,
                face,
            });
            return;
        };
        if !self.active {
            info!("Quest Pro tongue: fresh model output; overriding 12 tongue expressions");
            self.active = true;
        }
        let native = self.native_near(prediction.received_at);
        let camera = prediction.values[0].clamp(0.0, 1.0);
        let fused = fuse_visibility(
            settings.tongue_visibility,
            prediction.camera_weight,
            camera,
            native,
        );
        self.visible_latched = if self.visible_latched {
            fused >= prediction.threshold - 0.08
        } else {
            fused >= prediction.threshold
        };
        let values = map_tongue(&prediction.values, fused, self.visible_latched);
        for (shape, value) in TONGUE_SHAPES.into_iter().zip(values) {
            data.shapes[shape as usize].weight = value;
        }
        *self.output.write().unwrap() = Some(OutputSnapshot {
            source: TongueSource::EnhancedModel,
            native_tongue_out: native,
            fused_visibility: Some(fused),
            visible: Some(self.visible_latched),
            values,
            cheek_source,
            cheek_puffs,
            face,
        });
        if self.last_diagnostic.elapsed() >= Duration::from_secs(5) {
            info!(
                "Quest Pro tongue: seq={} visible={} mode={:?} camera={:.2} native={:?} fused={:.2} threshold={:.2} infer={:.1}ms age={}ms skipped={} heads={:?}",
                prediction.sequence, self.visible_latched, settings.tongue_visibility, camera,
                native, fused, prediction.threshold, prediction.inference_ms,
                prediction.received_at.elapsed().as_millis(), prediction.dropped_frames,
                prediction.values
            );
            self.last_diagnostic = Instant::now();
        }
    }
}

/// The cheek puffs, in `CHEEK_COLUMNS` order.
const CHEEK_SHAPES: [UnifiedExpressions; 2] = [
    UnifiedExpressions::CheekPuffLeft,
    UnifiedExpressions::CheekPuffRight,
];

/// Sends the model's cheek puffs, with the tongue in or out, once it has
/// learned them; otherwise the tracking module's stay. Says which were sent.
fn apply_cheeks(
    data: &mut UnifiedTrackingData,
    prediction: Option<&TonguePrediction>,
    settings: &QuestProSettings,
) -> TongueSource {
    let Some(prediction) = prediction.filter(|p| p.cheeks && settings.cheek_puffs) else {
        return TongueSource::TrackingModule;
    };
    for (shape, column) in CHEEK_SHAPES.into_iter().zip(CHEEK_COLUMNS) {
        data.shapes[shape as usize].weight = prediction.values[column].clamp(0.0, 1.0);
    }
    TongueSource::EnhancedModel
}

/// Sends the universal face model's cheek suck, jaw and brows while it is
/// fresh and the setting is on, for each output it was trained for; the
/// tracking module's values stay otherwise. Returns what was sent.
fn apply_face(
    data: &mut UnifiedTrackingData,
    prediction: Option<&TonguePrediction>,
    settings: &QuestProSettings,
) -> Vec<FaceValue> {
    let Some(face) = prediction
        .and_then(|prediction| prediction.face.as_ref())
        .filter(|_| settings.face_expressions)
    else {
        return vec![];
    };
    FACE_SHAPES
        .iter()
        .filter(|(index, _)| face.enabled[*index])
        .map(|&(index, shape)| {
            let value = face.values[index].clamp(0.0, 1.0);
            data.shapes[shape as usize].weight = value;
            FaceValue {
                expression: format!("{shape:?}"),
                value,
            }
        })
        .collect()
}

/// Combines camera and native visibility as the reference hub's modes do.
/// Without native tracking (no module, or it is silent) every mode uses the
/// camera alone.
fn fuse_visibility(
    mode: VisibilityMode,
    camera_weight: f32,
    camera: f32,
    native: Option<f32>,
) -> f32 {
    let Some(native) = native else {
        return camera;
    };
    match mode {
        VisibilityMode::Weighted => camera_weight * camera + (1.0 - camera_weight) * native,
        VisibilityMode::Camera => camera,
        VisibilityMode::Native => native,
        VisibilityMode::Agreement => camera.min(native),
    }
}

/// Per-frame blend factor for a smoothing strength of 0..100.
///
/// The reference maps its slider to an exponential moving average factor
/// `1 - 0.88 * s / 100` per frame at its default cadence. This converts that
/// to a time constant so the lag stays the same at any camera FPS.
fn smoothing_alpha(strength: f32, elapsed_s: f64) -> f32 {
    let reference = (1.0 - 0.88 * f64::from(strength.clamp(0.0, 100.0)) / 100.0).clamp(0.05, 1.0);
    if reference >= 1.0 {
        return 1.0;
    }
    let time_constant = -(1.0 / SMOOTHING_REFERENCE_FPS) / (1.0 - reference).ln();
    (1.0 - (-elapsed_s / time_constant).exp()) as f32
}

struct Smoother<const N: usize> {
    value: Option<[f32; N]>,
    last_headset_ns: u64,
    last_received_at: Option<Instant>,
}

impl<const N: usize> Default for Smoother<N> {
    fn default() -> Self {
        Self {
            value: None,
            last_headset_ns: 0,
            last_received_at: None,
        }
    }
}

type TongueSmoother = Smoother<MODEL_HEADS>;

impl<const N: usize> Smoother<N> {
    fn update(
        &mut self,
        raw: [f32; N],
        headset_ns: u64,
        received_at: Instant,
        strength: f32,
    ) -> [f32; N] {
        // Prefer the headset clock; network delivery adds jitter.
        let elapsed = if headset_ns > self.last_headset_ns && self.last_headset_ns != 0 {
            Some((headset_ns - self.last_headset_ns) as f64 / 1e9)
        } else {
            self.last_received_at.map(|previous| {
                received_at
                    .saturating_duration_since(previous)
                    .as_secs_f64()
            })
        };
        self.last_headset_ns = headset_ns;
        self.last_received_at = Some(received_at);
        let value = match (self.value, elapsed) {
            (Some(previous), Some(elapsed)) if elapsed > 0.0 && elapsed <= 0.25 => {
                let alpha = smoothing_alpha(strength, elapsed);
                std::array::from_fn(|index| {
                    previous[index] + alpha * (raw[index] - previous[index])
                })
            }
            _ => raw,
        };
        self.value = Some(value);
        value
    }
}

fn tongue_values(data: &UnifiedTrackingData) -> [f32; 12] {
    TONGUE_SHAPES.map(|shape| data.shapes[shape as usize].weight)
}

/// Maps the ten model heads to VRFT's twelve tongue expressions. TongueOut is
/// `max(fused visibility, extension)` as in the reference, so a confidently
/// visible tongue is never shown barely out when extension under-reads.
fn map_tongue(values: &[f32; MODEL_HEADS], fused: f32, visible: bool) -> [f32; 12] {
    if !visible {
        return [0.0; 12];
    }
    let horizontal = values[2].clamp(-1.0, 1.0);
    let vertical = values[3].clamp(-1.0, 1.0);
    let twist = values[9].clamp(-1.0, 1.0);
    [
        fused.clamp(0.0, 1.0).max(values[1].clamp(0.0, 1.0)),
        vertical.max(0.0),
        (-vertical).max(0.0),
        (-horizontal).max(0.0),
        horizontal.max(0.0),
        values[6].clamp(0.0, 1.0),
        values[5].clamp(0.0, 1.0),
        values[4].clamp(0.0, 1.0),
        values[8].clamp(0.0, 1.0),
        values[7].clamp(0.0, 1.0),
        (-twist).max(0.0),
        twist.max(0.0),
    ]
}

#[derive(Default)]
struct FeedState {
    status: String,
    latest: Option<Frame>,
    eyes: Option<Frame>,
    /// The latest five-camera frame, while the headset sends them. `latest`
    /// holds its mouth pair and `eyes` its eye pair at the snapshot rate.
    strip: Option<Frame>,
    /// The pupils found in `eyes`, for drawing over it.
    eye_pupils: [Option<PupilMark>; 2],
    /// Latest `QPSTAT1` status from the headset APK.
    headset: Option<Headset>,
    source: Option<SocketAddr>,
    /// Set while the headset app found speaks a protocol this daemon doesn't.
    mismatch: Option<Mismatch>,
    /// Why the tongue model is not running, shown in the preview.
    model_error: Option<String>,
    /// The universal face model, while inference runs.
    face_model: Option<FaceModelStatus>,
    /// When the headset is next tried, while backing off after a failure.
    retry_at: Option<Instant>,
}

impl FeedState {
    /// Forgets the headset connection and everything it delivered.
    fn disconnect(&mut self) {
        self.source = None;
        self.latest = None;
        self.eyes = None;
        self.strip = None;
        self.eye_pupils = [None; 2];
        self.headset = None;
    }
}

type Shared = Arc<RwLock<FeedState>>;

#[derive(Clone)]
struct PreviewState {
    feed: Shared,
    model: TongueState,
    output: OutputState,
    capture: CaptureManager,
    training: crate::training::TrainingManager,
    settings: SettingsStore,
    eyes: EyeState,
    pupils: PupilState,
    /// Set to try the headset again without waiting out the backoff.
    retry: Arc<AtomicBool>,
}

/// Quest Pro support once started: the frame hook, and the API routes and
/// status the daemon serves for it.
pub struct Running {
    pub overlay: QuestProOverlay,
    pub routes: Router,
    pub status: Box<dyn Fn() -> serde_json::Value + Send + Sync>,
}

pub fn start(root: &Path, running: Arc<AtomicBool>) -> Running {
    let shared = Arc::new(RwLock::new(FeedState {
        status: "Looking for the headset".into(),
        ..FeedState::default()
    }));
    let root = root.to_path_buf();
    let settings = SettingsStore::load(&root);
    let eye_state = EyeState::new(crate::eye::load_calibration(&root));
    let pupil_state = PupilState::default();
    let inference_state: TongueState = Arc::new(RwLock::new(None));
    let output_state: OutputState = Arc::new(RwLock::new(None));
    let capture = CaptureManager::default();
    let retry = Arc::new(AtomicBool::new(false));
    let training = crate::training::TrainingManager::new(root, capture.clone());
    let training_watch = training.clone();
    let watch_running = running.clone();
    thread::Builder::new()
        .name("quest-pro-training".into())
        .spawn(move || {
            let mut ticks = 0u32;
            while watch_running.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(250));
                ticks = ticks.wrapping_add(1);
                // Every 2 s.
                if ticks.is_multiple_of(8) {
                    training_watch.yield_to_vrchat();
                }
            }
            training_watch.cancel();
        })
        .expect("couldn't start the Quest Pro training thread");
    let browser_state = PreviewState {
        feed: shared.clone(),
        model: inference_state.clone(),
        output: output_state.clone(),
        capture: capture.clone(),
        training: training.clone(),
        settings: settings.clone(),
        eyes: eye_state.clone(),
        pupils: pupil_state.clone(),
        retry: retry.clone(),
    };
    let mut routes = Router::new();
    if BROWSER_PAGES {
        routes = routes.route(routes::PAGE, get(preview_page));
    }
    let routes = routes
        .route(routes::FRAME, get(latest_frame))
        .route(routes::EYE_FRAME, get(latest_eye_frame))
        .route(routes::BROW_FRAME, get(latest_brow_frame))
        .route(routes::STATUS, get(feed_status))
        .route(routes::SETTINGS, get(get_settings).post(update_settings))
        .route(routes::EYE_RECENTER, post(eye_recenter))
        .route(routes::EYE_RECENTER_CLEAR, post(eye_recenter_clear))
        .route(routes::RECONNECT, post(reconnect))
        .route(routes::CAPTURE_STATUS, get(capture_status))
        .route(routes::CAPTURE_START, post(capture_start))
        .route(CaptureCommand::Stop.route(), post(capture_stop))
        .route(CaptureCommand::Skip.route(), post(capture_skip))
        .route(CaptureCommand::Pause.route(), post(capture_pause))
        .with_state(browser_state.clone())
        .merge(crate::training::routes(training));
    let status = Box::new(move || {
        serde_json::to_value(status(&browser_state)).unwrap_or(serde_json::Value::Null)
    });

    let worker_feed = shared.clone();
    let worker_state = inference_state.clone();
    let worker_running = running.clone();
    let worker_settings = settings.clone();
    thread::Builder::new()
        .name("quest-pro-tongue".into())
        .spawn(move || inference_loop(worker_feed, worker_state, worker_settings, worker_running))
        .expect("couldn't start the Quest Pro tongue thread");
    let receiver_capture = capture.clone();
    let processors = Processors {
        eyes: eye_state.processor(settings.clone()),
        pupils: pupil_state.processor(),
    };
    thread::Builder::new()
        .name("quest-pro-camera".into())
        .spawn(move || receive_loop(shared, receiver_capture, processors, running, retry))
        .expect("couldn't start the Quest Pro camera thread");
    let overlay = QuestProOverlay {
        latest: inference_state,
        output: output_state,
        capture,
        eyes: EyeOverlay::new(eye_state, settings.clone()),
        pupils: PupilOverlay::new(pupil_state),
        settings,
        native_history: VecDeque::new(),
        active: false,
        visible_latched: false,
        last_diagnostic: Instant::now(),
    };
    Running {
        overlay,
        routes,
        status,
    }
}

/// Whether the browser pages (the preview and its training script) are
/// served. Off for now: the desktop app covers them, through the JSON routes.
pub const BROWSER_PAGES: bool = false;

async fn preview_page() -> Html<&'static str> {
    Html(include_str!("preview.html"))
}

async fn feed_status(State(preview): State<PreviewState>) -> Json<Status> {
    Json(status(&preview))
}

async fn reconnect(State(preview): State<PreviewState>) -> Json<Status> {
    info!("Quest Pro camera: retrying the headset now, as asked");
    preview.retry.store(true, Ordering::SeqCst);
    Json(status(&preview))
}

fn status(preview: &PreviewState) -> Status {
    let settings = preview.settings.get();
    let mut eyes = preview.eyes.status();
    eyes.pupils = preview.pupils.status(&settings);
    let eye_output_deg = eyes
        .sample
        .as_ref()
        .filter(|_| eyes.fresh && settings.eye_gaze)
        .map(|sample| {
            let (left, right) = output_gaze(sample, &settings);
            [left.map(f32::to_degrees), right.map(f32::to_degrees)]
        });
    let state = preview.feed.read().unwrap();
    let model = preview.model.read().unwrap().as_ref().map(|prediction| {
        let age = prediction.received_at.elapsed();
        ModelStatus {
            sequence: prediction.sequence,
            age_ms: age.as_millis() as u64,
            fresh: age <= TONGUE_FRESH_FOR,
            inference_ms: prediction.inference_ms,
            skipped_frames: prediction.dropped_frames,
            raw_values: prediction.raw,
            values: prediction.values,
            camera_weight: prediction.camera_weight,
            threshold: prediction.threshold,
        }
    });
    let output = preview
        .output
        .read()
        .unwrap()
        .as_ref()
        .map(|snapshot| OutputStatus {
            source: snapshot.source,
            native_tongue_out: snapshot.native_tongue_out,
            fused_visibility: snapshot.fused_visibility,
            visible: snapshot.visible,
            values: snapshot.values,
            cheek_source: snapshot.cheek_source,
            cheek_puffs: snapshot.cheek_puffs,
            face: snapshot.face.clone(),
        });
    Status {
        status: state.status.clone(),
        source: state.source.map(|address| address.to_string()),
        retry_in_ms: state
            .retry_at
            .map(|at| at.saturating_duration_since(Instant::now()).as_millis() as u64),
        sequence: state.latest.as_ref().map(|frame| frame.sequence),
        width: FRAME_WIDTH,
        height: FRAME_HEIGHT,
        frame_age_ms: state
            .latest
            .as_ref()
            .map(|frame| frame.received_at.elapsed().as_millis() as u64),
        eye_frame_sequence: state.eyes.as_ref().map(|frame| frame.sequence),
        eye_frame_age_ms: state
            .eyes
            .as_ref()
            .map(|frame| frame.received_at.elapsed().as_millis() as u64),
        brow_frame_sequence: state.strip.as_ref().map(|frame| frame.sequence),
        brow_frame_age_ms: state
            .strip
            .as_ref()
            .map(|frame| frame.received_at.elapsed().as_millis() as u64),
        // A mouth frame clears it, so it's set only while five-camera
        // frames are what arrives.
        five_cameras: state.strip.is_some(),
        headset: state.headset.clone(),
        headset_mismatch: state.mismatch.clone(),
        model_error: state.model_error.clone(),
        face_model: state.face_model.clone(),
        model,
        output,
        eyes,
        eye_output_deg,
        settings,
    }
}

async fn latest_frame(State(preview): State<PreviewState>) -> impl IntoResponse {
    let frame = preview.feed.read().unwrap().latest.clone();
    frame_response(frame)
}

async fn latest_eye_frame(State(preview): State<PreviewState>) -> impl IntoResponse {
    let (frame, pupils) = {
        let feed = preview.feed.read().unwrap();
        (feed.eyes.clone(), feed.eye_pupils)
    };
    let mut response = frame_response(frame);
    if response.status() == StatusCode::OK {
        let pupils = serde_json::to_string(&pupils).unwrap_or_default();
        if let Ok(value) = HeaderValue::from_str(&pupils) {
            response.headers_mut().insert(PUPILS_HEADER, value);
        }
    }
    response
}

/// The brow camera's view of the latest five-camera frame.
async fn latest_brow_frame(State(preview): State<PreviewState>) -> impl IntoResponse {
    let strip = preview.feed.read().unwrap().strip.clone();
    frame_response(strip.map(|strip| {
        let mut pixels = vec![0u8; VIEW_BYTES];
        CameraLayout::all().cut(&strip.pixels, BROW_CAMERA, &mut pixels);
        Frame {
            pixels: pixels.into(),
            ..strip
        }
    }))
}

async fn get_settings(State(preview): State<PreviewState>) -> Json<QuestProSettings> {
    Json(preview.settings.get())
}

async fn update_settings(
    State(preview): State<PreviewState>,
    Json(patch): Json<SettingsPatch>,
) -> Result<Json<QuestProSettings>, (StatusCode, String)> {
    let updated = preview
        .settings
        .apply(&patch)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    info!("Quest Pro settings updated: {patch:?}");
    Ok(Json(updated))
}

async fn eye_recenter(
    State(preview): State<PreviewState>,
) -> Result<Json<QuestProSettings>, (StatusCode, String)> {
    let offsets = preview
        .eyes
        .recenter_offsets()
        .map_err(|error| (StatusCode::CONFLICT, error))?;
    let patch = serde_json::json!({ "eye_offsets": offsets });
    let updated = preview
        .settings
        .merge(&patch)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    info!(
        "Quest Pro eyes: recentered, offsets left={:?} right={:?} degrees",
        offsets.left_deg, offsets.right_deg
    );
    Ok(Json(updated))
}

async fn eye_recenter_clear(
    State(preview): State<PreviewState>,
) -> Result<Json<QuestProSettings>, (StatusCode, String)> {
    preview
        .settings
        .merge(&serde_json::json!({ "eye_offsets": null }))
        .map(Json)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

fn frame_response(frame: Option<Frame>) -> axum::response::Response {
    match frame {
        Some(frame) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            headers.insert(
                FRAME_SEQUENCE_HEADER,
                HeaderValue::from_str(&frame.sequence.to_string()).unwrap(),
            );
            (StatusCode::OK, headers, frame.pixels.to_vec()).into_response()
        }
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn capture_status(State(preview): State<PreviewState>) -> Json<CaptureStatus> {
    Json(preview.capture.status())
}

async fn capture_start(
    State(preview): State<PreviewState>,
    Json(request): Json<CaptureRequest>,
) -> Result<Json<CaptureStatus>, (StatusCode, String)> {
    if preview.training.busy() {
        return Err((
            StatusCode::CONFLICT,
            "Wait for training to finish, or cancel it, before recording".into(),
        ));
    }
    let (camera_live, layout) = {
        let feed = preview.feed.read().unwrap();
        let fresh = |frame: &Option<Frame>| {
            frame
                .as_ref()
                .is_some_and(|frame| frame.received_at.elapsed() <= TONGUE_FRESH_FOR)
        };
        // While the headset sends all five cameras, recordings keep them all.
        let layout = if fresh(&feed.strip) {
            CameraLayout::all()
        } else {
            CameraLayout::mouth()
        };
        (fresh(&feed.latest), layout)
    };
    if !camera_live {
        return Err((
            StatusCode::CONFLICT,
            "The mouth cameras aren't live yet. Wait for them before recording".into(),
        ));
    }
    preview
        .capture
        .start(request.mode.name(), &request.poses, layout)
        .map(Json)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

async fn capture_stop(State(preview): State<PreviewState>) -> Json<CaptureStatus> {
    Json(preview.capture.stop())
}

async fn capture_pause(State(preview): State<PreviewState>) -> Json<CaptureStatus> {
    Json(preview.capture.pause())
}

async fn capture_skip(State(preview): State<PreviewState>) -> Json<CaptureStatus> {
    Json(preview.capture.skip_current())
}

fn set_status(shared: &Shared, message: impl Into<String>) {
    let message = message.into();
    info!("Quest Pro camera: {message}");
    shared.write().unwrap().status = message;
}

/// What the stream's eye messages feed.
struct Processors {
    eyes: EyeProcessor,
    pupils: PupilProcessor,
}

fn receive_loop(
    shared: Shared,
    capture: CaptureManager,
    mut processors: Processors,
    running: Arc<AtomicBool>,
    retry: Arc<AtomicBool>,
) {
    let explicit = std::env::var("VRFT_QUEST_PRO_ADDR").ok();
    let manual = explicit
        .as_deref()
        .and_then(|value| value.parse::<SocketAddr>().ok());
    if explicit.is_some() && manual.is_none() {
        set_status(
            &shared,
            "VRFT_QUEST_PRO_ADDR must be an IP:port such as 192.168.1.5:27274",
        );
        return;
    }
    let mut mdns = None;
    let mut events = None;
    if manual.is_none() {
        match ServiceDaemon::new().and_then(|daemon| {
            let receiver = daemon.browse(SERVICE_TYPE)?;
            Ok((daemon, receiver))
        }) {
            Ok((daemon, receiver)) => {
                mdns = Some(daemon);
                events = Some(receiver);
                set_status(&shared, "Looking for the headset on the network");
            }
            Err(error) => {
                set_status(
                    &shared,
                    format!("Can't search the network for the headset ({error}). Set VRFT_QUEST_PRO_ADDR to its IP:27274"),
                );
                return;
            }
        }
    }
    let _keep_mdns_alive = mdns;
    let mut candidate = manual;
    // A headset app that couldn't be read, and when it was: it's tried again
    // now and then, in case it was updated and its announcement was missed.
    let mut recheck: Option<(SocketAddr, Instant)> = None;
    // Failed connections in a row, which set how long to wait before the next.
    let mut failures = 0u32;
    // The last failure logged, so a headset that stays away is logged once.
    let mut logged: Option<String> = None;
    while running.load(Ordering::SeqCst) {
        let Some(address) = candidate else {
            if let Some(receiver) = &events {
                match receiver.recv_timeout(Duration::from_secs(1)) {
                    Ok(ServiceEvent::ServiceResolved(info)) => {
                        if let Some(advert) = Advert::from_service(&info) {
                            candidate = accept_advert(&shared, advert);
                        }
                    }
                    Ok(ServiceEvent::ServiceRemoved(_, _)) => forget_mismatch(&shared),
                    _ => {}
                }
            }
            if candidate.is_none() {
                let now = retry.swap(false, Ordering::SeqCst);
                candidate = recheck
                    .filter(|(_, since)| now || since.elapsed() >= MISMATCH_RECHECK)
                    .map(|(address, _)| address);
            }
            continue;
        };
        // A press of Retry from before this attempt means nothing now.
        retry.store(false, Ordering::SeqCst);
        let attempt = Instant::now();
        let error = match connect_and_receive(address, &shared, &capture, &mut processors, &running)
        {
            Ok(()) => break,
            Err(Disconnect::Mismatch(mismatch)) => {
                report_mismatch(&shared, mismatch);
                if manual.is_some() {
                    // Check again later, in case the headset app gets updated.
                    thread::sleep(Duration::from_secs(5));
                } else {
                    // Wait for mDNS to announce the headset app again.
                    candidate = None;
                    recheck = Some((address, Instant::now()));
                }
                continue;
            }
            Err(Disconnect::Unreachable(error)) => {
                format!("Can't reach the headset at {address} ({error})")
            }
            Err(Disconnect::Io(error)) => {
                format!("Lost the connection to the headset at {address} ({error})")
            }
        };
        if attempt.elapsed() >= RETRY_RESET_AFTER {
            failures = 0;
            logged = None;
        }
        let delay = retry_delay(failures);
        failures = failures.saturating_add(1);
        if logged.as_deref() != Some(error.as_str()) {
            warn!(
                "Quest Pro camera: {error}. Retrying, waiting up to {}s between tries",
                RETRY_MAX.as_secs()
            );
            logged = Some(error.clone());
        } else {
            debug!(
                "Quest Pro camera: {error}. Retrying in {}s",
                delay.as_secs()
            );
        }
        {
            let mut state = shared.write().unwrap();
            state.status = error;
            state.retry_at = Some(Instant::now() + delay);
        }
        // Keep retrying the address until mDNS announces another, even when
        // the headset app leaves: its announcement on coming back can be
        // missed, as when it's reinstalled, and the headset keeps its address.
        if let Some(announced) = wait_to_retry(delay, events.as_ref(), &shared, &retry, &running) {
            // The headset app announcing itself is worth trying straight away.
            candidate = announced;
            failures = 0;
        }
        shared.write().unwrap().retry_at = None;
    }
}

/// How long to wait after `failures` failed connections in a row.
fn retry_delay(failures: u32) -> Duration {
    RETRY_FIRST
        .saturating_mul(1u32 << failures.min(16))
        .min(RETRY_MAX)
}

/// Waits `delay` before retrying the headset, or less when Retry is pressed,
/// VRFT stops, or mDNS announces the headset app. An announcement returns
/// where to connect, `None` inside when the app can't be read.
fn wait_to_retry(
    delay: Duration,
    events: Option<&mdns_sd::Receiver<ServiceEvent>>,
    shared: &Shared,
    retry: &AtomicBool,
    running: &AtomicBool,
) -> Option<Option<SocketAddr>> {
    let until = Instant::now() + delay;
    while running.load(Ordering::SeqCst) && !retry.swap(false, Ordering::SeqCst) {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let slice = left.min(Duration::from_millis(200));
        match events {
            Some(receiver) => {
                if let Ok(ServiceEvent::ServiceResolved(info)) = receiver.recv_timeout(slice) {
                    if let Some(advert) = Advert::from_service(&info) {
                        return Some(accept_advert(shared, advert));
                    }
                }
            }
            None => thread::sleep(slice),
        }
    }
    None
}

/// Where to connect for `advert`, or `None` once it's reported as unreadable.
fn accept_advert(shared: &Shared, advert: Advert) -> Option<SocketAddr> {
    match check_protocol(advert.protocol, advert.apk_version.as_deref()) {
        Ok(()) => Some(advert.address),
        Err(mismatch) => {
            report_mismatch(shared, mismatch);
            None
        }
    }
}

fn report_mismatch(shared: &Shared, mismatch: Mismatch) {
    let message = mismatch_message(&mismatch);
    let mut state = shared.write().unwrap();
    // mDNS repeats its announcements; say it once.
    if state.mismatch.as_ref() != Some(&mismatch) {
        warn!(
            "Quest Pro camera: {message} (headset app speaks stream protocol {}; this VRFaceTracking reads {}..={})",
            mismatch.protocol,
            PROTOCOLS.start(),
            PROTOCOLS.end()
        );
    }
    state.disconnect();
    state.status = message;
    state.mismatch = Some(mismatch);
}

/// The mismatched headset app has left the network; stop asking for an update.
fn forget_mismatch(shared: &Shared) {
    if shared.write().unwrap().mismatch.take().is_some() {
        set_status(shared, "Looking for the headset on the network");
    }
}

fn connect_and_receive(
    address: SocketAddr,
    shared: &Shared,
    capture: &CaptureManager,
    processors: &mut Processors,
    running: &AtomicBool,
) -> Result<(), Disconnect> {
    debug!("Quest Pro camera: connecting to the headset at {address}");
    shared.write().unwrap().status = format!("Connecting to the headset at {address}");
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))
        .map_err(Disconnect::Unreachable)?;
    info!("Quest Pro camera: connected to the headset at {address}");
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_nodelay(true)?;
    // Headset apps from before the hello never read it.
    stream.write_all(&hello())?;
    {
        let mut state = shared.write().unwrap();
        state.disconnect();
        state.source = Some(address);
        state.status = "Connected. Waiting for camera frames from the headset".into();
    }
    let mut last_log = Instant::now();
    let mut last_frame_at = Instant::now();
    let mut eye_schedule = EyeSchedule::new(DEFAULT_EYE_FPS);
    while running.load(Ordering::SeqCst) {
        match read_message(&mut stream) {
            Ok(Some(Message::Mouth(frame))) => {
                capture.record(
                    frame.sequence,
                    frame.received_at,
                    &CameraLayout::mouth(),
                    &frame.pixels,
                );
                last_frame_at = Instant::now();
                if last_log.elapsed() >= Duration::from_secs(5) {
                    info!("Quest Pro camera frame {} from {address}", frame.sequence);
                    last_log = Instant::now();
                }
                let mut state = shared.write().unwrap();
                state.status = format!("Live · frame {}", frame.sequence);
                state.latest = Some(frame);
                state.strip = None;
            }
            Ok(Some(Message::Strip(strip))) => {
                capture.record(
                    strip.sequence,
                    strip.received_at,
                    &CameraLayout::all(),
                    &strip.pixels,
                );
                last_frame_at = Instant::now();
                if last_log.elapsed() >= Duration::from_secs(5) {
                    info!(
                        "Quest Pro camera five-camera frame {} from {address}",
                        strip.sequence
                    );
                    last_log = Instant::now();
                }
                let (mouth, eyes) = split_strip(&strip, &mut eye_schedule);
                // Measured before taking the lock, as for a snapshot.
                let pupils = eyes
                    .as_ref()
                    .map(|eyes| processors.pupils.process(&eyes.pixels, eyes.received_at));
                let mut state = shared.write().unwrap();
                state.status = format!("Live · frame {} · five cameras", strip.sequence);
                state.latest = Some(mouth);
                if let (Some(eyes), Some(pupils)) = (eyes, pupils) {
                    state.eyes = Some(eyes);
                    state.eye_pupils = pupils;
                }
                state.strip = Some(strip);
            }
            Ok(Some(Message::Eyes(frame))) => {
                let pupils = processors.pupils.process(&frame.pixels, frame.received_at);
                let mut state = shared.write().unwrap();
                state.eyes = Some(frame);
                state.eye_pupils = pupils;
            }
            Ok(Some(Message::Gaze(packet))) => processors.eyes.process(&packet, Instant::now()),
            Ok(Some(Message::Status(status))) => {
                // The headset app sends its status first on every connection.
                let apk_version = status
                    .get("apk_version")
                    .and_then(serde_json::Value::as_str);
                check_protocol(status_protocol(&status), apk_version)
                    .map_err(Disconnect::Mismatch)?;
                if let Some(eye) = status.get("eye") {
                    info!("Quest Pro headset eye pipeline: {eye}");
                }
                if let Some(fps) = status.get("eye_preview_fps").and_then(|v| v.as_u64()) {
                    eye_schedule = EyeSchedule::new(fps.min(30) as u32);
                }
                if status.get("camera_mask").and_then(|v| v.as_u64()) == Some(ALL_MASK.into()) {
                    info!("Quest Pro camera: the headset sends all five cameras");
                }
                let headset = serde_json::from_value(status).unwrap_or_else(|error| {
                    warn!(
                        "Quest Pro headset status this VRFaceTracking doesn't understand: {error}"
                    );
                    Headset::default()
                });
                let mut state = shared.write().unwrap();
                state.mismatch = None;
                state.headset = Some(headset);
            }
            Ok(None) => {}
            Err(error) => {
                shared.write().unwrap().disconnect();
                return Err(error.into());
            }
        }
        if last_frame_at.elapsed() >= Duration::from_secs(5) {
            let mut state = shared.write().unwrap();
            if state.latest.is_some() || state.status.starts_with("Live") {
                state.status = "Connected. Waiting for camera frames from the headset".into();
                state.latest = None;
                state.strip = None;
            }
        }
    }
    Ok(())
}

/// The `QPHELO1` sent to the headset app on connecting, saying which camera
/// frames this daemon reads, so the app sends five-camera frames only to a
/// daemon that reads them. The layout is that of every headset message: an
/// 8-byte magic, a version at byte 8 and the payload's length at byte 12.
fn hello() -> Vec<u8> {
    let payload = serde_json::json!({ "camera_masks": [MOUTH_MASK, EYES_MASK, ALL_MASK] });
    let payload = serde_json::to_vec(&payload).expect("the hello serializes");
    let mut message = b"QPHELO1\0".to_vec();
    message.extend(1u32.to_le_bytes());
    message.extend((payload.len() as u32).to_le_bytes());
    message.extend(payload);
    message
}

/// When to cut an eye pair out of the five-camera frames for the pupils: at
/// the headset app's eye snapshot rate, as it would send them without the
/// five-camera stream.
struct EyeSchedule {
    interval: Option<Duration>,
    next: Option<Instant>,
}

impl EyeSchedule {
    fn new(fps: u32) -> Self {
        Self {
            interval: (fps > 0).then(|| Duration::from_secs(1) / fps),
            next: None,
        }
    }

    fn due(&mut self, at: Instant) -> bool {
        let Some(interval) = self.interval else {
            return false;
        };
        if self.next.is_some_and(|next| at < next) {
            return false;
        }
        let mut next = self.next.unwrap_or(at);
        while next <= at {
            next += interval;
        }
        self.next = Some(next);
        true
    }
}

/// A five-camera frame's mouth pair, which the tongue model and the preview
/// read as they do the mouth stream, and its eye pair when one is due.
fn split_strip(strip: &Frame, eye_schedule: &mut EyeSchedule) -> (Frame, Option<Frame>) {
    let layout = CameraLayout::all();
    let pair = |cameras: &[u8]| Frame {
        pixels: layout
            .select(&strip.pixels, cameras)
            .expect("a strip has every camera")
            .into(),
        ..strip.clone()
    };
    let eyes = eye_schedule
        .due(strip.received_at)
        .then(|| pair(&EYE_CAMERAS));
    (pair(&MOUTH_CAMERAS), eyes)
}

pub(crate) fn base_model_dir(cwd: &std::path::Path) -> Result<PathBuf, String> {
    let model_dir = if let Some(explicit) = std::env::var_os("VRFT_TONGUE_MODEL_DIR") {
        PathBuf::from(explicit)
    } else {
        [
            cwd.join("models/quest-pro"),
            cwd.join(".local/quest-pro-models"),
        ]
        .into_iter()
        .find(|path| Role::Gate.find(path).is_some())
        .ok_or("the built-in tongue model is not installed. Download it on the Training page")?
    };
    Ok(model_dir)
}

/// The folder of the model pair in use.
fn model_dir() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    // A trained model in use runs without the built-in one.
    let dir = crate::training::selected_dir(&cwd, || base_model_dir(&cwd))?;
    if crate::training::complete_pair(&dir) {
        return Ok(dir);
    }
    // A universal face model alone: the built-in pair reads the mouth
    // cameras whenever the five cameras don't stream.
    if crate::training::has_face_model(&dir) {
        return base_model_dir(&cwd);
    }
    Err(format!("incomplete tongue model pair in {}", dir.display()))
}

/// The universal face model to run on five-camera frames, and the face
/// setup to enroll, as the model in use and `.local/face-enrollment.json`
/// say. `VRFT_FACE_MODEL` names a checkpoint to use instead.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FaceChoice {
    model: Option<PathBuf>,
    enrollment: Option<String>,
}

impl FaceChoice {
    fn current(cwd: &Path) -> Self {
        use vrft_tongue::universal::FILE_NAME;
        // The model in use's folder; for the built-in model, that's where
        // a built-in face model would sit beside the pair.
        let model = match std::env::var_os("VRFT_FACE_MODEL") {
            Some(path) => Some(PathBuf::from(path)),
            None => crate::training::selected_dir(cwd, || base_model_dir(cwd))
                .ok()
                .map(|dir| dir.join(FILE_NAME))
                .filter(|path| path.is_file()),
        };
        let enrollment = std::fs::read(cwd.join(crate::capture::ENROLLMENT_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| value["recording"].as_str().map(str::to_owned));
        Self { model, enrollment }
    }
}

/// The universal face model, loaded with the wearer's face setup.
struct FaceRuntime {
    choice: FaceChoice,
    model: Option<FaceModel>,
    status: FaceModelStatus,
}

impl FaceRuntime {
    fn start(cwd: &Path, choice: FaceChoice) -> Self {
        let mut status = FaceModelStatus {
            enrollment: choice.enrollment.clone(),
            ..FaceModelStatus::default()
        };
        let Some(path) = &choice.model else {
            return Self {
                choice,
                model: None,
                status,
            };
        };
        let loaded = FaceModel::load(path, Accelerator::from_env()).and_then(|mut model| {
            if let Some(id) = &choice.enrollment {
                let dir = crate::training::safe_child(&cwd.join(".local/tongue-captures"), id)
                    .map_err(anyhow::Error::msg)?;
                match Enrollment::from_recording(&dir).and_then(|setup| model.enroll(&setup)) {
                    Ok(()) => {}
                    // Without its setup the model still runs, reading every
                    // anchor as missing.
                    Err(error) => warn!("Quest Pro face: face setup {id} unusable ({error:#})"),
                }
            }
            Ok(model)
        });
        let model = match loaded {
            Ok(model) => {
                let info = model.info();
                info!(
                    "Quest Pro face: loaded {} device={} enrolled={:?} tongue_map={} untrained={:?}",
                    path.display(),
                    info.device,
                    info.enrolled,
                    info.tongue_map,
                    info.disabled_targets
                );
                status.loaded = true;
                status.device = Some(info.device.clone());
                status.enrolled = info.enrolled.clone();
                status.tongue_map = info.tongue_map;
                status.disabled_targets = info.disabled_targets.clone();
                Some(model)
            }
            Err(error) => {
                warn!("Quest Pro face: model unavailable ({error:#}); the mouth pair runs alone");
                status.error = Some(format!("{error:#}"));
                None
            }
        };
        Self {
            choice,
            model,
            status,
        }
    }
}

/// The universal face model's tongue and cheek puffs as the pair's twelve
/// heads: visibility, extension, horizontal, vertical, no shapes, puffs.
fn face_as_heads(face: &[f32; FACE_OUTPUTS]) -> [f32; MODEL_HEADS] {
    let mut heads = [0.0; MODEL_HEADS];
    heads[..4].copy_from_slice(&face[..4]);
    heads[CHEEK_COLUMNS[0]] = face[4];
    heads[CHEEK_COLUMNS[1]] = face[5];
    heads
}

fn start_model(dir: &std::path::Path) -> Result<TongueModel, String> {
    let model =
        TongueModel::load(dir, Accelerator::from_env()).map_err(|error| format!("{error:#}"))?;
    let info = model.info();
    info!(
        "Quest Pro tongue: loaded gate={} direction={} device={} camera_weight={:.2} threshold={:.2}",
        info.gate_size, info.direction_size, info.device, info.camera_weight, info.threshold
    );
    if !info.disabled_targets.is_empty() {
        info!(
            "Quest Pro tongue: model has no training for {}; those outputs stay at 0",
            info.disabled_targets.join(", ")
        );
    }
    Ok(model)
}

fn inference_loop(
    feed: Shared,
    latest: TongueState,
    settings: SettingsStore,
    running: Arc<AtomicBool>,
) {
    while running.load(Ordering::SeqCst) {
        // Turned off, the model isn't loaded, so it takes no GPU or CPU.
        if !settings.get().mouth_model {
            *latest.write().unwrap() = None;
            feed.write().unwrap().model_error = None;
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        let has_fresh_frame = feed
            .read()
            .unwrap()
            .latest
            .as_ref()
            .is_some_and(|frame| frame.received_at.elapsed() <= TONGUE_FRESH_FOR);
        if !has_fresh_frame {
            thread::sleep(Duration::from_millis(50));
            continue;
        }
        let context = InferenceContext {
            feed: &feed,
            latest: &latest,
            settings: &settings,
            running: &running,
        };
        let result = model_dir().and_then(|dir| run_model(&dir, &context));
        *latest.write().unwrap() = None;
        if let Err(error) = result {
            warn!(
                "Quest Pro tongue: model unavailable ({error}); retaining module tongue tracking"
            );
            feed.write().unwrap().model_error = Some(error);
        } else {
            continue;
        }
        for _ in 0..100 {
            if !running.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Shared state the inference worker reads from and publishes to.
struct InferenceContext<'a> {
    feed: &'a Shared,
    latest: &'a TongueState,
    settings: &'a SettingsStore,
    running: &'a AtomicBool,
}

/// Runs the pair in `dir` on each new frame until VRFT stops or another
/// model is selected, and the universal face model, when there is one, on
/// each five-camera frame instead.
fn run_model(dir: &std::path::Path, context: &InferenceContext) -> Result<(), String> {
    let mut model = start_model(dir)?;
    let InferenceContext {
        feed,
        latest,
        settings,
        running,
    } = context;
    feed.write().unwrap().model_error = None;
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let mut face = FaceRuntime::start(&cwd, FaceChoice::current(&cwd));
    feed.write().unwrap().face_model = Some(face.status.clone());
    let mut face_smoother = Smoother::<FACE_OUTPUTS>::default();
    let (camera_weight, threshold) = (model.info().camera_weight, model.info().threshold);
    let cheeks = CHEEK_COLUMNS.iter().all(|&column| {
        let name = vrft_tongue::TARGETS[column];
        !model
            .info()
            .disabled_targets
            .iter()
            .any(|disabled| disabled == name)
    });
    let mut last_sequence = None;
    let mut dropped_frames = 0u64;
    let mut smoother = TongueSmoother::default();
    let mut last_log = Instant::now();
    let mut last_selection_check = Instant::now();
    while running.load(Ordering::SeqCst) {
        if last_selection_check.elapsed() >= Duration::from_secs(1) {
            if model_dir().is_ok_and(|selected| selected != dir) {
                feed.write().unwrap().face_model = None;
                return Ok(());
            }
            // A new face model or face setup loads in place.
            let choice = FaceChoice::current(&cwd);
            if choice != face.choice {
                face = FaceRuntime::start(&cwd, choice);
                face_smoother = Smoother::default();
                feed.write().unwrap().face_model = Some(face.status.clone());
            }
            last_selection_check = Instant::now();
        }
        // Turned off: return, which unloads the model.
        if !settings.get().mouth_model {
            info!("Quest Pro tongue: mouth model turned off; unloading it");
            return Ok(());
        }
        let (frame, strip) = {
            let feed = feed.read().unwrap();
            (feed.latest.clone(), feed.strip.clone())
        };
        let Some(frame) = frame else {
            thread::sleep(Duration::from_millis(10));
            continue;
        };
        if Some(frame.sequence) == last_sequence || frame.received_at.elapsed() > TONGUE_FRESH_FOR {
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        let started = Instant::now();
        let strength = settings.get().tongue_smoothing;
        // Five cameras and a face model: it reads the whole strip. Otherwise
        // the pair reads the mouth cameras, as before.
        let strip = strip.filter(|strip| strip.sequence == frame.sequence);
        let face_result = match (face.model.as_mut(), strip) {
            (Some(face_model), Some(strip)) => Some(face_model.predict(&strip.pixels)),
            _ => None,
        };
        let face_values = match face_result {
            Some(Ok(prediction)) => face.model.as_ref().map(|model| (prediction.values, model)),
            Some(Err(error)) => {
                warn!("Quest Pro face: inference failed ({error:#}); the mouth pair takes over");
                face.status.error = Some(format!("{error:#}"));
                face.status.loaded = false;
                face.model = None;
                None
            }
            None => None,
        };
        let face_active = face_values.is_some();
        if face.status.active != face_active {
            face.status.active = face_active;
            if face_active {
                info!("Quest Pro face: five cameras live; the universal face model runs");
            }
            feed.write().unwrap().face_model = Some(face.status.clone());
        }
        let (values, face_reading, frame_weight, frame_threshold, frame_cheeks) = match face_values
        {
            Some((raw, face_model)) => {
                let enabled: [bool; FACE_OUTPUTS] =
                    std::array::from_fn(|index| face_model.enabled(index));
                let smoothed =
                    face_smoother.update(raw, frame.headset_ns, frame.received_at, strength);
                let info = face_model.info();
                (
                    face_as_heads(&raw),
                    Some(FaceReading {
                        values: smoothed,
                        enabled,
                    }),
                    info.camera_weight,
                    info.threshold,
                    enabled[4] && enabled[5],
                )
            }
            None => (
                model
                    .predict(&frame.pixels)
                    .map_err(|error| format!("{error:#}"))?,
                None,
                camera_weight,
                threshold,
                cheeks,
            ),
        };
        if let Some(previous) = last_sequence {
            dropped_frames += frame.sequence.saturating_sub(previous.saturating_add(1));
        }
        last_sequence = Some(frame.sequence);
        let inference_ms = started.elapsed().as_secs_f32() * 1000.0;
        let smoothed = smoother.update(values, frame.headset_ns, frame.received_at, strength);
        *latest.write().unwrap() = Some(TonguePrediction {
            sequence: frame.sequence,
            raw: values,
            values: smoothed,
            received_at: frame.received_at,
            inference_ms,
            dropped_frames,
            camera_weight: frame_weight,
            threshold: frame_threshold,
            cheeks: frame_cheeks,
            face: face_reading,
        });
        if last_log.elapsed() >= Duration::from_secs(5) {
            info!(
                "Quest Pro tongue: model seq={} infer={:.1}ms skipped={} source_age={}ms",
                frame.sequence,
                inference_ms,
                dropped_frames,
                frame.received_at.elapsed().as_millis()
            );
            last_log = Instant::now();
        }
    }
    Ok(())
}

fn invalid(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_string())
}

/// Reads one headset message. Returns `Ok(None)` when the stream is idle or
/// the message was skipped.
///
/// Every message starts with an 8-byte magic and 16 bytes that identify its
/// length, so camera frames, gaze samples and status can share one stream.
fn read_message(stream: &mut impl Read) -> std::io::Result<Option<Message>> {
    let mut prefix = [0u8; 16];
    let mut offset = 0;
    while offset < prefix.len() {
        match stream.read(&mut prefix[offset..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Camera stream closed",
                ))
            }
            Ok(count) => offset += count,
            Err(error)
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                if offset == 0 {
                    return Ok(None);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        }
    }
    let u32_at =
        |bytes: &[u8], at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    match &prefix[..8] {
        [b'Q', b'P', b'L', b'I', b'V', b'E', b'3', _] => {
            let mut header = [0u8; 64];
            header[..16].copy_from_slice(&prefix);
            stream.read_exact(&mut header[16..])?;
            let mask = u32_at(&header, 52);
            // The width says how many cameras the frame carries.
            let (width, bytes) = match mask {
                MOUTH_MASK | EYES_MASK => (WIDTH, FRAME_BYTES),
                ALL_MASK => (STRIP_WIDTH as usize, STRIP_BYTES),
                _ => return Err(invalid("Invalid QPLIVE3 frame header")),
            };
            if u32_at(&header, 8) != 3
                || u32_at(&header, 12) != 64
                || u32_at(&header, 32) != width as u32
                || u32_at(&header, 36) != HEIGHT as u32
                || u32_at(&header, 40) != width as u32
                || u32_at(&header, 44) != 1
                || u32_at(&header, 48) != bytes as u32
            {
                return Err(invalid("Invalid QPLIVE3 frame header"));
            }
            let mut pixels = vec![0u8; bytes];
            stream.read_exact(&mut pixels)?;
            let frame = Frame {
                sequence: u64::from_le_bytes(header[16..24].try_into().unwrap()),
                headset_ns: u64::from_le_bytes(header[24..32].try_into().unwrap()),
                pixels: pixels.into(),
                received_at: Instant::now(),
            };
            Ok(Some(match mask {
                MOUTH_MASK => Message::Mouth(frame),
                EYES_MASK => Message::Eyes(frame),
                _ => Message::Strip(frame),
            }))
        }
        b"QPGAZE1\0" => {
            let mut packet = [0u8; GAZE_PACKET_BYTES];
            packet[..16].copy_from_slice(&prefix);
            stream.read_exact(&mut packet[16..])?;
            GazePacket::parse(&packet)
                .map(|packet| Some(Message::Gaze(packet)))
                .map_err(|error| invalid(&error))
        }
        b"QPSTAT1\0" => {
            let length = u32_at(&prefix, 12) as usize;
            if u32_at(&prefix, 8) != 1 || length == 0 || length > STATUS_MAX_BYTES {
                return Err(invalid("Invalid QPSTAT1 header"));
            }
            let mut payload = vec![0u8; length];
            stream.read_exact(&mut payload)?;
            match serde_json::from_slice::<serde_json::Value>(&payload) {
                Ok(status @ serde_json::Value::Object(_)) => Ok(Some(Message::Status(status))),
                _ => {
                    warn!("Quest Pro camera: ignoring malformed headset status");
                    Ok(None)
                }
            }
        }
        // A newer headset app may send message types this daemon doesn't know.
        // Those keep their payload length where QPSTAT1 does, so skip them.
        [b'Q', b'P', ..] => {
            let length = u32_at(&prefix, 12) as usize;
            if length > SKIP_MAX_BYTES {
                return Err(invalid("Oversized headset stream message"));
            }
            let skipped = std::io::copy(
                &mut stream.by_ref().take(length as u64),
                &mut std::io::sink(),
            )?;
            if skipped < length as u64 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Headset stream ended mid-message",
                ));
            }
            debug!(
                "Quest Pro camera: skipped unknown {} message",
                String::from_utf8_lossy(&prefix[..8]).trim_end_matches('\0')
            );
            Ok(None)
        }
        _ => Err(invalid("Unknown headset stream message")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::QuestProSettings;
    use std::io::Write;

    fn overlay_with(
        prediction: Option<TonguePrediction>,
        settings: QuestProSettings,
    ) -> QuestProOverlay {
        let settings = SettingsStore::in_memory(settings);
        QuestProOverlay {
            latest: Arc::new(RwLock::new(prediction)),
            output: Arc::new(RwLock::new(None)),
            capture: CaptureManager::default(),
            eyes: EyeOverlay::new(EyeState::new(Err("test".into())), settings.clone()),
            pupils: PupilOverlay::new(PupilState::default()),
            settings,
            native_history: VecDeque::new(),
            active: false,
            visible_latched: false,
            last_diagnostic: Instant::now(),
        }
    }

    fn overlay(prediction: Option<TonguePrediction>) -> QuestProOverlay {
        overlay_with(prediction, QuestProSettings::default())
    }

    fn prediction(received_at: Instant) -> TonguePrediction {
        let values = [0.9, 0.6, -0.4, 0.7, 0.2, 0.3, 0.4, 0.5, 0.6, -0.8, 0.7, 0.1];
        TonguePrediction {
            sequence: 10,
            raw: values,
            values,
            received_at,
            inference_ms: 20.0,
            dropped_frames: 0,
            camera_weight: 0.95,
            threshold: 0.85,
            cheeks: true,
            face: None,
        }
    }

    fn shapes(data: &UnifiedTrackingData) -> Vec<f32> {
        TONGUE_SHAPES
            .iter()
            .map(|shape| data.shapes[*shape as usize].weight)
            .collect()
    }

    #[test]
    fn fresh_prediction_overrides_all_tongue_shapes() {
        let mut data = UnifiedTrackingData::default();
        let mut overlay = overlay_with(
            Some(prediction(Instant::now())),
            QuestProSettings {
                tongue_visibility: VisibilityMode::Weighted,
                ..QuestProSettings::default()
            },
        );
        // A tracking module frame with TongueOut 0.
        overlay.before_mutation(&data);
        overlay.apply(&mut data);
        // Weighted: 0.95 * 0.9 + 0.05 * 0 = 0.855 >= 0.85, and TongueOut is
        // max(fused visibility, extension 0.6).
        for (value, expected) in shapes(&data)
            .iter()
            .zip([0.855, 0.7, 0.0, 0.4, 0.0, 0.4, 0.3, 0.2, 0.6, 0.5, 0.8, 0.0])
        {
            assert!((value - expected).abs() < 0.0001, "{value} != {expected}");
        }
        let output = overlay.output.read().unwrap();
        let snapshot = output.as_ref().unwrap();
        assert_eq!(snapshot.source, TongueSource::EnhancedModel);
        assert_eq!(snapshot.values, tongue_values(&data));
    }

    #[test]
    fn with_the_mouth_model_off_the_headsets_tongue_and_cheeks_go_out() {
        let mut data = UnifiedTrackingData::default();
        data.shapes[UnifiedExpressions::TongueOut as usize].weight = 0.3;
        data.shapes[UnifiedExpressions::CheekPuffLeft as usize].weight = 0.2;
        let before = data.shapes.clone();
        let mut overlay = overlay_with(
            Some(prediction(Instant::now())),
            QuestProSettings {
                mouth_model: false,
                ..QuestProSettings::default()
            },
        );
        overlay.before_mutation(&data);
        overlay.apply(&mut data);
        for shape in TONGUE_SHAPES.into_iter().chain(CHEEK_SHAPES) {
            assert_eq!(
                data.shapes[shape as usize].weight,
                before[shape as usize].weight
            );
        }
        let output = overlay.output.read().unwrap();
        let snapshot = output.as_ref().unwrap();
        assert_eq!(snapshot.source, TongueSource::TrackingModule);
        assert_eq!(snapshot.cheek_source, TongueSource::TrackingModule);
    }

    #[test]
    fn stale_prediction_preserves_module_tongue_values() {
        let mut data = UnifiedTrackingData::default();
        data.shapes[UnifiedExpressions::TongueOut as usize].weight = 0.75;
        data.shapes[UnifiedExpressions::TongueCurlUp as usize].weight = 0.25;
        let mut overlay = overlay(Some(prediction(Instant::now() - Duration::from_secs(1))));
        overlay.active = true;
        overlay.apply(&mut data);
        assert_eq!(
            data.shapes[UnifiedExpressions::TongueOut as usize].weight,
            0.75
        );
        assert_eq!(
            data.shapes[UnifiedExpressions::TongueCurlUp as usize].weight,
            0.25
        );
        assert!(!overlay.active);
        let output = overlay.output.read().unwrap();
        let snapshot = output.as_ref().unwrap();
        assert_eq!(snapshot.source, TongueSource::TrackingModule);
        assert_eq!(snapshot.values, tongue_values(&data));
    }

    #[test]
    fn visible_to_hidden_clears_enhanced_shapes() {
        let mut data = UnifiedTrackingData::default();
        let mut prediction = prediction(Instant::now());
        prediction.values[0] = 0.1;
        let mut overlay = overlay(Some(prediction));
        overlay.apply(&mut data);
        assert!(shapes(&data).iter().all(|value| *value == 0.0));
    }

    fn cheeks(data: &UnifiedTrackingData) -> [f32; 2] {
        CHEEK_SHAPES.map(|shape| data.shapes[shape as usize].weight)
    }

    #[test]
    fn learned_cheek_puffs_replace_the_modules_with_the_tongue_in() {
        let mut prediction = prediction(Instant::now());
        prediction.values[0] = 0.1;
        let mut overlay = overlay(Some(prediction));
        let mut data = UnifiedTrackingData::default();
        data.shapes[UnifiedExpressions::CheekPuffLeft as usize].weight = 0.4;
        data.shapes[UnifiedExpressions::CheekPuffRight as usize].weight = 0.4;
        overlay.apply(&mut data);
        assert_eq!(cheeks(&data), [0.7, 0.1]);
        let output = overlay.output.read().unwrap().clone().unwrap();
        assert_eq!(output.cheek_source, TongueSource::EnhancedModel);
        assert_eq!(output.cheek_puffs, [0.7, 0.1]);
    }

    #[test]
    fn the_modules_cheek_puffs_stay_without_learned_ones() {
        let module = |data: &mut UnifiedTrackingData| {
            data.shapes[UnifiedExpressions::CheekPuffLeft as usize].weight = 0.4;
            data.shapes[UnifiedExpressions::CheekPuffRight as usize].weight = 0.3;
        };
        let mut untrained = prediction(Instant::now());
        untrained.cheeks = false;
        let switched_off = QuestProSettings {
            cheek_puffs: false,
            ..QuestProSettings::default()
        };
        for mut overlay in [
            overlay(Some(untrained)),
            overlay_with(Some(prediction(Instant::now())), switched_off),
            overlay(Some(prediction(Instant::now() - Duration::from_secs(1)))),
        ] {
            let mut data = UnifiedTrackingData::default();
            module(&mut data);
            overlay.apply(&mut data);
            assert_eq!(cheeks(&data), [0.4, 0.3]);
            let output = overlay.output.read().unwrap().clone().unwrap();
            assert_eq!(output.cheek_source, TongueSource::TrackingModule);
        }
    }

    fn face_reading() -> FaceReading {
        let mut values = [0.0; FACE_OUTPUTS];
        for (index, value) in values.iter_mut().enumerate() {
            *value = index as f32 / 20.0;
        }
        let mut enabled = [true; FACE_OUTPUTS];
        // Never trained: the frown.
        enabled[15] = false;
        enabled[16] = false;
        FaceReading { values, enabled }
    }

    #[test]
    fn the_face_model_sends_what_it_learned_while_fresh() {
        let mut fresh = prediction(Instant::now());
        fresh.face = Some(face_reading());
        let mut data = UnifiedTrackingData::default();
        data.shapes[UnifiedExpressions::BrowPinchLeft as usize].weight = 0.33;
        let mut overlay = overlay(Some(fresh.clone()));
        overlay.apply(&mut data);
        assert_eq!(
            data.shapes[UnifiedExpressions::BrowInnerUpLeft as usize].weight,
            9.0 / 20.0
        );
        assert_eq!(
            data.shapes[UnifiedExpressions::CheekSuckRight as usize].weight,
            7.0 / 20.0
        );
        assert_eq!(
            data.shapes[UnifiedExpressions::BrowPinchLeft as usize].weight,
            0.33,
            "an output it never learned stays the module's"
        );
        let output = overlay.output.read().unwrap().clone().unwrap();
        assert_eq!(output.face.len(), 9);
        assert!(output
            .face
            .iter()
            .any(|value| value.expression == "JawOpen" && value.value == 0.4));

        // Stale, or turned off: the module's values stay.
        for mut overlay in [
            self::overlay(Some(TonguePrediction {
                received_at: Instant::now() - Duration::from_secs(1),
                ..fresh.clone()
            })),
            overlay_with(
                Some(fresh.clone()),
                QuestProSettings {
                    face_expressions: false,
                    ..QuestProSettings::default()
                },
            ),
        ] {
            let mut data = UnifiedTrackingData::default();
            data.shapes[UnifiedExpressions::BrowInnerUpLeft as usize].weight = 0.7;
            overlay.apply(&mut data);
            assert_eq!(
                data.shapes[UnifiedExpressions::BrowInnerUpLeft as usize].weight,
                0.7
            );
            assert!(overlay
                .output
                .read()
                .unwrap()
                .as_ref()
                .unwrap()
                .face
                .is_empty());
        }
    }

    #[test]
    fn the_face_models_tongue_and_puffs_fill_the_pairs_heads() {
        let values = face_reading().values;
        let heads = face_as_heads(&values);
        assert_eq!(heads[..4], values[..4]);
        assert!(heads[4..10].iter().all(|value| *value == 0.0));
        assert_eq!(
            (heads[CHEEK_COLUMNS[0]], heads[CHEEK_COLUMNS[1]]),
            (values[4], values[5])
        );
    }

    #[test]
    fn tongue_out_is_the_larger_of_visibility_and_extension() {
        let values = [1.0, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(map_tongue(&values, 0.9, true)[0], 0.9);
        let extended = [1.0, 0.95, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(map_tongue(&extended, 0.9, true)[0], 0.95);
        assert_eq!(map_tongue(&values, 0.9, false), [0.0; 12]);
    }

    #[test]
    fn visibility_modes_follow_the_reference_formulas() {
        assert!(
            (fuse_visibility(VisibilityMode::Weighted, 0.8, 0.9, Some(0.4)) - 0.8).abs() < 1e-6
        );
        assert_eq!(
            fuse_visibility(VisibilityMode::Camera, 0.8, 0.9, Some(0.4)),
            0.9
        );
        assert_eq!(
            fuse_visibility(VisibilityMode::Native, 0.8, 0.9, Some(0.4)),
            0.4
        );
        assert_eq!(
            fuse_visibility(VisibilityMode::Agreement, 0.8, 0.9, Some(0.4)),
            0.4
        );
        for mode in [
            VisibilityMode::Weighted,
            VisibilityMode::Camera,
            VisibilityMode::Native,
            VisibilityMode::Agreement,
        ] {
            assert_eq!(fuse_visibility(mode, 0.8, 0.9, None), 0.9);
        }
    }

    #[test]
    fn agreement_mode_rejects_camera_only_detections() {
        let settings = QuestProSettings {
            tongue_visibility: VisibilityMode::Agreement,
            ..QuestProSettings::default()
        };
        let mut overlay = overlay_with(Some(prediction(Instant::now())), settings);
        let mut data = UnifiedTrackingData::default();
        overlay.before_mutation(&data);
        overlay.apply(&mut data);
        assert!(shapes(&data).iter().all(|value| *value == 0.0));
    }

    #[test]
    fn native_value_nearest_the_camera_frame_is_used() {
        let mut overlay = overlay(None);
        let base = Instant::now();
        overlay.native_history.extend([
            (base, 0.1),
            (base + Duration::from_millis(40), 0.9),
            (base + Duration::from_millis(80), 0.2),
        ]);
        assert_eq!(
            overlay.native_near(base + Duration::from_millis(45)),
            Some(0.9)
        );
        assert_eq!(
            overlay.native_near(base + Duration::from_millis(75)),
            Some(0.2)
        );
        assert_eq!(overlay.native_near(base), Some(0.1));
        assert_eq!(overlay.native_near(base + Duration::from_millis(500)), None);
    }

    #[test]
    fn camera_alone_drives_the_tongue_without_a_tracking_module() {
        let mut overlay = overlay(Some(prediction(Instant::now())));
        assert!(overlay.has_live_data());
        let mut data = UnifiedTrackingData::default();
        overlay.apply(&mut data);
        let output = overlay.output.read().unwrap().clone().unwrap();
        assert_eq!(output.native_tongue_out, None);
        assert!(output.visible.unwrap());
        assert!(data.shapes[UnifiedExpressions::TongueOut as usize].weight > 0.0);
    }

    #[test]
    fn smoothing_is_time_based_and_zero_means_raw() {
        assert_eq!(smoothing_alpha(0.0, 1.0 / 24.0), 1.0);
        // At the reference cadence the per-frame factor equals the slider map.
        let alpha = smoothing_alpha(55.0, 1.0 / 24.0);
        assert!((alpha - (1.0 - 0.88 * 0.55)).abs() < 1e-5, "{alpha}");
        assert!(smoothing_alpha(55.0, 1.0 / 48.0) < alpha);
        let now = Instant::now();
        let mut smoother = TongueSmoother::default();
        let first = smoother.update([1.0; MODEL_HEADS], 1_000_000_000, now, 55.0);
        assert_eq!(first, [1.0; MODEL_HEADS]);
        let second = smoother.update([0.0; MODEL_HEADS], 1_041_666_667, now, 55.0);
        assert!((second[0] - (1.0 - alpha)).abs() < 1e-4, "{second:?}");
        // A gap longer than 250 ms restarts from the raw value.
        let restarted = smoother.update([0.5; MODEL_HEADS], 2_000_000_000, now, 55.0);
        assert_eq!(restarted, [0.5; MODEL_HEADS]);
    }

    fn frame_bytes(mask: u32, sequence: u64) -> Vec<u8> {
        let width = if mask == ALL_MASK { 2000 } else { 800 };
        frame_with_width(mask, sequence, width)
    }

    fn frame_with_width(mask: u32, sequence: u64, width: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 64 + width as usize * 400];
        bytes[..7].copy_from_slice(b"QPLIVE3");
        for (at, value) in [
            (8, 3u32),
            (12, 64),
            (32, width),
            (36, 400),
            (40, width),
            (44, 1),
            (48, width * 400),
            (52, mask),
        ] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[16..24].copy_from_slice(&sequence.to_le_bytes());
        bytes[24..32].copy_from_slice(&(sequence * 1000).to_le_bytes());
        bytes[64] = 7;
        bytes
    }

    #[test]
    fn stream_reader_dispatches_every_message_type() {
        let mut stream = Vec::new();
        stream.extend(frame_bytes(MOUTH_MASK, 5));
        stream.extend(frame_bytes(EYES_MASK, 5));
        let mut gaze = [0u8; 64];
        gaze[..8].copy_from_slice(b"QPGAZE1\0");
        gaze[8..12].copy_from_slice(&1u32.to_le_bytes());
        gaze[12..16].copy_from_slice(&64u32.to_le_bytes());
        gaze[16..24].copy_from_slice(&9u64.to_le_bytes());
        stream.extend(gaze);
        let status = br#"{"eye":{"state":"running"}}"#;
        stream.extend(b"QPSTAT1\0");
        stream.extend(1u32.to_le_bytes());
        stream.extend((status.len() as u32).to_le_bytes());
        stream.extend(status);
        let mut reader = std::io::Cursor::new(stream);
        match read_message(&mut reader).unwrap() {
            Some(Message::Mouth(frame)) => {
                assert_eq!(
                    (frame.sequence, frame.headset_ns, frame.pixels[0]),
                    (5, 5000, 7)
                )
            }
            _ => panic!("expected a mouth frame"),
        }
        assert!(matches!(
            read_message(&mut reader).unwrap(),
            Some(Message::Eyes(_))
        ));
        assert!(matches!(
            read_message(&mut reader).unwrap(),
            Some(Message::Gaze(packet)) if packet.sequence == 9
        ));
        match read_message(&mut reader).unwrap() {
            Some(Message::Status(value)) => assert_eq!(value["eye"]["state"], "running"),
            _ => panic!("expected status"),
        }
        assert!(
            read_message(&mut reader).is_err(),
            "end of stream is an error"
        );
    }

    #[test]
    fn stream_reader_reads_five_camera_frames() {
        let mut stream = frame_bytes(ALL_MASK, 8);
        // Each column holds its camera's number.
        for (index, pixel) in stream[64..].iter_mut().enumerate() {
            *pixel = ((index % 2000) / 400) as u8;
        }
        let mut reader = std::io::Cursor::new(stream);
        let Some(Message::Strip(strip)) = read_message(&mut reader).unwrap() else {
            panic!("expected a five-camera frame");
        };
        assert_eq!((strip.sequence, strip.pixels.len()), (8, STRIP_BYTES));
        let mut schedule = EyeSchedule::new(5);
        let (mouth, eyes) = split_strip(&strip, &mut schedule);
        assert_eq!(mouth.sequence, 8);
        assert_eq!(mouth.pixels.len(), FRAME_BYTES);
        assert_eq!((mouth.pixels[0], mouth.pixels[400]), (2, 3));
        let eyes = eyes.expect("the first frame carries an eye pair");
        assert_eq!((eyes.pixels[0], eyes.pixels[400]), (0, 1));
        // Not again until the snapshot interval has passed.
        assert!(split_strip(&strip, &mut schedule).1.is_none());
    }

    #[test]
    fn eye_pairs_follow_the_snapshot_rate() {
        let start = Instant::now();
        let mut schedule = EyeSchedule::new(5);
        let sent = (0..24)
            .filter(|frame| schedule.due(start + Duration::from_secs(1) * *frame / 24))
            .count();
        assert_eq!(sent, 5);
        assert!(!EyeSchedule::new(0).due(start));
    }

    #[test]
    fn stream_reader_rejects_unknown_masks_and_non_headset_bytes() {
        // Three face cameras, and five cameras claiming the mouth's width.
        let mut reader = std::io::Cursor::new(frame_with_width(0x1c, 1, 1200));
        assert!(read_message(&mut reader).is_err());
        let mut reader = std::io::Cursor::new(frame_with_width(ALL_MASK, 1, 800));
        assert!(read_message(&mut reader).is_err());
        let mut reader = std::io::Cursor::new(frame_with_width(MOUTH_MASK, 1, 2000));
        assert!(read_message(&mut reader).is_err());
        let mut reader = std::io::Cursor::new(b"GET / HTTP/1.1\r\n".to_vec());
        assert!(read_message(&mut reader).is_err());
    }

    fn unknown_message(payload: &[u8]) -> Vec<u8> {
        let mut bytes = b"QPNEW1\0\0".to_vec();
        bytes.extend(1u32.to_le_bytes());
        bytes.extend((payload.len() as u32).to_le_bytes());
        bytes.extend(payload);
        bytes
    }

    #[test]
    fn stream_reader_skips_message_types_from_newer_headset_apps() {
        let mut stream = unknown_message(&[9; 300]);
        stream.extend(frame_bytes(MOUTH_MASK, 6));
        let mut reader = std::io::Cursor::new(stream);
        assert!(read_message(&mut reader).unwrap().is_none());
        assert!(matches!(
            read_message(&mut reader).unwrap(),
            Some(Message::Mouth(frame)) if frame.sequence == 6
        ));
    }

    #[test]
    fn stream_reader_rejects_truncated_or_oversized_unknown_messages() {
        let mut truncated = unknown_message(&[9; 300]);
        truncated.truncate(100);
        assert!(read_message(&mut std::io::Cursor::new(truncated)).is_err());
        let mut oversized = unknown_message(&[]);
        oversized[12..16].copy_from_slice(&(SKIP_MAX_BYTES as u32 + 1).to_le_bytes());
        assert!(read_message(&mut std::io::Cursor::new(oversized)).is_err());
    }

    #[test]
    fn protocol_check_says_which_side_to_update() {
        assert_eq!(check_protocol(*PROTOCOLS.start(), None), Ok(()));
        let newer = check_protocol(*PROTOCOLS.end() + 1, Some("2027.1.0")).unwrap_err();
        assert_eq!(newer.update, Update::Vrft);
        assert_eq!(
            mismatch_message(&newer),
            "The headset app (2027.1.0) is newer than this VRFaceTracking can read. Update VRFaceTracking"
        );
        let older = check_protocol(*PROTOCOLS.start() - 1, None).unwrap_err();
        assert_eq!(older.update, Update::HeadsetApp);
        assert_eq!(
            mismatch_message(&older),
            "The headset app is too old for this VRFaceTracking. Update the headset app"
        );
    }

    #[test]
    fn headset_apps_without_a_protocol_field_speak_the_legacy_protocol() {
        assert_eq!(
            status_protocol(&serde_json::json!({"apk_version": "0.2"})),
            3
        );
        assert_eq!(status_protocol(&serde_json::json!({"protocol": 4})), 4);
        assert_eq!(
            status_protocol(&serde_json::json!({"protocol": u64::MAX})),
            u32::MAX
        );
        assert_eq!(advertised_protocol(Some("4"), Some("3")), 4);
        assert_eq!(advertised_protocol(None, Some("3")), 3);
        assert_eq!(advertised_protocol(None, None), LEGACY_PROTOCOL);
    }

    #[test]
    fn retries_back_off_to_a_minute() {
        let delays: Vec<u64> = (0..8).map(|n| retry_delay(n).as_secs()).collect();
        assert_eq!(delays, [2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(retry_delay(u32::MAX), RETRY_MAX);
    }

    #[test]
    fn retry_cuts_the_wait_short() {
        let shared = Shared::default();
        let retry = AtomicBool::new(true);
        let running = AtomicBool::new(true);
        let started = Instant::now();
        let announced = wait_to_retry(RETRY_MAX, None, &shared, &retry, &running);
        assert!(announced.is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!retry.load(Ordering::SeqCst));
    }

    #[test]
    fn connection_ends_when_the_headset_app_speaks_a_newer_protocol() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let headset = thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            // The daemon says first which frames it reads.
            let mut hello = [0u8; 16];
            client.read_exact(&mut hello).unwrap();
            assert_eq!(&hello[..8], b"QPHELO1\0");
            let mut payload =
                vec![0u8; u32::from_le_bytes(hello[12..16].try_into().unwrap()) as usize];
            client.read_exact(&mut payload).unwrap();
            let payload: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            assert_eq!(payload["camera_masks"], serde_json::json!([12, 3, 31]));
            let status = br#"{"apk_version":"2027.1.0","protocol":99}"#;
            let mut message = b"QPSTAT1\0".to_vec();
            message.extend(1u32.to_le_bytes());
            message.extend((status.len() as u32).to_le_bytes());
            message.extend(status);
            client.write_all(&message).unwrap();
            // Hold the connection open until the daemon side hangs up.
            let _ = client.read(&mut [0u8; 1]);
        });
        let shared: Shared = Arc::new(RwLock::new(FeedState::default()));
        let settings = SettingsStore::in_memory(QuestProSettings::default());
        let mut processors = Processors {
            eyes: EyeState::new(Err("test".into())).processor(settings),
            pupils: PupilState::default().processor(),
        };
        let result = connect_and_receive(
            address,
            &shared,
            &CaptureManager::default(),
            &mut processors,
            &AtomicBool::new(true),
        );
        let Err(Disconnect::Mismatch(mismatch)) = result else {
            panic!("expected a protocol mismatch, got {result:?}");
        };
        assert_eq!(mismatch.protocol, 99);
        assert_eq!(mismatch.apk_version.as_deref(), Some("2027.1.0"));
        report_mismatch(&shared, mismatch);
        let state = shared.read().unwrap();
        assert!(state.source.is_none() && state.headset.is_none());
        assert!(state.status.ends_with("Update VRFaceTracking"));
        drop(state);
        headset.join().unwrap();
    }
}
