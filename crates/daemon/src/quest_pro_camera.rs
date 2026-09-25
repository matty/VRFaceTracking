use crate::daemon_status::{DaemonReport, DaemonStatus};
use crate::quest_pro_camera_capture::{CaptureManager, CaptureStatus};
use crate::quest_pro_eye::{
    output_gaze, EyeOverlay, EyeProcessor, EyeState, EyeStatus, GazePacket, GAZE_PACKET_BYTES,
};
use crate::quest_pro_settings::{QuestProSettings, SettingsStore, VisibilityMode};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{extract::State, Json, Router};
use log::{debug, info, warn};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use vrft_api::{UnifiedExpressions, UnifiedTrackingData};

const SERVICE_TYPE: &str = "_vrftcam._tcp.local.";
const WIDTH: usize = 800;
const HEIGHT: usize = 400;
const FRAME_BYTES: usize = WIDTH * HEIGHT;
/// Camera mask of the lower-face pair (cameras 2 and 3) used for the tongue.
const MOUTH_MASK: u32 = 0x0c;
/// Camera mask of the eye pair (cameras 0 and 1), sent as low-rate snapshots.
const EYES_MASK: u32 = 0x03;
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
const PREVIEW_PORT: u16 = 27275;
const TONGUE_HEADS: usize = 10;
const TONGUE_FRESH_FOR: Duration = Duration::from_millis(250);
const NATIVE_HISTORY: Duration = Duration::from_millis(1000);
/// Frame rate the reference smoothing slider was tuned at; the slider value is
/// converted to a time constant so smoothing does not depend on camera FPS.
const SMOOTHING_REFERENCE_FPS: f64 = 24.0;
const TONGUE_TARGETS: [&str; TONGUE_HEADS] = [
    "visibility",
    "extension",
    "horizontal",
    "vertical",
    "curl_up",
    "bend_down",
    "roll",
    "flat",
    "squish",
    "twist",
];
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
    Gaze(GazePacket),
    Status(serde_json::Value),
}

/// A headset app whose stream protocol this daemon doesn't read.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Mismatch {
    protocol: u32,
    apk_version: Option<String>,
    update: Update,
}

/// Which side needs updating to resolve a [`Mismatch`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Update {
    Vrft,
    HeadsetApp,
}

impl Mismatch {
    fn message(&self) -> String {
        let app = match &self.apk_version {
            Some(version) => format!("The headset app ({version})"),
            None => "The headset app".into(),
        };
        match self.update {
            Update::Vrft => format!("{app} is newer than this VRFT can read. Update VRFT"),
            Update::HeadsetApp => {
                format!("{app} is too old for this VRFT. Update the headset app")
            }
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
    Io(std::io::Error),
    Mismatch(Mismatch),
}

impl From<std::io::Error> for Disconnect {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone)]
struct TonguePrediction {
    sequence: u64,
    /// Unsmoothed model output, shown in the preview.
    raw: [f32; TONGUE_HEADS],
    /// Output after the user's smoothing setting; drives VRFT.
    values: [f32; TONGUE_HEADS],
    received_at: Instant,
    inference_ms: f32,
    dropped_frames: u64,
    camera_weight: f32,
    threshold: f32,
}

type TongueState = Arc<RwLock<Option<TonguePrediction>>>;

#[derive(Clone)]
struct OutputSnapshot {
    source: &'static str,
    native_tongue_out: f32,
    fused_visibility: Option<f32>,
    visible: Option<bool>,
    values: [f32; 12],
}

type OutputState = Arc<RwLock<Option<OutputSnapshot>>>;

/// Applies enhanced Quest Pro tongue and independent eye tracking on top of
/// the active tracking module's output.
pub struct QuestProOverlay {
    latest: TongueState,
    output: OutputState,
    capture: CaptureManager,
    settings: SettingsStore,
    eyes: EyeOverlay,
    /// Module TongueOut before the daemon's smoothing, with arrival times, so
    /// visibility fusion can use the value closest to each camera frame.
    native_history: VecDeque<(Instant, f32)>,
    active: bool,
    visible_latched: bool,
    last_diagnostic: Instant,
}

impl QuestProOverlay {
    pub fn set_module_loaded(&self, loaded: bool) {
        self.capture.set_module_loaded(loaded);
    }

