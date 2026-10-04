//! Exporting a trained model, with the recordings it was trained on, to a
//! folder another PC can import, and importing one from that folder or a
//! zip of it.
//!
//! An export is a folder holding `model/` (the model's own folder: the pair,
//! `report.json` and its log), `recordings/<id>/` for each recording, and
//! [`MANIFEST`], which lists every file with its size and SHA-256. An import
//! checks all of that before it copies anything in, and gives the model a
//! new id so it never replaces one already here.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use vrft_quest_pro_protocol::{CameraLayout, TrainingReport, TransferKind, TransferStatus};

/// The file that marks a folder as an exported model.
pub const MANIFEST: &str = "vrft-tongue-model.json";
const FORMAT: &str = "vrft-tongue-model";
/// Bumped when an export changes in a way older imports can't read.
const VERSION: u32 = 1;
const MODEL_DIR: &str = "model";
const RECORDINGS_DIR: &str = "recordings";
/// Files a recording can't be trained on without.
const RECORDING_FILES: [&str; 3] = ["metadata.json", "samples.jsonl", "frames.gray8"];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    name: String,
    /// When it was exported, in Unix milliseconds.
    exported_at: u64,
    /// The VRFT version that exported it.
    app_version: String,
    model: Vec<Entry>,
    recordings: Vec<RecordingEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordingEntry {
    id: String,
    files: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// From the export's folder, with `/` separators.
    path: String,
    bytes: u64,
    sha256: String,
}

/// The one export or import that may run at a time, and how the last went.
#[derive(Clone, Default)]
pub struct Transfers {
    state: Arc<Mutex<TransferStatus>>,
}

/// Bytes done and to do, for [`TransferStatus::fraction`].
struct Progress<'a> {
    state: &'a Mutex<TransferStatus>,
    done: u64,
    total: u64,
}

impl Progress<'_> {
    fn add(&mut self, bytes: u64) {
        self.done += bytes;
        let fraction = self.done as f64 / self.total.max(1) as f64;
        self.state.lock().unwrap().fraction = Some(fraction.min(1.) as f32);
    }
}

/// What a finished export or import reports.
#[derive(Default)]
struct Outcome {
    folder: Option<PathBuf>,
    model_id: Option<String>,
    recordings: u32,
    recordings_already_here: u32,
}

impl Transfers {
    pub fn status(&self) -> Option<TransferStatus> {
        let state = self.state.lock().unwrap();
        (state.serial > 0).then(|| state.clone())
    }

    pub fn busy(&self) -> bool {
        self.state.lock().unwrap().busy
    }

    /// Copies trained model `id` and its recordings into a new folder inside
    /// `folder`, in the background.
    pub fn export(
        &self,
        root: PathBuf,
        id: String,
        folder: PathBuf,
    ) -> Result<TransferStatus, String> {
        self.run(TransferKind::Export, move |progress| {
            export(&root, &id, &folder, progress)
        })
    }

    /// Adds the model exported to `path`, a folder or a zip of one, in the
    /// background.
    pub fn import(&self, root: PathBuf, path: PathBuf) -> Result<TransferStatus, String> {
        self.run(TransferKind::Import, move |progress| {
            import(&root, &path, progress)
        })
    }

