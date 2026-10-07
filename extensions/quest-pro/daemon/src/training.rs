//! Local capture review, training jobs and reversible model selection.
use crate::capture::CaptureManager;
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::builtin::{self, BuiltinModel};
use crate::transfer::Transfers;
use vrft_quest_pro_protocol::CameraLayout;
use vrft_quest_pro_protocol::{
    routes, BuiltinStatus, CaptureMode, Coverage, ExportModel, FrameQuery, ImportModel,
    ModelActivated, Models, RecordedPose, Recording, RecordingDeleted, RecordingId, RenameModel,
    ReviewRequest, ReviewSaved, SavedModel, TrainRequest, TrainerArchitecture, TrainerRequest,
    TrainingCancelled, TrainingProgress, TrainingReport, TrainingStage, TrainingStarted,
    TrainingStatus, TransferStatus, MOUTH_CAMERAS,
};
use vrft_tongue::Role;

/// Whether `dir` holds both halves of a model pair.
pub fn complete_pair(dir: &Path) -> bool {
    Role::Gate.find(dir).is_some() && Role::Direction.find(dir).is_some()
}

/// Whether `dir` holds a universal face model.
pub fn has_face_model(dir: &Path) -> bool {
    dir.join(vrft_tongue::universal::FILE_NAME).is_file()
}

/// Whether `dir` holds a model inference can use: a pair, or a universal
/// face model, which runs beside the built-in pair.
pub fn complete_model(dir: &Path) -> bool {
    complete_pair(dir) || has_face_model(dir)
}

fn architecture(dir: &Path) -> TrainerArchitecture {
    if has_face_model(dir) && !complete_pair(dir) {
        TrainerArchitecture::UniversalFace
    } else {
        TrainerArchitecture::StereoPair
    }
}
type ApiError = (StatusCode, String);
fn bad(error: impl ToString) -> ApiError {
    (StatusCode::BAD_REQUEST, error.to_string())
}

#[derive(Default)]
struct Job {
    child: Option<Child>,
    id: Option<String>,
    /// How the job ended, when the trainer couldn't say itself.
    terminal: Option<TrainingProgress>,
    /// The trainer runs below normal priority, as VRChat is running.
    below_normal: bool,
}

/// Sets the trainer's priority for whether VRChat is running, when that
/// changed.
fn give_way(job: &mut Job, vrchat: bool) {
    let Some(child) = job.child.as_ref() else {
        return;
    };
    if job.below_normal == vrchat {
        return;
    }
    // Recorded either way, so a failure is logged once, not every check.
    job.below_normal = vrchat;
    match crate::priority::set_below_normal(child, vrchat) {
        Ok(()) if vrchat => {
            log::info!("VRChat is running, so tongue training runs at below-normal priority")
        }
        Ok(()) => log::info!("VRChat has closed, so tongue training runs at normal priority"),
        Err(error) => log::warn!("Couldn't change tongue training's priority: {error}"),
    }
}

#[derive(Clone)]
pub struct TrainingManager {
    root: PathBuf,
    capture: CaptureManager,
    job: Arc<Mutex<Job>>,
    builtin: BuiltinModel,
    qftplus: BuiltinModel,
    transfers: Transfers,
}

impl TrainingManager {
    pub fn new(root: PathBuf, capture: CaptureManager) -> Self {
        Self {
            root,
            capture,
            job: Arc::new(Mutex::new(Job::default())),
            builtin: BuiltinModel::default(),
            qftplus: BuiltinModel::qftplus(),
            transfers: Transfers::default(),
        }
    }
    pub fn busy(&self) -> bool {
        let mut job = self.job.lock().unwrap();
        poll_job(&mut job, &self.root);
        job.child.is_some()
    }
    /// Runs the trainer below normal priority while VRChat runs, so training
    /// doesn't take CPU time VRChat needs, and at normal priority otherwise.
    pub fn yield_to_vrchat(&self) {
        let mut job = self.job.lock().unwrap();
        if job.child.is_some() {
            give_way(&mut job, crate::priority::vrchat_running());
        }
    }
    pub fn cancel(&self) {
        let mut job = self.job.lock().unwrap();
        if let Some(mut child) = job.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            job.terminal = Some(TrainingProgress::new(
                TrainingStage::Cancelled,
                "Training cancelled. Your active model is unchanged.",
            ));
        }
    }
    fn idle(&self) -> Result<(), ApiError> {
        if self.busy() || self.capture.status().active {
            Err(bad("Finish the current recording or training first"))
        } else {
            Ok(())
        }
    }
    /// For changing recordings and models, which an export may be copying.
    fn untouched(&self) -> Result<(), ApiError> {
        if self.transfers.busy() {
            Err(bad("Wait for the model export or import to finish"))
        } else {
            Ok(())
        }
    }
}

