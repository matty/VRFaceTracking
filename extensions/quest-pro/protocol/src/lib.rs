//! Quest Pro's part of the daemon's local API, as types both halves of the
//! extension compile against: `vrft-quest-pro-daemon` serves them under
//! `/ext/quest-pro` and in `/status`, and `vrft-quest-pro-gui` reads them.
//! The trainer (`vrft-tongue`) writes [`TrainingProgress`] and
//! [`TrainingReport`] too.
//!
//! Every struct has a default for each field, so a reader built against an
//! older or newer version still parses what it gets.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// The extension's id, in `config.json`, URLs and `/status`.
pub const ID: &str = "quest-pro";
pub const NAME: &str = "Quest Pro";

/// The mouth cameras' image: two 400 x 400 8-bit grayscale cameras side by
/// side, as are the eye cameras' snapshots and recorded frames.
pub const FRAME_WIDTH: u32 = 800;
pub const FRAME_HEIGHT: u32 = 400;
pub const FRAME_BYTES: usize = (FRAME_WIDTH * FRAME_HEIGHT) as usize;
/// Response header carrying a camera frame's sequence number.
pub const FRAME_SEQUENCE_HEADER: &str = "x-frame-sequence";

/// The tongue model's outputs: visibility, extension, horizontal, vertical,
/// six shape heads, then the left and right cheek puff.
pub const MODEL_HEADS: usize = 12;
/// VRFT's tongue expressions: TongueOut, Up, Down, Left, Right, then seven
/// shapes.
pub const TONGUE_SHAPES: usize = 12;

/// Routes, relative to the extension's base, `/ext/quest-pro`.
pub mod routes {
    /// GET: the browser preview page.
    pub const PAGE: &str = "/";
    /// GET: [`Status`](super::Status), which `/status` also carries.
    pub const STATUS: &str = "/status";
    /// GET: the latest mouth camera frame as raw pixels, with
    /// [`FRAME_SEQUENCE_HEADER`](super::FRAME_SEQUENCE_HEADER); 204 before
    /// the first.
    pub const FRAME: &str = "/frame";
    /// GET: the latest eye camera snapshot, as [`FRAME`] is sent.
    pub const EYE_FRAME: &str = "/eye-frame";
    /// GET [`Settings`](super::Settings); POST a
    /// [`SettingsPatch`](super::SettingsPatch), answered with them all.
    pub const SETTINGS: &str = "/settings";
    /// POST: takes the last second of gaze as straight ahead; answers
    /// [`Settings`](super::Settings).
    pub const EYE_RECENTER: &str = "/eye/recenter";
    /// POST: undoes recentering; answers [`Settings`](super::Settings).
    pub const EYE_RECENTER_CLEAR: &str = "/eye/recenter/clear";
    /// POST: tries the headset again now instead of waiting out the backoff;
    /// answers [`Status`](super::Status).
    pub const RECONNECT: &str = "/reconnect";
    /// GET: [`CaptureStatus`](super::CaptureStatus).
    pub const CAPTURE_STATUS: &str = "/capture/status";
    /// POST a [`CaptureRequest`](super::CaptureRequest); answers
    /// [`CaptureStatus`](super::CaptureStatus). The running recording's
    /// commands are at [`CaptureCommand::route`](super::CaptureCommand::route).
    pub const CAPTURE_START: &str = "/capture/start";
    /// GET: the preview page's training script.
    pub const TRAINING_SCRIPT: &str = "/training.js";
    /// GET: every [`Recording`](super::Recording).
    pub const TRAINING_SESSIONS: &str = "/training/sessions";
    /// POST a [`ReviewRequest`](super::ReviewRequest); answers
    /// [`ReviewSaved`](super::ReviewSaved).
    pub const TRAINING_REVIEW: &str = "/training/review";
    /// POST a [`RecordingId`](super::RecordingId); answers
    /// [`RecordingDeleted`](super::RecordingDeleted).
    pub const TRAINING_DELETE: &str = "/training/delete";
    /// GET with [`FrameQuery`](super::FrameQuery): one recorded frame's
    /// pixels.
    pub const TRAINING_FRAME: &str = "/training/frame";
    /// POST a [`TrainRequest`](super::TrainRequest); answers
    /// [`TrainingStarted`](super::TrainingStarted).
    pub const TRAINING_START: &str = "/training/start";
    /// POST; answers [`TrainingCancelled`](super::TrainingCancelled).
    pub const TRAINING_CANCEL: &str = "/training/cancel";
    /// GET: [`TrainingStatus`](super::TrainingStatus).
    pub const TRAINING_STATUS: &str = "/training/status";
    /// GET: [`Models`](super::Models).
    pub const TRAINING_MODELS: &str = "/training/models";
    /// POST a [`RecordingId`](super::RecordingId) naming a model; answers
    /// [`ModelActivated`](super::ModelActivated).
    pub const TRAINING_ACTIVATE: &str = "/training/activate";
    /// POST a [`RecordingId`](super::RecordingId) naming a trained model;
    /// answers [`RecordingDeleted`](super::RecordingDeleted). The model in
    /// use can't be deleted.
    pub const TRAINING_DELETE_MODEL: &str = "/training/models/delete";
    /// POST a [`RenameModel`](super::RenameModel); answers
    /// [`SavedModel`](super::SavedModel).
    pub const TRAINING_RENAME_MODEL: &str = "/training/models/rename";
    /// POST: starts installing the built-in model; answers
    /// [`BuiltinStatus`](super::BuiltinStatus).
    pub const TRAINING_BUILTIN: &str = "/training/builtin";
}

