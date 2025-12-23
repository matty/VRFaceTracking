// Thanks to VRCFaceTracking "SRanipalTrackingModule" for the initial implementation
// https://github.com/VRCFaceTracking/SRanipalTrackingModule/tree/master/SRanipalExtTrackingModule

use crate::ffi::{AnipalType, Error, EyeData_v2, LipData_v2};
use crate::mapping;
use anyhow::{anyhow, Result};
use api::{ModuleLogger, TrackingModule, UnifiedTrackingData};
use libloading::{Library, Symbol};
use std::path::PathBuf;
use std::ptr;

// Function signatures
type InitialFn = unsafe extern "C" fn(anipal_type: i32, config: *mut std::ffi::c_void) -> i32;
type ReleaseFn = unsafe extern "C" fn(anipal_type: i32) -> i32;
type GetEyeDataV2Fn = unsafe extern "C" fn(data: *mut EyeData_v2) -> i32;
type GetLipDataV2Fn = unsafe extern "C" fn(data: *mut LipData_v2) -> i32;

struct SRanipalContext {
    _lib: Library,
    initial: InitialFn,
    release: ReleaseFn,
    get_eye_data_v2: GetEyeDataV2Fn,
    get_lip_data_v2: GetLipDataV2Fn,
}

impl SRanipalContext {
    fn new(path: PathBuf) -> Result<Self> {
        unsafe {
            let lib = Library::new(path)?;

            let initial: Symbol<InitialFn> = lib.get(b"SRanipal_Initial")?;
            let release: Symbol<ReleaseFn> = lib.get(b"SRanipal_Release")?;
            let get_eye_data_v2: Symbol<GetEyeDataV2Fn> = lib.get(b"SRanipal_GetEyeData_v2")?;
            let get_lip_data_v2: Symbol<GetLipDataV2Fn> = lib.get(b"SRanipal_GetLipData_v2")?;

            Ok(Self {
                initial: *initial,
                release: *release,
                get_eye_data_v2: *get_eye_data_v2,
                get_lip_data_v2: *get_lip_data_v2,
                _lib: lib,
            })
        }
    }
}

pub struct SRanipalModule {
    context: Option<SRanipalContext>,
    logger: Option<ModuleLogger>,
    eye_enabled: bool,
    lip_enabled: bool,
}

impl SRanipalModule {
    pub fn new() -> Self {
        Self {
            context: None,
            logger: None,
            eye_enabled: false,
            lip_enabled: false,
        }
    }

    #[allow(dead_code)]
    fn find_sranipal_path(&self) -> Option<PathBuf> {
        let default_path = PathBuf::from("C:\\Program Files\\VIVE\\SRanipal\\sr_runtime.exe");
        if default_path.exists() {
            if let Ok(cwd) = std::env::current_dir() {
                let local_dll = cwd.join("SRanipal.dll");
                if local_dll.exists() {
                    return Some(local_dll);
                }
            }
        }
        None
    }
}

impl Default for SRanipalModule {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackingModule for SRanipalModule {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing SRanipal Module");

        // TODO: Robust path finding. For now, assume it's in the working directory.
        let dll_path = PathBuf::from("SRanipal.dll");

        match SRanipalContext::new(dll_path) {
            Ok(ctx) => {
                logger.info("Loaded SRanipal.dll");

                unsafe {
                    let eye_err = (ctx.initial)(AnipalType::EyeV2 as i32, ptr::null_mut());
                    if eye_err == Error::Work as i32 {
                        self.eye_enabled = true;
                        logger.info("Initialized Eye Tracking");
                    } else {
                        logger.warn(&format!("Failed to initialize Eye Tracking: {}", eye_err));
                    }

                    let lip_err = (ctx.initial)(AnipalType::LipV2 as i32, ptr::null_mut());
                    if lip_err == Error::Work as i32 {
                        self.lip_enabled = true;
                        logger.info("Initialized Lip Tracking");
                    } else {
                        logger.warn(&format!("Failed to initialize Lip Tracking: {}", lip_err));
                    }
                }
                self.context = Some(ctx);
            }
            Err(e) => {
                logger.error(&format!("Failed to load SRanipal.dll: {}", e));
                self.logger = Some(logger);
                return Err(anyhow!("Failed to load SRanipal.dll"));
            }
        }

        self.logger = Some(logger);
        Ok(())
    }

    fn update(&mut self, data: &mut UnifiedTrackingData) -> Result<()> {
        if let Some(ctx) = &self.context {
            unsafe {
                if self.eye_enabled {
                    let mut eye_data = EyeData_v2::default();
                    let err = (ctx.get_eye_data_v2)(&mut eye_data);
                    if err == Error::Work as i32 {
                        mapping::update_eye(data, &eye_data);
                    }
                }

                if self.lip_enabled {
                    let mut lip_data = LipData_v2::default();
                    let err = (ctx.get_lip_data_v2)(&mut lip_data);
                    if err == Error::Work as i32 {
                        mapping::update_lip(data, &lip_data);
                    }
                }
            }
        }
        Ok(())
    }

    fn unload(&mut self) {
        if let Some(ctx) = &self.context {
            unsafe {
                if self.eye_enabled {
                    (ctx.release)(AnipalType::EyeV2 as i32);
                }
                if self.lip_enabled {
                    (ctx.release)(AnipalType::LipV2 as i32);
                }
            }
        }
        if let Some(logger) = &self.logger {
            logger.info("SRanipal Module unloaded");
        }
    }
}
