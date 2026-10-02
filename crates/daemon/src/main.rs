use vrft_daemon::osc;
mod api;
mod config_file;
mod daemon_status;
mod dev_build;
mod extensions;
mod installed;
mod lifetime;
mod modules;
#[cfg(test)]
mod test_support;

use vrft_daemon::plugin_loader::{self, PluginKind};
use vrft_daemon::strategies;

use anyhow::{Context as _, Result};
use libloading::{Library, Symbol};
use log::{debug, error, info, trace, warn};
use osc::query::host::OscQueryHost;
use osc::query::target::VrchatTarget;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use vrft_api::{
    LogLevel, ModuleLogger, ProxyModule, TrackingModule, UnifiedExpressions, UnifiedSingleEyeData,
    UnifiedTrackingData,
};
use vrft_common::{MutationConfig, UnifiedTrackingMutator};
use vrft_extension::{ExtensionConfig, ExtensionReport, FrameHook, HostContext};

use daemon_status::{DaemonStatus, ModuleStatus, OutputTarget, RunMode};

/// How often a send error that keeps happening is logged again.
const SEND_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(10);

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
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    // Extension subcommands, such as Quest Pro's `train-tongue`, which the
    // daemon starts as child processes of itself.
    if let Some(name) = arguments.first() {
        for extension in extensions::built_in() {
            if let Some(result) = extension.run_subcommand(name, &arguments[1..]) {
                return result;
            }
        }
    }
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
    }
    env_logger::init();

    info!("Starting vrft_d {}...", env!("VRFT_VERSION"));
    // First, before this touches the config or plugins another daemon uses.
    let _instance = match lifetime::claim_instance() {
        Ok(instance) => instance,
        Err(error) => {
            error!("{error:#}");
            return Err(error);
        }
    };
    lifetime::end_children_with_daemon();
    let dev_build = dev_build::use_repository_root();
    if dev_build.is_none() {
        installed::use_data_dir();
    }
    debug!("Debug logging is active");
    trace!("Trace logging is active");

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        info!("Received Ctrl-C, shutting down...");
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");
    lifetime::end_if_stopping_stalls(running.clone());
    if let Some(owner) = lifetime::owner_pid(&arguments) {
        lifetime::stop_with_owner(owner, running.clone());
    }

    // `--camera-preview-only` is the name from before it applied to every
    // extension.
    let extensions_only = std::env::args()
        .any(|argument| argument == "--extensions-only" || argument == "--camera-preview-only");
    let mode = if extensions_only {
        RunMode::ExtensionsOnly
    } else {
        RunMode::Normal
    };
    let daemon_status = DaemonStatus::new(mode);

    let built_in = extensions::built_in();
    let extension_ids: Vec<&str> = built_in.iter().map(|extension| extension.id()).collect();
    let config_path = Path::new("config.json");
    let config = config_file::read_or_create(config_path, &extension_ids).unwrap_or_else(|e| {
        error!("Failed to load config: {e}. Using defaults.");
        daemon_status.set_config_error(Some(e));
        MutationConfig::default()
    });
    info!("Loaded Config: {:?}", config);

    // Resolve the single plugins directory (with dev-run parent fallback).
    let mut plugins_dir = Path::new("plugins").to_path_buf();
    if !plugins_dir.exists() {
        let parent = Path::new("../plugins");
        if parent.exists() {
            plugins_dir = parent.to_path_buf();
        }
    }

    if !plugins_dir.exists() {
        warn!("'plugins' directory not found. Creating it.");
        fs::create_dir_all(&plugins_dir)?;
    }
    if let Some(build) = &dev_build {
        dev_build::install_modules(build, Path::new("modules"), &plugins_dir);
    }

    // Updates to modules that were in use last time replace them now,
    // before any module loads.
    vrft_daemon::module_registry::apply_pending(&plugins_dir);

    let root = std::env::current_dir()?;
    let (mut hooks, api_extensions) =
        start_extensions(built_in, &config, &root, &running, mode, &daemon_status);
    // The host for managed (.NET / VRCFT) modules, outside the scanned
    // plugins tree so it is never mistaken for a plugin.
    let dotnet_host = plugin_loader::find_dotnet_host(&root).or_else(|| {
        dev_build
            .as_ref()
            .and_then(|_| dev_build::dotnet_host(&root))
    });
    // How the local API asks the tracking loop to load another module.
    let switch = modules::ModuleSwitch::default();
    let module_manager = modules::ModuleManager::new(
        root.join(&plugins_dir),
        root.join(config_path),
        config
            .module
            .registry_url
            .clone()
            .unwrap_or_else(|| vrft_daemon::module_registry::DEFAULT_REGISTRY_URL.into()),
        dotnet_host.clone(),
        (!extensions_only).then(|| switch.clone()),
    );
    api::start(
        daemon_status.clone(),
        running.clone(),
        root.join(config_path),
        root.join(&plugins_dir),
        module_manager,
        api_extensions,
    );
    if extensions_only {
        info!("Running extensions only, without tracking modules or OSC output");
        while running.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(200));
        }
        return Ok(());
    }

    let shared_data = Arc::new(RwLock::new(UnifiedTrackingData::default()));
    let shared_data_for_host = shared_data.clone();
    let shared_data_for_consumer = shared_data.clone();

    let debug_state = Arc::new(RwLock::new(HashMap::<String, f32>::new()));
    let debug_state_for_host = debug_state.clone();
    let debug_state_for_consumer = debug_state.clone();

    let vrchat_target = VrchatTarget::new(&config.osc.send_address, config.osc.send_port);
    if config.osc.output_mode == vrft_common::OutputMode::VRChat {
        daemon_status.set_vrchat(vrchat_target.clone());
    }
    let osc_context = strategies::OscContext {
        tracking_data: shared_data_for_host.clone(),
        vrchat: vrchat_target,
    };
    let (mut output, strategy_router) = strategies::create_strategy(&config, osc_context);
    // Set up before any module loads, which can take a while. Without it,
    // sends fail, and say why, until the setting is fixed.
    match output.initialize() {
        Ok(()) => info!("{:?} output initialized.", config.osc.output_mode),
        Err(e) => {
            error!(
                "Failed to set up {:?} output: {e:#}",
                config.osc.output_mode
            );
            daemon_status.set_config_error(Some(format!("Output: {e:#}")));
        }
    }

    // Native libraries of modules switched away from. They stay loaded, as a
    // module may leave threads or callbacks behind that would crash the
    // daemon if its code were unmapped.
    let mut retired: Vec<Library> = Vec::new();
    let mut current = load_module(
        &config.module.active,
        &plugins_dir,
        dotnet_host.as_deref(),
        &daemon_status,
        &mut retired,
    );
    daemon_status.set_output(OutputTarget {
        mode: format!("{:?}", config.osc.output_mode),
        address: config.osc.send_address.clone(),
        port: config.osc.send_port,
        max_fps: config.max_fps,
        smoothing: config.mutator.enabled.then_some(config.mutator.smoothness),
        vrchat: None,
    });
    // Tells the consumer thread's hooks whether a module loaded, at first
    // and as switches load, or fail to load, another.
    let (module_loaded_tx, module_loaded_rx) = std::sync::mpsc::channel::<bool>();
    let _ = module_loaded_tx.send(current.is_some());
    let mut data = UnifiedTrackingData::default();

    let osc_query = move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("osc-query")
            .build()
            .expect("Failed to create Tokio runtime");
        rt.block_on(async {
            let extensions_router = osc::query::extensions::get_router(debug_state_for_host);

            let app_router = if let Some(strategy_router) = strategy_router {
                extensions_router.merge(strategy_router)
            } else {
                extensions_router
            };

            if let Err(e) = OscQueryHost::start(0, app_router).await {
                error!("OSC Query Host failed: {}", e);
            }
        });
    };
    thread::Builder::new()
        .name("osc-query-host".into())
        .spawn(osc_query)
        .expect("couldn't start the OSC Query thread");

    let mut mutator = UnifiedTrackingMutator::new(config.clone());

    let (tx, rx) = sync_channel::<UnifiedTrackingData>(1);

    let running_consumer = running.clone();
    let target_frame_duration = config.frame_interval();
    if let (None, Some(fps)) = (target_frame_duration, config.max_fps) {
        warn!("Ignoring invalid max_fps value {fps}; running uncapped");
    }
    // How often to send frames the tracking module didn't produce, while an
    // extension has live data: max_fps, or 60 when it is unset or invalid,
    // and at least 10.
    let live_frame_interval = target_frame_duration
        .unwrap_or(Duration::from_secs(1) / 60)
        .min(Duration::from_millis(100));

    let consumer = move || {
        info!("Consumer Thread Started");

        let output = output;
        let mut last_frame_time = Instant::now();
        // Failed sends since the last that worked, and when one was last
        // logged: an unreachable target would otherwise log every frame.
        let mut send_errors = 0u64;
        let mut send_error_logged = Instant::now();

        // Hold last received data to prevent glitches on tracking loss
        let mut last_received_data: Option<UnifiedTrackingData> = None;

        while running_consumer.load(Ordering::SeqCst) {
            while let Ok(loaded) = module_loaded_rx.try_recv() {
                // Don't keep repeating the previous module's last frame.
                last_received_data = None;
                for hook in &mut hooks {
                    hook.module_loaded(loaded);
                }
            }
            // An extension with its own live data (such as headset cameras)
            // keeps frames flowing at full rate without a tracking module.
            let wait = if hooks.iter().any(|hook| hook.has_live_data()) {
                live_frame_interval
            } else {
                Duration::from_millis(100)
            };
            let mut from_module = false;
            let mut received_data = match rx.recv_timeout(wait) {
                Ok(data) => {
                    from_module = true;
                    last_received_data = Some(data.clone());
                    data
                }
                Err(_) => {
                    // On timeout, use last known data to prevent glitches
                    // Only fall back to default if we've never received any data
                    last_received_data.clone().unwrap_or_else(|| {
                        let mut d = UnifiedTrackingData::default();
                        d.eye.left.openness = 1.0;
                        d.eye.right.openness = 1.0;
                        d
                    })
                }
            };

            if let Ok(debug) = debug_state_for_consumer.read() {
                if !debug.is_empty() {
                    #[cfg(feature = "xtralog")]
                    {
                        use std::cell::Cell;
                        thread_local! {
                            static LAST_DEBUG_WARN: Cell<Option<Instant>> = const { Cell::new(None) };
                        }
                        let now = Instant::now();
                        let should_log = LAST_DEBUG_WARN.with(|cell| match cell.get() {
                            Some(last) if now.duration_since(last).as_secs() < 5 => false,
                            _ => {
                                cell.set(Some(now));
                                true
                            }
                        });
                        if should_log {
                            warn!("Debug overrides are being applied to tracking data.");
                        }
                    }

                    apply_debug_overrides(&debug, &mut received_data);
                }
            }

            let now = Instant::now();
            let dt = now.duration_since(last_frame_time).as_secs_f32();
            last_frame_time = now;

            if from_module {
                for hook in &mut hooks {
                    hook.before_mutation(&received_data);
                }
            }
            mutator.mutate(&mut received_data, dt);
            for hook in &mut hooks {
                hook.after_mutation(&mut received_data);
            }

            // Update shared data for OSC Query (non-blocking; host doesn't need every frame)
            if let Ok(mut write_guard) = shared_data_for_consumer.try_write() {
                *write_guard = received_data.clone();
            }

            match output.send(&received_data) {
                Ok(()) if send_errors > 0 => {
                    info!("Sending again after {send_errors} failed frame(s)");
                    send_errors = 0;
                }
                Ok(()) => {}
                Err(e) => {
                    send_errors += 1;
                    if send_errors == 1 || send_error_logged.elapsed() >= SEND_ERROR_LOG_INTERVAL {
                        error!("Failed to send OSC data ({send_errors} frame(s) so far): {e:#}");
                        send_error_logged = Instant::now();
                    }
                }
            }
        }
    };
    thread::Builder::new()
        .name("output".into())
        .spawn(consumer)
        .expect("couldn't start the output thread");

    info!("Entering Main Loop (Producer)...");

    let mut frame_count: u64 = 0;
    let mut log_interval: u64 = 1000;
    let mut last_log = Instant::now();
    let mut last_frame_time = Instant::now();

    while running.load(Ordering::SeqCst) {
        if let Some(key) = switch.take() {
            if let Some(old) = current.take() {
                info!("Unloading {} to switch modules", old.name);
                retire_module(old, &mut retired);
            }
            data = UnifiedTrackingData::default();
            current = load_module(
                &key,
                &plugins_dir,
                dotnet_host.as_deref(),
                &daemon_status,
                &mut retired,
            );
            let _ = module_loaded_tx.send(current.is_some());
        }

        let any_updated = current
            .as_mut()
            .is_some_and(|loaded| loaded.module.update(&mut data).is_ok());

        if any_updated {
            daemon_status.count_tracking_frame();
            let _ = tx.try_send(data.clone());

            frame_count += 1;
            if frame_count.is_multiple_of(log_interval) {
                let elapsed = last_log.elapsed().as_secs_f32();
                let fps = log_interval as f32 / elapsed;
                info!(
                    "Tracking Active: Processed {} frames (approx {:.1} FPS)",
                    frame_count, fps
                );
                last_log = Instant::now();

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
            last_frame_time = Instant::now();
        } else {
            thread::sleep(Duration::from_millis(5));
        }
    }

    info!("Shutting down...");

    if let Some(loaded) = current.take() {
        retire_module(loaded, &mut retired);
    }
    // Unloading them now could crash the exit, for the same reason they
    // were kept: a module's leftover threads may still be running its code.
    std::mem::forget(retired);
    Ok(())
}

