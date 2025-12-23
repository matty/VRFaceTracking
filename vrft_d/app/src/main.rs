mod osc;
mod steamvr;

mod dispatcher;
mod parameter_solver;
mod shape_legacy;
mod strategies;

use anyhow::Result;
use api::{LogLevel, ModuleLogger, TrackingModule, UnifiedExpressions, UnifiedTrackingData};
use common::{CalibrationData, CalibrationState, MutationConfig, UnifiedTrackingMutator};
use libloading::{Library, Symbol};
use log::{debug, error, info, trace, warn};
use osc::query::host::{CalibrationStatus, OscQueryHost};
use parameter_solver::ParameterSolver;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;
use steamvr::SteamVRManager;

use dispatcher::Dispatcher;

fn load_config(path: &Path) -> Result<MutationConfig> {
    if path.exists() {
        info!("Loading config from {:?}", path);
        let file = fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let config = serde_json::from_reader(reader)?;
        Ok(config)
    } else {
        info!("Config not found. Creating default at {:?}", path);
        let config = MutationConfig {
            smoothness: 0.0,
            mutator_enabled: true,
            calibration_enabled: false,
            ..Default::default()
        };
        let file = fs::File::create(path)?;
        let writer = std::io::BufWriter::new(file);
        serde_json::to_writer_pretty(writer, &config)?;
        Ok(config)
    }
}

extern "C" fn module_log_callback(level: LogLevel, target: *const i8, message: *const i8) {
    unsafe {
        let target_str = std::ffi::CStr::from_ptr(target)
            .to_str()
            .unwrap_or("unknown");
        let message_str = std::ffi::CStr::from_ptr(message).to_str().unwrap_or("");

        match level {
            LogLevel::Error => error!(target: target_str, "{}", message_str),
            LogLevel::Warn => warn!(target: target_str, "{}", message_str),
            LogLevel::Info => info!(target: target_str, "{}", message_str),
            LogLevel::Debug => debug!(target: target_str, "{}", message_str),
            LogLevel::Trace => trace!(target: target_str, "{}", message_str),
        }
    }
}