pub fn routes(manager: TrainingManager) -> Router {
    let mut router = Router::new();
    if crate::camera::BROWSER_PAGES {
        router = router.route(
            routes::TRAINING_SCRIPT,
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("training.js"),
                )
            }),
        );
    }
    router
        .route(routes::TRAINING_SESSIONS, get(sessions))
        .route(routes::TRAINING_REVIEW, post(review))
        .route(routes::TRAINING_DELETE, post(delete_recording))
        .route(routes::TRAINING_FRAME, get(frame))
        .route(routes::TRAINING_START, post(start))
        .route(routes::TRAINING_CANCEL, post(cancel))
        .route(routes::TRAINING_STATUS, get(status))
        .route(routes::TRAINING_MODELS, get(models))
        .route(routes::TRAINING_ACTIVATE, post(activate))
        .route(routes::TRAINING_DELETE_MODEL, post(delete_model))
        .route(routes::TRAINING_RENAME_MODEL, post(rename_model))
        .route(routes::TRAINING_BUILTIN, post(install_builtin))
        .route(routes::TRAINING_BUILTIN_CANCEL, post(cancel_builtin))
        .route(routes::TRAINING_QFTPLUS, post(install_qftplus))
        .route(routes::TRAINING_QFTPLUS_CANCEL, post(cancel_qftplus))
        .route(routes::TRAINING_QFTPLUS_REMOVE, post(remove_qftplus))
        .route(routes::TRAINING_EXPORT_MODEL, post(export_model))
        .route(routes::TRAINING_IMPORT_MODEL, post(import_model))
        .with_state(manager)
}

pub(crate) fn safe_child(root: &Path, id: &str) -> Result<PathBuf, String> {
    if id.is_empty()
        || id.len() > 120
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("Invalid recording or model identifier".into());
    }
    let child = root.join(id).canonicalize().map_err(|e| e.to_string())?;
    let parent = root.canonicalize().map_err(|e| e.to_string())?;
    if child.parent() != Some(parent.as_path()) {
        return Err("Path leaves the data directory".into());
    }
    Ok(child)
}

fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
fn save_json(path: &Path, value: &Value) -> Result<(), String> {
    let temp = path.with_extension("json.tmp");
    fs::write(
        &temp,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(temp, path).map_err(|e| e.to_string())
}

fn labels(path: &Path) -> Result<Vec<Value>, String> {
    let file = File::open(path.join("samples.jsonl")).map_err(|e| e.to_string())?;
    BufReader::new(file)
        .lines()
        .map(|line| {
            serde_json::from_str(&line.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        })
        .collect()
}

fn excluded(path: &Path, file: &str) -> Result<Vec<u64>, String> {
    let file = path.join(file);
    if !file.exists() {
        return Ok(vec![]);
    }
    let value = read_json(&file)?;
    serde_json::from_value(value["excluded_steps"].clone()).map_err(|e| e.to_string())
}

async fn sessions(
    State(manager): State<TrainingManager>,
) -> Result<Json<Vec<Recording>>, ApiError> {
    let root = manager.root.join(".local/tongue-captures");
    let mut result = vec![];
    if root.exists() {
        let active = manager.capture.status();
        for entry in fs::read_dir(&root).map_err(bad)? {
            let entry = entry.map_err(bad)?;
            if !entry.file_type().map_err(bad)?.is_dir() {
                continue;
            }
            // Hidden folders are imports still being copied in.
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let shown = path.to_string_lossy();
            let writing = |directory: &Option<String>| directory.as_deref() == Some(&*shown);
            // A recording still being written is not listed or trainable yet.
            if active.active && writing(&active.directory) {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            let details = (|| -> Result<Recording, String> {
                let metadata = read_json(&path.join("metadata.json"))?;
                let layout = CameraLayout::from_metadata(&metadata)?;
                let samples = labels(&path)?;
                if fs::metadata(path.join("frames.gray8"))
                    .map_err(|e| e.to_string())?
                    .len()
                    != samples.len() as u64 * layout.frame_bytes() as u64
                {
                    return Err("Frame/label count mismatch".into());
                }
                let skipped = excluded(&path, "excluded_steps.json")?;
                let rejected = excluded(&path, "review.json")?;
                let mut poses: BTreeMap<u64, Vec<&Value>> = BTreeMap::new();
                for sample in &samples {
                    poses
                        .entry(sample["step"].as_u64().ok_or("Invalid pose step")?)
                        .or_default()
                        .push(sample);
                }
                let usable: Vec<_> = poses
                    .iter()
                    .filter(|(step, values)| {
                        values.len() >= 8 && !skipped.contains(step) && !rejected.contains(step)
                    })
                    .flat_map(|(_, values)| values.iter().copied())
                    .collect();
                let target = |s: &Value, column: usize| s["targets"][column].as_f64().unwrap_or(0.);
                let visible: Vec<&Value> = usable
                    .iter()
                    .copied()
                    .filter(|s| target(s, 0) >= 0.5)
                    .collect();
                let positives = visible.len();
                let negatives = usable.len() - positives;
                // Visible frames per basic direction; the trainer needs 8 of each.
                let direction = |column: usize, sign: f64| {
                    visible
                        .iter()
                        .filter(|s| target(s, column) * sign > 0.1)
                        .count()
                };
                // Cheek puffs count with the tongue in or out; recordings
                // from before them have no such label.
                let puffed =
                    |column: usize| usable.iter().filter(|s| target(s, column) > 0.1).count();
                let coverage = Coverage {
                    out: positives as u64,
                    inside: negatives as u64,
                    left: direction(2, -1.) as u64,
                    right: direction(2, 1.) as u64,
                    up: direction(3, 1.) as u64,
                    down: direction(3, -1.) as u64,
                    cheek_left: puffed(10) as u64,
                    cheek_right: puffed(11) as u64,
                };
                let directions = [coverage.left, coverage.right, coverage.up, coverage.down]
                    .iter()
                    .all(|count| *count >= 8);
                let index = |sample: &Value| sample["index"].as_u64().unwrap_or(0);
                // Most of a pose's frames with the headset seeing the tongue
                // one way and the prompt asking for the other.
                let suspect = |values: &[&Value]| {
                    let judged: Vec<bool> = values
                        .iter()
                        .filter_map(|sample| {
                            let native = sample["native_tongue_out"].as_f64()?;
                            Some((native >= 0.5) != (target(sample, 0) >= 0.5))
                        })
                        .collect();
                    judged.len() >= 8
                        && judged.iter().filter(|disagrees| **disagrees).count() * 10
                            > judged.len() * 6
                };
                let poses = poses
                    .into_iter()
                    .map(|(step, values)| RecordedPose {
                        step,
                        name: values[0]["pose"].as_str().unwrap_or_default().to_owned(),
                        frames: values.len() as u64,
                        indices: [
                            index(values[0]),
                            index(values[values.len() / 2]),
                            index(values[values.len() - 1]),
                        ],
                        skipped: skipped.contains(&step),
                        excluded: rejected.contains(&step) || skipped.contains(&step),
                        suspect: suspect(&values),
                    })
                    .collect();
                Ok(Recording {
                    id: id.clone(),
                    mode: metadata["mode"].as_str().and_then(CaptureMode::from_name),
                    frames: samples.len() as u64,
                    poses,
                    positive_frames: positives as u64,
                    negative_frames: negatives as u64,
                    coverage,
                    basic_ready: positives >= 20 && negatives >= 20 && directions,
                    error: None,
                })
            })();
            result.push(details.unwrap_or_else(|error| Recording {
                id,
                error: Some(error),
                ..Recording::default()
            }));
        }
    }
    result.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Json(result))
}

async fn review(
    State(manager): State<TrainingManager>,
    Json(request): Json<ReviewRequest>,
) -> Result<Json<ReviewSaved>, ApiError> {
    manager.idle()?;
    manager.untouched()?;
    let path =
        safe_child(&manager.root.join(".local/tongue-captures"), &request.id).map_err(bad)?;
    let samples = labels(&path).map_err(bad)?;
    if request
        .excluded_steps
        .iter()
        .any(|step| !samples.iter().any(|s| s["step"].as_u64() == Some(*step)))
    {
        return Err(bad("Unknown pose step"));
    }
    save_json(
        &path.join("review.json"),
        &json!({"excluded_steps":request.excluded_steps}),
    )
    .map_err(bad)?;
    Ok(Json(ReviewSaved { saved: true }))
}

async fn delete_recording(
    State(manager): State<TrainingManager>,
    Json(request): Json<RecordingId>,
) -> Result<Json<RecordingDeleted>, ApiError> {
    // Idle: never delete the recording being captured or read by training.
    manager.idle()?;
    manager.untouched()?;
    let path =
        safe_child(&manager.root.join(".local/tongue-captures"), &request.id).map_err(bad)?;
    if !path.join("metadata.json").is_file() {
        return Err(bad("Not a recording"));
    }
    fs::remove_dir_all(&path).map_err(bad)?;
    // The face setup in use goes with its recording; the face model then
    // runs without one until the next.
    let pointer = manager.root.join(crate::capture::ENROLLMENT_FILE);
    let in_use = read_json(&pointer)
        .ok()
        .is_some_and(|value| value["recording"].as_str() == Some(request.id.as_str()));
    if in_use {
        fs::remove_file(&pointer).map_err(bad)?;
    }
    Ok(Json(RecordingDeleted { deleted: true }))
}

async fn frame(
    State(manager): State<TrainingManager>,
    Query(request): Query<FrameQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let path =
        safe_child(&manager.root.join(".local/tongue-captures"), &request.id).map_err(bad)?;
    let layout = CameraLayout::from_metadata(&read_json(&path.join("metadata.json")).map_err(bad)?)
        .map_err(bad)?;
    let frame_bytes = layout.frame_bytes() as u64;
    let mut file = File::open(path.join("frames.gray8")).map_err(bad)?;
    if request.index >= file.metadata().map_err(bad)?.len() / frame_bytes {
        return Err(bad("Frame is outside recording"));
    }
    file.seek(SeekFrom::Start(request.index * frame_bytes))
        .map_err(bad)?;
    let mut frame = vec![0; frame_bytes as usize];
    file.read_exact(&mut frame).map_err(bad)?;
    // Review shows the mouth pair, which every recording holds.
    let pixels = layout
        .select(&frame, &MOUTH_CAMERAS)
        .filter(|_| layout.view == vrft_quest_pro_protocol::VIEW as usize)
        .ok_or_else(|| bad("This recording has no headset-sized mouth cameras"))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        pixels,
    ))
}