/// Quest Pro's status, in `/status` under `extensions.quest-pro` and at
/// [`routes::STATUS`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    /// The daemon's own description of the headset connection.
    pub status: String,
    /// Address of the headset stream, once connected.
    pub source: Option<String>,
    /// How long until the daemon tries the headset again, while it's backing
    /// off after a failed or lost connection.
    pub retry_in_ms: Option<u64>,
    /// The latest mouth camera frame's sequence number.
    pub sequence: Option<u64>,
    pub width: u32,
    pub height: u32,
    pub frame_age_ms: Option<u64>,
    pub eye_frame_sequence: Option<u64>,
    pub eye_frame_age_ms: Option<u64>,
    /// What the headset app last reported about itself.
    pub headset: Option<Headset>,
    /// Set while the headset app speaks a stream protocol the daemon can't read.
    pub headset_mismatch: Option<Mismatch>,
    /// Why the tongue model isn't running.
    pub model_error: Option<String>,
    pub model: Option<ModelStatus>,
    pub output: Option<OutputStatus>,
    pub eyes: EyeStatus,
    /// Yaw and pitch in degrees that VRFT sends for its left and right eye,
    /// after swapping and inversion, while independent gaze is live.
    pub eye_output_deg: Option<[[f32; 2]; 2]>,
    pub settings: Settings,
}

/// The headset app's `QPSTAT1` status. Fields this version doesn't know are
/// kept in `other`, so the preview page still sees them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Headset {
    pub apk_version: Option<String>,
    /// The stream protocol the app speaks.
    pub protocol: Option<u64>,
    pub camera_fps: Option<u32>,
    pub eye: Option<HeadsetEye>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// The headset app's eye pipeline.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeadsetEye {
    pub enabled: bool,
    pub state: String,
    pub message: String,
    /// Whether the headset runs the per-eye model rather than Meta's stock one.
    pub model_patched: bool,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// A headset app whose stream protocol this daemon doesn't read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mismatch {
    pub protocol: u32,
    pub apk_version: Option<String>,
    pub update: Update,
}

/// Which side needs updating before the daemon can use the headset app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Update {
    #[default]
    Vrft,
    HeadsetApp,
}

/// The tongue model's latest prediction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelStatus {
    pub sequence: u64,
    pub age_ms: u64,
    pub fresh: bool,
    pub inference_ms: f32,
    pub skipped_frames: u64,
    /// Unsmoothed, as the preview shows it.
    pub raw_values: [f32; MODEL_HEADS],
    /// After the smoothing setting; these drive VRFT.
    pub values: [f32; MODEL_HEADS],
    pub camera_weight: f32,
    pub threshold: f32,
}

