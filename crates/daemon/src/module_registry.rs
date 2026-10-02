//! The VRCFT module registry: listing its modules, and installing, updating
//! and removing them.
//!
//! Each module goes in `plugins/registry/<ModuleId>/`, laid out as VRCFT lays
//! out its own installs: the download's files, plus the registry entry saved
//! as `module.json`, which tells discovery which `.dll` is the module and
//! which are its dependencies.
//!
//! A download is unpacked into `plugins/registry/.staging/<ModuleId>/` first,
//! then swapped in. Windows won't move files a running module has open, so an
//! update to the module in use waits in `.pending/` and is swapped in when
//! VRFT next starts, before any module loads.
use crate::plugin_loader::{self, contained, MANIFEST};
use anyhow::{bail, Context, Result};
use log::{info, warn};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use vrft_protocol::RegistryModule;

pub const DEFAULT_REGISTRY_URL: &str = "https://registry.vrcft.io/modules";
/// Where registry modules live, under `plugins/`.
pub const REGISTRY_DIR: &str = "registry";
const STAGING: &str = ".staging";
const PENDING: &str = ".pending";
const TRASH: &str = ".trash";
/// Largest download accepted. The biggest registry module is about 15 MB.
const MAX_DOWNLOAD: u64 = 256 << 20;
/// Largest total size a download may unpack to.
const MAX_UNPACKED: u64 = 1 << 30;

/// Whether `id` is safe to use as a folder name. Registry ids are GUIDs.
pub fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn registry_dir(plugins: &Path) -> PathBuf {
    plugins.join(REGISTRY_DIR)
}

/// Where module `id` is installed.
pub fn module_dir(plugins: &Path, id: &str) -> PathBuf {
    registry_dir(plugins).join(id)
}

/// Whether an update to module `id` waits for VRFT to restart.
pub fn pending(plugins: &Path, id: &str) -> bool {
    registry_dir(plugins).join(PENDING).join(id).is_dir()
}

/// Fetches the registry's module list.
pub fn fetch(agent: &ureq::Agent, url: &str) -> Result<Vec<RegistryModule>> {
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("couldn't reach the module registry at {url}"))?;
    if !response.status().is_success() {
        bail!("the module registry answered {}", response.status());
    }
    // The registry says text/plain, so read text and parse it here.
    let text = response
        .body_mut()
        .with_config()
        .limit(16 << 20)
        .read_to_string()
        .context("the module registry's reply was cut off")?;
    let mut modules: Vec<RegistryModule> = serde_json::from_str(&text)
        .context("the module registry sent a list this VRFaceTracking doesn't understand")?;
    modules.retain(|module| valid_id(&module.module_id) && !module.dll_file_name.is_empty());
    modules.sort_by_key(|module| std::cmp::Reverse(module.downloads));
    Ok(modules)
}

/// Downloads `url`, reporting progress from 0 to 1 when the size is known.
pub fn download(agent: &ureq::Agent, url: &str, mut progress: impl FnMut(f32)) -> Result<Vec<u8>> {
    if !url.starts_with("https://") {
        bail!("the module's download isn't HTTPS ({url}), so VRFaceTracking won't install it");
    }
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("couldn't download {url}"))?;
    if !response.status().is_success() {
        bail!("the download answered {}", response.status());
    }
    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD)
        .reader();
    let mut bytes = Vec::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = reader.read(&mut buffer).context("the download stopped")?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(total) = total {
            progress((bytes.len() as f64 / total.max(1) as f64).min(1.0) as f32);
        }
    }
    Ok(bytes)
}

/// Unpacks a download of `entry` (a `.zip`, or the module `.dll` itself)
/// into the staging folder, checks the module file is there, and writes
/// `module.json`. Returns the staged folder.
pub fn stage(plugins: &Path, entry: &RegistryModule, bytes: &[u8]) -> Result<PathBuf> {
    if !valid_id(&entry.module_id) {
        bail!("the registry gave this module an unusable id");
    }
    let dir = registry_dir(plugins).join(STAGING).join(&entry.module_id);
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("couldn't clear {}", dir.display()))?;
    }
    fs::create_dir_all(&dir).with_context(|| format!("couldn't create {}", dir.display()))?;
    let main = contained(&dir, &entry.dll_file_name)
        .context("the registry names the module's file with a path outside its folder")?;
    if bytes.starts_with(b"PK\x03\x04") {
        unzip(bytes, &dir, MAX_UNPACKED)?;
    } else if bytes.starts_with(b"MZ") {
        if let Some(parent) = main.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&main, bytes).with_context(|| format!("couldn't write {}", main.display()))?;
    } else {
        bail!("the download is neither a .zip nor a .dll");
    }
    if !main.is_file() {
        bail!(
            "the download doesn't contain {}, the file the registry names",
            entry.dll_file_name
        );
    }
    plugin_loader::detect_plugin_kind(&main)
        .with_context(|| format!("{} isn't a module library", entry.dll_file_name))?;
    fs::write(dir.join(MANIFEST), serde_json::to_vec_pretty(entry)?)?;
    Ok(dir)
}