    fn run(
        &self,
        kind: TransferKind,
        work: impl FnOnce(&mut Progress) -> Result<Outcome, String> + Send + 'static,
    ) -> Result<TransferStatus, String> {
        let started = {
            let mut state = self.state.lock().unwrap();
            if state.busy {
                return Err("A model export or import is already running".into());
            }
            *state = TransferStatus {
                kind,
                busy: true,
                fraction: Some(0.),
                serial: state.serial + 1,
                ..TransferStatus::default()
            };
            state.clone()
        };
        let state = self.state.clone();
        let transfer = move || {
            let mut progress = Progress {
                state: &state,
                done: 0,
                total: 0,
            };
            let result = work(&mut progress);
            let mut state = state.lock().unwrap();
            state.busy = false;
            match result {
                Ok(outcome) => {
                    state.fraction = Some(1.);
                    state.folder = outcome.folder;
                    state.model_id = outcome.model_id;
                    state.recordings = outcome.recordings;
                    state.recordings_already_here = outcome.recordings_already_here;
                }
                Err(error) => {
                    log::warn!("Model {kind:?} failed: {error}");
                    state.error = Some(error);
                }
            }
        };
        std::thread::Builder::new()
            .name("quest-pro-model-transfer".into())
            .spawn(transfer)
            .expect("couldn't start the model transfer thread");
        Ok(started)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// A recording or model id, as [`crate::training`] accepts one.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 120
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// The regular files directly in `dir`, by name, leaving out unfinished
/// writes.
fn files_in(dir: &Path) -> Result<Vec<(String, u64)>, String> {
    let mut files = vec![];
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !kind.is_file() || name.ends_with(".tmp") || name.ends_with(".download") {
            continue;
        }
        files.push((name, entry.metadata().map_err(|e| e.to_string())?.len()));
    }
    files.sort();
    Ok(files)
}

/// Copies `from` to `to`, returning its SHA-256.
fn copy_hashed(from: &Path, to: &Path, progress: &mut Progress) -> Result<String, String> {
    let mut input = File::open(from).map_err(|e| format!("{}: {e}", from.display()))?;
    let mut output = File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|e| e.to_string())?;
        hasher.update(&buffer[..read]);
        progress.add(read as u64);
    }
    output.flush().map_err(|e| e.to_string())?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_file(path: &Path, progress: &mut Progress) -> Result<String, String> {
    let mut input = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        progress.add(read as u64);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// A folder name from the model's name that Windows accepts.
fn folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_().,'".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').trim();
    if cleaned.is_empty() {
        "Tongue model".into()
    } else {
        cleaned.chars().take(80).collect()
    }
}

/// `parent/name`, or `parent/name 2` and so on when that's taken.
fn unused(parent: &Path, name: &str) -> PathBuf {
    let mut path = parent.join(name);
    let mut number = 2;
    while path.exists() {
        path = parent.join(format!("{name} {number}"));
        number += 1;
    }
    path
}

/// The recordings a model's `report.json` says it was trained on, by id.
fn trained_on(report: &Value) -> Vec<String> {
    let mut ids: Vec<String> = report["recordings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|path| {
            // Written on Windows or not, the id is the last part.
            path.rsplit(['/', '\\']).next().map(str::to_owned)
        })
        .filter(|id| valid_id(id))
        .collect();
    ids.dedup();
    ids
}

