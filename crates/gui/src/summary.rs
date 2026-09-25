//! Turns daemon status into what the app shows: a headline, one reading per
//! part of the pipeline, and the tongue position. Nothing here touches GPUI.
use crate::daemon::{RunMode, Status, Update};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// A camera frame older than this is not live. Matches the daemon, which
/// falls back to the module's tongue after the same interval.
const CAMERA_LIVE_MS: u64 = 250;

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

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub tone: Tone,
    pub value: String,
    pub detail: String,
}

impl Reading {
    pub fn new(tone: Tone, value: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            tone,
            value: value.into(),
            detail: detail.into(),
        }
    }

    fn unavailable() -> Self {
        Self::new(Tone::Off, "—", "")
    }
}

/// Rates measured by the app from counters in successive statuses.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    pub tracking_fps: Option<f32>,
    pub camera_fps: Option<f32>,
}

impl Rates {
    /// Whether the tracking module is delivering face data.
    pub fn tracking(&self) -> bool {
        self.tracking_fps.is_some_and(|fps| fps >= 0.5)
    }
}

pub fn camera_live(status: &Status) -> bool {
    status.frame_age_ms.is_some_and(|age| age <= CAMERA_LIVE_MS)
}

fn preview_only(status: &Status) -> bool {
    status
        .daemon
        .as_ref()
        .is_some_and(|daemon| daemon.mode == RunMode::CameraPreviewOnly)
}

fn runtime_label(runtime: Option<&str>) -> &'static str {
    match runtime {
        Some("native") => "Native",
        Some("dotnet") => ".NET",
        _ => "Module",
    }
}

fn fps(value: f32) -> String {
    format!("{value:.0} fps")
}

/// The one-line answer to "is it working?", with the next step when it isn't.
pub fn headline(connection: &Connection, status: Option<&Status>, rates: &Rates) -> Reading {
    let status = match (connection, status) {
        (Connection::Connecting, _) => {
            return Reading::new(
                Tone::Waiting,
                "Connecting to VRFT",
                "Looking for vrft_d on this PC.",
            )
        }
        (Connection::Offline { .. }, _) | (Connection::Online, None) => {
            return Reading::new(
                Tone::Problem,
                "VRFT isn't running",
                "This app shows what vrft_d is doing, so vrft_d needs to be running.",
            )
        }
        (Connection::Online, Some(status)) => status,
    };
    let Some(daemon) = &status.daemon else {
        return Reading::new(
            Tone::Good,
            "Connected to VRFT",
            "Update vrft_d to see the tracking module and output here.",
        );
    };
    if daemon.mode == RunMode::CameraPreviewOnly {
        return Reading::new(
            Tone::Waiting,
            "Camera preview only",
            "VRFT is showing the Quest Pro cameras without a tracking module or OSC output. \
             Restart it without --camera-preview-only to track.",
        );
    }
    let Some(module) = &daemon.module else {
        return Reading::new(
            Tone::Waiting,
            "Starting up",
            "VRFT is loading its tracking module.",
        );
    };
    if !module.loaded {
        let reason = module
            .error
            .clone()
            .unwrap_or_else(|| "Check module.active in config.json.".into());
        return Reading::new(
            Tone::Problem,
            format!("{} didn't load", module.name),
            reason,
        );
    }
    if !rates.tracking() {
        return Reading::new(
            Tone::Waiting,
            "Waiting for face tracking",
            format!(
                "{} is loaded, but no face data is arriving. Put the headset on and make sure it's streaming face tracking.",
                module.name
            ),
        );
    }
    let mut body = match &daemon.output {
        Some(output) => format!(
            "Sending {} OSC to {}:{} at {}.",
            output.mode,
            output.address,
            output.port,
            fps(rates.tracking_fps.unwrap_or_default())
        ),
        None => format!(
            "Tracking at {}.",
            fps(rates.tracking_fps.unwrap_or_default())
        ),
    };
    if status
        .output
        .as_ref()
        .is_some_and(|output| output.source == "enhanced model")
    {
        body.push_str(" Tongue tracking comes from the mouth cameras.");
    }
    Reading::new(Tone::Good, "Tracking is live", body)
}

pub fn engine(status: Option<&Status>, address: &str) -> Reading {
    let Some(status) = status else {
        return Reading::new(Tone::Off, "Not running", address);
    };
    match &status.daemon {
        None => Reading::new(Tone::Good, "Running", address),
        Some(daemon) if daemon.mode == RunMode::CameraPreviewOnly => {
            Reading::new(Tone::Waiting, "Preview only", "No module or OSC output")
        }
        Some(daemon) => Reading::new(Tone::Good, "Running", format!("Version {}", daemon.version)),
    }
}

