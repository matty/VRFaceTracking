//! Turns daemon status into what the app shows: a headline, and one reading
//! per part of the core pipeline. Extensions add their own readings. Nothing
//! here touches GPUI.
use crate::client::{ModuleStatus, RunMode, Status};
use crate::nav::PageId;
use rust_i18n::t;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum Connection {
    Connecting,
    Online,
    Offline { error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Waiting,
    Problem,
    Off,
}

/// Where a reading's problem is fixed, for a button beside it in place of
/// words saying what to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// The page with the control that fixes it.
    Open(PageId),
    /// `config.json`, in the system's editor.
    Config,
}

impl Fix {
    pub fn label(self) -> String {
        match self {
            Fix::Open(page) => t!("summary.open_page", page = page.label()).into(),
            Fix::Config => t!("summary.open_config").into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub tone: Tone,
    pub value: String,
    pub detail: String,
    pub fix: Option<Fix>,
}

impl Reading {
    pub fn new(tone: Tone, value: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            tone,
            value: value.into(),
            detail: detail.into(),
            fix: None,
        }
    }

    pub fn fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }

    /// Nothing to show, such as while the daemon isn't running.
    pub fn unavailable() -> Self {
        Self::new(Tone::Off, "\u{2014}", t!("summary.unavailable"))
    }
}

/// Rates measured by the app from counters in successive statuses.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    pub tracking_fps: Option<f32>,
}

impl Rates {
    /// Whether the tracking module is delivering face data.
    pub fn tracking(&self) -> bool {
        self.tracking_fps.is_some_and(|fps| fps >= 0.5)
    }
}

fn extensions_only(status: &Status) -> bool {
    status
        .daemon
        .as_ref()
        .is_some_and(|daemon| daemon.mode == RunMode::ExtensionsOnly)
}

fn runtime_label(runtime: Option<&str>) -> Cow<'static, str> {
    match runtime {
        Some("native") => t!("summary.runtime_native"),
        Some("dotnet") => ".NET".into(),
        _ => t!("summary.runtime_module"),
    }
}

pub fn fps(value: f32) -> String {
    t!("summary.fps", fps = format!("{value:.0}")).into()
}

/// What the app is doing to VRFT itself: starting, stopping or restarting
/// it, or how that last went.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Launch {
    #[default]
    Idle,
    Starting,
    Stopping,
    /// Stopping, to start again.
    Restarting,
    /// Stopped because the user asked, and not started since.
    StoppedByUser,
    /// Starting or stopping failed, and why.
    Failed(String),
}

/// The one-line answer to "is it working?", with the next step when it
/// isn't. The window's title bar shows the same.
pub fn headline(
    connection: &Connection,
    status: Option<&Status>,
    rates: &Rates,
    launch: &Launch,
) -> Reading {
    let online = *connection == Connection::Online;
    match launch {
        Launch::Restarting => {
            return Reading::new(
                Tone::Waiting,
                t!("summary.restarting"),
                t!("summary.applying_change"),
            )
        }
        Launch::Stopping => return Reading::new(Tone::Waiting, t!("summary.stopping"), ""),
        Launch::Starting if !online => {
            return Reading::new(Tone::Waiting, t!("summary.starting"), "")
        }
        Launch::Failed(reason) if !online => {
            return Reading::new(Tone::Problem, t!("summary.couldnt_start"), reason)
        }
        Launch::Failed(reason) => {
            return Reading::new(Tone::Problem, t!("summary.didnt_stop"), reason)
        }
        _ => {}
    }
    let status = match (connection, status) {
        (Connection::Connecting, _) => {
            return Reading::new(Tone::Waiting, t!("summary.connecting"), "")
        }
        (Connection::Offline { .. }, _) if *launch == Launch::StoppedByUser => {
            return Reading::new(
                Tone::Off,
                t!("summary.stopped"),
                t!("summary.stopped_detail"),
            )
        }
        (Connection::Offline { .. }, _) | (Connection::Online, None) => {
            return Reading::new(
                Tone::Problem,
                t!("summary.not_running"),
                t!("summary.not_running_detail"),
            )
        }
        (Connection::Online, Some(status)) => status,
    };
    let Some(daemon) = &status.daemon else {
        return Reading::new(
            Tone::Good,
            t!("summary.connected"),
            t!("summary.connected_old_daemon"),
        );
    };
    if daemon.mode == RunMode::ExtensionsOnly {
        return Reading::new(
            Tone::Waiting,
            t!("summary.extensions_only"),
            t!("summary.extensions_only_detail"),
        );
    }
    let Some(module) = &daemon.module else {
        return Reading::new(
            Tone::Waiting,
            t!("summary.starting_up"),
            t!("summary.loading_module"),
        );
    };
    if !module.chosen() {
        return Reading::new(
            Tone::Waiting,
            t!("summary.no_module"),
            t!("summary.no_module_detail"),
        )
        .fix(Fix::Open(PageId::MODULES));
    }
    if module.loading {
        return Reading::new(
            Tone::Waiting,
            t!("summary.loading_named", module = module.display_name()),
            t!("summary.starting_module"),
        );
    }
    if !module.loaded {
        return Reading::new(
            Tone::Problem,
            t!("summary.module_didnt_load", module = module.display_name()),
            module.error.clone().unwrap_or_default(),
        )
        .fix(Fix::Open(PageId::MODULES));
    }
    if !rates.tracking() {
        return Reading::new(
            Tone::Waiting,
            t!("summary.waiting_for_tracking"),
            t!("summary.no_face_data", module = module.display_name()),
        );
    }
    // The tiles under it say where it goes and how fast.
    Reading::new(Tone::Good, t!("summary.live"), "")
}