/// Unpacks the `.zip` in `bytes` into `dir`, failing once more than `limit`
/// bytes come out.
fn unzip(bytes: &[u8], dir: &Path, limit: u64) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("the download isn't a valid .zip")?;
    let mut unpacked = 0u64;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        let Some(name) = file.enclosed_name() else {
            warn!(
                "Skipping {:?}: its path leaves the module folder",
                file.name()
            );
            continue;
        };
        // macOS metadata that some modules ship by accident.
        let apple = name.components().any(|component| {
            let part = component.as_os_str().to_string_lossy();
            part == "__MACOSX" || part.starts_with("._")
        });
        if apple || file.is_symlink() {
            continue;
        }
        let path = dir.join(&name);
        if file.is_dir() {
            fs::create_dir_all(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&path)
            .with_context(|| format!("couldn't write {}", path.display()))?;
        // Counted as written, as a .zip can say a file is smaller than it is.
        let left = limit - unpacked;
        unpacked += std::io::copy(&mut (&mut file).take(left + 1), &mut out)?;
        if unpacked > limit {
            bail!("the download unpacks to more than {} MB", limit >> 20);
        }
    }
    Ok(())
}

/// How a staged module was put in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placed {
    Installed,
    /// The installed files are in use, so the new ones replace them when
    /// VRFT next starts.
    AfterRestart,
}

/// Moves the staged module `id` into place, replacing any installed version.
pub fn place(plugins: &Path, id: &str, staged: &Path) -> Result<Placed> {
    let target = module_dir(plugins, id);
    let waiting = registry_dir(plugins).join(PENDING).join(id);
    let mut old = None;
    if target.exists() {
        match move_to_trash(plugins, &target) {
            Ok(trashed) => old = Some(trashed),
            Err(_) => {
                if waiting.exists() {
                    fs::remove_dir_all(&waiting)?;
                }
                fs::create_dir_all(waiting.parent().unwrap())?;
                fs::rename(staged, &waiting)
                    .context("couldn't keep the update for the next start")?;
                return Ok(Placed::AfterRestart);
            }
        }
    }
    if waiting.exists() {
        let _ = fs::remove_dir_all(&waiting);
    }
    if let Err(error) = fs::rename(staged, &target) {
        // Put the installed version back, before emptying the trash loses it.
        if let Some(old) = old {
            if let Err(restore) = fs::rename(&old, &target) {
                warn!(
                    "Couldn't put {} back as {}: {restore}",
                    old.display(),
                    target.display()
                );
            }
        }
        return Err(error)
            .with_context(|| format!("couldn't move the module into {}", target.display()));
    }
    // Only goes if nothing else is being staged.
    let _ = fs::remove_dir(registry_dir(plugins).join(STAGING));
    empty_trash(plugins);
    Ok(Placed::Installed)
}

/// Removes installed module `id`, and any update waiting for it.
pub fn uninstall(plugins: &Path, id: &str) -> Result<()> {
    if !valid_id(id) {
        bail!("{id} isn't a module id");
    }
    let target = module_dir(plugins, id);
    let waiting = registry_dir(plugins).join(PENDING).join(id);
    if waiting.exists() {
        fs::remove_dir_all(&waiting)?;
    }
    if !target.exists() {
        bail!("that module isn't installed from the registry");
    }
    move_to_trash(plugins, &target).map(drop).context(
        "its files are in use. Choose another module, restart VRFaceTracking, then remove it",
    )?;
    empty_trash(plugins);
    Ok(())
}