pub fn module(status: Option<&Status>, rates: &Rates) -> Reading {
    let Some(status) = status else {
        return Reading::unavailable();
    };
    let Some(daemon) = &status.daemon else {
        return Reading::new(Tone::Off, "Unknown", "Update vrft_d to see it");
    };
    if daemon.mode == RunMode::CameraPreviewOnly {
        return Reading::new(Tone::Off, "Not loaded", "Preview mode");
    }
    let Some(module) = &daemon.module else {
        return Reading::new(Tone::Waiting, "Loading", "");
    };
    let runtime = runtime_label(module.runtime.as_deref());
    if let Some(error) = &module.error {
        return Reading::new(Tone::Problem, &module.name, error);
    }
    if !module.loaded {
        return Reading::new(Tone::Problem, &module.name, "Not loaded");
    }
    match rates.tracking_fps.filter(|_| rates.tracking()) {
        Some(rate) => Reading::new(
            Tone::Good,
            &module.name,
            format!("{runtime} · {}", fps(rate)),
        ),
        None => Reading::new(
            Tone::Waiting,
            &module.name,
            format!("{runtime} · no data yet"),
        ),
    }
}

pub fn output(status: Option<&Status>, rates: &Rates) -> Reading {
    let Some(status) = status else {
        return Reading::unavailable();
    };
    if preview_only(status) {
        return Reading::new(Tone::Off, "Off", "Preview mode");
    }
    let Some(output) = status
        .daemon
        .as_ref()
        .and_then(|daemon| daemon.output.as_ref())
    else {
        return Reading::new(Tone::Off, "Unknown", "Update vrft_d to see it");
    };
    let mut detail = format!("{}:{}", output.address, output.port);
    if let Some(max) = output.max_fps {
        detail.push_str(&format!(" · up to {}", fps(max)));
    }
    let tone = if rates.tracking() {
        Tone::Good
    } else {
        Tone::Off
    };
    Reading::new(tone, &output.mode, detail)
}

pub fn headset(status: Option<&Status>) -> Reading {
    let Some(status) = status else {
        return Reading::unavailable();
    };
    if let Some(mismatch) = &status.headset_mismatch {
        let app = match &mismatch.apk_version {
            Some(version) => format!("Headset app {version}"),
            None => "The headset app".into(),
        };
        return match mismatch.update {
            Update::Vrft => Reading::new(
                Tone::Problem,
                "Update VRFT",
                format!("{app} is newer than this VRFT"),
            ),
            Update::HeadsetApp => Reading::new(
                Tone::Problem,
                "Update headset app",
                format!("{app} is too old for this VRFT"),
            ),
        };
    }
    let Some(source) = &status.source else {
        return Reading::new(
            Tone::Waiting,
            "Not connected",
            first_sentence(&status.status),
        );
    };
    let place = match source.parse::<SocketAddr>() {
        Ok(address) if address.ip().is_loopback() => "Over USB".to_string(),
        Ok(address) => address.ip().to_string(),
        Err(_) => source.clone(),
    };
    let detail = match status
        .headset
        .as_ref()
        .and_then(|headset| headset.apk_version.as_deref())
    {
        Some(version) => format!("{place} · app {version}"),
        None => place,
    };
    Reading::new(Tone::Good, "Connected", detail)
}

pub fn mouth_cameras(status: Option<&Status>, rates: &Rates) -> Reading {
    let Some(status) = status else {
        return Reading::unavailable();
    };
    if camera_live(status) {
        let rate = rates.camera_fps.or_else(|| {
            status
                .headset
                .as_ref()
                .and_then(|headset| headset.camera_fps)
                .map(|fps| fps as f32)
        });
        let detail = rate.map(fps).unwrap_or_default();
        return Reading::new(Tone::Good, "Live", detail);
    }
    if status.source.is_some() {
        Reading::new(Tone::Waiting, "Waiting", "No frames from the headset")
    } else {
        Reading::new(Tone::Off, "Off", "Headset not connected")
    }
}

