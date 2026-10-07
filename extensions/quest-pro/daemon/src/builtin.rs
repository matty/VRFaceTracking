//! Installs the QFTPlus Model, the base model: QFT+'s universal face model
//! (`universal-face-v2`) from QFT+'s own release, only when the user asks for
//! it on the Training page, since its weights were trained on Ava-256
//! (CC BY-NC 4.0) and private renders and VRFT never ships them or downloads
//! them unprompted. With it comes the mouth-camera pair, which reads the
//! mouth cameras whenever the headset sends only those: the v8 demo
//! checkpoints from the Qpro-Enhanced-FT v0.1.10 release (MIT license).
//!
//! Also installs what personal training needs: that pair, which training
//! starts from, and the synthetic training examples it mixes in, from
//! VRFaceTracking's own release. Every download is verified by SHA-256,
//! files already present are never overwritten, and each release zip is
//! removed once its files are out of it.

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use vrft_quest_pro_protocol::BuiltinStage;
use vrft_tongue::Role;

/// A release zip the install downloads, checked by SHA-256.
struct Release {
    url: &'static str,
    name: &'static str,
    sha256: &'static str,
    /// Its size, for the download the Training page offers.
    megabytes: u32,
    /// What it is, for errors.
    what: &'static str,
}

const MODEL_RELEASE: Release = Release {
    url: "https://github.com/n0tmast3r/Qpro-Enhanced-FT/releases/download/v0.1.10/QproFaceTracking-0.1.10-poc.zip",
    name: "QproFaceTracking-0.1.10-poc.zip",
    sha256: "db40f4b8331a50ca6c2ec37372f1ab4b44cfbe6d21e09ca04244aaaa339cb18f",
    megabytes: 140,
    what: "the mouth-camera model",
};
/// Rendered training examples (tools/tongue-synth), stored at the model's
/// input size. Personal training mixes them in, so a model keeps knowing the
/// faces and tongue positions a user's own recording doesn't show.
const EXAMPLES_RELEASE: Release = Release {
    url: "https://github.com/matty/VRFaceTracking/releases/download/tongue-synthetic-v4/tongue-synthetic-v4.zip",
    name: "tongue-synthetic-v4.zip",
    sha256: "4620d22c3bed55945330ac907e794396c4b073a53fa3e85863947dfa29e123f5",
    megabytes: 123,
    what: "the training examples",
};
/// Where the examples unpack, beside the mouth-camera pair, and their files.
const EXAMPLES_DIR: &str = "tongue-synthetic-v4";
const EXAMPLES_FILES: [&str; 3] = ["metadata.json", "samples.jsonl", "frames.gray8"];
const MODELS: [(&str, &str); 2] = [
    (
        "qpro-stereo-tongue-v8-gate.pt",
        "57d9d04f1a569e40836cbb7a7af217986f8b65ffad103ff86bc2a2d07afc35ff",
    ),
    (
        "qpro-stereo-tongue-v8-direction.pt",
        "1900e8761c9ceaf89069121af1016ba24c33849836a5b7ee94b4dfd9fc7db396",
    ),
];
/// QFT+'s release whose per-frame logic VRFT's `universal_v2` port matches
/// (v0.4.0-rc.25.2). Its package holds the model's two files.
const QFTPLUS_RELEASE: Release = Release {
    url: "https://github.com/Yeusepe/QFTPlus/releases/download/v0.4.0-rc.25.2/QproFaceTracking.App-0.4.0-rc.25.2-full.nupkg",
    name: "QproFaceTracking.App-0.4.0-rc.25.2-full.nupkg",
    sha256: "fffa879943724c5621ecdef42085bcb73aae19a5ccc12b5255d74220ba9bc61d",
    megabytes: 194,
    what: "QFT+'s model",
};
/// Where QFT+'s model goes, apart from the mouth-camera pair.
const QFTPLUS_DIR: &str = "models/qftplus";
/// Where its files sit in the package.
const QFTPLUS_ENTRY: &str = "lib/app/models";
/// Its files: the graph, then the heads that name it.
const QFTPLUS_FILES: [(&str, &str); 2] = [
    (
        "universal-face-v2.area.onnx",
        "991fd0bad7afe52865feed0ba9c463254b21663826f704ba04512daa776ea935",
    ),
    (
        vrft_tongue::universal_v2::FILE_NAME,
        "8995bef488beb9aea3606306be0ae2ba2c4759bfc4b221b1833a5663ff7d5199",
    ),
];
/// What an install stops with when it was cancelled rather than failed.
const CANCELLED: &str = "cancelled";

