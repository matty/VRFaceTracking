//! What the daemon is running: its mode, extensions, tracking module and OSC
//! target. The API server reports it in `/status` for the desktop app.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use vrft_daemon::osc::query::target::VrchatTarget;
pub use vrft_protocol::{DaemonReport, ExtensionReport, ModuleStatus, OutputTarget, RunMode};

#[derive(Debug)]
struct Setup {
    mode: RunMode,
    module: Option<ModuleStatus>,
    output: Option<OutputTarget>,
    /// VRChat as OSCQuery finds it, for the VRChat output.
    vrchat: Option<VrchatTarget>,
    extensions: Vec<ExtensionReport>,
    config_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DaemonStatus {
    setup: Arc<RwLock<Setup>>,
    tracking_frames: Arc<AtomicU64>,
}

impl DaemonStatus {
    pub fn new(mode: RunMode) -> Self {
        Self {
            setup: Arc::new(RwLock::new(Setup {
                mode,
                module: None,
                output: None,
                vrchat: None,
                extensions: Vec::new(),
                config_error: None,
            })),
            tracking_frames: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn set_module(&self, module: ModuleStatus) {
        self.setup.write().unwrap().module = Some(module);
    }

    pub fn set_output(&self, output: OutputTarget) {
        self.setup.write().unwrap().output = Some(output);
    }

    pub fn set_vrchat(&self, target: VrchatTarget) {
        self.setup.write().unwrap().vrchat = Some(target);
    }

    pub fn set_extensions(&self, extensions: Vec<ExtensionReport>) {
        self.setup.write().unwrap().extensions = extensions;
    }

    /// Why `config.json` couldn't be read, when the daemon fell back to its
    /// defaults.
    pub fn set_config_error(&self, error: Option<String>) {
        self.setup.write().unwrap().config_error = error;
    }

    pub fn count_tracking_frame(&self) {
        self.tracking_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn report(&self) -> DaemonReport {
        let setup = self.setup.read().unwrap();
        DaemonReport {
            version: env!("VRFT_VERSION").into(),
            mode: setup.mode,
            module: setup.module.clone(),
            output: setup.output.clone().map(|mut output| {
                output.vrchat = setup.vrchat.as_ref().map(VrchatTarget::link);
                output
            }),
            extensions: setup.extensions.clone(),
            config_error: setup.config_error.clone(),
            tracking_frames: self.tracking_frames.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_reflects_setup_and_counts_frames() {
        let status = DaemonStatus::new(RunMode::Normal);
        let empty = status.report();
        assert_eq!(empty.module, None);
        assert_eq!(empty.tracking_frames, 0);

        status.set_module(ModuleStatus {
            name: "vd_module.dll".into(),
            runtime: Some("native".into()),
            loaded: true,
            ..ModuleStatus::default()
        });
        status.set_output(OutputTarget {
            mode: "VRChat".into(),
            address: "127.0.0.1".into(),
            port: 9000,
            max_fps: Some(60.0),
            smoothing: Some(0.4),
            vrchat: None,
        });
        let clone = status.clone();
        clone.count_tracking_frame();
        clone.count_tracking_frame();

        let report = status.report();
        assert_eq!(report.mode, RunMode::Normal);
        assert_eq!(report.module.unwrap().name, "vd_module.dll");
        assert_eq!(report.output.unwrap().port, 9000);
        assert_eq!(report.tracking_frames, 2);
    }

    #[test]
    fn report_serializes_mode_in_snake_case() {
        let report = DaemonStatus::new(RunMode::ExtensionsOnly).report();
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["mode"], "extensions_only");
        assert!(json["module"].is_null());
    }
}