fn export(
    root: &Path,
    id: &str,
    folder: &Path,
    progress: &mut Progress,
) -> Result<Outcome, String> {
    if !valid_id(id) || id == "demo" {
        return Err("Only a trained model can be exported".into());
    }
    let model = root.join(".local/tongue-models").join(id);
    let report: Value = serde_json::from_slice(
        &fs::read(model.join("report.json")).map_err(|_| "That model isn't there any more")?,
    )
    .map_err(|e| e.to_string())?;
    if !crate::training::complete_pair(&model) {
        return Err("That model's files are incomplete".into());
    }
    if !folder.is_dir() {
        return Err(format!("{} isn't a folder", folder.display()));
    }
    let captures = root.join(".local/tongue-captures");
    let recordings: Vec<(String, PathBuf)> = trained_on(&report)
        .into_iter()
        .map(|id| {
            let path = captures.join(&id);
            (id, path)
        })
        .filter(|(_, path)| path.join("samples.jsonl").is_file())
        .collect();

    let model_files = files_in(&model)?;
    let recording_files = recordings
        .iter()
        .map(|(_, path)| files_in(path))
        .collect::<Result<Vec<_>, _>>()?;
    progress.total = model_files
        .iter()
        .chain(recording_files.iter().flatten())
        .map(|(_, bytes)| bytes)
        .sum();

    let name = report["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Tongue model")
        .trim()
        .to_owned();
    let out = unused(folder, &folder_name(&name));
    fs::create_dir(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let written = (|| -> Result<Manifest, String> {
        let copy_all = |from: &Path,
                        files: &[(String, u64)],
                        to: &str,
                        progress: &mut Progress|
         -> Result<Vec<Entry>, String> {
            fs::create_dir_all(out.join(to)).map_err(|e| e.to_string())?;
            files
                .iter()
                .map(|(file, bytes)| {
                    let path = format!("{to}/{file}");
                    let sha256 = copy_hashed(&from.join(file), &out.join(&path), progress)?;
                    Ok(Entry {
                        path,
                        bytes: *bytes,
                        sha256,
                    })
                })
                .collect()
        };
        let model = copy_all(&model, &model_files, MODEL_DIR, progress)?;
        let mut entries = vec![];
        for ((id, path), files) in recordings.iter().zip(&recording_files) {
            entries.push(RecordingEntry {
                id: id.clone(),
                files: copy_all(path, files, &format!("{RECORDINGS_DIR}/{id}"), progress)?,
            });
        }
        let manifest = Manifest {
            format: FORMAT.into(),
            version: VERSION,
            name,
            exported_at: now_ms(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            model,
            recordings: entries,
        };
        fs::write(
            out.join(MANIFEST),
            serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        Ok(manifest)
    })();
    match written {
        Ok(manifest) => Ok(Outcome {
            folder: Some(out),
            model_id: Some(id.to_owned()),
            recordings: manifest.recordings.len() as u32,
            recordings_already_here: 0,
        }),
        Err(error) => {
            // Only the folder this export made.
            let _ = fs::remove_dir_all(&out);
            Err(error)
        }
    }
}

/// A relative path from a manifest, when it stays inside the export.
fn inside(base: &Path, path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path);
    if path.is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(format!("The export lists a file outside itself: {path}"));
    }
    Ok(base.join(relative))
}

/// A folder removed when dropped, for an unpacked zip or a half-done copy.
struct Scratch(Option<PathBuf>);

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn import(root: &Path, path: &Path, progress: &mut Progress) -> Result<Outcome, String> {
    let local = root.join(".local");
    fs::create_dir_all(&local).map_err(|e| e.to_string())?;
    let mut unpacked = Scratch(None);
    let base = if path.is_file()
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        let scratch = local.join(format!("tongue-import-{}", now_ms()));
        unpacked.0 = Some(scratch.clone());
        unzip(path, &scratch, progress)?
    } else if path.is_file() && path.file_name().is_some_and(|name| name == MANIFEST) {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    } else if path.is_dir() {
        find_export(path)?
    } else {
        return Err("Choose an exported model's folder, or a zip of one".into());
    };

    let manifest: Manifest = serde_json::from_slice(
        &fs::read(base.join(MANIFEST)).map_err(|_| "That folder isn't an exported model")?,
    )
    .map_err(|e| format!("The export's {MANIFEST} can't be read: {e}"))?;
    if manifest.format != FORMAT {
        return Err("That folder isn't an exported model".into());
    }
    if manifest.version > VERSION {
        return Err(
            "That model was exported by a newer VRFaceTracking. Update to import it".into(),
        );
    }
    check(&base, &manifest, progress)?;

    // Copy it in: every file's size is known, so progress runs on from the
    // check.
    let models = local.join("tongue-models");
    let captures = local.join("tongue-captures");
    fs::create_dir_all(&models).map_err(|e| e.to_string())?;
    fs::create_dir_all(&captures).map_err(|e| e.to_string())?;
    let mut already_here = 0;
    let mut placed = vec![];
    for recording in &manifest.recordings {
        let existing = captures.join(&recording.id);
        if same_recording(&existing, recording) {
            already_here += 1;
            progress.add(recording.files.iter().map(|file| file.bytes).sum());
            placed.push(existing);
            continue;
        }
        let target = unused_id(&captures, &recording.id);
        copy_in(&base, &recording.files, &target, progress)?;
        placed.push(target);
    }
    let id = format!("{}-{}", now_ms(), std::process::id());
    let model = models.join(&id);
    copy_in(&base, &manifest.model, &model, progress)?;
    // Point the model's report at the recordings where they now are.
    let listed: Vec<Value> = placed
        .iter()
        .map(|path| Value::String(path.display().to_string()))
        .collect();
    for file in ["report.json", "request.json"] {
        let path = model.join(file);
        if let Ok(mut value) = fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string()))
        {
            if value.get("recordings").is_some() {
                value["recordings"] = Value::Array(listed.clone());
                let _ = fs::write(&path, serde_json::to_vec_pretty(&value).unwrap_or_default());
            }
        }
    }
    Ok(Outcome {
        folder: None,
        model_id: Some(id),
        recordings: manifest.recordings.len() as u32,
        recordings_already_here: already_here,
    })
}

/// The export in `dir`: the folder itself, or its only subfolder when the
/// folder around it was chosen.
fn find_export(dir: &Path) -> Result<PathBuf, String> {
    if dir.join(MANIFEST).is_file() {
        return Ok(dir.to_path_buf());
    }
    let inner: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join(MANIFEST).is_file())
        .collect();
    match inner.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err("That folder isn't an exported model".into()),
        _ => Err("That folder holds several exported models. Choose one of them".into()),
    }
}