/// Where the tongue, or the cheek puffs, VRFT sends came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TongueSource {
    #[default]
    #[serde(rename = "tracking module")]
    TrackingModule,
    #[serde(rename = "enhanced model")]
    EnhancedModel,
}

/// The tongue VRFT sends.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputStatus {
    pub source: TongueSource,
    /// The tracking module's TongueOut, when a module is sending frames.
    pub native_tongue_out: Option<f32>,
    pub fused_visibility: Option<f32>,
    pub visible: Option<bool>,
    /// TongueOut, Up, Down, Left, Right, then seven shapes.
    pub values: [f32; TONGUE_SHAPES],
    /// The model's cheek puffs are used once it has learned them.
    pub cheek_source: TongueSource,
    /// CheekPuffLeft and CheekPuffRight as sent.
    pub cheek_puffs: [f32; 2],
}

/// Independent per-eye gaze.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EyeStatus {
    /// Where the calibration in use came from.
    pub calibration: String,
    pub calibration_error: Option<String>,
    pub convergence_calibrated: bool,
    pub rate_hz: f32,
    pub age_ms: Option<u64>,
    pub fresh: bool,
    pub dropped_invalid: u64,
    pub sample: Option<EyeSample>,
    /// Pupil sizes measured from the eye camera snapshots.
    pub pupils: PupilStatus,
}

/// Pupil sizes measured from the eye camera snapshots.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PupilStatus {
    /// Whether VRFT is sending the measured sizes.
    pub fresh: bool,
    /// Snapshots measured per second.
    pub rate_hz: f32,
    /// Snapshots in which neither pupil could be found.
    pub missed: u64,
    /// Each eye's pupil as sent, in millimetres, from the left then the
    /// right half of the snapshot.
    pub diameter_mm: [Option<f32>; 2],
    /// How dilated the pupils are, 0 to 1 of the range seen so far.
    pub dilation: Option<f32>,
}

/// The latest gaze, in degrees.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EyeSample {
    pub sequence: u64,
    pub engine_profile: u32,
    /// Whether the headset runs the per-eye model rather than Meta's stock one.
    pub model_patched: bool,
    pub tag0_deg: [f64; 2],
    pub tag1_deg: [f64; 2],
    /// Filtered, calibrated, recentered physical-eye angles.
    pub left_deg: [f64; 2],
    pub right_deg: [f64; 2],
}

/// The settings people change, saved by the daemon in
/// `.local/quest-pro-settings.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 0 is most responsive, 100 smoothest. Matches the reference hub slider.
    pub tongue_smoothing: f32,
    pub tongue_visibility: VisibilityMode,
    /// Use independent per-eye gaze when the headset streams it.
    pub eye_gaze: bool,
    /// Publish the physical right eye on VRFT's left channel and vice versa,
    /// as the reference implementation does after testing in VRChat.
    pub eye_swap_output: bool,
    /// Troubleshooting: negate both horizontal gaze values.
    pub eye_invert_yaw: bool,
    pub eye_offsets: Option<EyeOffsets>,
    /// Send each cheek's puff from the tongue model, once it has learned them.
    pub cheek_puffs: bool,
    /// Measure pupil size from the eye camera snapshots.
    pub pupils: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tongue_smoothing: 55.0,
            tongue_visibility: VisibilityMode::Weighted,
            eye_gaze: true,
            eye_swap_output: true,
            eye_invert_yaw: false,
            eye_offsets: None,
            cheek_puffs: true,
            pupils: true,
        }
    }
}

/// Changes to some [`Settings`]. Recentering, which sets the eye offsets,
/// has its own routes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SettingsPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tongue_smoothing: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tongue_visibility: Option<VisibilityMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eye_gaze: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eye_swap_output: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eye_invert_yaw: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cheek_puffs: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pupils: Option<bool>,
}