pub fn tongue_model(status: Option<&Status>) -> Reading {
    let Some(status) = status else {
        return Reading::unavailable();
    };
    match (&status.model, &status.model_error) {
        (Some(model), _) if model.fresh => Reading::new(
            Tone::Good,
            "Running",
            format!("{:.1} ms per frame", model.inference_ms),
        ),
        (_, Some(error)) => Reading::new(Tone::Problem, "Not running", error),
        (Some(model), None) => Reading::new(
            Tone::Waiting,
            "Paused",
            format!("No prediction for {} ms", model.age_ms),
        ),
        (None, None) if camera_live(status) => Reading::new(
            Tone::Waiting,
            "Starting",
            "Waiting for the first prediction",
        ),
        (None, None) => Reading::new(Tone::Off, "Idle", "Starts with the mouth cameras"),
    }
}

pub fn eyes(status: Option<&Status>) -> Reading {
    let Some(status) = status else {
        return Reading::unavailable();
    };
    if status.eye_output_deg.is_some() {
        return Reading::new(
            Tone::Good,
            "Separate",
            format!("{:.0} Hz", status.eyes.rate_hz),
        );
    }
    let headset_eye = status
        .headset
        .as_ref()
        .and_then(|headset| headset.eye.as_ref());
    if let Some(eye) = headset_eye.filter(|eye| eye.state == "error") {
        return Reading::new(Tone::Problem, "Error", &eye.message);
    }
    if status.eyes.fresh {
        return Reading::new(Tone::Waiting, "Combined", "Separate gaze is turned off");
    }
    if let Some(error) = &status.eyes.calibration_error {
        return Reading::new(Tone::Off, "Unavailable", error);
    }
    Reading::new(Tone::Off, "Off", "No eye data from the headset")
}

/// What the eye pipeline is doing, in a sentence, with the next step when it
/// isn't sending each eye.
pub fn eye_message(status: Option<&Status>) -> String {
    let Some(status) = status else {
        return "VRFT needs to be running to show the eyes.".into();
    };
    if let Some(error) = &status.eyes.calibration_error {
        return format!("Separate eye tracking isn't available: {error}");
    }
    if status.eye_output_deg.is_some() {
        let mut message = format!(
            "Tracking each eye separately, {:.0} times a second.",
            status.eyes.rate_hz
        );
        if status
            .eyes
            .sample
            .as_ref()
            .is_some_and(|sample| !sample.model_patched)
        {
            message.push_str(
                " The headset is using its standard eye model, so both eyes move together and won't cross.",
            );
        }
        return message;
    }
    if status.eyes.fresh {
        return "The headset is sending each eye, but separate tracking is turned off, so VRFT sends one combined gaze.".into();
    }
    let headset_message = status
        .headset
        .as_ref()
        .and_then(|headset| headset.eye.as_ref())
        .map(|eye| eye.message.as_str())
        .filter(|message| !message.is_empty());
    if let Some(message) = headset_message {
        return format!("Headset: {message}");
    }
    if status.source.is_some() {
        "The headset is connected but isn't sending eye data. Turn on Independent eye gaze in the headset app, then restart the stream.".into()
    } else {
        "Waiting for the headset. Until it connects, VRFT sends the standard combined gaze.".into()
    }
}

/// Angle with an explicit sign, as the preview shows gaze.
pub fn signed_degrees(value: f32) -> String {
    format!("{}{value:.1}°", if value >= 0. { "+" } else { "" })
}

/// Where the two gaze rays cross, from each eye's yaw in degrees. Yaw is
/// positive to the right, so eyes looking at something close have the left
/// eye turned further right than the right eye.
pub fn eyes_meet(left_yaw: f32, right_yaw: f32) -> String {
    const EYE_SEPARATION_M: f32 = 0.063;
    let vergence = left_yaw - right_yaw;
    let place = if vergence > 0.3 {
        let distance = EYE_SEPARATION_M / 2. / (vergence.to_radians() / 2.).tan();
        if distance < 5. {
            format!("About {distance:.2} m")
        } else {
            "Far away".into()
        }
    } else if vergence < -0.3 {
        "Diverging".into()
    } else {
        "Far away".into()
    };
    format!("{place} ({})", signed_degrees(vergence))
}

/// The daemon's connection messages can run on with retry details; a tile
/// has room for the first sentence.
fn first_sentence(text: &str) -> &str {
    text.split_once(". ").map_or(text, |(first, _)| first)
}

/// Where the model has the tongue on the source that drives VRChat, as the
/// preview page draws it: horizontal and vertical in -1..1, as in a mirror.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TongueReading {
    pub state: TongueState,
    /// How far out, 0..1.
    pub out: f32,
    pub horizontal: f32,
    pub vertical: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TongueState {
    NotTracked,
    In,
    Out,
}