/// One value of an eye.
type EyeValue = fn(&mut UnifiedSingleEyeData) -> &mut f32;

/// Each eye value the debug API can set, as `Eye{Left,Right,Combined}<name>`
/// names it.
const DEBUG_EYE_VALUES: [(&str, EyeValue); 4] = [
    ("Openness", |eye| &mut eye.openness),
    ("Pupil", |eye| &mut eye.pupil_diameter_mm),
    ("GazeX", |eye| &mut eye.gaze.x),
    ("GazeY", |eye| &mut eye.gaze.y),
];

/// Sets the values the debug API was given: expressions by name, with or
/// without `v2/`, and eye values for one eye or, overriding that, both.
fn apply_debug_overrides(debug: &HashMap<String, f32>, data: &mut UnifiedTrackingData) {
    for (index, shape) in data.shapes.iter_mut().enumerate() {
        let Ok(expression) = UnifiedExpressions::try_from(index) else {
            break;
        };
        let name = format!("{expression:?}");
        if let Some(&value) = debug
            .get(&format!("v2/{name}"))
            .or_else(|| debug.get(&name))
        {
            shape.weight = value;
        }
    }
    for (name, value_of) in DEBUG_EYE_VALUES {
        let both = debug.get(&format!("EyeCombined{name}"));
        for (side, eye) in [("Left", &mut data.eye.left), ("Right", &mut data.eye.right)] {
            if let Some(&value) = both.or_else(|| debug.get(&format!("Eye{side}{name}"))) {
                *value_of(eye) = value;
            }
        }
    }
}