/// How the camera model's visibility confidence combines with the tracking
/// module's native TongueOut before the show/hide threshold is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityMode {
    /// `w * camera + (1 - w) * native`, with `w` from the gate checkpoint.
    #[default]
    Weighted,
    /// Camera confidence only; ignores native TongueOut.
    Camera,
    /// Native TongueOut only. Direction still comes from the cameras.
    Native,
    /// `min(camera, native)`: shown only when both see the tongue out, so
    /// fewer false positives but more misses. It decides out or in only;
    /// direction still comes from the cameras.
    Agreement,
}

/// Per-eye yaw/pitch offsets in degrees, captured by recentering while the
/// wearer looks at a distant point straight ahead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct EyeOffsets {
    pub left_deg: [f64; 2],
    pub right_deg: [f64; 2],
}

/// The basic recording's poses that teach the cheek puffs, which can be
/// recorded on their own to add them to recordings made before them.
pub const CHEEK_POSES: [&str; 3] = [
    "Left cheek puffed",
    "Right cheek puffed",
    "Both cheeks puffed",
];

/// A guided tongue recording.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    /// Basic held poses.
    #[default]
    Core,
    /// More directions.
    Direction,
    /// Tongue in, with the mouth doing other things.
    Negatives,
    /// Follow a moving dot.
    Follow,
}

impl CaptureMode {
    pub const ALL: [CaptureMode; 4] = [Self::Core, Self::Direction, Self::Negatives, Self::Follow];

    /// The name in URLs, recordings' metadata and ids.
    pub fn name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Direction => "direction",
            Self::Negatives => "negatives",
            Self::Follow => "follow",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureRequest {
    pub mode: CaptureMode,
    /// Only these poses, by name, such as to fill in what's missing; every
    /// pose when empty. Not for following the dot.
    pub poses: Vec<String>,
}

/// Commands for the running recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureCommand {
    /// Leaves the current pose out and moves on.
    Skip,
    /// Pauses, or resumes when paused.
    Pause,
    Stop,
}

impl CaptureCommand {
    /// POST here; the answer is [`CaptureStatus`].
    pub fn route(self) -> &'static str {
        match self {
            Self::Skip => "/capture/skip",
            Self::Pause => "/capture/pause",
            Self::Stop => "/capture/stop",
        }
    }
}

/// The guided tongue recording, if one is running, else how the last ended.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureStatus {
    pub active: bool,
    pub mode: Option<CaptureMode>,
    pub pose: Option<String>,
    pub instruction: Option<String>,
    /// The pose after this one, so the wearer can get ready for it.
    pub next_pose: Option<String>,
    pub seconds_remaining: Option<f32>,
    /// The current step lasts `step_seconds`; only the time after
    /// `settle_seconds` is saved.
    pub step_seconds: f32,
    pub settle_seconds: f32,
    /// Follow the dot: `[seconds, horizontal, vertical]` keyframes of the
    /// current round, and the seconds since its dot started (negative while
    /// getting ready).
    pub path: Option<Vec<[f32; 3]>>,
    pub path_elapsed: Option<f32>,
    pub recording: bool,
    pub skipped: bool,
    pub paused: bool,
    /// Why the recording paused by itself; `None` when the user paused it.
    /// It resumes by itself when the reason goes away.
    pub pause_reason: Option<PauseReason>,
    /// 1-based.
    pub step: Option<usize>,
    pub total_steps: Option<usize>,
    pub samples: u64,
    /// Folder of the running recording, or of the last one.
    pub directory: Option<String>,
    /// Whether the tracking module's own face tracking is arriving. Optional:
    /// recordings save its TongueOut beside each frame when it is.
    pub native_recent: bool,
    pub message: String,
}

/// Why a recording paused by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// The mouth cameras stopped sending.
    CamerasStopped,
}

impl PauseReason {
    /// What to say, on screen and aloud.
    pub fn message(self) -> &'static str {
        match self {
            Self::CamerasStopped => "Paused: the mouth cameras stopped",
        }
    }
}