/// Unpacks the export in zip `path` into `scratch`, returning its folder
/// there. The export may be at the zip's top or in one folder inside it.
fn unzip(path: &Path, scratch: &Path, progress: &mut Progress) -> Result<PathBuf, String> {
    let mut zip = zip::ZipArchive::new(File::open(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("That zip can't be read: {e}"))?;
    let manifest = zip
        .file_names()
        .filter(|name| {
            let name = name.trim_end_matches('/');
            name == MANIFEST
                || name
                    .strip_suffix(MANIFEST)
                    .and_then(|prefix| prefix.strip_suffix('/'))
                    .is_some_and(|folder| !folder.contains('/'))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let prefix = match manifest.as_slice() {
        [only] => only.strip_suffix(MANIFEST).unwrap_or_default().to_owned(),
        [] => return Err("That zip doesn't hold an exported model".into()),
        _ => return Err("That zip holds several exported models. Unzip it and choose one".into()),
    };
    // Unpacking counts as much as checking and copying afterwards.
    let mut total = 0;
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(|e| e.to_string())?;
        if entry.name().starts_with(&prefix) {
            total += entry.size();
        }
    }
    progress.total = total * 3;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| e.to_string())?;
        if !entry.name().starts_with(&prefix) || entry.is_dir() {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            return Err(format!(
                "The zip lists a file outside itself: {}",
                entry.name()
            ));
        };
        let target = scratch.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&target).map_err(|e| e.to_string())?;
        let copied = std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        progress.add(copied);
    }
    Ok(scratch.join(prefix.trim_end_matches('/')))
}

