//! The daemon's local HTTP API on 127.0.0.1:27275, as types both sides
//! compile against: `vrft_d` serves them and the desktop app reads them.
//!
//! Every struct has a default for each field, so a reader built against an
//! older or newer version still parses what it gets. Each extension's own
//! routes and status are defined in that extension's protocol crate; here
//! they are only an id and JSON.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod layout;

pub const PORT: u16 = 27275;
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:27275";

/// `vrft_d --owner-pid <pid>`: the daemon stops when process `pid`, the
/// desktop app that started it, exits, however it exits.
pub const OWNER_PID_ARG: &str = "--owner-pid";

/// A named mutex each running daemon holds, and only daemons: a
/// `vrft_d train-tongue` run is also `vrft_d.exe`, but isn't one. A second
/// daemon refuses to start while it exists, as both would send to VRChat.
pub const DAEMON_INSTANCE: &str = "Local\\VRFaceTracking.vrft_d";

/// Shared memory holding the running daemon's process id, a `u32`, while it
/// runs, so the desktop app can end one that has never answered, and no
/// other `vrft_d.exe`.
pub const DAEMON_PID: &str = "Local\\VRFaceTracking.vrft_d.pid";

/// A named mutex held while `config.json` is read, changed and written: by
/// the daemon, and by the desktop app while the daemon doesn't run to do it.
pub const CONFIG_LOCK: &str = "Local\\VRFaceTracking.config";

/// The daemon's own routes.
pub mod routes {
    /// GET: [`Status`](super::Status).
    pub const STATUS: &str = "/status";
    /// POST a [`ShutdownRequest`](super::ShutdownRequest).
    pub const SHUTDOWN: &str = "/shutdown";
    /// POST an [`EnableRequest`](super::EnableRequest), answered with an
    /// [`EnableResponse`](super::EnableResponse).
    pub const EXTENSIONS_ENABLED: &str = "/extensions/enabled";
    /// GET [`Config`](super::Config); POST a [`ConfigPatch`](super::ConfigPatch),
    /// answered with the new [`Config`](super::Config). Changes apply when
    /// VRFT next starts.
    pub const CONFIG: &str = "/config";
    /// GET [`Modules`](super::Modules): installed tracking modules, the
    /// module registry and installs in progress.
    pub const MODULES: &str = "/modules";
    /// POST (empty JSON object `{}`): fetch the module registry again.
    pub const MODULES_REFRESH: &str = "/modules/registry/refresh";
    /// POST a [`ModuleRequest`](super::ModuleRequest): install a registry
    /// module, or update an installed one, in the background.
    pub const MODULES_INSTALL: &str = "/modules/install";
    /// POST a [`ModuleRequest`](super::ModuleRequest): remove an installed
    /// registry module, answered with [`Modules`](super::Modules).
    pub const MODULES_UNINSTALL: &str = "/modules/uninstall";
    /// POST a [`UseModuleRequest`](super::UseModuleRequest): make a module
    /// the tracking module, saved in `config.json` and loaded straight away
    /// without restarting VRFT. Answered with [`Modules`](super::Modules);
    /// `/status` shows it loading.
    pub const MODULES_USE: &str = "/modules/use";
    /// GET [`PipelineTrace`](super::PipelineTrace): a recent frame's values
    /// after each stage of the pipeline. 204 until a frame has been
    /// recorded; the daemon records them for a few seconds after each GET.
    pub const DEBUG_PIPELINE: &str = "/debug/pipeline";

    /// Where extension `id`'s routes are served, such as `/ext/quest-pro`.
    pub fn extension(id: &str) -> String {
        format!("/ext/{id}")
    }
}

/// `/status`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    /// Missing from daemons built before the desktop app existed.
    pub daemon: Option<DaemonReport>,
    /// Each running extension's status, by extension id, in the type that
    /// extension's protocol defines.
    pub extensions: BTreeMap<String, serde_json::Value>,
}

