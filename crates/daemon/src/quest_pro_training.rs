//! Local capture review, training jobs and reversible model selection.
use crate::quest_pro_camera_capture::CaptureManager;
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
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

const GATE: &str = "qpro-stereo-tongue-v8-gate.pt";
const DIRECTION: &str = "qpro-stereo-tongue-v8-direction.pt";
type ApiError = (StatusCode, String);
fn bad(error: impl ToString) -> ApiError {
    (StatusCode::BAD_REQUEST, error.to_string())
}

#[derive(Default)]
struct Job {
    child: Option<Child>,
    id: Option<String>,
    terminal: Option<Value>,
}

#[derive(Clone)]
pub struct TrainingManager {
    root: PathBuf,
    capture: CaptureManager,
    job: Arc<Mutex<Job>>,
}

impl TrainingManager {
    pub fn new(root: PathBuf, capture: CaptureManager) -> Self {
        Self {
            root,
            capture,
            job: Arc::new(Mutex::new(Job::default())),
        }
    }
    pub fn busy(&self) -> bool {
        let mut job = self.job.lock().unwrap();
        poll_job(&mut job, &self.root);
        job.child.is_some()
    }
    pub fn cancel(&self) {
        let mut job = self.job.lock().unwrap();
        if let Some(mut child) = job.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            job.terminal = Some(
                json!({"stage":"cancelled", "message":"Training cancelled. Your active model is unchanged."}),
            );
        }
    }
    fn idle(&self) -> Result<(), ApiError> {
        if self.busy() || self.capture.status().active {
            Err(bad("Finish the current recording or training first"))
        } else {
            Ok(())
        }
    }
}

pub fn routes(manager: TrainingManager) -> Router {
    Router::new()
        .route(
            "/training.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("quest_pro_training.js"),
                )
            }),
        )
        .route("/training/sessions", get(sessions))
        .route("/training/review", post(review))
        .route("/training/delete", post(delete_recording))
        .route("/training/frame", get(frame))
        .route("/training/start", post(start))
        .route("/training/cancel", post(cancel))
        .route("/training/status", get(status))
        .route("/training/models", get(models))
        .route("/training/activate", post(activate))
        .with_state(manager)
}