/// What to say when the daemon couldn't read `config.json`.
pub fn config_error(error: &str) -> String {
    t!("summary.config_error", error = error).into()
}

pub fn engine(status: Option<&Status>, address: &str) -> Reading {
    let Some(status) = status else {
        return Reading::new(
            Tone::Off,
            t!("summary.engine_not_running"),
            t!("summary.engine_start_hint"),
        );
    };
    match &status.daemon {
        None => Reading::new(Tone::Good, t!("summary.engine_running"), address),
        Some(daemon) if daemon.mode == RunMode::ExtensionsOnly => Reading::new(
            Tone::Waiting,
            t!("summary.extensions_only"),
            t!("summary.engine_extensions_only_detail"),
        ),
        Some(daemon) => Reading::new(
            Tone::Good,
            t!("summary.engine_running"),
            t!("summary.version", version = daemon.version),
        ),
    }
}

pub fn module(status: Option<&Status>, rates: &Rates) -> Reading {
    let Some(status) = status else {
        return Reading::unavailable();
    };
    let Some(daemon) = &status.daemon else {
        return Reading::new(
            Tone::Off,
            t!("summary.unknown"),
            t!("summary.update_to_see"),
        );
    };
    if daemon.mode == RunMode::ExtensionsOnly {
        return Reading::new(
            Tone::Off,
            t!("summary.not_loaded"),
            t!("summary.extensions_only_mode"),
        );
    }
    let Some(module) = &daemon.module else {
        return Reading::new(Tone::Waiting, t!("summary.loading"), "");
    };
    if !module.chosen() {
        return Reading::new(
            Tone::Off,
            t!("summary.none_chosen"),
            t!("summary.choose_module"),
        )
        .fix(Fix::Open(PageId::MODULES));
    }
    let runtime = runtime_label(module.runtime.as_deref());
    let name = module.display_name();
    if module.loading {
        return Reading::new(
            Tone::Waiting,
            name,
            t!("summary.runtime_loading", runtime = runtime),
        );
    }
    if let Some(error) = &module.error {
        return Reading::new(Tone::Problem, name, error).fix(Fix::Open(PageId::MODULES));
    }
    if !module.loaded {
        return Reading::new(Tone::Problem, name, t!("summary.not_loaded"))
            .fix(Fix::Open(PageId::MODULES));
    }
    match rates.tracking_fps.filter(|_| rates.tracking()) {
        Some(rate) => Reading::new(
            Tone::Good,
            name,
            t!("summary.runtime_rate", runtime = runtime, rate = fps(rate)),
        ),
        None => Reading::new(
            Tone::Waiting,
            name,
            t!("summary.runtime_no_data", runtime = runtime),
        ),
    }
}

/// A short label for how the module in use stands, for a pill beside it:
/// from what VRFT says about the module it's running or loading.
pub fn module_state(status: Option<&ModuleStatus>) -> (Tone, Cow<'static, str>) {
    match status {
        Some(status) if status.loading => (Tone::Waiting, t!("summary.loading")),
        Some(status) if status.loaded => (Tone::Good, t!("summary.state_in_use")),
        Some(_) => (Tone::Problem, t!("summary.state_didnt_load")),
        None => (Tone::Waiting, t!("summary.state_chosen")),
    }
}