/// `tracking` says whether the module is delivering face data; without it,
/// the module's tongue values are only the last ones it sent.
pub fn tongue(status: &Status, tracking: bool) -> TongueReading {
    let not_tracked = TongueReading {
        state: TongueState::NotTracked,
        out: 0.0,
        horizontal: 0.0,
        vertical: 0.0,
    };
    if let Some(output) = status
        .output
        .as_ref()
        .filter(|output| tracking || output.source == "enhanced model")
    {
        let values = output.values;
        let visible = output.visible.unwrap_or(values[0] > 0.05);
        return TongueReading {
            state: if visible {
                TongueState::Out
            } else {
                TongueState::In
            },
            out: values[0].clamp(0.0, 1.0),
            horizontal: (values[4] - values[3]).clamp(-1.0, 1.0),
            vertical: (values[1] - values[2]).clamp(-1.0, 1.0),
        };
    }
    match &status.model {
        Some(model) if model.fresh => {
            let [visibility, extension, horizontal, vertical, ..] = model.values;
            let visible = visibility >= model.threshold;
            TongueReading {
                state: if visible {
                    TongueState::Out
                } else {
                    TongueState::In
                },
                out: if visible {
                    visibility.max(extension).clamp(0.0, 1.0)
                } else {
                    0.0
                },
                horizontal: horizontal.clamp(-1.0, 1.0),
                vertical: vertical.clamp(-1.0, 1.0),
            }
        }
        _ => not_tracked,
    }
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
    use crate::daemon::{
        DaemonReport, HeadsetMismatch, ModuleStatus, OutputTarget, TongueModel, TongueOutput,
    };

    fn tracking_status() -> Status {
        Status {
            status: "Streaming".into(),
            daemon: Some(DaemonReport {
                version: "0.1.0".into(),
                mode: RunMode::Normal,
                module: Some(ModuleStatus {
                    name: "vd_module.dll".into(),
                    runtime: Some("native".into()),
                    loaded: true,
                    error: None,
                }),
                output: Some(OutputTarget {
                    mode: "VRChat".into(),
                    address: "127.0.0.1".into(),
                    port: 9000,
                    max_fps: Some(60.0),
                }),
                tracking_frames: 100,
            }),
            ..Status::default()
        }
    }

    const LIVE: Rates = Rates {
        tracking_fps: Some(60.0),
        camera_fps: Some(24.0),
    };

    #[test]
    fn headline_follows_the_pipeline() {
        let offline = headline(
            &Connection::Offline {
                error: "refused".into(),
            },
            None,
            &Rates::default(),
        );
        assert_eq!(offline.tone, Tone::Problem);
        assert_eq!(offline.value, "VRFT isn't running");

        let status = tracking_status();
        let waiting = headline(&Connection::Online, Some(&status), &Rates::default());
        assert_eq!(waiting.value, "Waiting for face tracking");

        let live = headline(&Connection::Online, Some(&status), &LIVE);
        assert_eq!(live.tone, Tone::Good);
        assert_eq!(
            live.detail,
            "Sending VRChat OSC to 127.0.0.1:9000 at 60 fps."
        );
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
        let reading = headline(&Connection::Online, Some(&status), &LIVE);
        assert_eq!(reading.tone, Tone::Problem);
        assert_eq!(reading.value, "vd_module.dll didn't load");
        assert_eq!(reading.detail, "Not found in plugins");
        assert_eq!(module_reading(&status).tone, Tone::Problem);
    }

    fn module_reading(status: &Status) -> Reading {
        module(Some(status), &LIVE)
    }

    #[test]
    fn preview_mode_turns_module_and_output_off() {
        let mut status = tracking_status();
        status.daemon.as_mut().unwrap().mode = RunMode::CameraPreviewOnly;
        assert_eq!(module(Some(&status), &LIVE).value, "Not loaded");
        assert_eq!(output(Some(&status), &LIVE).value, "Off");
        assert_eq!(
            headline(&Connection::Online, Some(&status), &LIVE).value,
            "Camera preview only"
        );
    }

    #[test]
    fn headset_over_usb_is_named() {
        let status = Status {
            source: Some("127.0.0.1:27274".into()),
            ..Status::default()
        };
        assert_eq!(headset(Some(&status)).detail, "Over USB");
        let status = Status {
            source: Some("192.168.1.20:27274".into()),
            ..Status::default()
        };
        assert_eq!(headset(Some(&status)).detail, "192.168.1.20");
    }

    #[test]
    fn cameras_are_live_only_with_a_recent_frame() {
        let mut status = Status {
            source: Some("192.168.1.20:27274".into()),
            frame_age_ms: Some(40),
            ..Status::default()
        };
        assert_eq!(mouth_cameras(Some(&status), &LIVE).detail, "24 fps");
        status.frame_age_ms = Some(900);
        assert_eq!(mouth_cameras(Some(&status), &LIVE).tone, Tone::Waiting);
    }

    #[test]
    fn tongue_prefers_what_vrchat_receives() {
        let status = Status {
            output: Some(TongueOutput {
                source: "enhanced model".into(),
                visible: Some(true),
                values: [0.8, 0.5, 0.0, 0.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                ..TongueOutput::default()
            }),
            ..Status::default()
        };
        let reading = tongue(&status, false);
        assert_eq!(reading.state, TongueState::Out);
        assert_eq!(reading.vertical, 0.5);
        assert_eq!(reading.horizontal, -0.25);
    }

    #[test]
    fn tongue_falls_back_to_the_model_in_preview_mode() {
        let mut status = tracking_status();
        status.daemon.as_mut().unwrap().mode = RunMode::CameraPreviewOnly;
        let status = Status {
            model: Some(TongueModel {
                fresh: true,
                threshold: 0.5,
                values: [0.4, 0.9, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                ..TongueModel::default()
            }),
            ..status
        };
        let reading = tongue(&status, false);
        assert_eq!(reading.state, TongueState::In);
        assert_eq!(reading.out, 0.0);
        assert_eq!(
            tongue(&Status::default(), false).state,
            TongueState::NotTracked
        );
    }

    #[test]
    fn module_tongue_is_not_tracked_without_face_data() {
        let status = Status {
            output: Some(TongueOutput {
                source: "tracking module".into(),
                ..TongueOutput::default()
            }),
            ..Status::default()
        };
        assert_eq!(tongue(&status, false).state, TongueState::NotTracked);
        assert_eq!(tongue(&status, true).state, TongueState::In);
    }

    #[test]
    fn headset_says_which_side_to_update_when_protocols_differ() {
        let status = Status {
            headset_mismatch: Some(HeadsetMismatch {
                apk_version: Some("2027.1.0".into()),
                update: Update::Vrft,
            }),
            ..Status::default()
        };
        let reading = headset(Some(&status));
        assert_eq!(reading.tone, Tone::Problem);
        assert_eq!(reading.value, "Update VRFT");
        assert_eq!(
            reading.detail,
            "Headset app 2027.1.0 is newer than this VRFT"
        );
        let status = Status {
            headset_mismatch: Some(HeadsetMismatch {
                apk_version: None,
                update: Update::HeadsetApp,
            }),
            ..Status::default()
        };
        assert_eq!(headset(Some(&status)).value, "Update headset app");
    }

    #[test]
    fn headset_detail_keeps_the_first_sentence() {
        let status = Status {
            status: "Lost the connection to the headset (Camera stream closed). Retrying 10.0.1.196:27274".into(),
            ..Status::default()
        };
        assert_eq!(
            headset(Some(&status)).detail,
            "Lost the connection to the headset (Camera stream closed)"
        );
    }

    #[test]
    fn eyes_meet_at_the_vergence_distance() {
        // 7.2 degrees of vergence puts the crossing about half a metre away.
        assert_eq!(eyes_meet(3.6, -3.6), "About 0.50 m (+7.2°)");
        assert_eq!(eyes_meet(0.1, 0.0), "Far away (+0.1°)");
        assert_eq!(eyes_meet(-1.0, 1.0), "Diverging (-2.0°)");
    }

    #[test]
    fn eye_message_explains_what_is_missing() {
        assert_eq!(
            eye_message(Some(&Status::default())),
            "Waiting for the headset. Until it connects, VRFT sends the standard combined gaze."
        );
        let status = Status {
            eye_output_deg: Some([[1.0, 0.0], [-1.0, 0.0]]),
            eyes: crate::daemon::Eyes {
                rate_hz: 90.0,
                ..Default::default()
            },
            ..Status::default()
        };
        assert_eq!(
            eye_message(Some(&status)),
            "Tracking each eye separately, 90 times a second."
        );
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
}