    /// Records the tracking module's own values before any daemon filtering.
    pub fn observe(&mut self, data: &UnifiedTrackingData) {
        let native = data.shapes[UnifiedExpressions::TongueOut as usize]
            .weight
            .clamp(0.0, 1.0);
        self.capture.update_native(native);
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

    /// Native TongueOut observed closest to `at`.
    fn native_near(&self, at: Instant) -> Option<f32> {
        self.native_history
            .iter()
            .min_by_key(|(observed, _)| {
                if *observed > at {
                    *observed - at
                } else {
                    at - *observed
                }
            })
            .map(|(_, value)| *value)
    }

    pub fn apply(&mut self, data: &mut UnifiedTrackingData) {
        self.eyes.apply(data);
        let settings = self.settings.get();
        let current_native = data.shapes[UnifiedExpressions::TongueOut as usize]
            .weight
            .clamp(0.0, 1.0);
        let current = self.latest.read().unwrap().clone();
        let fresh =
            current.filter(|prediction| prediction.received_at.elapsed() <= TONGUE_FRESH_FOR);
        let Some(prediction) = fresh else {
            if self.active {
                info!("Quest Pro tongue: camera or inference stale; using tracking module tongue values");
                self.active = false;
                self.visible_latched = false;
            }
            *self.output.write().unwrap() = Some(OutputSnapshot {
                source: "tracking module",
                native_tongue_out: current_native,
                fused_visibility: None,
                visible: None,
                values: tongue_values(data),
            });
            return;
        };
        if !self.active {
            info!("Quest Pro tongue: fresh model output; overriding 12 tongue expressions");
            self.active = true;
        }
        let native = self
            .native_near(prediction.received_at)
            .unwrap_or(current_native);
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
            source: "enhanced model",
            native_tongue_out: native,
            fused_visibility: Some(fused),
            visible: Some(self.visible_latched),
            values,
        });
        if self.last_diagnostic.elapsed() >= Duration::from_secs(5) {
            info!(
                "Quest Pro tongue: seq={} visible={} mode={:?} camera={:.2} native={:.2} fused={:.2} threshold={:.2} infer={:.1}ms age={}ms skipped={} heads={:?}",
                prediction.sequence, self.visible_latched, settings.tongue_visibility, camera,
                native, fused, prediction.threshold, prediction.inference_ms,
                prediction.received_at.elapsed().as_millis(), prediction.dropped_frames,
                prediction.values
            );
            self.last_diagnostic = Instant::now();
        }
    }
}