/// Why the module in use didn't load, once VRFT has finished trying.
pub fn module_failure(status: Option<&ModuleStatus>) -> Option<String> {
    status
        .filter(|status| status.chosen() && !status.loading && !status.loaded)
        .map(|status| {
            status
                .error
                .clone()
                .unwrap_or_else(|| t!("summary.it_didnt_load").into())
        })
}

pub fn output(status: Option<&Status>, rates: &Rates) -> Reading {
    let Some(status) = status else {
        return Reading::unavailable();
    };
    if extensions_only(status) {
        return Reading::new(
            Tone::Off,
            t!("summary.off"),
            t!("summary.extensions_only_mode"),
        );
    }
    let Some(output) = status
        .daemon
        .as_ref()
        .and_then(|daemon| daemon.output.as_ref())
    else {
        return Reading::new(
            Tone::Off,
            t!("summary.unknown"),
            t!("summary.update_to_see"),
        );
    };
    let address = format!("{}:{}", output.address, output.port);
    let detail = match output.max_fps {
        Some(max) => t!("summary.output_up_to", address = address, rate = fps(max)).into(),
        None => address,
    };
    let tone = if rates.tracking() {
        Tone::Good
    } else {
        Tone::Off
    };
    Reading::new(tone, &output.mode, detail)
}

/// Measures how fast a counter grows, over roughly the last two seconds.
#[derive(Debug, Default)]
pub struct RateMeter {
    samples: VecDeque<(Instant, u64)>,
}

impl RateMeter {
    const WINDOW: Duration = Duration::from_secs(2);
    const MIN_SPAN: Duration = Duration::from_millis(400);