/// A tracking module the daemon is running.
struct LoadedModule {
    name: String,
    module: Box<dyn TrackingModule>,
    /// A native module's library, which must outlive `module`.
    lib: Option<Library>,
}

/// Opens `plugin`: loads a native library, or starts the .NET host on a
/// managed one.
fn open_module(
    plugin: &plugin_loader::DiscoveredPlugin,
    dotnet_host: Option<&Path>,
) -> Result<LoadedModule> {
    match plugin.kind {
        PluginKind::Native => unsafe {
            let lib = Library::new(&plugin.path).context("Failed to load")?;
            let create: Symbol<unsafe extern "C" fn() -> Box<dyn TrackingModule>> = lib
                .get(b"create_module")
                .context("Failed to load: it has no create_module")?;
            let module = create();
            Ok(LoadedModule {
                name: plugin.name.clone(),
                module,
                lib: Some(lib),
            })
        },
        PluginKind::Managed => {
            let host = dotnet_host.with_context(|| {
                format!(
                    "It's a VRCFT module, and {} isn't installed to run it",
                    plugin_loader::DOTNET_HOST
                )
            })?;
            info!("Starting VrcftRuntime for module: {:?}", plugin.path);
            let mut proxy = ProxyModule::new();
            proxy
                .start(host, &plugin.path)
                .context("Failed to start VrcftRuntime")?;
            Ok(LoadedModule {
                name: plugin.name.clone(),
                module: Box::new(proxy),
                lib: None,
            })
        }
    }
}