/// What the daemon is running.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonReport {
    pub version: String,
    pub mode: RunMode,
    /// The tracking module, once the daemon has looked for it.
    pub module: Option<ModuleStatus>,
    pub output: Option<OutputTarget>,
    /// Every extension built into the daemon, enabled or not.
    pub extensions: Vec<ExtensionReport>,
    /// Why `config.json` couldn't be read, when the daemon fell back to its
    /// defaults.
    pub config_error: Option<String>,
    /// Frames the tracking module has delivered since startup. A client
    /// derives the tracking rate from how fast this grows.
    pub tracking_frames: u64,
    /// The daemon's process id, so the desktop app can stop exactly this
    /// process and no other `vrft_d.exe`. Missing from older daemons.
    pub pid: Option<u32>,
}

impl DaemonReport {
    pub fn extension(&self, id: &str) -> Option<&ExtensionReport> {
        self.extensions.iter().find(|extension| extension.id == id)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Normal,
    /// `--extensions-only`: extensions run, but no tracking module loads and
    /// nothing is sent over OSC. Called `camera_preview_only` before it
    /// applied to every extension.
    #[serde(alias = "camera_preview_only")]
    ExtensionsOnly,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleStatus {
    /// The `module.active` value from `config.json`: a module's file name,
    /// or its path under `plugins/`.
    pub name: String,
    /// What to call it, once the plugin has been found.
    pub label: Option<String>,
    /// `native` or `dotnet`, once the plugin has been found.
    pub runtime: Option<String>,
    /// Being loaded, at startup or after switching modules.
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
}

impl ModuleStatus {
    /// Whether `config.json` names a module at all. A new install has none
    /// until its first-launch setup chooses one.
    pub fn chosen(&self) -> bool {
        !self.name.trim().is_empty()
    }

    /// What to call the module.
    pub fn display_name(&self) -> String {
        self.label
            .clone()
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| module_name(&self.name))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputTarget {
    pub mode: String,
    pub address: String,
    pub port: u16,
    pub max_fps: Option<f32>,
    /// The smoothing applied before sending, 0 to 1; `None` when it's off.
    pub smoothing: Option<f32>,
    /// What OSCQuery has found of VRChat; `None` for other outputs.
    pub vrchat: Option<VrchatLink>,
}

/// VRChat as found over OSCQuery (mDNS), for the VRChat output.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VrchatLink {
    /// VRChat is running, with OSC on, at the address tracking goes to.
    pub found: bool,
    /// Whether the avatar in use takes face tracking; `None` until its
    /// parameters have been read.
    pub avatar_face_tracking: Option<bool>,
    /// Where tracking is sent, `host:port`: the configured host, at the port
    /// VRChat reports once found.
    pub sending_to: String,
}

/// How one extension built into the daemon stands.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtensionReport {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Why an enabled extension failed to start.
    pub error: Option<String>,
}

/// The settings in `config.json` people change, and what they can choose
/// from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where the file is.
    pub path: String,
    /// The tracking module, as `config.json` names it: its path under
    /// `plugins/`, such as `vd_module.dll`, or an older config's bare file
    /// name, matched to the module it names where there is one.
    pub module: String,
    /// Every tracking module found in `plugins/`.
    pub modules: Vec<Plugin>,
    /// Whether VRCFT (.NET) modules can run: VRFT's host for them is
    /// installed.
    pub dotnet_host: bool,
    /// Where tracking goes: `VRChat`, `Resonite` or `Generic`.
    pub output_mode: String,
    pub output_modes: Vec<String>,
    pub send_address: String,
    pub send_port: u16,
    /// Most tracking frames a second; `None` for no limit.
    pub max_fps: Option<f32>,
    pub smoothing_enabled: bool,
    /// 0 to 1.
    pub smoothing: f32,
    /// `mutator.filter`, `mutator.correctors` and `mutator.adjustment`.
    pub tuning: Tuning,
    /// The groups `tuning.adjustment.ranges` can set, in the order to show
    /// them. Empty from a VRFT too old to have tracking tuning.
    pub adjustment_groups: Vec<AdjustmentGroup>,
    /// The desktop app's first-launch setup was finished or skipped.
    pub setup_done: bool,
}