fn main() -> Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
    }
    env_logger::init();

    info!("Starting...");
    debug!("Debug logging is active");
    trace!("Trace logging is active");

    let args: Vec<String> = std::env::args().collect();
    let enable_steamvr = args.iter().any(|arg| arg == "--enable-steamvr");

    let _steamvr_manager = if enable_steamvr {
        match SteamVRManager::init() {
            Ok(Some(manager)) => {
                let manifest_path = std::env::current_dir()?.join("vrft_d.vrmanifest");
                if manifest_path.exists() {
                    if let Err(e) = manager.register_manifest(&manifest_path) {
                        warn!("Failed to register SteamVR manifest: {}", e);
                    }
                } else {
                    warn!("SteamVR manifest not found at {:?}", manifest_path);
                }
                Some(manager)
            }
            Ok(None) => None,
            Err(e) => {
                warn!("Error initializing SteamVR: {}", e);
                None
            }
        }
    } else {
        info!("SteamVR integration disabled by default. Use --enable-steamvr to enable.");
        None
    };

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        info!("Received Ctrl-C, shutting down...");
        r.store(false, Ordering::SeqCst);
    })
        .expect("Error setting Ctrl-C handler");

    let mut data = UnifiedTrackingData::default();

    struct LoadedModule {
        name: String,
        module: Box<dyn TrackingModule>,
    }

    let mut modules: Vec<LoadedModule> = Vec::new();
    let modules_dir = Path::new("plugins");
    if modules_dir.exists() {
        for entry in fs::read_dir(modules_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path
                .extension()
                .map_or(false, |ext| ext == "dll" || ext == "so" || ext == "dylib")
            {
                let filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                info!("Loading module: {:?}", path);

                match (|| -> Result<Box<dyn TrackingModule>> {
                    unsafe {
                        let lib = Library::new(&path)?;
                        let func: Symbol<unsafe extern "C" fn() -> Box<dyn TrackingModule>> =
                            lib.get(b"create_module")?;
                        let module = func();
                        std::mem::forget(lib);
                        Ok(module)
                    }
                })() {
                    Ok(module) => {
                        info!("✓ Successfully loaded module: {}", filename);
                        modules.push(LoadedModule {
                            name: filename,
                            module,
                        });
                    }
                    Err(e) => {
                        error!("✗ Failed to load module {:?}: {}", path, e);
                    }
                }
            }
        }
    } else {
        warn!("'plugins' directory not found. Creating it.");
        fs::create_dir("plugins")?;
    }

    if modules.is_empty() {
        warn!("No modules loaded!");
    } else {
        info!("Loaded {} module(s) successfully", modules.len());
    }

    let shared_data = Arc::new(RwLock::new(UnifiedTrackingData::default()));
    let shared_data_for_host = shared_data.clone();
    let shared_data_for_consumer = shared_data.clone();

    let debug_state = Arc::new(RwLock::new(HashMap::<String, f32>::new()));
    let debug_state_for_host = debug_state.clone();
    let debug_state_for_consumer = debug_state.clone();

    let calibration_status = Arc::new(RwLock::new(CalibrationStatus::default()));
    let calibration_status_for_host = calibration_status.clone();
    let calibration_status_for_consumer = calibration_status.clone();

    let calibration_data_shared = Arc::new(RwLock::new(CalibrationData::default()));
    let calibration_data_for_host = calibration_data_shared.clone();
    let calibration_data_for_consumer = calibration_data_shared.clone();

    let calibration_request = Arc::new(RwLock::new(None::<f32>));
    let calibration_request_for_host = calibration_request.clone();
    let calibration_request_for_consumer = calibration_request.clone();

    let config_path = Path::new("config.json");
    let config = load_config(config_path).unwrap_or_else(|e| {
        error!("Failed to load config: {}. Using defaults.", e);
        MutationConfig::default()
    });
    info!("Loaded Config: {:?}", config);

    let osc_context = strategies::OscContext {
        tracking_data: shared_data_for_host.clone(),
    };
    let (strategy, strategy_router, avatar_change_rx) =
        strategies::create_strategy(&config, osc_context);
    let mut transport_manager = Dispatcher::new(strategy);

    if let Err(e) = transport_manager.initialize() {
        error!("Failed to initialize transport manager: {}", e);
        return Err(e);
    }
    info!(
        "Transport Manager initialized with {:?} Strategy.",
        config.output_mode
    );

    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
        rt.block_on(async {
            let extensions_router = osc::query::extensions::get_router(
                debug_state_for_host,
                calibration_status_for_host,
                calibration_data_for_host,
                calibration_request_for_host,
            );

            let app_router = if let Some(strategy_router) = strategy_router {
                extensions_router.merge(strategy_router)
            } else {
                extensions_router
            };

            if let Err(e) = OscQueryHost::start(0, app_router).await {
                error!("OSC Query Host failed: {}", e);
            }
        });
    });

    let mut mutator = UnifiedTrackingMutator::new(config.clone());

    let calibration_path = Path::new("calibration.json");
    if calibration_path.exists() {
        info!("Loading calibration from {:?}", calibration_path);
        if let Err(e) = mutator.load_calibration(calibration_path) {
            error!("Failed to load calibration: {}", e);
        }
    } else if mutator.config.calibration_enabled {
        info!("No calibration found; calibration is enabled and will learn continuously.");
    } else {
        info!(
            "No calibration file found and calibration is disabled in config. Skipping initial calibration."
        );
    }

    info!("Initializing Modules...");
    for module_wrapper in &mut modules {
        let logger_name = format!("vrft_d::plugins::{}", module_wrapper.name);
        let logger = ModuleLogger::new(module_log_callback, logger_name);

        match module_wrapper.module.initialize(logger) {
            Ok(_) => {
                info!("✓ Initialized module: {}", module_wrapper.name);
            }
            Err(e) => {
                error!(
                    "✗ Failed to initialize module {}: {}",
                    module_wrapper.name, e
                );
            }
        }
    }

    let (tx, rx) = sync_channel::<UnifiedTrackingData>(1);

    let running_consumer = running.clone();

    thread::spawn(move || {
        info!("Consumer Thread Started");

        let avatar_change_rx = avatar_change_rx;

        let transport_manager = transport_manager;
        let mut last_frame_time = std::time::Instant::now();
        let mut was_calibrating = false;

        while running_consumer.load(Ordering::SeqCst) {
            let mut received_data =
                rx.recv_timeout(Duration::from_millis(100))
                    .unwrap_or_else(|_| {
                        let mut d = UnifiedTrackingData::default();
                        d.eye.left.openness = 1.0;
                        d.eye.right.openness = 1.0;
                        d
                    });

            if let Ok(debug) = debug_state_for_consumer.read() {
                if !debug.is_empty() {
                    #[cfg(feature = "xtralog")]
                    {
                        static mut LAST_DEBUG_WARN: Option<std::time::Instant> = None;
                        let now = std::time::Instant::now();
                        let should_log = unsafe {
                            match LAST_DEBUG_WARN {
                                Some(last) if now.duration_since(last).as_secs() < 5 => false,
                                _ => {
                                    LAST_DEBUG_WARN = Some(now);
                                    true
                                }
                            }
                        };
                        if should_log {
                            warn!("Debug overrides are being applied to tracking data.");
                        }
                    }

                    for i in 0..UnifiedExpressions::Max as usize {
                        if let Some(name) = ParameterSolver::get_expression_name(i) {
                            if let Some(&val) = debug.get(name) {
                                received_data.shapes[i].weight = val;
                            }
                            else if let Some(short_name) = name.strip_prefix("v2/") {
                                if let Some(&val) = debug.get(short_name) {
                                    received_data.shapes[i].weight = val;
                                }
                            }
                        }
                    }

                    if let Some(&val) = debug.get("EyeLeftOpenness") {
                        received_data.eye.left.openness = val;
                    }
                    if let Some(&val) = debug.get("EyeRightOpenness") {
                        received_data.eye.right.openness = val;
                    }

                    if let Some(&val) = debug.get("EyeLeftPupil") {
                        received_data.eye.left.pupil_diameter_mm = val;
                    }
                    if let Some(&val) = debug.get("EyeRightPupil") {
                        received_data.eye.right.pupil_diameter_mm = val;
                    }

                    if let Some(&x) = debug.get("EyeLeftGazeX") {
                        received_data.eye.left.gaze.x = x;
                    }
                    if let Some(&y) = debug.get("EyeLeftGazeY") {
                        received_data.eye.left.gaze.y = y;
                    }
                    if let Some(&x) = debug.get("EyeRightGazeX") {
                        received_data.eye.right.gaze.x = x;
                    }
                    if let Some(&y) = debug.get("EyeRightGazeY") {
                        received_data.eye.right.gaze.y = y;
                    }

                    if let Some(&val) = debug.get("EyeCombinedOpenness") {
                        received_data.eye.left.openness = val;
                        received_data.eye.right.openness = val;
                    }
                    if let Some(&val) = debug.get("EyeCombinedPupil") {
                        received_data.eye.left.pupil_diameter_mm = val;
                        received_data.eye.right.pupil_diameter_mm = val;
                    }
                    if let Some(&x) = debug.get("EyeCombinedGazeX") {
                        received_data.eye.left.gaze.x = x;
                        received_data.eye.right.gaze.x = x;
                    }
                    if let Some(&y) = debug.get("EyeCombinedGazeY") {
                        received_data.eye.left.gaze.y = y;
                        received_data.eye.right.gaze.y = y;
                    }
                }
            }

            let now = std::time::Instant::now();
            let dt = now.duration_since(last_frame_time).as_secs_f32();
            last_frame_time = now;

            if let Ok(mut req) = calibration_request_for_consumer.write() {
                if let Some(duration) = *req {
                    if matches!(
                        mutator.calibration_state,
                        CalibrationState::Uncalibrated | CalibrationState::Calibrated
                    ) {
                        info!("Starting calibration from HTTP request: {}s", duration);
                        mutator.start_calibration(duration);
                    }
                    *req = None;
                }
            }

            mutator.mutate(&mut received_data, dt);

            let is_calibrating_now = matches!(
                mutator.calibration_state,
                CalibrationState::Collecting { .. }
            );
            if was_calibrating && !is_calibrating_now {
                info!("Calibration finished! Saving to calibration.json");
                if let Err(e) = mutator.save_calibration(Path::new("calibration.json")) {
                    error!("Failed to save calibration: {}", e);
                }
            }
            was_calibrating = is_calibrating_now;

            if let Ok(mut st) = calibration_status_for_consumer.write() {
                let (is_cal, elapsed, duration, progress) = mutator.calibration_status();
                st.is_calibrating = is_cal;
                st.elapsed = elapsed;
                st.duration = duration;
                st.progress = progress;
            }
            if let Ok(mut cd) = calibration_data_for_consumer.write() {
                *cd = mutator.get_calibration_data();
            }

            if let Ok(mut write_guard) = shared_data_for_consumer.write() {
                *write_guard = received_data.clone();
            }

            if let Err(e) = transport_manager.send(&received_data) {
                error!("Failed to send OSC data: {}", e);
            }

            if let Some(rx) = &avatar_change_rx {
                while let Ok(avatar_id) = rx.try_recv() {
                    info!("Switching calibration profile to avatar: {}", avatar_id);
                    if let Err(e) = mutator.calibration_manager.switch_profile(&avatar_id) {
                        error!("Failed to switch calibration profile: {}", e);
                    }
                }
            }

            static mut LAST_SAVE: Option<std::time::Instant> = None;
            let now = std::time::Instant::now();
            let should_save = unsafe {
                match LAST_SAVE {
                    Some(last) if now.duration_since(last).as_secs() < 30 => false,
                    _ => {
                        LAST_SAVE = Some(now);
                        true
                    }
                }
            };

            if should_save && mutator.config.calibration_enabled && mutator.has_calibration_data() {
                if let Err(e) = mutator.save_calibration(Path::new("calibration.json")) {
                    error!("Failed to auto-save calibration: {}", e);
                } else {
                    #[cfg(feature = "xtralog")]
                    info!("Auto-saved calibration.");
                }
            }
        }
    });

    info!("Entering Main Loop (Producer)...");

    let mut frame_count: u64 = 0;
    let mut log_interval: u64 = 1000;
    let mut last_log = std::time::Instant::now();
    let mut last_frame_time = std::time::Instant::now();
    let target_frame_duration = config.max_fps.map(|fps| Duration::from_secs_f32(1.0 / fps));

    while running.load(Ordering::SeqCst) {
        let mut any_updated = false;

        let active_plugin = &config.active_plugin;
        let mut active_module_found = false;

        for module_wrapper in &mut modules {
            if module_wrapper.name == *active_plugin {
                active_module_found = true;
                if module_wrapper.module.update(&mut data).is_ok() {
                    any_updated = true;
                }
            }
        }

        if !active_module_found && !modules.is_empty() {
            static mut LAST_PLUGIN_WARN: Option<std::time::Instant> = None;
            let now = std::time::Instant::now();
            let should_log = unsafe {
                match LAST_PLUGIN_WARN {
                    Some(last) if now.duration_since(last).as_secs() < 5 => false,
                    _ => {
                        LAST_PLUGIN_WARN = Some(now);
                        true
                    }
                }
            };
            if should_log {
                warn!(
                    "Active plugin '{}' not found among loaded modules!",
                    active_plugin
                );
            }
        }

        if any_updated {
            let _ = tx.try_send(data.clone());

            frame_count += 1;
            if frame_count % log_interval == 0 {
                let elapsed = last_log.elapsed().as_secs_f32();
                let fps = log_interval as f32 / elapsed;
                info!(
                    "Tracking Active: Processed {} frames (approx {:.1} FPS)",
                    frame_count, fps
                );
                last_log = std::time::Instant::now();

                if frame_count >= 1_000_000 {
                    log_interval = 1_000_000;
                } else if frame_count >= 100_000 {
                    log_interval = 100_000;
                } else if frame_count >= 10_000 {
                    log_interval = 10_000;
                }
            }

            if let Some(target_duration) = target_frame_duration {
                let elapsed = last_frame_time.elapsed();
                if elapsed < target_duration {
                    thread::sleep(target_duration - elapsed);
                }
            }
            last_frame_time = std::time::Instant::now();
        } else {
            thread::sleep(Duration::from_millis(5));
        }
    }

    info!("Shutting down...");
    for module_wrapper in &mut modules {
        module_wrapper.module.unload();
    }
    Ok(())
}
