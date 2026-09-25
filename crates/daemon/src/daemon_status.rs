//! What the daemon is running: its mode, tracking module and OSC target.
//! The preview server reports it in `/status` for the desktop app.
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Normal,
    /// `--camera-preview-only`: no tracking module and no OSC output.
    CameraPreviewOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModuleStatus {
    /// The `module.active` file name from `config.json`.
    pub name: String,
    /// `native` or `dotnet`, once the plugin has been found.
    pub runtime: Option<&'static str>,
    pub loaded: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutputTarget {
    pub mode: String,
    pub address: String,
    pub port: u16,
    pub max_fps: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DaemonReport {
    pub version: &'static str,
    pub mode: RunMode,
    pub module: Option<ModuleStatus>,
    pub output: Option<OutputTarget>,
    /// Frames the tracking module has delivered since startup. A client
    /// derives the tracking rate from how fast this grows.
    pub tracking_frames: u64,
}

#[derive(Debug)]
struct Setup {
    mode: RunMode,
    module: Option<ModuleStatus>,
    output: Option<OutputTarget>,
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

    pub fn count_tracking_frame(&self) {
        self.tracking_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn report(&self) -> DaemonReport {
        let setup = self.setup.read().unwrap();
        DaemonReport {
            version: env!("VRFT_VERSION"),
            mode: setup.mode,
            module: setup.module.clone(),
            output: setup.output.clone(),
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
            runtime: Some("native"),
            loaded: true,
            error: None,
        });
        status.set_output(OutputTarget {
            mode: "VRChat".into(),
            address: "127.0.0.1".into(),
            port: 9000,
            max_fps: Some(60.0),
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
        let report = DaemonStatus::new(RunMode::CameraPreviewOnly).report();
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["mode"], "camera_preview_only");
        assert!(json["module"].is_null());
    }
}