/// Combines camera and native visibility as the reference hub's modes do.
fn fuse_visibility(mode: VisibilityMode, camera_weight: f32, camera: f32, native: f32) -> f32 {
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

#[derive(Default)]
struct TongueSmoother {
    value: Option<[f32; TONGUE_HEADS]>,
    last_headset_ns: u64,
    last_received_at: Option<Instant>,
}

impl TongueSmoother {
    fn update(
        &mut self,
        raw: [f32; TONGUE_HEADS],
        headset_ns: u64,
        received_at: Instant,
        strength: f32,
    ) -> [f32; TONGUE_HEADS] {
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
fn map_tongue(values: &[f32; TONGUE_HEADS], fused: f32, visible: bool) -> [f32; 12] {
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
    /// Latest `QPSTAT1` status object from the headset APK.
    headset: Option<serde_json::Value>,
    source: Option<SocketAddr>,
    /// Set while the headset app found speaks a protocol this daemon doesn't.
    mismatch: Option<Mismatch>,
    /// Why the tongue model is not running, shown in the preview.
    model_error: Option<String>,
}

impl FeedState {
    /// Forgets the headset connection and everything it delivered.
    fn disconnect(&mut self) {
        self.source = None;
        self.latest = None;
        self.eyes = None;
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
    training: crate::quest_pro_training::TrainingManager,
    settings: SettingsStore,
    eyes: EyeState,
    daemon: DaemonStatus,
    /// Cleared to shut the daemon down, as Ctrl-C does.
    running: Arc<AtomicBool>,
}

#[derive(Serialize)]
struct ModelStatus {
    sequence: u64,
    age_ms: u128,
    fresh: bool,
    inference_ms: f32,
    skipped_frames: u64,
    raw_values: [f32; TONGUE_HEADS],
    values: [f32; TONGUE_HEADS],
    camera_weight: f32,
    threshold: f32,
}

#[derive(Serialize)]
struct OutputStatus {
    source: &'static str,
    native_tongue_out: f32,
    fused_visibility: Option<f32>,
    visible: Option<bool>,
    values: [f32; 12],
}

#[derive(Serialize)]
struct Status {
    status: String,
    source: Option<String>,
    sequence: Option<u64>,
    width: usize,
    height: usize,
    frame_age_ms: Option<u128>,
    eye_frame_sequence: Option<u64>,
    eye_frame_age_ms: Option<u128>,
    headset: Option<serde_json::Value>,
    headset_mismatch: Option<Mismatch>,
    model_error: Option<String>,
    model: Option<ModelStatus>,
    output: Option<OutputStatus>,
    eyes: EyeStatus,
    /// The (yaw, pitch) degrees VRFT sends for its left and right eye when
    /// independent gaze is fresh, after swapping and inversion settings.
    eye_output_deg: Option<[[f32; 2]; 2]>,
    settings: QuestProSettings,
    daemon: DaemonReport,
}

pub fn start(running: Arc<AtomicBool>, daemon: DaemonStatus) -> QuestProOverlay {
    let shared = Arc::new(RwLock::new(FeedState {
        status: "Looking for the headset".into(),
        ..FeedState::default()
    }));
    let root = std::env::current_dir().expect("VRFT working directory");
    let settings = SettingsStore::load(&root);
    let eye_state = EyeState::new(crate::quest_pro_eye::load_calibration(&root));
    let inference_state: TongueState = Arc::new(RwLock::new(None));
    let output_state: OutputState = Arc::new(RwLock::new(None));
    let capture = CaptureManager::default();
    let training = crate::quest_pro_training::TrainingManager::new(root, capture.clone());
    let training_shutdown = training.clone();
    let shutdown_running = running.clone();
    thread::spawn(move || {
        while shutdown_running.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(250));
        }
        training_shutdown.cancel();
    });
    let browser_state = PreviewState {
        feed: shared.clone(),
        model: inference_state.clone(),
        output: output_state.clone(),
        capture: capture.clone(),
        training: training.clone(),
        settings: settings.clone(),
        eyes: eye_state.clone(),
        daemon,
        running: running.clone(),
    };
    thread::spawn(move || {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                warn!("Camera preview runtime failed: {error}");
                return;
            }
        };
        runtime.block_on(async move {
            let router = Router::new()
                .route("/", get(preview_page))
                .route("/frame", get(latest_frame))
                .route("/eye-frame", get(latest_eye_frame))
                .route("/status", get(feed_status))
                .route("/settings", get(get_settings).post(update_settings))
                .route("/eye/recenter", post(eye_recenter))
                .route("/eye/recenter/clear", post(eye_recenter_clear))
                .route("/capture/status", get(capture_status))
                .route("/capture/start", post(capture_start))
                .route("/capture/stop", post(capture_stop))
                .route("/capture/skip", post(capture_skip))
                .route("/capture/pause", post(capture_pause))
                .route("/shutdown", post(shutdown))
                .with_state(browser_state)
                .merge(crate::quest_pro_training::routes(training));
            let address = SocketAddr::from(([127, 0, 0, 1], PREVIEW_PORT));
            match tokio::net::TcpListener::bind(address).await {
                Ok(listener) => {
                    info!("Quest Pro camera preview: http://127.0.0.1:{PREVIEW_PORT}/");
                    if let Err(error) = axum::serve(listener, router).await {
                        warn!("Camera preview server stopped: {error}");
                    }
                }
                Err(error) => warn!("Camera preview bind failed: {error}"),
            }
        });
    });

    let worker_feed = shared.clone();
    let worker_state = inference_state.clone();
    let worker_running = running.clone();
    let worker_settings = settings.clone();
    thread::spawn(move || {
        inference_loop(worker_feed, worker_state, worker_settings, worker_running)
    });
    let receiver_capture = capture.clone();
    let eye_processor = eye_state.processor(settings.clone());
    thread::spawn(move || receive_loop(shared, receiver_capture, eye_processor, running));
    QuestProOverlay {
        latest: inference_state,
        output: output_state,
        capture,
        eyes: EyeOverlay::new(eye_state, settings.clone()),
        settings,
        native_history: VecDeque::new(),
        active: false,
        visible_latched: false,
        last_diagnostic: Instant::now(),
    }
}

