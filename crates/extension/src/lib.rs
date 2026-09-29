//! What a daemon extension implements, and what the daemon gives it.
//!
//! An extension adds optional support, such as for the Quest Pro's cameras,
//! on top of the tracking module. Extensions are compiled into `vrft_d`, each
//! behind a Cargo feature, and switched on or off in `config.json`:
//!
//! ```json
//! { "extensions": { "quest-pro": { "enabled": false } } }
//! ```
//!
//! Enabled extensions start before the tracking module loads. The daemon
//! serves each one's routes under `/ext/<id>` on its local API, puts each
//! one's status in `/status` under `extensions.<id>`, and runs each one's
//! [`FrameHook`] on every tracking frame.
//!
//! An extension defines what its routes and status carry in its own protocol
//! crate, which its desktop app half reads with, so both halves compile
//! against one definition.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use vrft_api::UnifiedTrackingData;

pub use axum::Router;
pub use vrft_protocol::{ExtensionReport, RunMode};

/// Bumped when a change to this API needs extensions updating.
pub const API_VERSION: u32 = 1;

/// What the daemon gives an extension when it starts.
pub struct HostContext {
    /// The daemon's working directory, where `config.json`, `models/` and
    /// `.local/` live.
    pub root: PathBuf,
    /// Cleared when the daemon shuts down. Threads the extension starts should
    /// end once it is.
    pub running: Arc<AtomicBool>,
    pub mode: RunMode,
}

/// Returns an extension's status for `/status`, as the type its protocol
/// crate defines. Called from the API server's threads, so it must be quick.
pub type StatusFn = Box<dyn Fn() -> serde_json::Value + Send + Sync>;

/// What a started extension hands back to the daemon.
#[derive(Default)]
pub struct Started {
    /// Served under `/ext/<id>`.
    pub routes: Option<Router>,
    /// Whether `routes` serve a browser page at `/`. The daemon's own `/`
    /// redirects to the first such page.
    pub page: bool,
    pub status: Option<StatusFn>,
    pub frame_hook: Option<Box<dyn FrameHook>>,
}

/// Runs on the daemon's output thread for every tracking frame, so it must
/// not block.
pub trait FrameHook: Send {
    /// Whether the tracking module loaded. Called before any frame, and again
    /// whenever switching modules loads, or fails to load, another one.
    fn module_loaded(&mut self, _loaded: bool) {}

    /// The tracking module's values, before the daemon smooths them. Only
    /// called for frames the module produced, not for the held frames the
    /// daemon repeats while the module is silent.
    fn before_mutation(&mut self, _data: &UnifiedTrackingData) {}

    /// Whether this hook has live data of its own, so the daemon keeps
    /// sending frames at full rate while the tracking module is silent or
    /// absent.
    fn has_live_data(&self) -> bool {
        false
    }

    /// Changes the values the daemon is about to send. Extensions run in the
    /// order the daemon lists them, so a later one sees an earlier one's
    /// changes.
    fn after_mutation(&mut self, _data: &mut UnifiedTrackingData) {}
}

pub trait DaemonExtension: Send {
    /// Stable identifier used in `config.json`, URLs and `/status`, such as
    /// `quest-pro`.
    fn id(&self) -> &'static str;

    /// Name shown to people, such as `Quest Pro`.
    fn name(&self) -> &'static str;

    /// Handles `vrft_d <name> ...` when `name` is one of this extension's
    /// subcommands, whether or not the extension is enabled. Returns `None`
    /// for any other name.
    fn run_subcommand(&self, _name: &str, _arguments: &[String]) -> Option<anyhow::Result<()>> {
        None
    }

    /// Starts the extension's work, returning quickly.
    fn start(self: Box<Self>, host: HostContext) -> anyhow::Result<Started>;
}

/// `config.json`'s `extensions.<id>` block. Settings people change while
/// VRFT runs are the extension's to keep, not this file's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionConfig {
    /// Extensions built into the daemon are on unless turned off.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

impl Default for ExtensionConfig {
    fn default() -> Self {
        Self {
            enabled: enabled_by_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_enabled() {
        let config: ExtensionConfig = serde_json::from_str("{}").unwrap();
        assert!(config.enabled);
        let config: ExtensionConfig =
            serde_json::from_str(r#"{"enabled": false, "unknown": 5}"#).unwrap();
        assert!(!config.enabled);
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json, serde_json::json!({"enabled": false}));
    }
}