/// The tracking tuning in `config.json`, named as it is there.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tuning {
    pub filter: FilterTuning,
    pub correctors: CorrectorsTuning,
    pub adjustment: AdjustmentTuning,
}

/// The Euro filter's default cutoff frequency (Hz) for its speed estimate.
pub const DEFAULT_D_CUTOFF: f32 = 0.1;

/// `mutator.filter`: Euro filter settings. `min_cutoff` and `beta` are
/// worked out from `mutator.smoothness` unless set here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterTuning {
    /// Cutoff frequency (Hz) when the value is still; lower is smoother
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_cutoff: Option<f32>,
    /// How fast the cutoff rises with speed; higher lags less on fast moves
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beta: Option<f32>,
    /// Cutoff frequency (Hz) for the speed estimate itself
    pub d_cutoff: f32,
    /// Whether head pose is smoothed along with the face
    pub head: bool,
}

impl Default for FilterTuning {
    fn default() -> Self {
        Self {
            min_cutoff: None,
            beta: None,
            d_cutoff: DEFAULT_D_CUTOFF,
            head: true,
        }
    }
}

/// `mutator.correctors`: VRCFaceTracking's "Unified Correctors".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CorrectorsTuning {
    /// Whether the default pipeline runs the correctors
    pub enabled: bool,
    /// Keep `MouthClosed` at or below `JawOpen`
    pub mouth_closed_clamp: bool,
    /// Reduce each lip suck as the lip on that side opens
    pub lip_suck_limiter: bool,
    /// How much each eyelid and brow follows the other side, 0 (none) to
    /// 1 (both the average)
    pub eyelid_blend: f32,
    /// Give both eyes the average vertical gaze
    pub eye_look_symmetrize: bool,
}

impl Default for CorrectorsTuning {
    fn default() -> Self {
        Self {
            enabled: true,
            mouth_closed_clamp: true,
            lip_suck_limiter: true,
            eyelid_blend: 0.0,
            eye_look_symmetrize: false,
        }
    }
}

/// `mutator.adjustment`: VRCFaceTracking's "Parameter Adjustment". Each
/// group listed in `ranges` has its `[floor, ceil]` stretched to the full
/// range, so `"jaw": [0, 0.8]` makes 80% jaw open drive the avatar's jaw
/// fully open.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdjustmentTuning {
    /// Whether the default pipeline runs the adjustment
    pub enabled: bool,
    /// `[floor, ceil]` by [`AdjustmentGroup::key`]; a group that isn't here
    /// uses its full range.
    pub ranges: BTreeMap<String, [f32; 2]>,
}

/// A set of values sharing one adjustment range.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdjustmentGroup {
    pub key: String,
    pub label: String,
    /// The full range: 0 to 1 for the face, -1 to 1 for head pose.
    pub min: f32,
    pub max: f32,
}

/// A tracking module found in `plugins/`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Plugin {
    /// The registry id, for a module installed from the module registry.
    pub module_id: Option<String>,
    /// Its path under `plugins/` with `/` separators, which `config.json`
    /// names. For a module directly in `plugins/`, its file name.
    pub file: String,
    /// What to call it.
    pub name: String,
    /// `native` or `dotnet`.
    pub runtime: String,
}

/// `/modules`: tracking modules, installed and available.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Modules {
    /// The tracking module chosen in `config.json`, as the matching
    /// [`InstalledModule::file`] when it's installed.
    pub active: String,
    /// Whether VRFT can run VRCFT (.NET) modules: its `VrcftRuntime.exe`
    /// host is installed.
    pub dotnet_host: bool,
    pub installed: Vec<InstalledModule>,
    pub registry: RegistryState,
    /// Installs and updates, running or finished since VRFT started.
    pub operations: Vec<ModuleOperation>,
}