async fn preview_page() -> Html<&'static str> {
    Html(include_str!("quest_pro_camera_preview.html"))
}

async fn feed_status(State(preview): State<PreviewState>) -> Json<Status> {
    let settings = preview.settings.get();
    let eyes = preview.eyes.status();
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
            age_ms: age.as_millis(),
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
        });
    Json(Status {
        status: state.status.clone(),
        source: state.source.map(|address| address.to_string()),
        sequence: state.latest.as_ref().map(|frame| frame.sequence),
        width: WIDTH,
        height: HEIGHT,
        frame_age_ms: state
            .latest
            .as_ref()
            .map(|frame| frame.received_at.elapsed().as_millis()),
        eye_frame_sequence: state.eyes.as_ref().map(|frame| frame.sequence),
        eye_frame_age_ms: state
            .eyes
            .as_ref()
            .map(|frame| frame.received_at.elapsed().as_millis()),
        headset: state.headset.clone(),
        headset_mismatch: state.mismatch.clone(),
        model_error: state.model_error.clone(),
        model,
        output,
        eyes,
        eye_output_deg,
        settings,
        daemon: preview.daemon.report(),
    })
}

async fn latest_frame(State(preview): State<PreviewState>) -> impl IntoResponse {
    let frame = preview.feed.read().unwrap().latest.clone();
    frame_response(frame)
}

async fn latest_eye_frame(State(preview): State<PreviewState>) -> impl IntoResponse {
    let frame = preview.feed.read().unwrap().eyes.clone();
    frame_response(frame)
}

async fn get_settings(State(preview): State<PreviewState>) -> Json<QuestProSettings> {
    Json(preview.settings.get())
}

async fn update_settings(
    State(preview): State<PreviewState>,
    Json(patch): Json<serde_json::Value>,
) -> Result<Json<QuestProSettings>, (StatusCode, String)> {
    let updated = preview
        .settings
        .merge(&patch)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    info!("Quest Pro settings updated: {patch}");
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
                "x-frame-sequence",
                HeaderValue::from_str(&frame.sequence.to_string()).unwrap(),
            );
            (StatusCode::OK, headers, frame.pixels.to_vec()).into_response()
        }
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

/// Requires a JSON body, so a web page can't stop the daemon with a plain
/// cross-site form post: browsers only send JSON cross-site after a CORS
/// preflight, which this server doesn't answer.
#[derive(Deserialize)]
struct ShutdownRequest {
    #[serde(default)]
    requested_by: String,
}