    pub fn record(&mut self, at: Instant, count: u64) {
        // A smaller count means the daemon restarted.
        if self.samples.back().is_some_and(|&(_, last)| count < last) {
            self.samples.clear();
        }
        self.samples.push_back((at, count));
        while self
            .samples
            .front()
            .is_some_and(|&(first, _)| at.duration_since(first) > Self::WINDOW)
        {
            self.samples.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn per_second(&self) -> Option<f32> {
        let (&(first_at, first), &(last_at, last)) = (self.samples.front()?, self.samples.back()?);
        let span = last_at.duration_since(first_at);
        (span >= Self::MIN_SPAN).then(|| (last - first) as f32 / span.as_secs_f32())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{DaemonReport, OutputTarget};

    fn tracking_status() -> Status {
        Status {
            daemon: Some(DaemonReport {
                version: "0.1.0".into(),
                mode: RunMode::Normal,
                module: Some(ModuleStatus {
                    name: "vd_module.dll".into(),
                    runtime: Some("native".into()),
                    loaded: true,
                    ..ModuleStatus::default()
                }),
                output: Some(OutputTarget {
                    mode: "VRChat".into(),
                    address: "127.0.0.1".into(),
                    port: 9000,
                    max_fps: Some(60.0),
                    smoothing: Some(0.4),
                    vrchat: None,
                }),
                extensions: Vec::new(),
                config_error: None,
                tracking_frames: 100,
                pid: None,
            }),
            ..Status::default()
        }
    }

    const LIVE: Rates = Rates {
        tracking_fps: Some(60.0),
    };

    #[test]
    fn headline_follows_the_pipeline() {
        let offline = headline(
            &Connection::Offline {
                error: "refused".into(),
            },
            None,
            &Rates::default(),
            &Launch::Idle,
        );
        assert_eq!(offline.tone, Tone::Problem);
        assert_eq!(offline.value, "Tracking isn't running");

        let status = tracking_status();
        let waiting = headline(
            &Connection::Online,
            Some(&status),
            &Rates::default(),
            &Launch::Idle,
        );
        assert_eq!(waiting.value, "Waiting for face tracking…");

        let live = headline(&Connection::Online, Some(&status), &LIVE, &Launch::Idle);
        assert_eq!(live.tone, Tone::Good);
        assert_eq!(live.value, "Tracking is on");
    }

    #[test]
    fn headline_reports_a_module_that_failed_to_load() {
        let mut status = tracking_status();
        let module = status
            .daemon
            .as_mut()
            .and_then(|daemon| daemon.module.as_mut())
            .unwrap();
        module.loaded = false;
        module.error = Some("Not found in plugins".into());
        let reading = headline(&Connection::Online, Some(&status), &LIVE, &Launch::Idle);
        assert_eq!(reading.tone, Tone::Problem);
        assert_eq!(reading.value, "Virtual Desktop didn't load");
        assert_eq!(reading.detail, "Not found in plugins");
        assert_eq!(reading.fix, Some(Fix::Open(PageId::MODULES)));
        assert_eq!(super::module(Some(&status), &LIVE).tone, Tone::Problem);
    }

    #[test]
    fn extensions_only_mode_turns_module_and_output_off() {
        let mut status = tracking_status();
        status.daemon.as_mut().unwrap().mode = RunMode::ExtensionsOnly;
        assert_eq!(module(Some(&status), &LIVE).value, "Not loaded");
        assert_eq!(output(Some(&status), &LIVE).value, "Off");
        assert_eq!(
            headline(&Connection::Online, Some(&status), &LIVE, &Launch::Idle).value,
            "Add-ons only"
        );
    }

    #[test]
    fn headline_follows_what_the_app_is_doing_to_vrft() {
        let offline = Connection::Offline {
            error: "refused".into(),
        };
        let reading = |connection: &Connection, launch: Launch| {
            let status = tracking_status();
            let status = (*connection == Connection::Online).then_some(&status);
            headline(connection, status, &LIVE, &launch)
        };
        let stopped = reading(&offline, Launch::StoppedByUser);
        assert_eq!(
            (stopped.tone, stopped.value.as_str()),
            (Tone::Off, "Tracking is stopped")
        );
        assert_eq!(reading(&offline, Launch::Idle).tone, Tone::Problem);
        assert_eq!(
            reading(&offline, Launch::Starting).value,
            "Starting tracking…"
        );
        assert_eq!(reading(&offline, Launch::Restarting).tone, Tone::Waiting);
        // A restart passes through offline; it isn't an error.
        assert_eq!(
            reading(&Connection::Online, Launch::Restarting).value,
            "Restarting tracking…"
        );
        let failed = reading(&offline, Launch::Failed("no exe".into()));
        assert_eq!(
            (failed.value.as_str(), failed.detail.as_str()),
            ("Tracking couldn't start", "no exe")
        );
        assert_eq!(
            reading(&Connection::Online, Launch::Failed("stuck".into())).value,
            "Tracking didn't stop"
        );
        // Once started, the headline is about tracking again.
        assert_eq!(
            reading(&Connection::Online, Launch::Starting).value,
            "Tracking is on"
        );
    }

    #[test]
    fn a_fix_that_opens_a_page_is_labelled_with_it() {
        let page = PageId::new("demo/settings", "Settings");
        assert_eq!(Fix::Open(page).label(), "Open Settings");
        assert_eq!(Fix::Config.label(), "Open config.json");
    }

    #[test]
    fn rate_meter_measures_growth_and_resets_on_restart() {
        let start = Instant::now();
        let mut meter = RateMeter::default();
        meter.record(start, 0);
        assert_eq!(meter.per_second(), None, "one sample has no rate");
        meter.record(start + Duration::from_millis(500), 30);
        meter.record(start + Duration::from_millis(1000), 60);
        assert_eq!(meter.per_second(), Some(60.0));
        meter.record(start + Duration::from_millis(1250), 5);
        assert_eq!(meter.per_second(), None, "a restart starts over");
    }

    #[test]
    fn the_module_in_use_shows_how_loading_went() {
        let mut status = ModuleStatus {
            name: "vd_module.dll".into(),
            loading: true,
            ..ModuleStatus::default()
        };
        assert_eq!(module_state(Some(&status)).1, "Loading…");
        assert_eq!(module_failure(Some(&status)), None);
        status.loading = false;
        status.loaded = true;
        assert_eq!(module_state(Some(&status)).1, "In use");
        assert_eq!(module_failure(Some(&status)), None);
        status.loaded = false;
        status.error = Some("Failed to load".into());
        assert_eq!(module_state(Some(&status)).1, "Didn't load");
        assert_eq!(
            module_failure(Some(&status)).as_deref(),
            Some("Failed to load")
        );
        assert_eq!(module_state(None).1, "Chosen");
    }

    #[test]
    fn no_module_chosen_points_at_modules() {
        let mut status = tracking_status();
        status.daemon.as_mut().unwrap().module = Some(ModuleStatus::default());
        let reading = headline(&Connection::Online, Some(&status), &LIVE, &Launch::Idle);
        assert_eq!(reading.value, "No tracking module chosen");
        assert_eq!(reading.fix, Some(Fix::Open(PageId::MODULES)));
        assert_eq!(module(Some(&status), &LIVE).tone, Tone::Off);
        assert_eq!(module_failure(Some(&ModuleStatus::default())), None);
    }
}