fn safe_child(root: &Path, id: &str) -> Result<PathBuf, String> {
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

async fn sessions(State(manager): State<TrainingManager>) -> Result<Json<Value>, ApiError> {
    let root = manager.root.join(".local/tongue-captures");
    let mut result = vec![];
    if root.exists() {
        let active = manager.capture.status();
        for entry in fs::read_dir(&root).map_err(bad)? {
            let entry = entry.map_err(bad)?;
            if !entry.file_type().map_err(bad)?.is_dir() {
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
            let details = (|| -> Result<Value, String> {
                let metadata = read_json(&path.join("metadata.json"))?;
                let samples = labels(&path)?;
                if fs::metadata(path.join("frames.gray8"))
                    .map_err(|e| e.to_string())?
                    .len()
                    != samples.len() as u64 * 320000
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
                    .filter(|sample| sample["native_tongue_out"].as_f64().is_some())
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
                let coverage = json!({"out":positives, "in":negatives,
                    "left":direction(2, -1.), "right":direction(2, 1.),
                    "up":direction(3, 1.), "down":direction(3, -1.)});
                let directions = ["left", "right", "up", "down"]
                    .iter()
                    .all(|name| coverage[name].as_u64().unwrap_or(0) >= 8);
                let poses: Vec<Value> = poses.into_iter().map(|(step, values)| json!({
                    "step":step, "name":values[0]["pose"], "frames":values.len(),
                    "indices":[values[0]["index"],values[values.len()/2]["index"],values[values.len()-1]["index"]],
                    "skipped":skipped.contains(&step), "excluded":rejected.contains(&step) || skipped.contains(&step)
                })).collect();
                Ok(
                    json!({"id":id, "mode":metadata["mode"], "frames":samples.len(), "poses":poses,
                    "positive_frames":positives,"negative_frames":negatives,"coverage":coverage,
                    "basic_ready":positives >= 20 && negatives >= 20 && directions}),
                )
            })();
            result.push(details.unwrap_or_else(|error| json!({"id":id,"error":error})));
        }
    }
    result.sort_by_key(|v| v["id"].as_str().unwrap_or("").to_owned());
    Ok(Json(json!(result)))
}

#[derive(Deserialize)]
struct ReviewRequest {
    id: String,
    excluded_steps: Vec<u64>,
}
async fn review(
    State(manager): State<TrainingManager>,
    Json(request): Json<ReviewRequest>,
) -> Result<Json<Value>, ApiError> {
    manager.idle()?;
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
    Ok(Json(json!({"saved":true})))
}

#[derive(Deserialize)]
struct DeleteRequest {
    id: String,
}
async fn delete_recording(
    State(manager): State<TrainingManager>,
    Json(request): Json<DeleteRequest>,
) -> Result<Json<Value>, ApiError> {
    // Idle: never delete the recording being captured or read by training.
    manager.idle()?;
    let path =
        safe_child(&manager.root.join(".local/tongue-captures"), &request.id).map_err(bad)?;
    if !path.join("metadata.json").is_file() {
        return Err(bad("Not a recording"));
    }
    fs::remove_dir_all(&path).map_err(bad)?;
    Ok(Json(json!({"deleted":true})))
}

#[derive(Deserialize)]
struct FrameRequest {
    id: String,
    index: u64,
}
async fn frame(
    State(manager): State<TrainingManager>,
    Query(request): Query<FrameRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let path =
        safe_child(&manager.root.join(".local/tongue-captures"), &request.id).map_err(bad)?;
    let mut file = File::open(path.join("frames.gray8")).map_err(bad)?;
    if request.index >= file.metadata().map_err(bad)?.len() / 320000 {
        return Err(bad("Frame is outside recording"));
    }
    file.seek(SeekFrom::Start(request.index * 320000))
        .map_err(bad)?;
    let mut pixels = vec![0; 320000];
    file.read_exact(&mut pixels).map_err(bad)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        pixels,
    ))
}