/// What a [`BuiltinModel`] installs.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Package {
    /// What personal training needs: the mouth-camera pair, then the
    /// training examples.
    #[default]
    Pair,
    /// The QFTPlus Model: the mouth-camera pair, then QFT+'s universal face
    /// model.
    QftPlus,
}

#[derive(Clone, Default)]
pub struct InstallState {
    pub installing: bool,
    /// Download progress, 0 to 1, while installing.
    pub fraction: Option<f64>,
    /// Why the last attempt failed.
    pub error: Option<String>,
    pub stage: Option<BuiltinStage>,
    /// Bytes downloaded, and in the whole release when the server says.
    pub received: Option<u64>,
    pub total: Option<u64>,
    pub bytes_per_second: Option<f64>,
    /// The last attempt was cancelled.
    pub cancelled: bool,
}

#[derive(Clone, Default)]
pub struct BuiltinModel {
    state: Arc<Mutex<InstallState>>,
    cancel: Arc<AtomicBool>,
    package: Package,
}

fn models_dir(root: &Path) -> PathBuf {
    root.join("models/quest-pro")
}

fn qftplus_dir(root: &Path) -> PathBuf {
    root.join(QFTPLUS_DIR)
}

/// QFT+'s model, once both its files are in place: the heads' `.npz`, with
/// the graph beside it.
pub fn qftplus_model(root: &Path) -> Option<PathBuf> {
    let dir = qftplus_dir(root);
    QFTPLUS_FILES
        .iter()
        .all(|(name, _)| dir.join(name).is_file())
        .then(|| dir.join(vrft_tongue::universal_v2::FILE_NAME))
}

/// Megabytes installing the QFTPlus Model would still download: QFT+'s
/// model and the mouth-camera pair.
pub fn qftplus_megabytes(root: &Path) -> u32 {
    let pair = if installed(root) {
        0
    } else {
        MODEL_RELEASE.megabytes
    };
    let qftplus = if qftplus_model(root).is_some() {
        0
    } else {
        QFTPLUS_RELEASE.megabytes
    };
    pair + qftplus
}

/// Removes QFT+'s model, leaving anything else in its folder.
pub fn remove_qftplus(root: &Path) -> Result<(), String> {
    let dir = qftplus_dir(root);
    for (name, _) in QFTPLUS_FILES {
        match fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("couldn't remove {name}: {error}")),
        }
    }
    // Only goes when empty.
    let _ = fs::remove_dir(&dir);
    Ok(())
}

/// Whether the mouth-camera pair (or a replacement pair) is in place.
pub fn installed(root: &Path) -> bool {
    let dir = models_dir(root);
    Role::Gate.find(&dir).is_some() && Role::Direction.find(&dir).is_some()
}

/// The synthetic training examples, once they're in place.
pub fn examples_dir(root: &Path) -> Option<PathBuf> {
    let dir = models_dir(root).join(EXAMPLES_DIR);
    EXAMPLES_FILES
        .iter()
        .all(|file| dir.join(file).is_file())
        .then_some(dir)
}

/// Megabytes installing what training needs would still download.
pub fn download_megabytes(root: &Path) -> u32 {
    let pair = if installed(root) {
        0
    } else {
        MODEL_RELEASE.megabytes
    };
    let examples = if examples_dir(root).is_some() {
        0
    } else {
        EXAMPLES_RELEASE.megabytes
    };
    pair + examples
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", hasher.finalize()))
}

impl BuiltinModel {
    /// Installs the QFTPlus Model rather than what training needs.
    pub fn qftplus() -> Self {
        Self {
            package: Package::QftPlus,
            ..Self::default()
        }
    }

    pub fn state(&self) -> InstallState {
        self.state.lock().unwrap().clone()
    }

    /// Starts installing in the background, unless already installing.
    pub fn start(&self, root: PathBuf) {
        {
            let mut state = self.state.lock().unwrap();
            if state.installing {
                return;
            }
            *state = InstallState {
                installing: true,
                stage: Some(BuiltinStage::Connecting),
                ..InstallState::default()
            };
        }
        self.cancel.store(false, Ordering::SeqCst);
        let this = self.clone();
        let download = move || {
            let result = this.install(&root);
            let cancelled = result.as_ref().err().map(String::as_str) == Some(CANCELLED);
            *this.state.lock().unwrap() = InstallState {
                error: result.err().filter(|_| !cancelled),
                cancelled,
                ..InstallState::default()
            };
        };
        let name = match self.package {
            Package::Pair => "quest-pro-model-download",
            Package::QftPlus => "qftplus-model-download",
        };
        std::thread::Builder::new()
            .name(name.into())
            .spawn(download)
            .expect("couldn't start the model download thread");
    }