/// Finds the module `wanted` names in `plugins`, loads and initializes it,
/// and reports how that went in `/status`.
fn load_module(
    wanted: &str,
    plugins: &Path,
    dotnet_host: Option<&Path>,
    daemon_status: &DaemonStatus,
    retired: &mut Vec<Library>,
) -> Option<LoadedModule> {
    let discovered = plugin_loader::discover_plugins(plugins);
    info!(
        "Discovered {} plugin(s) under {:?}",
        discovered.len(),
        plugins
    );
    let mut status = ModuleStatus {
        name: wanted.to_string(),
        loading: true,
        ..ModuleStatus::default()
    };
    let Some(plugin) = plugin_loader::find_plugin(&discovered, wanted) else {
        error!(
            "Active plugin '{}' not found among {} discovered plugin(s) in {:?}",
            wanted,
            discovered.len(),
            plugins
        );
        status.loading = false;
        status.error = Some(format!("Not found in {}", plugins.display()));
        daemon_status.set_module(status);
        return None;
    };
    status.label = Some(config_file::plugin_name(plugin));
    status.runtime = Some(config_file::runtime_name(plugin.kind).into());
    daemon_status.set_module(status.clone());
    info!(
        "Loading active plugin: {:?} ({:?})",
        plugin.path, plugin.kind
    );

    let result = open_module(plugin, dotnet_host).and_then(|mut loaded| {
        let logger = ModuleLogger::new(
            module_log_callback,
            format!("vrft_d::plugins::{}", plugin.name),
        );
        match loaded.module.initialize(logger) {
            Ok(()) => Ok(loaded),
            Err(error) => {
                retire_module(loaded, retired);
                Err(error.context("Failed to initialize"))
            }
        }
    });
    status.loading = false;
    let loaded = match result {
        Ok(loaded) => {
            info!("✓ Loaded and initialized module: {}", plugin.name);
            status.loaded = true;
            Some(loaded)
        }
        Err(error) => {
            error!("✗ Module {:?} didn't load: {error:#}", plugin.path);
            status.error = Some(format!("{error:#}"));
            None
        }
    };
    daemon_status.set_module(status);
    loaded
}