/// A tracking module in `plugins/`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstalledModule {
    /// Its path under `plugins/` with `/` separators, which `config.json`
    /// names. For a module directly in `plugins/`, its file name.
    pub file: String,
    pub name: String,
    /// `native` or `dotnet`.
    pub runtime: String,
    /// Set for a module installed from the registry, or any module folder
    /// with a `module.json`, as VRCFT installs them.
    pub module_id: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub usage: Option<String>,
    pub page_url: Option<String>,
    /// The registry's version, when it differs from the installed one.
    pub update: Option<String>,
    /// Downloaded, and replaces the installed files when VRFT next starts,
    /// because they were in use.
    pub pending_restart: bool,
    /// Installed from the registry into `plugins/registry`, so the app can
    /// remove it.
    pub removable: bool,
}

/// The module registry, as last fetched.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RegistryState {
    pub url: String,
    pub modules: Vec<RegistryModule>,
    /// Fetching now.
    pub loading: bool,
    /// Why the last fetch failed.
    pub error: Option<String>,
    /// When `modules` were fetched, in Unix seconds.
    pub fetched_unix: Option<u64>,
}

/// One module in the registry, in the registry's own JSON names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct RegistryModule {
    pub module_id: String,
    pub module_name: String,
    pub author_name: String,
    pub module_description: String,
    pub usage_instructions: String,
    pub version: String,
    pub dll_file_name: String,
    pub download_url: String,
    pub module_page_url: String,
    pub downloads: u64,
    pub ratings: u64,
    pub rating: f64,
    pub last_updated: String,
}

/// An install or update.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleOperation {
    pub module_id: String,
    pub name: String,
    pub state: OperationState,
    /// Download progress, 0 to 1, when the size is known.
    pub fraction: Option<f32>,
    /// What happened, for people.
    pub message: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    #[default]
    Downloading,
    Installing,
    Done,
    Failed,
}

impl OperationState {
    pub fn running(self) -> bool {
        matches!(self, Self::Downloading | Self::Installing)
    }
}

/// Names a registry module, as JSON so a web page can't post it cross-site.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleRequest {
    pub module_id: String,
}

/// Makes a module the tracking module.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UseModuleRequest {
    /// The module's [`InstalledModule::file`].
    pub file: String,
}

/// Sets `extensions.<id>.enabled` in `config.json`'s JSON, keeping everything
/// else in it.
pub fn set_extension_enabled(
    config: &mut serde_json::Value,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    let root = config
        .as_object_mut()
        .ok_or("config.json isn't a JSON object")?;
    let extensions = root
        .entry("extensions")
        .or_insert_with(|| serde_json::json!({}));
    if !extensions.is_object() {
        *extensions = serde_json::json!({});
    }
    let block = extensions
        .as_object_mut()
        .expect("just made an object")
        .entry(id)
        .or_insert_with(|| serde_json::json!({}));
    if !block.is_object() {
        *block = serde_json::json!({});
    }
    block["enabled"] = serde_json::Value::Bool(enabled);
    Ok(())
}

/// What to call a tracking module, from its file name or its path under
/// `plugins/`.
pub fn module_name(file: &str) -> String {
    let file = file.rsplit(['/', '\\']).next().unwrap_or(file);
    match file {
        "vd_module.dll" => "Virtual Desktop".into(),
        "test_logger.dll" => "Test logger (no tracking)".into(),
        "babble_module.dll" => "Project Babble".into(),
        "steamlink_module.dll" => "SteamLink".into(),
        "etvr_module.dll" => "EyeTrackVR".into(),
        "livelink_module.dll" => "LiveLink".into(),
        "meowface_module.dll" => "MeowFace".into(),
        "ifacialmocap_module.dll" => "iFacialMocap".into(),
        "cymple_module.dll" => "Cymple".into(),
        _ => file
            .strip_suffix(".dll")
            .unwrap_or(file)
            .replace(['_', '-'], " "),
    }
}

/// Changes to some of the [`Config`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_port: Option<u16>,
    /// 0 for no limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fps: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smoothing_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smoothing: Option<f32>,
    /// Replaces all the tracking tuning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tuning: Option<Tuning>,
    /// Marks the first-launch setup finished, or not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_done: Option<bool>,
}

