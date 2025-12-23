use anyhow::{Context, Result};
use libloading::{Library, Symbol};
use log::{debug, info, warn};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct OpenVRPaths {
    runtime: Vec<String>,
}

pub struct SteamVRManager {
    #[allow(dead_code)]
    library: Arc<Library>,
    shutdown_fn: Symbol<'static, unsafe extern "C" fn()>,
}

impl SteamVRManager {
    pub fn init() -> Result<Option<Self>> {
        let runtime_path = match get_openvr_runtime_path() {
            Ok(p) => p,
            Err(e) => {
                warn!("Could not find OpenVR runtime path: {}", e);
                return Ok(None);
            }
        };

        debug!("Found OpenVR Runtime: {:?}", runtime_path);

        let dll_path = runtime_path
            .join("bin")
            .join("win64")
            .join("openvr_api.dll");
        if !dll_path.exists() {
            warn!("openvr_api.dll not found at {:?}", dll_path);
            return Ok(None);
        }

        let lib = unsafe { Library::new(&dll_path) }.context("Failed to load openvr_api.dll")?;
        let lib = Arc::new(lib);

        let init_fn: Symbol<unsafe extern "C" fn(*mut i32, i32, *const i8) -> u32> =
            unsafe { lib.get(b"VR_InitInternal2") }.context("Failed to find VR_InitInternal2")?;

        let shutdown_fn: Symbol<unsafe extern "C" fn()> =
            unsafe { lib.get(b"VR_ShutdownInternal") }
                .context("Failed to find VR_ShutdownInternal")?;

        let mut error_code: i32 = 0;
        let _token = unsafe { init_fn(&mut error_code, 2, std::ptr::null()) };

        if error_code != 0 {
            warn!("VR_InitInternal2 failed with error code: {}", error_code);
            return Ok(None);
        }

        info!("SteamVR initialized successfully (Background Mode)");

        let shutdown_fn_static: Symbol<'static, unsafe extern "C" fn()> =
            unsafe { std::mem::transmute(shutdown_fn) };

        Ok(Some(Self {
            library: lib,
            shutdown_fn: shutdown_fn_static,
        }))
    }

    pub fn register_manifest(&self, manifest_path: &Path) -> Result<()> {
        let runtime_path = get_openvr_runtime_path()?;
        let vrpathreg = runtime_path.join("bin").join("win64").join("vrpathreg.exe");

        if !vrpathreg.exists() {
            warn!("vrpathreg.exe not found at {:?}", vrpathreg);
            return Ok(());
        }

        let status = std::process::Command::new(&vrpathreg)
            .arg("addmanifest")
            .arg(manifest_path)
            .status()
            .context("Failed to execute vrpathreg.exe")?;

        if status.success() {
            info!("Successfully registered manifest via vrpathreg");
        } else {
            warn!("vrpathreg returned non-zero exit code: {:?}", status.code());
        }

        Ok(())
    }
}

impl Drop for SteamVRManager {
    fn drop(&mut self) {
        unsafe {
            (self.shutdown_fn)();
        }
        info!("SteamVR Shutdown");
    }
}

fn get_openvr_runtime_path() -> Result<PathBuf> {
    let local_app_data = std::env::var("LOCALAPPDATA").context("LOCALAPPDATA not set")?;
    let openvr_paths_file = Path::new(&local_app_data)
        .join("openvr")
        .join("openvrpaths.vrpath");

    if !openvr_paths_file.exists() {
        return Err(anyhow::anyhow!("openvrpaths.vrpath not found"));
    }

    let content = std::fs::read_to_string(&openvr_paths_file)?;
    let paths: OpenVRPaths = serde_json::from_str(&content)?;

    if let Some(path) = paths.runtime.first() {
        Ok(PathBuf::from(path))
    } else {
        Err(anyhow::anyhow!(
            "No runtime path found in openvrpaths.vrpath"
        ))
    }
}