/// Moves `dir` aside in one step, so a module is never left half deleted.
/// Returns where it went.
fn move_to_trash(plugins: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let trash = registry_dir(plugins).join(TRASH);
    fs::create_dir_all(&trash)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_nanos())
        .unwrap_or_default();
    let name = dir.file_name().unwrap_or_default().to_string_lossy();
    let trashed = trash.join(format!("{name}-{stamp}"));
    fs::rename(dir, &trashed)?;
    Ok(trashed)
}

fn empty_trash(plugins: &Path) {
    let trash = registry_dir(plugins).join(TRASH);
    if trash.exists() {
        if let Err(error) = fs::remove_dir_all(&trash) {
            warn!("Couldn't empty {}: {error}", trash.display());
        }
    }
}

/// Swaps in updates that waited for a restart, and clears leftovers. Runs at
/// startup, before any module loads.
pub fn apply_pending(plugins: &Path) {
    let registry = registry_dir(plugins);
    let _ = fs::remove_dir_all(registry.join(STAGING));
    empty_trash(plugins);
    let Ok(entries) = fs::read_dir(registry.join(PENDING)) else {
        return;
    };
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().to_string();
        if !valid_id(&id) {
            continue;
        }
        let staged = registry.join(STAGING).join(&id);
        let moved = fs::create_dir_all(registry.join(STAGING))
            .and_then(|_| fs::rename(entry.path(), &staged));
        match moved
            .map_err(anyhow::Error::from)
            .and_then(|_| place(plugins, &id, &staged))
        {
            Ok(Placed::Installed) => info!("Applied the update to module {id}"),
            Ok(Placed::AfterRestart) => warn!("Module {id} is still in use; its update waits"),
            Err(error) => warn!("Couldn't apply the update to module {id}: {error:#}"),
        }
    }
    let _ = fs::remove_dir(registry.join(PENDING));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{managed_dll, temp_dir};
    use std::io::Write;

    #[test]
    fn a_failed_update_keeps_the_installed_version() {
        let plugins = temp_dir("registry_restore");
        let entry = entry("Link.dll");
        let staged = stage(&plugins, &entry, &managed_dll()).unwrap();
        place(&plugins, &entry.module_id, &staged).unwrap();
        // The update can't be moved in, as it isn't there.
        let missing = registry_dir(&plugins).join(STAGING).join("missing");
        assert!(place(&plugins, &entry.module_id, &missing).is_err());
        let dir = module_dir(&plugins, &entry.module_id);
        assert!(dir.join("Link.dll").is_file());
        assert_eq!(plugin_loader::read_manifest(&dir).unwrap().version, "1.0.6");
        let _ = fs::remove_dir_all(&plugins);
    }

    #[test]
    fn unpacking_stops_at_the_limit() {
        let plugins = temp_dir("registry_limit");
        let bytes = zip_of(&[("a.dll", &[0u8; 600]), ("b.dll", &[0u8; 600])]);
        let error = unzip(&bytes, &plugins.join("small"), 1000).unwrap_err();
        assert!(error.to_string().contains("unpacks to more"), "{error}");
        unzip(&bytes, &plugins.join("big"), 1200).unwrap();
        let _ = fs::remove_dir_all(&plugins);
    }

    fn entry(file: &str) -> RegistryModule {
        RegistryModule {
            module_id: "2a8c8080-2a76-46af-bf76-1da7c0127ef8".into(),
            module_name: "Steam Link".into(),
            version: "1.0.6".into(),
            dll_file_name: file.into(),
            ..RegistryModule::default()
        }
    }

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        for (name, bytes) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    #[test]
    fn installs_a_zip_and_lists_only_its_module() {
        let plugins = temp_dir("registry_zip");
        let dll = managed_dll();
        let bytes = zip_of(&[
            ("net7.0/Link.dll", &dll),
            ("ModuleLibs/native.dll", &dll),
            ("__MACOSX/net7.0/._Link.dll", b"junk"),
        ]);
        let entry = entry("net7.0/Link.dll");
        let staged = stage(&plugins, &entry, &bytes).unwrap();
        assert_eq!(
            place(&plugins, &entry.module_id, &staged).unwrap(),
            Placed::Installed
        );

        let dir = module_dir(&plugins, &entry.module_id);
        assert!(dir.join("net7.0/Link.dll").is_file());
        assert!(!dir.join("__MACOSX").exists());
        let found = plugin_loader::discover_plugins(&plugins);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "Link.dll");
        assert_eq!(found[0].manifest.as_ref().unwrap().version, "1.0.6");

        uninstall(&plugins, &entry.module_id).unwrap();
        assert!(!dir.exists());
        assert!(plugin_loader::discover_plugins(&plugins).is_empty());
        let _ = fs::remove_dir_all(&plugins);
    }

    #[test]
    fn installs_a_bare_dll_and_updates_it() {
        let plugins = temp_dir("registry_dll");
        let entry = entry("Link.dll");
        let staged = stage(&plugins, &entry, &managed_dll()).unwrap();
        place(&plugins, &entry.module_id, &staged).unwrap();
        let newer = RegistryModule {
            version: "1.0.7".into(),
            ..entry.clone()
        };
        let staged = stage(&plugins, &newer, &managed_dll()).unwrap();
        assert_eq!(
            place(&plugins, &newer.module_id, &staged).unwrap(),
            Placed::Installed
        );
        let manifest = plugin_loader::read_manifest(&module_dir(&plugins, &entry.module_id));
        assert_eq!(manifest.unwrap().version, "1.0.7");
        assert!(!registry_dir(&plugins).join(TRASH).exists());
        let _ = fs::remove_dir_all(&plugins);
    }

    #[test]
    fn a_waiting_update_is_applied_at_startup() {
        let plugins = temp_dir("registry_pending");
        let entry = entry("Link.dll");
        let staged = stage(&plugins, &entry, &managed_dll()).unwrap();
        let waiting = registry_dir(&plugins).join(PENDING).join(&entry.module_id);
        fs::create_dir_all(waiting.parent().unwrap()).unwrap();
        fs::rename(&staged, &waiting).unwrap();
        assert!(pending(&plugins, &entry.module_id));

        apply_pending(&plugins);
        assert!(!pending(&plugins, &entry.module_id));
        assert!(module_dir(&plugins, &entry.module_id)
            .join("Link.dll")
            .is_file());
        let _ = fs::remove_dir_all(&plugins);
    }

    #[test]
    fn refuses_what_it_cannot_install_safely() {
        let plugins = temp_dir("registry_refuse");
        let dll = managed_dll();
        let missing = stage(
            &plugins,
            &entry("Other.dll"),
            &zip_of(&[("Link.dll", &dll)]),
        );
        assert!(missing.unwrap_err().to_string().contains("Other.dll"));
        assert!(stage(&plugins, &entry("../Link.dll"), &dll).is_err());
        assert!(stage(&plugins, &entry("Link.dll"), b"<html>").is_err());
        let bad_id = RegistryModule {
            module_id: "../x".into(),
            ..entry("Link.dll")
        };
        assert!(stage(&plugins, &bad_id, &dll).is_err());
        assert!(uninstall(&plugins, "..").is_err());
        let _ = fs::remove_dir_all(&plugins);
    }

    /// Installs every module in the live registry into a scratch folder and
    /// checks each is found as a managed module. Needs the network:
    /// `cargo test -p vrft-daemon --lib -- --ignored live_registry`.
    #[test]
    #[ignore]
    fn live_registry_modules_install() {
        let plugins = temp_dir("registry_live");
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(120)))
            .build()
            .into();
        let modules = fetch(&agent, DEFAULT_REGISTRY_URL).unwrap();
        assert!(!modules.is_empty());
        let mut failures = Vec::new();
        for entry in &modules {
            let result = download(&agent, &entry.download_url, |_| {})
                .and_then(|bytes| stage(&plugins, entry, &bytes))
                .and_then(|staged| place(&plugins, &entry.module_id, &staged));
            if let Err(error) = result {
                failures.push(format!("{}: {error:#}", entry.module_name));
            }
        }
        let found = plugin_loader::discover_plugins(&plugins);
        for plugin in &found {
            eprintln!(
                "{} ({:?}) at {}",
                plugin.name,
                plugin.kind,
                plugin.path.display()
            );
        }
        eprintln!("failed: {failures:#?}");
        assert_eq!(found.len() + failures.len(), modules.len());
        assert!(found.iter().all(|plugin| plugin.manifest.is_some()));
        let _ = fs::remove_dir_all(&plugins);
    }

    #[test]
    fn ids_are_folder_safe() {
        assert!(valid_id("9058ac35-c6be-4e8b-8e3b-a16b00aa5488"));
        assert!(!valid_id(""));
        assert!(!valid_id("a/b"));
        assert!(!valid_id(".."));
    }
}