/// A JSON body, so a web page can't stop the daemon with a plain cross-site
/// form post: browsers only send JSON cross-site after a CORS preflight,
/// which the daemon doesn't answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShutdownRequest {
    pub requested_by: String,
}

/// Turns an extension on or off in `config.json`, from the next start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnableRequest {
    pub id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnableResponse {
    pub restart_required: bool,
}

/// `/debug/pipeline`: one recent frame's values after each stage between the
/// tracking module and the output, so a view can show what each stage did.
/// The daemon only records frames while someone asks for them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PipelineTrace {
    /// The tracking module produced this frame. Otherwise the daemon is
    /// repeating the module's last frame, or has none to repeat.
    pub fresh: bool,
    /// Each value, in the order every stage's `values` lists them.
    pub params: Vec<TraceParam>,
    /// In the order they run.
    pub stages: Vec<TraceStage>,
}

/// One value the pipeline carries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TraceParam {
    /// As the debug API names it, such as `JawOpen` or `EyeLeftOpenness`.
    pub name: String,
    /// `eyes`, `brows`, `nose`, `cheeks`, `jaw`, `mouth`, `lips`, `tongue`,
    /// `throat` or `head`.
    pub group: String,
    /// The range the value is meant to stay in.
    pub min: f32,
    pub max: f32,
}

/// What one stage left the values as.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TraceStage {
    pub kind: StageKind,
    /// The stage's own name: the mutation's, or an extension's.
    pub name: String,
    /// It ran on this frame. A stage that's turned off is listed, with no
    /// values, so a view can show it in its place.
    pub active: bool,
    /// After this stage, in `params` order. Empty when it didn't run.
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageKind {
    /// What the tracking module sent.
    Module,
    /// Values set through the debug API.
    Overrides,
    Adjustment,
    Correctors,
    Smoothing,
    /// An extension changing values before they're sent.
    Extension,
    /// A stage this reader doesn't know.
    #[default]
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_and_tolerates_missing_fields() {
        let status = Status {
            daemon: Some(DaemonReport {
                version: "0.1.0".into(),
                mode: RunMode::ExtensionsOnly,
                module: Some(ModuleStatus {
                    name: "vd_module.dll".into(),
                    runtime: Some("native".into()),
                    loaded: true,
                    ..ModuleStatus::default()
                }),
                output: None,
                extensions: vec![ExtensionReport {
                    id: "quest-pro".into(),
                    name: "Quest Pro".into(),
                    enabled: true,
                    error: None,
                }],
                config_error: None,
                tracking_frames: 5,
                pid: Some(1234),
            }),
            extensions: BTreeMap::from([("quest-pro".into(), serde_json::json!({"a": 1}))]),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(serde_json::from_str::<Status>(&json).unwrap(), status);

        let older: Status = serde_json::from_str(r#"{"status": "Live"}"#).unwrap();
        assert_eq!(older, Status::default());
    }

    #[test]
    fn modules_are_named_from_their_file_or_path() {
        assert_eq!(module_name("vd_module.dll"), "Virtual Desktop");
        assert_eq!(
            module_name("registry/abc/net7.0/Steam_Link.dll"),
            "Steam Link"
        );
        let status = ModuleStatus {
            name: "registry/abc/net7.0/Link.dll".into(),
            label: Some("SteamLink VRCFT Module".into()),
            ..ModuleStatus::default()
        };
        assert_eq!(status.display_name(), "SteamLink VRCFT Module");
    }

    #[test]
    fn the_old_preview_mode_name_still_reads() {
        let report: DaemonReport =
            serde_json::from_str(r#"{"mode": "camera_preview_only"}"#).unwrap();
        assert_eq!(report.mode, RunMode::ExtensionsOnly);
        assert_eq!(
            serde_json::to_value(RunMode::ExtensionsOnly).unwrap(),
            "extensions_only"
        );
    }
}