async fn shutdown(
    State(preview): State<PreviewState>,
    Json(request): Json<ShutdownRequest>,
) -> StatusCode {
    let requester = if request.requested_by.is_empty() {
        "a local client"
    } else {
        request.requested_by.as_str()
    };
    info!("Shutdown requested by {requester}");
    preview.running.store(false, Ordering::SeqCst);
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct CaptureRequest {
    mode: String,
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
    let camera_live = preview
        .feed
        .read()
        .unwrap()
        .latest
        .as_ref()
        .is_some_and(|frame| frame.received_at.elapsed() <= TONGUE_FRESH_FOR);
    if !camera_live {
        return Err((
            StatusCode::CONFLICT,
            "The mouth cameras aren't live yet. Wait for them before recording".into(),
        ));
    }
    if !preview.capture.status().native_recent {
        return Err((
            StatusCode::CONFLICT,
            "VRFT isn't receiving the headset's face tracking. Recording needs VRFT running normally (not --camera-preview-only), with Virtual Desktop streaming face tracking".into(),
        ));
    }
    preview
        .capture
        .start(&request.mode)
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

fn receive_loop(
    shared: Shared,
    capture: CaptureManager,
    mut eyes: EyeProcessor,
    running: Arc<AtomicBool>,
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
            continue;
        };
        match connect_and_receive(address, &shared, &capture, &mut eyes, &running) {
            Ok(()) => break,
            Err(Disconnect::Mismatch(mismatch)) => {
                report_mismatch(&shared, mismatch);
                if manual.is_some() {
                    // Check again later, in case the headset app gets updated.
                    thread::sleep(Duration::from_secs(5));
                } else {
                    // Wait for mDNS to announce the headset app again.
                    candidate = None;
                }
            }
            Err(Disconnect::Io(error)) => {
                set_status(
                    &shared,
                    format!("Lost the connection to the headset ({error}). Retrying {address}"),
                );
                if manual.is_none() {
                    // Keep the address for quick reconnect until mDNS reports a change.
                    if let Some(receiver) = &events {
                        while let Ok(event) = receiver.try_recv() {
                            match event {
                                ServiceEvent::ServiceResolved(info) => {
                                    if let Some(advert) = Advert::from_service(&info) {
                                        candidate = accept_advert(&shared, advert);
                                    }
                                }
                                ServiceEvent::ServiceRemoved(_, _) => candidate = None,
                                _ => {}
                            }
                        }
                    }
                }
                thread::sleep(Duration::from_secs(2));
            }
        }
    }
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
    let message = mismatch.message();
    let mut state = shared.write().unwrap();
    // mDNS repeats its announcements; say it once.
    if state.mismatch.as_ref() != Some(&mismatch) {
        warn!(
            "Quest Pro camera: {message} (headset app speaks stream protocol {}; this VRFT reads {}..={})",
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
    eyes: &mut EyeProcessor,
    running: &AtomicBool,
) -> Result<(), Disconnect> {
    set_status(shared, format!("Connecting to the headset at {address}"));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_nodelay(true)?;
    {
        let mut state = shared.write().unwrap();
        state.disconnect();
        state.source = Some(address);
        state.status = "Connected. Waiting for camera frames from Virtual Desktop".into();
    }
    let mut last_log = Instant::now();
    let mut last_frame_at = Instant::now();
    while running.load(Ordering::SeqCst) {
        match read_message(&mut stream) {
            Ok(Some(Message::Mouth(frame))) => {
                capture.record(frame.sequence, frame.received_at, &frame.pixels);
                last_frame_at = Instant::now();
                if last_log.elapsed() >= Duration::from_secs(5) {
                    info!("Quest Pro camera frame {} from {address}", frame.sequence);
                    last_log = Instant::now();
                }
                let mut state = shared.write().unwrap();
                state.status = format!("Live · frame {}", frame.sequence);
                state.latest = Some(frame);
            }
            Ok(Some(Message::Eyes(frame))) => {
                shared.write().unwrap().eyes = Some(frame);
            }
            Ok(Some(Message::Gaze(packet))) => eyes.process(&packet, Instant::now()),
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
                let mut state = shared.write().unwrap();
                state.mismatch = None;
                state.headset = Some(status);
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
                state.status = "Connected. Waiting for camera frames from Virtual Desktop".into();
                state.latest = None;
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct ModelHandshake {
    version: u32,
    targets: Vec<String>,
    camera_weight: f32,
    threshold: f32,
    device: String,
    gate_size: usize,
    direction_size: usize,
    /// `"none"` when the helper returns raw frames and leaves smoothing to
    /// the daemon. Older helpers smooth internally and omit the field.
    #[serde(default)]
    smoothing: Option<String>,
    /// Heads a personal checkpoint could not train; the helper sends them as 0.
    #[serde(default)]
    disabled_targets: Vec<String>,
}

impl ModelHandshake {
    fn smooths_internally(&self) -> bool {
        self.smoothing.as_deref() != Some("none")
    }
}

struct ModelPaths {
    python: PathBuf,
    script: PathBuf,
    gate: PathBuf,
    direction: PathBuf,
}

pub(crate) fn python_path(cwd: &std::path::Path) -> Result<PathBuf, String> {
    let python = if let Some(explicit) = std::env::var_os("VRFT_TONGUE_PYTHON") {
        PathBuf::from(explicit)
    } else {
        [
            cwd.join(".local/tongue-python/Scripts/python.exe"),
            cwd.join("../toolchain/tongue-python/Scripts/python.exe"),
        ]
        .into_iter()
        .find(|path| path.is_file())
        .ok_or("tongue Python runtime missing; run setup-quest-pro-tongue.ps1")?
    };
    if !python.is_file() {
        return Err("Tongue Python runtime does not exist".into());
    }
    Ok(python)
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
        .find(|path| path.join("qpro-stereo-tongue-v8-gate.pt").is_file())
        .ok_or("tongue model pair missing; run setup-quest-pro-tongue.ps1")?
    };
    Ok(model_dir)
}

fn model_paths() -> Result<ModelPaths, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let script = cwd.join("tongue_inference.py");
    if !script.is_file() {
        return Err(format!("inference helper missing: {}", script.display()));
    }
    let python = python_path(&cwd)?;
    let model_dir = crate::quest_pro_training::selected_dir(&cwd, base_model_dir(&cwd)?)?;
    let gate = model_dir.join("qpro-stereo-tongue-v8-gate.pt");
    let direction = model_dir.join("qpro-stereo-tongue-v8-direction.pt");
    if !gate.is_file() || !direction.is_file() {
        return Err(format!(
            "incomplete tongue model pair in {}",
            model_dir.display()
        ));
    }
    Ok(ModelPaths {
        python,
        script,
        gate,
        direction,
    })
}

fn start_model(
    paths: &ModelPaths,
) -> Result<(Child, ChildStdin, BufReader<ChildStdout>, ModelHandshake), String> {
    let mut command = Command::new(&paths.python);
    command
        .arg(&paths.script)
        .arg("--gate")
        .arg(&paths.gate)
        .arg("--direction")
        .arg(&paths.direction)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start tongue Python: {error}"))?;
    let ready = (|| {
        let stdin = child.stdin.take().ok_or("model stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("model stdout unavailable")?;
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        let handshake: ModelHandshake = serde_json::from_str(&line)
            .map_err(|error| format!("invalid model startup response: {error}"))?;
        if handshake.version != 1
            || handshake
                .targets
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                != TONGUE_TARGETS
            || !handshake.camera_weight.is_finite()
            || !(0.0..=1.0).contains(&handshake.camera_weight)
            || !handshake.threshold.is_finite()
            || !(0.0..=1.0).contains(&handshake.threshold)
        {
            return Err("unsupported tongue model metadata".into());
        }
        Ok((stdin, reader, handshake))
    })();
    let (stdin, reader, handshake) = match ready {
        Ok(ready) => ready,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    info!("Quest Pro tongue: loaded gate={} direction={} device={} camera_weight={:.2} threshold={:.2}",
        handshake.gate_size, handshake.direction_size, handshake.device,
        handshake.camera_weight, handshake.threshold);
    if !handshake.disabled_targets.is_empty() {
        info!(
            "Quest Pro tongue: model has no training for {}; those outputs stay at 0",
            handshake.disabled_targets.join(", ")
        );
    }
    if handshake.smooths_internally() {
        warn!("Quest Pro tongue: inference helper smooths internally; the smoothing setting is ignored until tongue_inference.py is updated");
    }
    Ok((child, stdin, reader, handshake))
}

fn inference_loop(
    feed: Shared,
    latest: TongueState,
    settings: SettingsStore,
    running: Arc<AtomicBool>,
) {
    while running.load(Ordering::SeqCst) {
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
        let result = model_paths().and_then(|paths| run_model(&paths, &context));
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

fn run_model(paths: &ModelPaths, context: &InferenceContext) -> Result<(), String> {
    let (mut child, mut stdin, mut stdout, handshake) = start_model(paths)?;
    context.feed.write().unwrap().model_error = None;
    let result = run_model_frames(&mut stdin, &mut stdout, &handshake, context, &paths.gate);
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn run_model_frames(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    handshake: &ModelHandshake,
    context: &InferenceContext,
    selected_gate: &std::path::Path,
) -> Result<(), String> {
    let InferenceContext {
        feed,
        latest,
        settings,
        running,
    } = context;
    let mut last_sequence = None;
    let mut dropped_frames = 0u64;
    let mut smoother = TongueSmoother::default();
    let mut last_log = Instant::now();
    let mut last_selection_check = Instant::now();
    while running.load(Ordering::SeqCst) {
        if last_selection_check.elapsed() >= Duration::from_secs(1) {
            if model_paths().is_ok_and(|paths| paths.gate != selected_gate) {
                return Ok(());
            }
            last_selection_check = Instant::now();
        }
        let frame = feed.read().unwrap().latest.clone();
        let Some(frame) = frame else {
            thread::sleep(Duration::from_millis(10));
            continue;
        };
        if Some(frame.sequence) == last_sequence || frame.received_at.elapsed() > TONGUE_FRESH_FOR {
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        let started = Instant::now();
        stdin
            .write_all(&frame.sequence.to_le_bytes())
            .map_err(|error| error.to_string())?;
        stdin
            .write_all(&frame.pixels)
            .map_err(|error| error.to_string())?;
        stdin.flush().map_err(|error| error.to_string())?;
        let mut response = [0u8; 8 + TONGUE_HEADS * 4];
        stdout
            .read_exact(&mut response)
            .map_err(|error| error.to_string())?;
        let result_sequence = u64::from_le_bytes(response[..8].try_into().unwrap());
        if result_sequence != frame.sequence {
            return Err("model response sequence mismatch".into());
        }
        let mut values = [0f32; TONGUE_HEADS];
        for (index, value) in values.iter_mut().enumerate() {
            let offset = 8 + index * 4;
            *value = f32::from_le_bytes(response[offset..offset + 4].try_into().unwrap());
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err("model returned non-finite tongue values".into());
        }
        if let Some(previous) = last_sequence {
            dropped_frames += frame.sequence.saturating_sub(previous.saturating_add(1));
        }
        last_sequence = Some(frame.sequence);
        let inference_ms = started.elapsed().as_secs_f32() * 1000.0;
        let current = settings.get();
        let strength = if handshake.smooths_internally() {
            0.0
        } else {
            current.tongue_smoothing
        };
        let smoothed = smoother.update(values, frame.headset_ns, frame.received_at, strength);
        *latest.write().unwrap() = Some(TonguePrediction {
            sequence: frame.sequence,
            raw: values,
            values: smoothed,
            received_at: frame.received_at,
            inference_ms,
            dropped_frames,
            camera_weight: handshake.camera_weight,
            threshold: handshake.threshold,
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
            if u32_at(&header, 8) != 3
                || u32_at(&header, 12) != 64
                || u32_at(&header, 32) != WIDTH as u32
                || u32_at(&header, 36) != HEIGHT as u32
                || u32_at(&header, 40) != WIDTH as u32
                || u32_at(&header, 44) != 1
                || u32_at(&header, 48) != FRAME_BYTES as u32
                || (mask != MOUTH_MASK && mask != EYES_MASK)
            {
                return Err(invalid("Invalid QPLIVE3 frame header"));
            }
            let mut pixels = vec![0u8; FRAME_BYTES];
            stream.read_exact(&mut pixels)?;
            let frame = Frame {
                sequence: u64::from_le_bytes(header[16..24].try_into().unwrap()),
                headset_ns: u64::from_le_bytes(header[24..32].try_into().unwrap()),
                pixels: pixels.into(),
                received_at: Instant::now(),
            };
            Ok(Some(if mask == MOUTH_MASK {
                Message::Mouth(frame)
            } else {
                Message::Eyes(frame)
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
    use crate::quest_pro_settings::QuestProSettings;

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
        let values = [0.9, 0.6, -0.4, 0.7, 0.2, 0.3, 0.4, 0.5, 0.6, -0.8];
        TonguePrediction {
            sequence: 10,
            raw: values,
            values,
            received_at,
            inference_ms: 20.0,
            dropped_frames: 0,
            camera_weight: 0.95,
            threshold: 0.85,
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
        let mut overlay = overlay(Some(prediction(Instant::now())));
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
        assert_eq!(snapshot.source, "enhanced model");
        assert_eq!(snapshot.values, tongue_values(&data));
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
        assert_eq!(snapshot.source, "tracking module");
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

    #[test]
    fn tongue_out_is_the_larger_of_visibility_and_extension() {
        let values = [1.0, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(map_tongue(&values, 0.9, true)[0], 0.9);
        let extended = [1.0, 0.95, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(map_tongue(&extended, 0.9, true)[0], 0.95);
        assert_eq!(map_tongue(&values, 0.9, false), [0.0; 12]);
    }

    #[test]
    fn visibility_modes_follow_the_reference_formulas() {
        assert!((fuse_visibility(VisibilityMode::Weighted, 0.8, 0.9, 0.4) - 0.8).abs() < 1e-6);
        assert_eq!(fuse_visibility(VisibilityMode::Camera, 0.8, 0.9, 0.4), 0.9);
        assert_eq!(fuse_visibility(VisibilityMode::Native, 0.8, 0.9, 0.4), 0.4);
        assert_eq!(
            fuse_visibility(VisibilityMode::Agreement, 0.8, 0.9, 0.4),
            0.4
        );
    }

    #[test]
    fn agreement_mode_rejects_camera_only_detections() {
        let settings = QuestProSettings {
            tongue_visibility: VisibilityMode::Agreement,
            ..QuestProSettings::default()
        };
        let mut overlay = overlay_with(Some(prediction(Instant::now())), settings);
        let mut data = UnifiedTrackingData::default();
        overlay.observe(&data);
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
        let first = smoother.update([1.0; TONGUE_HEADS], 1_000_000_000, now, 55.0);
        assert_eq!(first, [1.0; TONGUE_HEADS]);
        let second = smoother.update([0.0; TONGUE_HEADS], 1_041_666_667, now, 55.0);
        assert!((second[0] - (1.0 - alpha)).abs() < 1e-4, "{second:?}");
        // A gap longer than 250 ms restarts from the raw value.
        let restarted = smoother.update([0.5; TONGUE_HEADS], 2_000_000_000, now, 55.0);
        assert_eq!(restarted, [0.5; TONGUE_HEADS]);
    }

    fn frame_bytes(mask: u32, sequence: u64) -> Vec<u8> {
        let mut bytes = vec![0u8; 64 + FRAME_BYTES];
        bytes[..7].copy_from_slice(b"QPLIVE3");
        for (at, value) in [
            (8, 3u32),
            (12, 64),
            (32, 800),
            (36, 400),
            (40, 800),
            (44, 1),
            (48, FRAME_BYTES as u32),
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
    fn stream_reader_rejects_unknown_masks_and_non_headset_bytes() {
        let mut reader = std::io::Cursor::new(frame_bytes(0x1f, 1));
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
            newer.message(),
            "The headset app (2027.1.0) is newer than this VRFT can read. Update VRFT"
        );
        let older = check_protocol(*PROTOCOLS.start() - 1, None).unwrap_err();
        assert_eq!(older.update, Update::HeadsetApp);
        assert_eq!(
            older.message(),
            "The headset app is too old for this VRFT. Update the headset app"
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
    fn connection_ends_when_the_headset_app_speaks_a_newer_protocol() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let headset = thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
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
        let mut eyes = EyeState::new(Err("test".into())).processor(settings);
        let result = connect_and_receive(
            address,
            &shared,
            &CaptureManager::default(),
            &mut eyes,
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
        assert!(state.status.ends_with("Update VRFT"));
        drop(state);
        headset.join().unwrap();
    }
}