async fn start(
    State(manager): State<TrainingManager>,
    Json(request): Json<TrainRequest>,
) -> Result<Json<TrainingStarted>, ApiError> {
    manager.idle()?;
    if request.name.trim().is_empty()
        || request.name.len() > 100
        || !(1..=60).contains(&request.epochs)
    {
        return Err(bad("Invalid training name or epoch count"));
    }
    if request.recordings.is_empty() {
        return Err(bad("Tick at least one recording to train on"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut recordings = vec![];
    for id in request.recordings.iter().filter(|id| seen.insert(*id)) {
        recordings.push(safe_child(&manager.root.join(".local/tongue-captures"), id).map_err(bad)?);
    }
    let base = crate::camera::base_model_dir(&manager.root).map_err(bad)?;
    // The synthetic examples keep what the user's recordings don't show.
    // They hold the pair's 224 px mouth views, which the face model can't
    // read.
    if request.architecture == TrainerArchitecture::StereoPair {
        recordings.extend(builtin::examples_dir(&manager.root));
    }
    // Training runs in a child vrft_d, so cancelling can simply end it.
    let trainer = std::env::current_exe().map_err(bad)?;
    let id = format!(
        "{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(bad)?
            .as_millis(),
        std::process::id()
    );
    let parent = manager.root.join(".local/tongue-models");
    fs::create_dir_all(&parent).map_err(bad)?;
    let output = parent.join(&id);
    fs::create_dir(&output).map_err(bad)?;
    let trainer_request = TrainerRequest {
        name: Some(request.name.trim().to_owned()),
        device: request.device,
        base_model_dir: base,
        recordings,
        architecture: request.architecture,
    };
    save_json(
        &output.join("request.json"),
        &serde_json::to_value(&trainer_request).map_err(bad)?,
    )
    .map_err(bad)?;
    let log = File::create(output.join("training.log")).map_err(bad)?;
    let mut command = Command::new(trainer);
    command
        .arg("train-tongue")
        .arg("--request")
        .arg(output.join("request.json"))
        .arg("--output")
        .arg(&output)
        .arg("--epochs")
        .arg(request.epochs.to_string())
        .current_dir(&manager.root)
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(bad)?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let vrchat = crate::priority::vrchat_running();
    // Reserve the job under the mutex immediately before spawning.
    let mut job = manager.job.lock().unwrap();
    if job.child.is_some() {
        return Err(bad("A training job is already running"));
    }
    let child = command.spawn().map_err(bad)?;
    // Left running, it would keep the CPU after the daemon stops.
    vrft_api::job::end_with_daemon(&child);
    job.child = Some(child);
    job.id = Some(id.clone());
    job.terminal = None;
    job.below_normal = false;
    give_way(&mut job, vrchat);
    Ok(Json(TrainingStarted { id }))
}

async fn cancel(State(manager): State<TrainingManager>) -> Json<TrainingCancelled> {
    manager.cancel();
    Json(TrainingCancelled { cancelled: true })
}

/// What the trainer last wrote to job `id`'s `progress.json`.
fn read_progress(root: &Path, id: &str) -> Option<TrainingProgress> {
    let path = root
        .join(".local/tongue-models")
        .join(id)
        .join("progress.json");
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

async fn status(State(manager): State<TrainingManager>) -> Json<TrainingStatus> {
    let mut job = manager.job.lock().unwrap();
    poll_job(&mut job, &manager.root);
    let progress = job.terminal.clone().or_else(|| {
        job.id
            .as_ref()
            .and_then(|id| read_progress(&manager.root, id))
    });
    Json(TrainingStatus {
        busy: job.child.is_some(),
        id: job.id.clone(),
        progress,
        active_id: active_id(&manager.root),
        model_override: std::env::var_os("VRFT_TONGUE_MODEL_DIR").is_some(),
        builtin: Some(builtin_status(&manager)),
        qftplus: Some(qftplus_status(&manager)),
        transfer: manager.transfers.status(),
    })
}

fn builtin_status(manager: &TrainingManager) -> BuiltinStatus {
    let state = manager.builtin.state();
    BuiltinStatus {
        installed: builtin::installed(&manager.root),
        examples_missing: builtin::examples_dir(&manager.root).is_none(),
        download_megabytes: Some(builtin::download_megabytes(&manager.root)),
        installing: state.installing,
        fraction: state.fraction.map(|fraction| fraction as f32),
        error: state.error,
        stage: state.stage,
        received_bytes: state.received,
        total_bytes: state.total,
        bytes_per_second: state.bytes_per_second,
        cancelled: state.cancelled,
    }
}

/// Downloads and verifies the built-in model pair and the training
/// examples in the background.
async fn install_builtin(State(manager): State<TrainingManager>) -> Json<BuiltinStatus> {
    if builtin::download_megabytes(&manager.root) > 0 {
        manager.builtin.start(manager.root.clone());
    }
    Json(builtin_status(&manager))
}

async fn cancel_builtin(State(manager): State<TrainingManager>) -> Json<BuiltinStatus> {
    manager.builtin.cancel();
    Json(builtin_status(&manager))
}

fn qftplus_status(manager: &TrainingManager) -> BuiltinStatus {
    let state = manager.qftplus.state();
    BuiltinStatus {
        installed: builtin::qftplus_model(&manager.root).is_some(),
        examples_missing: false,
        download_megabytes: Some(builtin::qftplus_megabytes(&manager.root)),
        installing: state.installing,
        fraction: state.fraction.map(|fraction| fraction as f32),
        error: state.error,
        stage: state.stage,
        received_bytes: state.received,
        total_bytes: state.total,
        bytes_per_second: state.bytes_per_second,
        cancelled: state.cancelled,
    }
}

/// Downloads QFT+'s universal face model from QFT+'s release in the
/// background. Only ever on the user's say-so: its weights are for
/// non-commercial use.
async fn install_qftplus(State(manager): State<TrainingManager>) -> Json<BuiltinStatus> {
    if builtin::qftplus_megabytes(&manager.root) > 0 {
        manager.qftplus.start(manager.root.clone());
    }
    Json(qftplus_status(&manager))
}

async fn cancel_qftplus(State(manager): State<TrainingManager>) -> Json<BuiltinStatus> {
    manager.qftplus.cancel();
    Json(qftplus_status(&manager))
}

/// Removes QFT+'s model; the face model in use unloads within a second.
async fn remove_qftplus(
    State(manager): State<TrainingManager>,
) -> Result<Json<BuiltinStatus>, ApiError> {
    if manager.qftplus.state().installing {
        return Err(bad("Wait for QFT+'s model to finish downloading"));
    }
    builtin::remove_qftplus(&manager.root).map_err(bad)?;
    Ok(Json(qftplus_status(&manager)))
}

/// Starts copying a trained model and its recordings into a new folder.
async fn export_model(
    State(manager): State<TrainingManager>,
    Json(request): Json<ExportModel>,
) -> Result<Json<TransferStatus>, ApiError> {
    safe_child(&manager.root.join(".local/tongue-models"), &request.id).map_err(bad)?;
    manager
        .transfers
        .export(manager.root.clone(), request.id, request.folder)
        .map(Json)
        .map_err(bad)
}

/// Starts adding an exported model from its folder or a zip.
async fn import_model(
    State(manager): State<TrainingManager>,
    Json(request): Json<ImportModel>,
) -> Result<Json<TransferStatus>, ApiError> {
    manager
        .transfers
        .import(manager.root.clone(), request.path)
        .map(Json)
        .map_err(bad)
}

fn poll_job(job: &mut Job, root: &Path) {
    if let Some(child) = job.child.as_mut() {
        if let Ok(Some(exit)) = child.try_wait() {
            job.child = None;
            if exit.success() {
                // Put the finished model straight into use, unless an
                // environment override pins the model directory.
                let selected = match job.id.as_deref() {
                    Some(id) if std::env::var_os("VRFT_TONGUE_MODEL_DIR").is_none() => {
                        select_model(root, id)
                    }
                    _ => Ok(()),
                };
                if let Err(error) = selected {
                    job.terminal = Some(TrainingProgress::new(
                        TrainingStage::Failed,
                        format!("Training finished, but the new model could not be switched on: {error}"),
                    ));
                }
            } else {
                let reported = job.id.as_ref().and_then(|id| read_progress(root, id));
                job.terminal = Some(
                    reported
                        .filter(|progress| progress.stage == TrainingStage::Failed)
                        .unwrap_or_else(|| {
                            TrainingProgress::new(
                                TrainingStage::Failed,
                                "the trainer exited with an error. See training.log in the model's folder.",
                            )
                        }),
                );
            }
        }
    }
}

fn active_id(root: &Path) -> String {
    read_json(&root.join(".local/tongue-active.json"))
        .ok()
        .and_then(|v| v["id"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "demo".into())
}

/// The folder of the model in use; `base` finds the built-in one, and is
/// only asked when that's the one in use.
pub fn selected_dir(
    root: &Path,
    base: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    if std::env::var_os("VRFT_TONGUE_MODEL_DIR").is_some() {
        return base();
    }
    let id = active_id(root);
    if id == "demo" {
        return base();
    }
    safe_child(&root.join(".local/tongue-models"), &id)
}

async fn models(State(manager): State<TrainingManager>) -> Result<Json<Models>, ApiError> {
    let mut result = vec![SavedModel {
        id: "demo".into(),
        architecture: TrainerArchitecture::StereoPair,
        name: Some("Built-in model".into()),
        report: None,
    }];
    let root = manager.root.join(".local/tongue-models");
    if root.exists() {
        for entry in fs::read_dir(root).map_err(bad)? {
            let entry = entry.map_err(bad)?;
            // Hidden folders are imports still being copied in.
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if let Ok(report) = read_json(&path.join("report.json")) {
                if complete_model(&path) {
                    let report: Option<TrainingReport> = serde_json::from_value(report).ok();
                    result.push(SavedModel {
                        id: entry.file_name().to_string_lossy().into_owned(),
                        architecture: architecture(&path),
                        name: report
                            .as_ref()
                            .map(|report| report.name.clone())
                            .filter(|name| !name.is_empty()),
                        report,
                    });
                }
            }
        }
    }
    Ok(Json(Models {
        active_id: active_id(&manager.root),
        models: result,
    }))
}

/// Deletes a trained model, unless it's the one in use and there's another
/// to switch to. The last trained model in use, with the built-in one not
/// downloaded, can go; the built-in one is then in use again.
async fn delete_model(
    State(manager): State<TrainingManager>,
    Json(request): Json<RecordingId>,
) -> Result<Json<RecordingDeleted>, ApiError> {
    manager.idle()?;
    manager.untouched()?;
    if request.id == "demo" {
        return Err(bad("The built-in model can't be deleted"));
    }
    let path = safe_child(&manager.root.join(".local/tongue-models"), &request.id).map_err(bad)?;
    if !path.join("report.json").is_file() && !complete_model(&path) {
        return Err(bad("Not a trained model"));
    }
    let in_use = request.id == active_id(&manager.root);
    if in_use {
        let Json(models) = models(State(manager.clone())).await?;
        let others = models
            .models
            .iter()
            .any(|model| model.id != "demo" && model.id != request.id);
        if others || builtin::installed(&manager.root) {
            return Err(bad("Switch to another model before deleting this one"));
        }
        select_model(&manager.root, "demo").map_err(bad)?;
    }
    fs::remove_dir_all(&path).map_err(bad)?;
    Ok(Json(RecordingDeleted { deleted: true }))
}

/// Renames a trained model, in its `report.json`.
async fn rename_model(
    State(manager): State<TrainingManager>,
    Json(request): Json<RenameModel>,
) -> Result<Json<SavedModel>, ApiError> {
    manager.untouched()?;
    let name = request.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(bad("Give the model a name of up to 100 characters"));
    }
    if request.id == "demo" {
        return Err(bad("The built-in model can't be renamed"));
    }
    let path = safe_child(&manager.root.join(".local/tongue-models"), &request.id).map_err(bad)?;
    let report_path = path.join("report.json");
    let mut report = read_json(&report_path).map_err(bad)?;
    report["name"] = json!(name);
    save_json(&report_path, &report).map_err(bad)?;
    Ok(Json(SavedModel {
        id: request.id,
        architecture: architecture(&path),
        name: Some(name.to_owned()),
        report: serde_json::from_value(report).ok(),
    }))
}

async fn activate(
    State(manager): State<TrainingManager>,
    Json(request): Json<RecordingId>,
) -> Result<Json<ModelActivated>, ApiError> {
    manager.idle()?;
    select_model(&manager.root, &request.id).map_err(bad)?;
    Ok(Json(ModelActivated {
        active_id: request.id,
    }))
}

/// Point live inference at a saved pair, or at the starting model for "demo".
/// Inference notices the change within a second and reloads.
fn select_model(root: &Path, id: &str) -> Result<(), String> {
    if std::env::var_os("VRFT_TONGUE_MODEL_DIR").is_some() {
        return Err(
            "Remove VRFT_TONGUE_MODEL_DIR and restart VRFaceTracking to use saved model selection"
                .into(),
        );
    }
    if id != "demo" {
        let path = safe_child(&root.join(".local/tongue-models"), id)?;
        read_json(&path.join("report.json"))?;
        if !complete_model(&path) {
            return Err("Model pair is incomplete".into());
        }
    }
    fs::create_dir_all(root.join(".local")).map_err(|e| e.to_string())?;
    save_json(&root.join(".local/tongue-active.json"), &json!({"id":id}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_face_model_alone_is_a_model() {
        let root = test_root("face-only");
        let dir = root.join(".local/tongue-models/face");
        fs::create_dir_all(&dir).unwrap();
        assert!(!complete_model(&dir));
        fs::write(dir.join(vrft_tongue::universal::FILE_NAME), b"weights").unwrap();
        assert!(complete_model(&dir) && !complete_pair(&dir));
        assert_eq!(architecture(&dir), TrainerArchitecture::UniversalFace);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reject_path_traversal_before_filesystem_access() {
        for id in ["", "..", "../capture", "C:\\file", "a/b", "a.b"] {
            assert!(safe_child(Path::new("."), id)
                .unwrap_err()
                .starts_with("Invalid"));
        }
    }

    fn test_root(name: &str) -> PathBuf {
        let parent =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.local/tongue-tests-rust");
        fs::create_dir_all(&parent).unwrap();
        let root = parent.join(format!(
            "{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn remove_test_root(root: PathBuf) {
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../.local/tongue-tests-rust")
            .canonicalize()
            .unwrap();
        let target = root.canonicalize().unwrap();
        assert_eq!(target.parent(), Some(parent.as_path()));
        fs::remove_dir_all(target).unwrap();
    }

    /// A capture whose frame file has the right length without writing pixels.
    fn capture(root: &Path, id: &str, poses: &[(&str, [f32; 10])]) -> PathBuf {
        capture_seen(root, id, poses, None)
    }

    /// A recording whose headset tracking saw `native` throughout, or what
    /// each pose asked for.
    fn capture_seen(
        root: &Path,
        id: &str,
        poses: &[(&str, [f32; 10])],
        native: Option<f32>,
    ) -> PathBuf {
        let path = root.join(".local/tongue-captures").join(id);
        fs::create_dir_all(&path).unwrap();
        save_json(&path.join("metadata.json"), &json!({"mode":"core"})).unwrap();
        let mut lines = String::new();
        let mut index = 0;
        for (step, (pose, targets)) in poses.iter().enumerate() {
            for _ in 0..8 {
                lines += &json!({"index":index, "step":step, "pose":pose, "targets":targets,
                    "native_tongue_out":native.unwrap_or(targets[0])})
                .to_string();
                lines.push('\n');
                index += 1;
            }
        }
        fs::write(path.join("samples.jsonl"), lines).unwrap();
        File::create(path.join("frames.gray8"))
            .unwrap()
            .set_len(index * 320000)
            .unwrap();
        path
    }

    fn out(horizontal: f32, vertical: f32) -> [f32; 10] {
        [1., 1., horizontal, vertical, 0., 0., 0., 0., 0., 0.]
    }

    #[tokio::test]
    async fn recordings_report_direction_coverage_and_can_be_deleted() {
        let root = test_root("sessions");
        let hidden = [0.; 10];
        capture(
            &root,
            "basic",
            &[
                ("Neutral", hidden),
                ("Speech", hidden),
                ("Smile", hidden),
                ("Straight", out(0., 0.)),
                ("Left", out(-1., 0.)),
                ("Right", out(1., 0.)),
                ("Up", out(0., 1.)),
                ("Down", out(0., -1.)),
            ],
        );
        capture(
            &root,
            "partial",
            &[("Neutral", hidden), ("Left", out(-1., 0.))],
        );
        let manager = TrainingManager::new(root.clone(), CaptureManager::default());
        let Json(list) = sessions(State(manager.clone())).await.unwrap();
        let basic = &list[0];
        assert_eq!(basic.id, "basic");
        assert_eq!(
            basic.coverage,
            Coverage {
                out: 40,
                inside: 24,
                left: 8,
                right: 8,
                up: 8,
                down: 8,
                cheek_left: 0,
                cheek_right: 0,
            }
        );
        assert!(basic.basic_ready);
        assert_eq!(list[1].coverage.right, 0);
        assert!(!list[1].basic_ready);

        fs::create_dir_all(root.join(".local/tongue-captures/not-a-recording")).unwrap();
        let delete = |id: &str| {
            delete_recording(State(manager.clone()), Json(RecordingId { id: id.into() }))
        };
        assert!(delete("not-a-recording").await.is_err());
        assert!(delete("..").await.is_err());
        let pointer = root.join(crate::capture::ENROLLMENT_FILE);
        fs::write(&pointer, br#"{"recording":"partial"}"#).unwrap();
        assert!(delete("partial").await.unwrap().0.deleted);
        assert!(!root.join(".local/tongue-captures/partial").exists());
        assert!(!pointer.exists(), "the face setup in use goes with it");
        assert!(root.join(".local/tongue-captures/basic").exists());
        remove_test_root(root);
    }

    #[test]
    fn finished_training_is_switched_on() {
        let root = test_root("finished");
        let pair = root.join(".local/tongue-models/personal");
        fs::create_dir_all(&pair).unwrap();
        for role in [Role::Gate, Role::Direction] {
            fs::write(role.safetensors(&pair), b"test").unwrap();
        }
        save_json(&pair.join("report.json"), &json!({"name":"test"})).unwrap();
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "exit 0"]);
            command
        } else {
            Command::new("true")
        };
        let mut job = Job {
            child: Some(command.spawn().unwrap()),
            id: Some("personal".into()),
            ..Job::default()
        };
        while job.child.is_some() {
            poll_job(&mut job, &root);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(job.terminal.is_none());
        assert_eq!(active_id(&root), "personal");
        remove_test_root(root);
    }

    #[tokio::test]
    async fn poses_the_headset_disagreed_with_are_flagged() {
        let root = test_root("suspect");
        let hidden = [0.; 10];
        capture_seen(
            &root,
            "doubtful",
            &[("Neutral", hidden), ("Tongue out", out(0., 0.))],
            Some(1.0),
        );
        let manager = TrainingManager::new(root.clone(), CaptureManager::default());
        let Json(list) = sessions(State(manager)).await.unwrap();
        let poses = &list[0].poses;
        assert!(
            poses[0].suspect,
            "the headset saw a tongue in a tongue-in pose"
        );
        assert!(!poses[1].suspect, "it agreed with the tongue-out pose");
        remove_test_root(root);
    }

    #[tokio::test]
    async fn trained_models_can_be_renamed_and_deleted_but_not_while_in_use() {
        let root = test_root("manage");
        let pair = root.join(".local/tongue-models/personal");
        fs::create_dir_all(&pair).unwrap();
        for role in [Role::Gate, Role::Direction] {
            fs::write(role.safetensors(&pair), b"test").unwrap();
        }
        save_json(
            &pair.join("report.json"),
            &json!({"name": "Old", "seconds": 5.0}),
        )
        .unwrap();
        let manager = TrainingManager::new(root.clone(), CaptureManager::default());

        let Json(renamed) = rename_model(
            State(manager.clone()),
            Json(RenameModel {
                id: "personal".into(),
                name: "  Evening  ".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(renamed.name.as_deref(), Some("Evening"));
        let report = read_json(&pair.join("report.json")).unwrap();
        assert_eq!(report["name"], "Evening");
        assert_eq!(report["seconds"], 5.0, "the rest of the report is kept");

        select_model(&root, "personal").unwrap();
        let delete =
            |id: &str| delete_model(State(manager.clone()), Json(RecordingId { id: id.into() }));
        // With the built-in model there to switch to, the model in use stays.
        let builtin = root.join("models/quest-pro");
        fs::create_dir_all(&builtin).unwrap();
        for role in [Role::Gate, Role::Direction] {
            fs::write(role.safetensors(&builtin), b"test").unwrap();
        }
        assert!(delete("personal").await.is_err(), "the model in use stays");
        assert!(delete("demo").await.is_err());
        select_model(&root, "demo").unwrap();
        assert!(delete("personal").await.unwrap().0.deleted);
        assert!(!pair.exists());
        remove_test_root(root);
    }

    #[tokio::test]
    async fn the_last_model_in_use_can_go_when_the_builtin_one_is_not_downloaded() {
        let root = test_root("last");
        for id in ["first", "second"] {
            let pair = root.join(".local/tongue-models").join(id);
            fs::create_dir_all(&pair).unwrap();
            for role in [Role::Gate, Role::Direction] {
                fs::write(role.safetensors(&pair), b"test").unwrap();
            }
            save_json(&pair.join("report.json"), &json!({"name": id})).unwrap();
        }
        let manager = TrainingManager::new(root.clone(), CaptureManager::default());
        let delete =
            |id: &str| delete_model(State(manager.clone()), Json(RecordingId { id: id.into() }));
        select_model(&root, "first").unwrap();
        assert!(
            delete("first").await.is_err(),
            "there's another to switch to"
        );
        assert!(delete("second").await.unwrap().0.deleted);
        assert!(delete("first").await.unwrap().0.deleted);
        assert_eq!(active_id(&root), "demo");
        remove_test_root(root);
    }

    #[tokio::test]
    async fn incomplete_pair_cannot_replace_selection_and_restore_is_atomic() {
        // Exercise pointer replacement on Windows as well as preserving base files.
        let parent =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.local/tongue-tests-rust");
        fs::create_dir_all(&parent).unwrap();
        let root = parent.join(format!(
            "selection-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pair = root.join(".local/tongue-models/personal");
        fs::create_dir_all(&pair).unwrap();
        fs::write(Role::Gate.safetensors(&pair), b"test gate").unwrap();
        save_json(&pair.join("report.json"), &json!({"name":"test"})).unwrap();
        let manager = TrainingManager::new(root.clone(), CaptureManager::default());
        assert!(activate(
            State(manager.clone()),
            Json(RecordingId {
                id: "personal".into()
            })
        )
        .await
        .is_err());
        assert_eq!(active_id(&root), "demo");
        fs::write(Role::Direction.safetensors(&pair), b"test direction").unwrap();
        let _ = activate(
            State(manager.clone()),
            Json(RecordingId {
                id: "personal".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(active_id(&root), "personal");
        let _ = activate(State(manager), Json(RecordingId { id: "demo".into() }))
            .await
            .unwrap();
        assert_eq!(active_id(&root), "demo");
        assert_eq!(
            fs::read(Role::Gate.safetensors(&pair)).unwrap(),
            b"test gate"
        );
        let target = root.canonicalize().unwrap();
        assert_eq!(
            target.parent(),
            Some(parent.canonicalize().unwrap().as_path())
        );
        fs::remove_dir_all(target).unwrap();
    }
}
