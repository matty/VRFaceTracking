//! Installs the QFTPlus Model, the base model: QFT+'s universal face model
//! (`universal-face-v2`) from QFT+'s own release, only when the user asks for
//! it on the Training page. With it comes the mouth-camera pair, which reads
//! the mouth cameras whenever the headset sends only those: the v8 demo
//! checkpoints from the Qpro-Enhanced-FT v0.1.10 release.
//! If a release can't be used, the same files come from the copies in VRFT's
//! repository (`third_party/models/`).
//! Every download is verified by SHA-256, files already present are never
//! overwritten, and each release is removed once its files are out of it.

use log::warn;
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
    /// Where its files sit in the zip.
    entry: &'static str,
    /// The files to keep and their SHA-256, in the order they go in.
    files: &'static [(&'static str, &'static str)],
    /// The folder of VRFT's repository holding copies of those files.
    mirror: &'static str,
}

/// VRFT's repository, for the copies used when a release can't be.
const MIRROR_BASE: &str = "https://raw.githubusercontent.com/matty/VRFaceTracking/main";

const MODEL_RELEASE: Release = Release {
    url: "https://github.com/n0tmast3r/Qpro-Enhanced-FT/releases/download/v0.1.10/QproFaceTracking-0.1.10-poc.zip",
    name: "QproFaceTracking-0.1.10-poc.zip",
    sha256: "db40f4b8331a50ca6c2ec37372f1ab4b44cfbe6d21e09ca04244aaaa339cb18f",
    megabytes: 140,
    what: "the mouth-camera model",
    entry: "QproFaceTracking-0.1.10-poc/models",
    files: &MODELS,
    mirror: "third_party/models/qpro-enhanced-ft",
};
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
    entry: "lib/app/models",
    files: &QFTPLUS_FILES,
    mirror: "third_party/models/qftplus",
};
/// Where QFT+'s model goes, apart from the mouth-camera pair.
const QFTPLUS_DIR: &str = "models/qftplus";
/// Its files: the graph, then the heads that name it.
const QFTPLUS_FILES: [(&str, &str); 2] = [
    (
        vrft_tongue::universal_v2::GRAPH_FILE,
        "991fd0bad7afe52865feed0ba9c463254b21663826f704ba04512daa776ea935",
    ),
    (
        vrft_tongue::universal_v2::FILE_NAME,
        "8995bef488beb9aea3606306be0ae2ba2c4759bfc4b221b1833a5663ff7d5199",
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

/// The QFTPlus Model's download.
#[derive(Clone, Default)]
pub struct BuiltinModel {
    state: Arc<Mutex<InstallState>>,
    cancel: Arc<AtomicBool>,
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

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Lower-case hex of a digest.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Those of `files` not yet in `dir` with the right contents.
fn missing_files(
    dir: &Path,
    files: &'static [(&'static str, &'static str)],
) -> Vec<(&'static str, &'static str)> {
    files
        .iter()
        .filter(|(name, hash)| {
            let path = dir.join(name);
            !(path.is_file() && sha256_file(&path).as_deref() == Ok(*hash))
        })
        .copied()
        .collect()
}

/// The size a response says it is.
fn content_length(response: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
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
            .name("qftplus-model-download".into())
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

    /// The pair first: the Training page tells the two downloads apart by
    /// whether the pair is in place yet.
    fn install(&self, root: &Path) -> Result<(), String> {
        self.install_pair(root)?;
        self.install_qftplus(root)
    }

    fn install_qftplus(&self, root: &Path) -> Result<(), String> {
        self.install_files(&qftplus_dir(root), root, &QFTPLUS_RELEASE, MIRROR_BASE)
    }

    fn install_pair(&self, root: &Path) -> Result<(), String> {
        self.install_files(&models_dir(root), root, &MODEL_RELEASE, MIRROR_BASE)
    }

    /// Puts `release`'s files in `dir`, from the release or, when that can't
    /// be used, from the copies at `mirror_base`, checking each file. Files
    /// already there are never overwritten.
    fn install_files(
        &self,
        dir: &Path,
        root: &Path,
        release: &Release,
        mirror_base: &str,
    ) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let missing = missing_files(dir, release.files);
        if missing.is_empty() {
            return Ok(());
        }
        for (name, _) in &missing {
            if dir.join(name).exists() {
                return Err(format!(
                    "{} differs from {}; move it away first. VRFaceTracking never overwrites model files.",
                    dir.join(name).display(),
                    release.what
                ));
            }
        }
        let upstream = match self.install_from_release(dir, root, release) {
            Ok(()) => return Ok(()),
            Err(error) if error == CANCELLED => return Err(error),
            Err(error) => error,
        };
        warn!("{upstream}; trying VRFaceTracking's own copy");
        self.install_from_mirror(dir, release, mirror_base)
            .map_err(|error| {
                if error == CANCELLED {
                    error
                } else {
                    format!("{upstream}; VRFaceTracking's own copy failed too: {error}")
                }
            })
    }

    /// Takes the files out of the release zip, which only they are needed from.
    fn install_from_release(
        &self,
        dir: &Path,
        root: &Path,
        release: &Release,
    ) -> Result<(), String> {
        let archive = self.download(root, release)?;
        self.check_cancelled()?;
        self.set_stage(BuiltinStage::Unpacking);
        let mut zip = zip::ZipArchive::new(File::open(&archive).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        // In listed order, so QFT+'s heads never appear without their graph.
        for (name, hash) in missing_files(dir, release.files) {
            let mut entry = zip
                .by_name(&format!("{}/{name}", release.entry))
                .map_err(|_| format!("{name} is missing from the release of {}", release.what))?;
            let pending = dir.join(format!("{name}.download"));
            let mut out = File::create(&pending).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            drop(out);
            if sha256_file(&pending)? != hash {
                let _ = fs::remove_file(&pending);
                return Err(format!("{name} failed its SHA-256 check"));
            }
            fs::rename(&pending, dir.join(name)).map_err(|e| e.to_string())?;
        }
        drop(zip);
        let _ = fs::remove_file(&archive);
        Ok(())
    }

    /// Downloads whichever files are still missing from VRFT's repository.
    fn install_from_mirror(
        &self,
        dir: &Path,
        release: &Release,
        mirror_base: &str,
    ) -> Result<(), String> {
        for (name, hash) in missing_files(dir, release.files) {
            self.check_cancelled()?;
            self.set_stage(BuiltinStage::Connecting);
            let url = format!("{mirror_base}/{}/{name}", release.mirror);
            let mut response = ureq::get(&url)
                .call()
                .map_err(|e| format!("could not download {name}: {e}"))?;
            let total = content_length(&response);
            self.progress(0, total, None);
            self.set_stage(BuiltinStage::Downloading);
            let pending = dir.join(format!("{name}.download"));
            if let Err(error) = self.receive(response.body_mut(), &pending, total, hash) {
                let _ = fs::remove_file(&pending);
                return Err(error);
            }
            fs::rename(&pending, dir.join(name)).map_err(|e| e.to_string())?;
        }
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
        let total = content_length(&response);
        self.progress(0, total, None);
        self.set_stage(BuiltinStage::Downloading);
        let pending = local.join(format!("{}.download", release.name));
        if let Err(error) = self.receive(response.body_mut(), &pending, total, release.sha256) {
            let _ = fs::remove_file(&pending);
            return Err(error);
        }
        fs::rename(&pending, &archive).map_err(|e| e.to_string())?;
        Ok(archive)
    }

    /// Writes the download to `pending`, checking its SHA-256 as it arrives.
    fn receive(
        &self,
        body: &mut ureq::Body,
        pending: &Path,
        total: Option<u64>,
        sha256: &str,
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
        if hex(&hasher.finalize()) != sha256 {
            return Err("the download failed its SHA-256 check".into());
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
        let qftplus = BuiltinModel::default();
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

    /// The copies the install falls back to, as checked out: wrong bytes here
    /// would only show the day a release has gone.
    #[test]
    fn the_repository_copies_match_the_pinned_hashes() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        for release in [&MODEL_RELEASE, &QFTPLUS_RELEASE] {
            for (name, hash) in release.files {
                let path = repo.join(release.mirror).join(name);
                assert_eq!(sha256_file(&path).as_deref(), Ok(*hash), "{name}");
            }
        }
    }

    /// Answers each path in `files` with its bytes and anything else with a
    /// 404, returning the base URL.
    fn serve(files: Vec<(String, Vec<u8>)>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut request = [0u8; 4096];
                let read = stream.read(&mut request).unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]);
                let path = request.split_whitespace().nth(1).unwrap_or_default();
                let found = files.iter().find(|(served, _)| served == path);
                let (status, body): (_, &[u8]) = match found {
                    Some((_, body)) => ("200 OK", body),
                    None => ("404 Not Found", b""),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        base
    }

    fn sha256_of(bytes: &[u8]) -> &'static str {
        hex(&Sha256::digest(bytes)).leak()
    }

    /// A release whose zip is at `url`, and whose copies are `contents`.
    fn test_release(url: String, zip_sha256: &'static str, contents: &[&[u8]]) -> Release {
        let files: Vec<(&'static str, &'static str)> = ["first.bin", "second.bin"]
            .into_iter()
            .zip(contents)
            .map(|(name, bytes)| (name, sha256_of(bytes)))
            .collect();
        Release {
            url: url.leak(),
            name: "release.zip",
            sha256: zip_sha256,
            megabytes: 1,
            what: "the test model",
            entry: "inner",
            files: files.leak(),
            mirror: "copies",
        }
    }

    #[test]
    fn a_gone_release_installs_from_the_repository_copies() {
        let root = scratch("mirror");
        let dir = root.join("models");
        let base = serve(vec![
            ("/copies/first.bin".into(), b"first".to_vec()),
            ("/copies/second.bin".into(), b"second".to_vec()),
        ]);
        let release = test_release(format!("{base}/gone.zip"), "", &[b"first", b"second"]);

        BuiltinModel::default()
            .install_files(&dir, &root, &release, &base)
            .unwrap();

        assert_eq!(fs::read(dir.join("first.bin")).unwrap(), b"first");
        assert_eq!(fs::read(dir.join("second.bin")).unwrap(), b"second");
        assert!(!dir.join("second.bin.download").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_copy_with_the_wrong_hash_is_refused_and_both_failures_are_reported() {
        let root = scratch("mirror-bad");
        let dir = root.join("models");
        let base = serve(vec![
            ("/copies/first.bin".into(), b"first".to_vec()),
            ("/copies/second.bin".into(), b"tampered".to_vec()),
        ]);
        let release = test_release(format!("{base}/gone.zip"), "", &[b"first", b"second"]);

        let error = BuiltinModel::default()
            .install_files(&dir, &root, &release, &base)
            .unwrap_err();

        assert!(
            error.contains("could not download the test model"),
            "{error}"
        );
        assert!(error.contains("SHA-256"), "{error}");
        assert!(!dir.join("second.bin").exists());
        assert!(!dir.join("second.bin.download").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelling_does_not_fall_back_to_the_copies() {
        let root = scratch("mirror-cancel");
        let dir = root.join("models");
        let base = serve(vec![("/copies/first.bin".into(), b"first".to_vec())]);
        let release = test_release(format!("{base}/gone.zip"), "", &[b"first"]);
        let model = BuiltinModel::default();
        model.cancel.store(true, Ordering::SeqCst);

        let error = model
            .install_files(&dir, &root, &release, &base)
            .unwrap_err();

        assert_eq!(error, CANCELLED);
        assert!(!dir.join("first.bin").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_working_release_needs_no_copies_and_is_removed_afterwards() {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in [
            ("inner/first.bin", b"first".as_slice()),
            ("inner/other", b"x"),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        let zip = zip.finish().unwrap().into_inner();
        let zip_sha256 = sha256_of(&zip);

        let root = scratch("release");
        let dir = root.join("models");
        let base = serve(vec![("/release.zip".into(), zip)]);
        let release = test_release(format!("{base}/release.zip"), zip_sha256, &[b"first"]);

        BuiltinModel::default()
            .install_files(&dir, &root, &release, &format!("{base}/no-copies-here"))
            .unwrap();

        assert_eq!(fs::read(dir.join("first.bin")).unwrap(), b"first");
        assert!(!dir.join("other").exists());
        assert!(!root.join(".local/release.zip").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