/// One tongue recording on this PC.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recording {
    /// `<unix ms>-<mode>-<pid>`.
    pub id: String,
    pub mode: Option<CaptureMode>,
    pub frames: u64,
    pub poses: Vec<RecordedPose>,
    pub positive_frames: u64,
    pub negative_frames: u64,
    pub coverage: Coverage,
    /// Enough of everything for training on its own.
    pub basic_ready: bool,
    /// Why the recording can't be read; nothing else is filled in then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordedPose {
    pub step: u64,
    pub name: String,
    pub frames: u64,
    /// Frame indices of the pose's start, middle and end.
    pub indices: [u64; 3],
    /// Skipped while recording, so it can't be ticked back in.
    pub skipped: bool,
    /// Left out of training, whether skipped or unticked in review.
    pub excluded: bool,
    /// The headset's own tongue tracking mostly disagreed with what the pose
    /// asked for, so it may have gone wrong and is worth a look.
    pub suspect: bool,
}

/// Usable frames in a recording: tongue out and in, out in each basic
/// direction, and each cheek puffed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Coverage {
    pub out: u64,
    #[serde(rename = "in")]
    pub inside: u64,
    pub left: u64,
    pub right: u64,
    pub up: u64,
    pub down: u64,
    /// Frames with each cheek puffed, which training needs 8 of to learn it.
    pub cheek_left: u64,
    pub cheek_right: u64,
}

/// Leaves the given poses of a recording out of training.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewRequest {
    pub id: String,
    pub excluded_steps: Vec<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewSaved {
    pub saved: bool,
}

/// A recording or a model, by id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingId {
    pub id: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingDeleted {
    pub deleted: bool,
}

/// One recorded frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameQuery {
    pub id: String,
    pub index: u64,
}

/// Where training runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingDevice {
    /// The GPU when one works, else the CPU.
    #[default]
    Auto,
    Cpu,
    Gpu,
    Cuda,
}

impl TrainingDevice {
    /// The name the trainer reads.
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Cuda => "cuda",
        }
    }
}

/// Trains a personal model from recordings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrainRequest {
    pub name: String,
    /// Recording ids.
    pub recordings: Vec<String>,
    pub device: TrainingDevice,
    /// 1 to 60.
    pub epochs: u32,
}