    /// Stops a running install at its next step, removing what it had
    /// downloaded.
    pub fn cancel(&self) {
        if self.state.lock().unwrap().installing {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }

    fn check_cancelled(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::SeqCst) {
            Err(CANCELLED.into())
        } else {
            Ok(())
        }
    }

    fn set_stage(&self, stage: BuiltinStage) {
        self.state.lock().unwrap().stage = Some(stage);
    }

    fn progress(&self, received: u64, total: Option<u64>, bytes_per_second: Option<f64>) {
        let mut state = self.state.lock().unwrap();
        state.received = Some(received);
        state.total = total;
        state.fraction = total.map(|total| received as f64 / total.max(1) as f64);
        state.bytes_per_second = bytes_per_second;
    }

    fn install(&self, root: &Path) -> Result<(), String> {
        match self.package {
            Package::Pair => {
                self.install_pair(root)?;
                self.install_examples(root)
            }
            // The pair first: the Training page tells the two downloads
            // apart by whether the pair is in place yet.
            Package::QftPlus => {
                self.install_pair(root)?;
                self.install_qftplus(root)
            }
        }
    }

    /// Takes QFT+'s model out of its release package, checking each file.
    /// Like the pair, files already there are never overwritten.
    fn install_qftplus(&self, root: &Path) -> Result<(), String> {
        let dir = qftplus_dir(root);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let missing: Vec<_> = QFTPLUS_FILES
            .iter()
            .filter(|(name, hash)| {
                let path = dir.join(name);
                !(path.is_file() && sha256_file(&path).as_deref() == Ok(*hash))
            })
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        for (name, _) in &missing {
            if dir.join(name).exists() {
                return Err(format!(
                    "{} differs from QFT+'s model; move it away first. VRFaceTracking never overwrites model files.",
                    dir.join(name).display()
                ));
            }
        }
        let archive = self.download(root, &QFTPLUS_RELEASE)?;
        self.check_cancelled()?;
        self.set_stage(BuiltinStage::Unpacking);
        let mut zip = zip::ZipArchive::new(File::open(&archive).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        // The graph goes in first, so the heads never appear without it.
        for (name, hash) in missing {
            let mut entry = zip
                .by_name(&format!("{QFTPLUS_ENTRY}/{name}"))
                .map_err(|_| format!("{name} is missing from QFT+'s release"))?;
            let pending = dir.join(format!("{name}.download"));
            let mut out = File::create(&pending).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            drop(out);
            if sha256_file(&pending)? != *hash {
                let _ = fs::remove_file(&pending);
                return Err(format!("{name} failed its SHA-256 check"));
            }
            fs::rename(&pending, dir.join(name)).map_err(|e| e.to_string())?;
        }
        drop(zip);
        // The rest of QFT+'s app isn't needed.
        let _ = fs::remove_file(&archive);
        Ok(())
    }

    fn install_pair(&self, root: &Path) -> Result<(), String> {
        let dir = models_dir(root);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let missing: Vec<_> = MODELS
            .iter()
            .filter(|(name, hash)| {
                let path = dir.join(name);
                !(path.is_file() && sha256_file(&path).as_deref() == Ok(*hash))
            })
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        for (name, _) in &missing {
            if dir.join(name).exists() {
                return Err(format!(
                    "{} differs from the mouth-camera model; move it away first. VRFaceTracking never overwrites model files.",
                    dir.join(name).display()
                ));
            }
        }
        let archive = self.download(root, &MODEL_RELEASE)?;
        self.check_cancelled()?;
        self.set_stage(BuiltinStage::Unpacking);
        let mut zip = zip::ZipArchive::new(File::open(&archive).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        for (name, hash) in missing {
            let entry_name = format!("QproFaceTracking-0.1.10-poc/models/{name}");
            let mut entry = zip
                .by_name(&entry_name)
                .map_err(|_| format!("{name} is missing from the release"))?;
            let pending = dir.join(format!("{name}.download"));
            let mut out = File::create(&pending).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            drop(out);
            if sha256_file(&pending)? != *hash {
                let _ = fs::remove_file(&pending);
                return Err(format!("{name} failed its SHA-256 check"));
            }
            fs::rename(&pending, dir.join(name)).map_err(|e| e.to_string())?;
        }
        drop(zip);
        // Only the pair is needed, so the release doesn't keep 140 MB on disk.
        let _ = fs::remove_file(&archive);
        Ok(())
    }

    /// Unpacks the training examples beside the pair, through a temporary
    /// folder so training never reads a half-unpacked set.
    fn install_examples(&self, root: &Path) -> Result<(), String> {
        if examples_dir(root).is_some() {
            return Ok(());
        }
        let archive = self.download(root, &EXAMPLES_RELEASE)?;
        self.check_cancelled()?;
        self.set_stage(BuiltinStage::Unpacking);
        let dir = models_dir(root).join(EXAMPLES_DIR);
        let pending = models_dir(root).join(format!("{EXAMPLES_DIR}.download"));
        let _ = fs::remove_dir_all(&pending);
        fs::create_dir_all(&pending).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(File::open(&archive).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|e| e.to_string())?;
            // The files sit in one folder; take each by its own name.
            let name = entry.name().rsplit(['/', '\\']).next().unwrap_or("");
            if !entry.is_file() || !EXAMPLES_FILES.contains(&name) {
                continue;
            }
            let mut out = File::create(pending.join(name)).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        }
        drop(zip);
        if let Some(file) = EXAMPLES_FILES.iter().find(|f| !pending.join(f).is_file()) {
            let _ = fs::remove_dir_all(&pending);
            return Err(format!("{file} is missing from the training examples"));
        }
        // What an interrupted install from before may have left.
        let _ = fs::remove_dir_all(&dir);
        fs::rename(&pending, &dir).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(&archive);
        Ok(())
    }

    /// The verified release zip in `.local/`, downloaded unless it's there.
    fn download(&self, root: &Path, release: &Release) -> Result<PathBuf, String> {
        let local = root.join(".local");
        fs::create_dir_all(&local).map_err(|e| e.to_string())?;
        let archive = local.join(release.name);
        if archive.is_file() {
            self.set_stage(BuiltinStage::Verifying);
            if sha256_file(&archive)? == release.sha256 {
                return Ok(archive);
            }
            self.set_stage(BuiltinStage::Connecting);
        }
        let mut response = ureq::get(release.url)
            .call()
            .map_err(|e| format!("could not download {}: {e}", release.what))?;
        let total = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        self.progress(0, total, None);
        self.set_stage(BuiltinStage::Downloading);
        let pending = local.join(format!("{}.download", release.name));
        if let Err(error) = self.receive(response.body_mut(), &pending, total, release) {
            let _ = fs::remove_file(&pending);
            return Err(error);
        }
        fs::rename(&pending, &archive).map_err(|e| e.to_string())?;
        Ok(archive)
    }

    /// Writes the release to `pending`, checking its SHA-256 as it arrives.
    fn receive(
        &self,
        body: &mut ureq::Body,
        pending: &Path,
        total: Option<u64>,
        release: &Release,
    ) -> Result<(), String> {
        let mut out = File::create(pending).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut reader = body.with_config().limit(512 << 20).reader();
        let mut buffer = vec![0u8; 1 << 16];
        let mut received = 0u64;
        // The speed, smoothed and updated about four times a second.
        let mut since = (Instant::now(), 0u64);
        let mut speed: Option<f64> = None;
        loop {
            self.check_cancelled()?;
            let read = reader
                .read(&mut buffer)
                .map_err(|e| format!("download interrupted: {e}"))?;
            if read == 0 {
                break;
            }
            out.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
            hasher.update(&buffer[..read]);
            received += read as u64;
            let elapsed = since.0.elapsed().as_secs_f64();
            if elapsed >= 0.25 {
                let now = (received - since.1) as f64 / elapsed;
                speed = Some(speed.map_or(now, |before| before * 0.6 + now * 0.4));
                since = (Instant::now(), received);
            }
            self.progress(received, total, speed);
        }
        drop(out);
        self.set_stage(BuiltinStage::Verifying);
        if format!("{:x}", hasher.finalize()) != release.sha256 {
            return Err("the downloaded release failed its SHA-256 check".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Installs from the release zip `setup` left in `.local/`, without
    /// downloading; skipped when that zip isn't there.
    #[test]
    fn installs_from_a_verified_release_and_never_overwrites() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let cached = repo.join(".local").join(MODEL_RELEASE.name);
        if !cached.is_file() {
            eprintln!("skipped: no cached release zip");
            return;
        }
        let root = repo.join(".local/tongue-tests-rust").join(format!(
            "builtin-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join(".local")).unwrap();
        fs::hard_link(&cached, root.join(".local").join(MODEL_RELEASE.name)).unwrap();
        assert!(!installed(&root));
        let builtin = BuiltinModel::default();
        builtin.install_pair(&root).unwrap();
        assert!(installed(&root));

        let gate = models_dir(&root).join(MODELS[0].0);
        fs::write(&gate, b"personal").unwrap();
        fs::remove_file(models_dir(&root).join(MODELS[1].0)).unwrap();
        assert!(builtin
            .install_pair(&root)
            .unwrap_err()
            .contains("never overwrites"));
        assert_eq!(fs::read(&gate).unwrap(), b"personal");
        fs::remove_dir_all(root).unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        repo.join(".local/tongue-tests-rust").join(format!(
            "{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn qftplus_model_needs_both_files_and_removes_only_them() {
        let root = scratch("qftplus-files");
        let dir = qftplus_dir(&root);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            qftplus_megabytes(&root),
            MODEL_RELEASE.megabytes + QFTPLUS_RELEASE.megabytes
        );
        fs::write(dir.join(QFTPLUS_FILES[1].0), b"heads").unwrap();
        assert_eq!(qftplus_model(&root), None);
        fs::write(dir.join(QFTPLUS_FILES[0].0), b"graph").unwrap();
        assert_eq!(
            qftplus_model(&root),
            Some(dir.join(vrft_tongue::universal_v2::FILE_NAME))
        );
        // The mouth-camera pair comes with it.
        assert_eq!(qftplus_megabytes(&root), MODEL_RELEASE.megabytes);
        let pair = models_dir(&root);
        fs::create_dir_all(&pair).unwrap();
        for role in [Role::Gate, Role::Direction] {
            fs::write(role.safetensors(&pair), b"test").unwrap();
        }
        assert_eq!(qftplus_megabytes(&root), 0);

        fs::write(dir.join("notes.txt"), b"mine").unwrap();
        remove_qftplus(&root).unwrap();
        assert_eq!(qftplus_model(&root), None);
        assert!(dir.join("notes.txt").is_file());
        fs::remove_file(dir.join("notes.txt")).unwrap();
        remove_qftplus(&root).unwrap();
        assert!(!dir.exists());
        fs::remove_dir_all(root).unwrap();
    }

    /// Installs from QFT+'s release package left in `.local/`, without
    /// downloading; skipped when that package isn't there.
    #[test]
    fn installs_qftplus_from_its_verified_release() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let cached = repo.join(".local").join(QFTPLUS_RELEASE.name);
        if !cached.is_file() {
            eprintln!("skipped: no cached QFT+ release package");
            return;
        }
        let root = scratch("qftplus");
        fs::create_dir_all(root.join(".local")).unwrap();
        fs::hard_link(&cached, root.join(".local").join(QFTPLUS_RELEASE.name)).unwrap();
        let qftplus = BuiltinModel::qftplus();
        qftplus.install_qftplus(&root).unwrap();
        assert!(qftplus_model(&root).is_some());
        assert!(!installed(&root), "the pair comes from its own release");

        let graph = qftplus_dir(&root).join(QFTPLUS_FILES[0].0);
        fs::write(&graph, b"other").unwrap();
        fs::remove_file(qftplus_dir(&root).join(QFTPLUS_FILES[1].0)).unwrap();
        assert!(qftplus
            .install_qftplus(&root)
            .unwrap_err()
            .contains("never overwrites"));
        assert_eq!(fs::read(&graph).unwrap(), b"other");
        fs::remove_dir_all(root).unwrap();
    }

    /// Unpacks the training examples from the release zip that
    /// `pack_synthetic` and 7-Zip made in `.local/tongue-synthetic-release/`,
    /// without downloading; skipped when that zip isn't there.
    #[test]
    fn installs_the_training_examples_whole() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let packed = repo
            .join(".local/tongue-synthetic-release")
            .join(EXAMPLES_RELEASE.name);
        if !packed.is_file() {
            eprintln!("skipped: no packed training examples");
            return;
        }
        let root = repo.join(".local/tongue-tests-rust").join(format!(
            "examples-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join(".local")).unwrap();
        fs::hard_link(&packed, root.join(".local").join(EXAMPLES_RELEASE.name)).unwrap();
        assert_eq!(
            download_megabytes(&root),
            MODEL_RELEASE.megabytes + EXAMPLES_RELEASE.megabytes
        );
        BuiltinModel::default().install_examples(&root).unwrap();
        let dir = examples_dir(&root).expect("examples in place");
        assert_eq!(download_megabytes(&root), MODEL_RELEASE.megabytes);
        let recording = vrft_tongue::recordings::Recording::open(&dir).unwrap();
        assert!(recording.synthetic && !recording.samples.is_empty());
        assert!(!models_dir(&root)
            .join(format!("{EXAMPLES_DIR}.download"))
            .exists());
        fs::remove_dir_all(root).unwrap();
    }
}