#[derive(Deserialize)]
struct TrainRequest {
    name: String,
    recordings: Vec<String>,
    device: String,
    epochs: u32,
}
async fn start(
    State(manager): State<TrainingManager>,
    Json(request): Json<TrainRequest>,
) -> Result<Json<Value>, ApiError> {
    manager.idle()?;
    if request.name.trim().is_empty()
        || request.name.len() > 100
        || !(1..=60).contains(&request.epochs)
        || !["auto", "cpu", "cuda"].contains(&request.device.as_str())
    {
        return Err(bad("Invalid training name, device, or epoch count"));
    }
    if request.recordings.is_empty() {
        return Err(bad("Tick at least one recording to train on"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut recordings = vec![];
    for id in request.recordings.iter().filter(|id| seen.insert(*id)) {
        recordings.push(safe_child(&manager.root.join(".local/tongue-captures"), id).map_err(bad)?);
    }
    let python = crate::quest_pro_camera::python_path(&manager.root).map_err(bad)?;
    let base = crate::quest_pro_camera::base_model_dir(&manager.root).map_err(bad)?;
    let script = manager.root.join("train_vrft_tongue.py");
    if !script.is_file() {
        return Err(bad("Training helper missing from this VRFT installation"));
    }
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
    save_json(&output.join("request.json"), &json!({"name":request.name.trim(), "recordings":recordings, "base_model_dir":base,"device":request.device})).map_err(bad)?;
    let log = File::create(output.join("training.log")).map_err(bad)?;
    let mut command = Command::new(python);
    command
        .arg(script)
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
    // Reserve the job under the mutex immediately before spawning.
    let mut job = manager.job.lock().unwrap();
    if job.child.is_some() {
        return Err(bad("A training job is already running"));
    }
    job.child = Some(command.spawn().map_err(bad)?);
    job.id = Some(id.clone());
    job.terminal = None;
    Ok(Json(json!({"id":id})))
}

async fn cancel(State(manager): State<TrainingManager>) -> Json<Value> {
    manager.cancel();
    Json(json!({"cancelled":true}))
}

async fn status(State(manager): State<TrainingManager>) -> Json<Value> {
    let mut job = manager.job.lock().unwrap();
    poll_job(&mut job, &manager.root);
    let progress = job.terminal.clone().or_else(|| {
        job.id.as_ref().and_then(|id| {
            read_json(
                &manager
                    .root
                    .join(".local/tongue-models")
                    .join(id)
                    .join("progress.json"),
            )
            .ok()
        })
    });
    Json(
        json!({"busy":job.child.is_some(),"id":job.id,"progress":progress,
        "active_id":active_id(&manager.root), "model_override":std::env::var_os("VRFT_TONGUE_MODEL_DIR").is_some()}),
    )
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
                    job.terminal = Some(json!({"stage":"failed",
                        "message":format!("Training finished, but the new model could not be switched on: {error}")}));
                }
            } else {
                let reported = job.id.as_ref().and_then(|id| {
                    read_json(
                        &root
                            .join(".local/tongue-models")
                            .join(id)
                            .join("progress.json"),
                    )
                    .ok()
                });
                job.terminal = Some(reported.filter(|v| v["stage"] == "failed").unwrap_or_else(||
                    json!({"stage":"failed","message":"the trainer exited with an error. See training.log in the model's folder."})));
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

pub fn selected_dir(root: &Path, base: PathBuf) -> Result<PathBuf, String> {
    if std::env::var_os("VRFT_TONGUE_MODEL_DIR").is_some() {
        return Ok(base);
    }
    let id = active_id(root);
    if id == "demo" {
        return Ok(base);
    }
    safe_child(&root.join(".local/tongue-models"), &id)
}

async fn models(State(manager): State<TrainingManager>) -> Result<Json<Value>, ApiError> {
    let mut result = vec![json!({"id":"demo", "name":"Built-in model", "report":null})];
    let root = manager.root.join(".local/tongue-models");
    if root.exists() {
        for entry in fs::read_dir(root).map_err(bad)? {
            let entry = entry.map_err(bad)?;
            let path = entry.path();
            if let Ok(report) = read_json(&path.join("report.json")) {
                if path.join(GATE).is_file() && path.join(DIRECTION).is_file() {
                    result.push(json!({"id":entry.file_name().to_string_lossy(),"name":report["name"],"report":report}));
                }
            }
        }
    }
    Ok(Json(
        json!({"active_id":active_id(&manager.root),"models":result}),
    ))
}

#[derive(Deserialize)]
struct ActivateRequest {
    id: String,
}
async fn activate(
    State(manager): State<TrainingManager>,
    Json(request): Json<ActivateRequest>,
) -> Result<Json<Value>, ApiError> {
    manager.idle()?;
    select_model(&manager.root, &request.id).map_err(bad)?;
    Ok(Json(json!({"active_id":request.id})))
}

/// Point live inference at a saved pair, or at the starting model for "demo".
/// Inference notices the change within a second and reloads.
fn select_model(root: &Path, id: &str) -> Result<(), String> {
    if std::env::var_os("VRFT_TONGUE_MODEL_DIR").is_some() {
        return Err(
            "Remove VRFT_TONGUE_MODEL_DIR and restart VRFT to use saved model selection".into(),
        );
    }
    if id != "demo" {
        let path = safe_child(&root.join(".local/tongue-models"), id)?;
        read_json(&path.join("report.json"))?;
        if !path.join(GATE).is_file() || !path.join(DIRECTION).is_file() {
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
    fn reject_path_traversal_before_filesystem_access() {
        for id in ["", "..", "../capture", "C:\\file", "a/b", "a.b"] {
            assert!(safe_child(Path::new("."), id)
                .unwrap_err()
                .starts_with("Invalid"));
        }
    }

    fn test_root(name: &str) -> PathBuf {
        let parent =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local/tongue-tests-rust");
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
            .join("../../.local/tongue-tests-rust")
            .canonicalize()
            .unwrap();
        let target = root.canonicalize().unwrap();
        assert_eq!(target.parent(), Some(parent.as_path()));
        fs::remove_dir_all(target).unwrap();
    }

    /// A capture whose frame file has the right length without writing pixels.
    fn capture(root: &Path, id: &str, poses: &[(&str, [f32; 10])]) -> PathBuf {
        let path = root.join(".local/tongue-captures").join(id);
        fs::create_dir_all(&path).unwrap();
        save_json(&path.join("metadata.json"), &json!({"mode":"core"})).unwrap();
        let mut lines = String::new();
        let mut index = 0;
        for (step, (pose, targets)) in poses.iter().enumerate() {
            for _ in 0..8 {
                lines += &json!({"index":index, "step":step, "pose":pose, "targets":targets,
                    "native_tongue_out":targets[0]})
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
        assert_eq!(basic["id"], "basic");
        assert_eq!(
            basic["coverage"],
            json!({"out":40, "in":24, "left":8, "right":8, "up":8, "down":8})
        );
        assert_eq!(basic["basic_ready"], true);
        assert_eq!(list[1]["coverage"]["right"], 0);
        assert_eq!(list[1]["basic_ready"], false);

        fs::create_dir_all(root.join(".local/tongue-captures/not-a-recording")).unwrap();
        let delete = |id: &str| {
            delete_recording(
                State(manager.clone()),
                Json(DeleteRequest { id: id.into() }),
            )
        };
        assert!(delete("not-a-recording").await.is_err());
        assert!(delete("..").await.is_err());
        assert_eq!(delete("partial").await.unwrap().0["deleted"], true);
        assert!(!root.join(".local/tongue-captures/partial").exists());
        assert!(root.join(".local/tongue-captures/basic").exists());
        remove_test_root(root);
    }

    #[test]
    fn finished_training_is_switched_on() {
        let root = test_root("finished");
        let pair = root.join(".local/tongue-models/personal");
        fs::create_dir_all(&pair).unwrap();
        for file in [GATE, DIRECTION] {
            fs::write(pair.join(file), b"test").unwrap();
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
            terminal: None,
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
    async fn incomplete_pair_cannot_replace_selection_and_restore_is_atomic() {
        // Exercise pointer replacement on Windows as well as preserving base files.
        let parent =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local/tongue-tests-rust");
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
        fs::write(pair.join(GATE), b"test gate").unwrap();
        save_json(&pair.join("report.json"), &json!({"name":"test"})).unwrap();
        let manager = TrainingManager::new(root.clone(), CaptureManager::default());
        assert!(activate(
            State(manager.clone()),
            Json(ActivateRequest {
                id: "personal".into()
            })
        )
        .await
        .is_err());
        assert_eq!(active_id(&root), "demo");
        fs::write(pair.join(DIRECTION), b"test direction").unwrap();
        let _ = activate(
            State(manager.clone()),
            Json(ActivateRequest {
                id: "personal".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(active_id(&root), "personal");
        let _ = activate(State(manager), Json(ActivateRequest { id: "demo".into() }))
            .await
            .unwrap();
        assert_eq!(active_id(&root), "demo");
        assert_eq!(fs::read(pair.join(GATE)).unwrap(), b"test gate");
        let target = root.canonicalize().unwrap();
        assert_eq!(
            target.parent(),
            Some(parent.canonicalize().unwrap().as_path())
        );
        fs::remove_dir_all(target).unwrap();
    }
}