/// Unloads `loaded`, keeping a native module's library loaded in `retired`.
fn retire_module(loaded: LoadedModule, retired: &mut Vec<Library>) {
    let LoadedModule {
        name,
        mut module,
        lib,
    } = loaded;
    module.unload();
    // The module's code lives in the library, so it goes first.
    drop(module);
    retired.extend(lib);
    info!("Unloaded module: {name}");
}

/// Starts each enabled extension, returning their frame hooks and what the
/// local API serves for them.
fn start_extensions(
    built_in: Vec<Box<dyn vrft_extension::DaemonExtension>>,
    config: &MutationConfig,
    root: &Path,
    running: &Arc<AtomicBool>,
    mode: RunMode,
    daemon_status: &DaemonStatus,
) -> (Vec<Box<dyn FrameHook>>, Vec<api::ApiExtension>) {
    let mut hooks = Vec::new();
    let mut served = Vec::new();
    let mut reports = Vec::new();
    for extension in built_in {
        let (id, name) = (extension.id(), extension.name());
        let settings = match config.extensions.get(id) {
            None => ExtensionConfig::default(),
            Some(value) => serde_json::from_value(value.clone()).unwrap_or_else(|error| {
                warn!("Ignoring extensions.{id} in config.json ({error}); using its defaults");
                ExtensionConfig::default()
            }),
        };
        let mut report = ExtensionReport {
            id: id.into(),
            name: name.into(),
            enabled: settings.enabled,
            error: None,
        };
        if settings.enabled {
            let host = HostContext {
                root: root.to_path_buf(),
                running: running.clone(),
                mode,
            };
            match extension.start(host) {
                Ok(started) => {
                    info!("✓ Started extension: {name}");
                    hooks.extend(started.frame_hook);
                    served.push(api::ApiExtension {
                        id,
                        routes: started.routes,
                        page: started.page,
                        status: started.status,
                    });
                }
                Err(error) => {
                    error!("✗ Failed to start extension {name}: {error:#}");
                    report.error = Some(format!("{error:#}"));
                }
            }
        } else {
            info!("Extension {name} is turned off in config.json");
        }
        reports.push(report);
    }
    daemon_status.set_extensions(reports);
    (hooks, served)
}
