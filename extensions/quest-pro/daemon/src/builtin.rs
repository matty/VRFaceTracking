//! Installs the built-in tongue model pair: the v8 demo checkpoints from the
//! Qpro-Enhanced-FT v0.1.10 release (MIT license), verified by SHA-256.
//! Files already present are never overwritten, and the release zip is
//! removed once the pair is out of it.

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use vrft_quest_pro_protocol::BuiltinStage;
use vrft_tongue::Role;

const RELEASE_URL: &str = "https://github.com/n0tmast3r/Qpro-Enhanced-FT/releases/download/v0.1.10/QproFaceTracking-0.1.10-poc.zip";
const RELEASE_NAME: &str = "QproFaceTracking-0.1.10-poc.zip";
const RELEASE_SHA256: &str = "db40f4b8331a50ca6c2ec37372f1ab4b44cfbe6d21e09ca04244aaaa339cb18f";
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
/// What an install stops with when it was cancelled rather than failed.
const CANCELLED: &str = "cancelled";

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
}

fn models_dir(root: &Path) -> PathBuf {
    root.join("models/quest-pro")
}

/// Whether the built-in pair (or a replacement pair) is in place.
pub fn installed(root: &Path) -> bool {
    let dir = models_dir(root);
    Role::Gate.find(&dir).is_some() && Role::Direction.find(&dir).is_some()
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", hasher.finalize()))
}

impl BuiltinModel {
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
        std::thread::Builder::new()
            .name("quest-pro-model-download".into())
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
                    "{} differs from the built-in model; move it away first. VRFaceTracking never overwrites model files.",
                    dir.join(name).display()
                ));
            }
        }
        let archive = self.download(root)?;
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

    /// The verified release zip in `.local/`, downloaded unless it's there.
    fn download(&self, root: &Path) -> Result<PathBuf, String> {
        let local = root.join(".local");
        fs::create_dir_all(&local).map_err(|e| e.to_string())?;
        let archive = local.join(RELEASE_NAME);
        if archive.is_file() {
            self.set_stage(BuiltinStage::Verifying);
            if sha256_file(&archive)? == RELEASE_SHA256 {
                return Ok(archive);
            }
            self.set_stage(BuiltinStage::Connecting);
        }
        let mut response = ureq::get(RELEASE_URL)
            .call()
            .map_err(|e| format!("could not download the built-in model: {e}"))?;
        let total = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        self.progress(0, total, None);
        self.set_stage(BuiltinStage::Downloading);
        let pending = local.join(format!("{RELEASE_NAME}.download"));
        if let Err(error) = self.receive(response.body_mut(), &pending, total) {
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
        if format!("{:x}", hasher.finalize()) != RELEASE_SHA256 {
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
        let cached = repo.join(".local").join(RELEASE_NAME);
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
        fs::hard_link(&cached, root.join(".local").join(RELEASE_NAME)).unwrap();
        assert!(!installed(&root));
        let builtin = BuiltinModel::default();
        builtin.install(&root).unwrap();
        assert!(installed(&root));

        let gate = models_dir(&root).join(MODELS[0].0);
        fs::write(&gate, b"personal").unwrap();
        fs::remove_file(models_dir(&root).join(MODELS[1].0)).unwrap();
        assert!(builtin
            .install(&root)
            .unwrap_err()
            .contains("never overwrites"));
        assert_eq!(fs::read(&gate).unwrap(), b"personal");
        fs::remove_dir_all(root).unwrap();
    }
}