/// What the daemon asks the trainer (`vrft_d train-tongue`) for, in the new
/// model folder's `request.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrainerRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub device: TrainingDevice,
    /// The model pair training starts from.
    pub base_model_dir: PathBuf,
    /// The recordings' folders.
    pub recordings: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrainingStarted {
    /// The new model's id.
    pub id: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrainingCancelled {
    pub cancelled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrainingStatus {
    pub busy: bool,
    /// The last job started since VRFT started.
    pub id: Option<String>,
    pub progress: Option<TrainingProgress>,
    /// The model in use; `demo` is the built-in one.
    pub active_id: String,
    /// `VRFT_TONGUE_MODEL_DIR` pins the model, so selection is off.
    pub model_override: bool,
    /// The built-in model pair; absent from daemons that can't install it.
    pub builtin: Option<BuiltinStatus>,
}

/// Where a training run is, as the trainer writes it to `progress.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrainingProgress {
    pub stage: TrainingStage,
    pub message: String,
    /// 0 to 1, across the whole run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<f64>,
    /// Which model is training: `gate` or `direction`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epochs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Once `complete`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<TrainingReport>,
}

impl TrainingProgress {
    pub fn new(stage: TrainingStage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingStage {
    Checking,
    Training,
    Calibrating,
    Complete,
    Failed,
    Cancelled,
    /// A stage from a newer trainer.
    #[default]
    #[serde(other)]
    Unknown,
}

/// What a finished training run made, as the trainer writes it to
/// `report.json` beside the model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrainingReport {
    pub name: String,
    pub device: String,
    /// The recordings' folders.
    pub recordings: Vec<String>,
    pub frames: Option<u64>,
    pub coverage: Value,
    pub epochs: u32,
    pub supported_targets: Vec<String>,
    pub disabled_targets: Vec<String>,
    pub base_model_dir: String,
    pub calibration: ReportCalibration,
    pub seconds: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReportCalibration {
    pub camera_weight: f64,
    pub threshold: f64,
    pub plateau: Option<Value>,
}

/// The built-in model pair.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuiltinStatus {
    pub installed: bool,
    pub installing: bool,
    /// Download progress, 0 to 1.
    pub fraction: Option<f32>,
    /// Why the last install failed.
    pub error: Option<String>,
}

/// The built-in model and every personal one trained on this PC.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Models {
    pub active_id: String,
    pub models: Vec<SavedModel>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedModel {
    /// `demo` for the built-in model, else `<unix ms>-<pid>`.
    pub id: String,
    pub name: Option<String>,
    pub report: Option<TrainingReport>,
}

/// Gives a trained model a new name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenameModel {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelActivated {
    pub active_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_and_keeps_unknown_headset_fields() {
        let json = serde_json::json!({
            "status": "Live", "source": "192.168.1.20:27274", "sequence": 812,
            "width": 800, "height": 400, "frame_age_ms": 31,
            "headset": {"apk_version": "0.2", "protocol": 3, "camera_fps": 24, "eye_preview_fps": 2,
                        "eye": {"enabled": true, "state": "running", "message": "",
                                "model_patched": true, "restore_pending": false}},
            "output": {"source": "enhanced model", "native_tongue_out": 0.4,
                       "values": [0.9, 0, 0.2, 0, 0.1, 0, 0, 0, 0, 0, 0, 0]},
        });
        let status: Status = serde_json::from_value(json).unwrap();
        let headset = status.headset.as_ref().unwrap();
        assert_eq!(headset.camera_fps, Some(24));
        assert_eq!(headset.other["eye_preview_fps"], 2);
        assert_eq!(
            headset.eye.as_ref().unwrap().other["restore_pending"],
            false
        );
        assert_eq!(
            status.output.as_ref().unwrap().source,
            TongueSource::EnhancedModel
        );
        assert!(
            status.settings.eye_gaze,
            "missing settings take the defaults"
        );

        let again: Status = serde_json::from_value(serde_json::to_value(&status).unwrap()).unwrap();
        assert_eq!(again, status);
        let written = serde_json::to_value(&status).unwrap();
        assert_eq!(written["headset"]["eye_preview_fps"], 2);
    }

    #[test]
    fn a_settings_patch_carries_only_what_changes_and_rejects_the_unknown() {
        let patch = SettingsPatch {
            eye_gaze: Some(false),
            ..SettingsPatch::default()
        };
        assert_eq!(
            serde_json::to_value(&patch).unwrap(),
            serde_json::json!({"eye_gaze": false})
        );
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"not_a_setting": 1}"#).is_err());
    }

    #[test]
    fn capture_modes_and_training_stages_read_by_name() {
        for mode in CaptureMode::ALL {
            assert_eq!(serde_json::to_value(mode).unwrap(), mode.name());
            assert_eq!(CaptureMode::from_name(mode.name()), Some(mode));
        }
        let progress: TrainingProgress =
            serde_json::from_str(r#"{"stage": "sharpening", "message": "New"}"#).unwrap();
        assert_eq!(progress.stage, TrainingStage::Unknown);
        let progress: TrainingProgress = serde_json::from_str(
            r#"{"stage": "complete", "message": "Done", "fraction": 1.0,
                "report": {"name": "Mine", "recordings": ["a"], "seconds": 12.5,
                           "calibration": {"camera_weight": 0.7, "threshold": 0.5}}}"#,
        )
        .unwrap();
        assert_eq!(progress.stage, TrainingStage::Complete);
        assert_eq!(progress.report.unwrap().seconds, Some(12.5));
    }

    #[test]
    fn an_unreadable_recording_is_only_its_id_and_error() {
        let recording = Recording {
            id: "broken".into(),
            error: Some("Frame/label count mismatch".into()),
            ..Recording::default()
        };
        let json = serde_json::to_value(&recording).unwrap();
        assert_eq!(json["error"], "Frame/label count mismatch");
        let recording: Recording =
            serde_json::from_str(r#"{"id": "fine", "coverage": {"in": 4}}"#).unwrap();
        assert_eq!(recording.error, None);
        assert_eq!(recording.coverage.inside, 4);
    }
}