/// Checks every file the manifest lists is there, whole and unchanged, and
/// that the model and recordings can be used.
fn check(base: &Path, manifest: &Manifest, progress: &mut Progress) -> Result<(), String> {
    let all = || {
        manifest
            .model
            .iter()
            .chain(manifest.recordings.iter().flat_map(|r| &r.files))
    };
    // Checking, then copying, after any unpacking.
    let bytes: u64 = all().map(|file| file.bytes).sum();
    progress.total = progress.done + bytes * 2;
    let model_prefix = format!("{MODEL_DIR}/");
    for file in &manifest.model {
        if !file.path.starts_with(&model_prefix) || file.path[model_prefix.len()..].contains('/') {
            return Err(format!(
                "The export lists a model file in the wrong place: {}",
                file.path
            ));
        }
    }
    for recording in &manifest.recordings {
        if !valid_id(&recording.id) {
            return Err(format!(
                "The export has a recording with an unusable name: {}",
                recording.id
            ));
        }
        let prefix = format!("{RECORDINGS_DIR}/{}/", recording.id);
        for file in &recording.files {
            if !file.path.starts_with(&prefix) || file.path[prefix.len()..].contains('/') {
                return Err(format!(
                    "The export lists a recording file in the wrong place: {}",
                    file.path
                ));
            }
        }
        for needed in RECORDING_FILES {
            if !recording
                .files
                .iter()
                .any(|file| file.path == format!("{prefix}{needed}"))
            {
                return Err(format!(
                    "Recording {} in the export has no {needed}",
                    recording.id
                ));
            }
        }
    }
    for file in all() {
        let path = inside(base, &file.path)?;
        let size = fs::metadata(&path)
            .map_err(|_| format!("The export is missing {}", file.path))?
            .len();
        if size != file.bytes {
            return Err(format!("{} in the export is incomplete", file.path));
        }
        if hash_file(&path, progress)? != file.sha256 {
            return Err(format!(
                "{} in the export has changed or is damaged",
                file.path
            ));
        }
    }

    let model = base.join(MODEL_DIR);
    if !crate::training::complete_pair(&model) {
        return Err("The export's model is missing half of its pair".into());
    }
    let report =
        fs::read(model.join("report.json")).map_err(|_| "The export's model has no report.json")?;
    serde_json::from_slice::<TrainingReport>(&report)
        .map_err(|e| format!("The export's report.json can't be read: {e}"))?;
    for recording in &manifest.recordings {
        let dir = base.join(RECORDINGS_DIR).join(&recording.id);
        let samples =
            BufReader::new(File::open(dir.join("samples.jsonl")).map_err(|e| e.to_string())?)
                .lines()
                .map_while(Result::ok)
                .filter(|line| !line.trim().is_empty())
                .count() as u64;
        let frames = fs::metadata(dir.join("frames.gray8"))
            .map_err(|e| e.to_string())?
            .len();
        let metadata: Value = fs::read(dir.join("metadata.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| format!("Recording {} in the export has no metadata", recording.id))?;
        let layout = CameraLayout::from_metadata(&metadata)
            .map_err(|e| format!("Recording {} in the export: {e}", recording.id))?;
        if frames != samples * layout.frame_bytes() as u64 {
            return Err(format!(
                "Recording {} in the export has a frame/label count mismatch",
                recording.id
            ));
        }
    }
    Ok(())
}

/// Whether `dir` already holds this recording, file for file.
fn same_recording(dir: &Path, recording: &RecordingEntry) -> bool {
    dir.is_dir()
        && recording.files.iter().all(|file| {
            let name = file.path.rsplit('/').next().unwrap_or_default();
            let path = dir.join(name);
            fs::metadata(&path).is_ok_and(|meta| meta.len() == file.bytes)
                && sha256(&path).as_deref() == Some(file.sha256.as_str())
        })
}

fn sha256(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(format!("{:x}", hasher.finalize()))
}

/// `parent/id`, or `parent/id-imported`, `-imported-2` and so on when that's
/// taken.
fn unused_id(parent: &Path, id: &str) -> PathBuf {
    let mut path = parent.join(id);
    let mut number = 1;
    while path.exists() {
        path = parent.join(if number == 1 {
            format!("{id}-imported")
        } else {
            format!("{id}-imported-{number}")
        });
        number += 1;
    }
    path
}

/// Copies `files` from the export into a new folder `target`, through a
/// hidden folder beside it so a half-done copy is never listed.
fn copy_in(
    base: &Path,
    files: &[Entry],
    target: &Path,
    progress: &mut Progress,
) -> Result<(), String> {
    let parent = target.parent().ok_or("No folder to import into")?;
    let name = target
        .file_name()
        .ok_or("No folder to import into")?
        .to_string_lossy();
    let staging = parent.join(format!(".importing-{name}"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir(&staging).map_err(|e| e.to_string())?;
    let mut cleanup = Scratch(Some(staging.clone()));
    for file in files {
        let from = inside(base, &file.path)?;
        let to = staging.join(file.path.rsplit('/').next().unwrap_or_default());
        copy_hashed(&from, &to, progress)?;
    }
    fs::rename(&staging, target).map_err(|e| e.to_string())?;
    cleanup.0 = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vrft_quest_pro_protocol::FRAME_BYTES;
    use vrft_tongue::Role;

    fn test_root(name: &str) -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../.local/tongue-tests-rust")
            .join(format!(
                "{name}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// A trained model `id` over one two-frame recording.
    fn trained(root: &Path, id: &str, recording: &str) {
        let capture = root.join(".local/tongue-captures").join(recording);
        fs::create_dir_all(&capture).unwrap();
        fs::write(capture.join("metadata.json"), br#"{"mode":"core"}"#).unwrap();
        fs::write(
            capture.join("samples.jsonl"),
            "{\"step\":0}\n{\"step\":0}\n",
        )
        .unwrap();
        fs::write(capture.join("frames.gray8"), vec![7u8; FRAME_BYTES * 2]).unwrap();
        let model = root.join(".local/tongue-models").join(id);
        fs::create_dir_all(&model).unwrap();
        for role in [Role::Gate, Role::Direction] {
            fs::write(role.safetensors(&model), b"weights").unwrap();
        }
        let report = json!({"name": "Evening", "recordings": [capture.display().to_string()]});
        fs::write(model.join("report.json"), report.to_string()).unwrap();
    }

    fn run(work: impl FnOnce(&mut Progress) -> Result<Outcome, String>) -> Result<Outcome, String> {
        let state = Mutex::new(TransferStatus::default());
        let mut progress = Progress {
            state: &state,
            done: 0,
            total: 0,
        };
        let outcome = work(&mut progress);
        if outcome.is_ok() {
            assert!(progress.done >= progress.total, "progress reaches the end");
        }
        outcome
    }

    #[test]
    fn an_export_imports_as_a_new_model_with_its_recordings() {
        let from = test_root("export-from");
        trained(&from, "personal", "session-1");
        let out = from.join("exports");
        fs::create_dir_all(&out).unwrap();
        let exported = run(|progress| export(&from, "personal", &out, progress)).unwrap();
        let folder = exported.folder.unwrap();
        assert_eq!(folder, out.join("Evening"));
        assert!(folder.join(MANIFEST).is_file());
        assert!(folder.join("recordings/session-1/frames.gray8").is_file());
        // A second export doesn't touch the first.
        let again = run(|progress| export(&from, "personal", &out, progress)).unwrap();
        assert_eq!(again.folder.unwrap(), out.join("Evening 2"));

        let to = test_root("export-to");
        let imported = run(|progress| import(&to, &folder, progress)).unwrap();
        let id = imported.model_id.unwrap();
        assert_eq!(
            (imported.recordings, imported.recordings_already_here),
            (1, 0)
        );
        let model = to.join(".local/tongue-models").join(&id);
        assert!(crate::training::complete_pair(&model));
        let report: Value =
            serde_json::from_slice(&fs::read(model.join("report.json")).unwrap()).unwrap();
        let listed = report["recordings"][0].as_str().unwrap();
        assert!(
            Path::new(listed).join("frames.gray8").is_file(),
            "the report points at the copy"
        );

        // The same recording isn't copied twice.
        let twice = run(|progress| import(&to, &folder, progress)).unwrap();
        assert_eq!(twice.recordings_already_here, 1);
        assert_ne!(twice.model_id.unwrap(), id);
        fs::remove_dir_all(from).unwrap();
        fs::remove_dir_all(to).unwrap();
    }

    #[test]
    fn a_damaged_or_foreign_export_is_refused_before_anything_is_copied() {
        let from = test_root("damaged-from");
        trained(&from, "personal", "session-1");
        let folder = run(|progress| export(&from, "personal", &from, progress))
            .unwrap()
            .folder
            .unwrap();
        let to = test_root("damaged-to");
        let frames = folder.join("recordings/session-1/frames.gray8");
        let mut bytes = fs::read(&frames).unwrap();
        bytes[0] ^= 1;
        fs::write(&frames, bytes).unwrap();
        let error = run(|progress| import(&to, &folder, progress))
            .err()
            .unwrap();
        assert!(error.contains("changed or is damaged"), "{error}");
        fs::remove_file(&frames).unwrap();
        let error = run(|progress| import(&to, &folder, progress))
            .err()
            .unwrap();
        assert!(error.contains("missing"), "{error}");
        assert!(
            !to.join(".local/tongue-models").exists(),
            "nothing was copied"
        );

        let mut manifest: Value =
            serde_json::from_slice(&fs::read(folder.join(MANIFEST)).unwrap()).unwrap();
        manifest["model"][0]["path"] = json!("../../outside.bin");
        fs::write(folder.join(MANIFEST), manifest.to_string()).unwrap();
        assert!(run(|progress| import(&to, &folder, progress)).is_err());
        assert!(run(|progress| import(&to, &from.join(".local"), progress)).is_err());
        fs::remove_dir_all(from).unwrap();
        fs::remove_dir_all(to).unwrap();
    }

    #[test]
    fn an_export_zipped_with_its_folder_imports() {
        let from = test_root("zip-from");
        trained(&from, "personal", "session-1");
        let folder = run(|progress| export(&from, "personal", &from, progress))
            .unwrap()
            .folder
            .unwrap();
        let archive = from.join("Evening.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let mut add = |relative: &str| {
            zip.start_file(format!("Evening/{relative}"), options)
                .unwrap();
            zip.write_all(&fs::read(folder.join(relative)).unwrap())
                .unwrap();
        };
        add(MANIFEST);
        for (file, _) in files_in(&folder.join("model")).unwrap() {
            add(&format!("model/{file}"));
        }
        for file in RECORDING_FILES {
            add(&format!("recordings/session-1/{file}"));
        }
        zip.finish().unwrap();

        let to = test_root("zip-to");
        let imported = run(|progress| import(&to, &archive, progress)).unwrap();
        assert!(crate::training::complete_pair(
            &to.join(".local/tongue-models")
                .join(imported.model_id.unwrap())
        ));
        let leftovers = fs::read_dir(to.join(".local"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("tongue-import-")
            })
            .count();
        assert_eq!(leftovers, 0, "the unpacked zip is removed");
        fs::remove_dir_all(from).unwrap();
        fs::remove_dir_all(to).unwrap();
    }
}
