//! Turns the daemon's Quest Pro status into what the app shows: one reading
//! per part of the headset pipeline, and the tongue position. Nothing here
//! touches GPUI.
use crate::daemon::{Status, TongueSource, Update};
use crate::pages;
use rust_i18n::t;
use std::borrow::Cow;
use std::net::SocketAddr;
pub use vrft_gui_core::summary::{fps, Connection, Fix, Reading, Tone};

/// A camera frame older than this is not live. Matches the daemon, which
/// falls back to the module's tongue after the same interval.
const CAMERA_LIVE_MS: u64 = 250;

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

pub fn headset(status: Option<&Status>) -> Reading {
    let Some(status) = status else {
        return Reading::unavailable();
    };
    if let Some(mismatch) = &status.headset_mismatch {
        let version = mismatch.apk_version.as_deref();
        return match mismatch.update {
            Update::Vrft => Reading::new(
                Tone::Problem,
                t!("summary.update_vrft"),
                match version {
                    Some(version) => t!("summary.app_version_newer", version = version),
                    None => t!("summary.app_newer"),
                },
            ),
            Update::HeadsetApp => Reading::new(
                Tone::Problem,
                t!("summary.update_headset_app"),
                match version {
                    Some(version) => t!("summary.app_version_too_old", version = version),
                    None => t!("summary.app_too_old"),
                },
            ),
        };
    }
    let Some(source) = &status.source else {
        return Reading::new(
            Tone::Waiting,
            t!("summary.not_streaming"),
            brief_state(&status.status),
        );
    };
    let place = match source.parse::<SocketAddr>() {
        Ok(address) if address.ip().is_loopback() => t!("summary.over_usb").into_owned(),
        Ok(address) => address.ip().to_string(),
        Err(_) => source.clone(),
    };
    let detail = match status
        .headset
        .as_ref()
        .and_then(|headset| headset.apk_version.as_deref())
    {
        Some(version) => t!(
            "summary.place_app_version",
            place = place,
            version = version
        )
        .into_owned(),
        None => place,
    };
    Reading::new(Tone::Good, t!("summary.streaming"), detail)
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
        return Reading::new(Tone::Good, t!("summary.live"), detail);
    }
    if status.source.is_some() {
        Reading::new(
            Tone::Waiting,
            t!("summary.waiting"),
            t!("summary.no_frames_from_headset"),
        )
    } else {
        Reading::new(
            Tone::Off,
            t!("summary.off"),
            t!("summary.headset_not_streaming"),
        )
    }
}

pub fn tongue_model(status: Option<&Status>) -> Reading {
    let Some(status) = status else {
        return Reading::unavailable();
    };
    if !status.settings.mouth_model {
        return Reading::new(
            Tone::Off,
            t!("summary.off"),
            t!("summary.headset_tracks_mouth"),
        );
    }
    match (&status.model, &status.model_error) {
        (Some(model), _) if model.fresh => Reading::new(
            Tone::Good,
            t!("summary.running"),
            t!(
                "summary.ms_per_frame",
                ms = format!("{:.1}", model.inference_ms)
            ),
        ),
        (_, Some(error)) => Reading::new(Tone::Problem, t!("summary.not_running"), error),
        (Some(model), None) => Reading::new(
            Tone::Waiting,
            t!("summary.paused"),
            t!("summary.no_prediction_for", ms = model.age_ms),
        ),
        (None, None) if camera_live(status) => Reading::new(
            Tone::Waiting,
            t!("summary.starting"),
            t!("summary.waiting_first_prediction"),
        ),
        (None, None) => Reading::new(
            Tone::Off,
            t!("summary.idle"),
            t!("summary.starts_with_mouth_cameras"),
        ),
    }
}

/// Whether the mouth cameras can be recorded, and if not, why.
pub fn recording_cameras(status: Option<&Status>) -> Reading {
    let Some(status) = status else {
        return Reading::new(Tone::Problem, t!("summary.vrft_not_running"), "");
    };
    if camera_live(status) {
        return Reading::new(Tone::Good, t!("summary.live"), "");
    }
    let fix = Fix::Open(pages::HEADSET);
    if status.headset_mismatch.is_some() {
        let headset = headset(Some(status));
        return Reading::new(Tone::Problem, headset.value, headset.detail).fix(fix);
    }
    if status.source.is_none() {
        return Reading::new(Tone::Problem, t!("summary.headset_not_streaming"), "").fix(fix);
    }
    Reading::new(
        Tone::Waiting,
        t!("summary.no_frames"),
        t!("summary.no_frames_detail"),
    )
    .fix(fix)
}

/// Whether the headset sends all five cameras, which the face setup and
/// recordings need.
pub fn five_cameras(status: Option<&Status>) -> Reading {
    let Some(status) = status.filter(|status| camera_live(status)) else {
        return Reading::new(Tone::Waiting, t!("summary.five_cameras_waiting"), "");
    };
    if status.five_cameras {
        return Reading::new(Tone::Good, t!("summary.five_cameras_on"), "");
    }
    Reading::new(
        Tone::Waiting,
        t!("summary.five_cameras_off"),
        t!("summary.five_cameras_off_detail"),
    )
    .fix(Fix::Open(pages::HEADSET))
}

pub fn eyes(status: Option<&Status>) -> Reading {
    let Some(status) = status else {
        return Reading::unavailable();
    };
    if status.eye_output_deg.is_some() {
        return Reading::new(
            Tone::Good,
            t!("summary.per_eye"),
            t!("summary.hz", rate = format!("{:.0}", status.eyes.rate_hz)),
        );
    }
    let headset_eye = status
        .headset
        .as_ref()
        .and_then(|headset| headset.eye.as_ref());
    if let Some(eye) = headset_eye.filter(|eye| eye.state == "error") {
        return Reading::new(Tone::Problem, t!("summary.error"), &eye.message);
    }
    if status.eyes.fresh {
        // Off by choice is fine; off because the headset's model is stock
        // isn't.
        return if status.settings.eye_gaze {
            Reading::new(
                Tone::Waiting,
                t!("summary.combined"),
                t!("summary.combined_detail"),
            )
            .fix(Fix::Open(pages::HEADSET))
        } else {
            Reading::new(Tone::Off, t!("summary.combined"), t!("summary.per_eye_off"))
        };
    }
    if let Some(error) = &status.eyes.calibration_error {
        return Reading::new(Tone::Off, t!("summary.unavailable"), error);
    }
    // The headset app's own switch turns its eye data on.
    Reading::new(Tone::Off, t!("summary.off"), t!("summary.no_eye_data"))
        .fix(Fix::Open(pages::HEADSET))
}

/// Angle with an explicit sign, as the preview shows gaze.
pub fn signed_degrees(value: f32) -> String {
    format!("{}{value:.1}°", if value >= 0. { "+" } else { "" })
}

/// A yaw in words: yaw is positive to the right.
pub fn across(yaw: f32) -> String {
    if yaw.abs() < 0.05 {
        t!("summary.straight_ahead").into_owned()
    } else if yaw > 0. {
        t!("summary.degrees_right", degrees = format!("{yaw:.1}")).into_owned()
    } else {
        t!("summary.degrees_left", degrees = format!("{:.1}", -yaw)).into_owned()
    }
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
            t!("summary.about_metres", metres = format!("{distance:.2}"))
        } else {
            t!("summary.far_away")
        }
    } else if vergence < -0.3 {
        t!("summary.diverging")
    } else {
        t!("summary.far_away")
    };
    t!(
        "summary.eyes_meet",
        place = place,
        angle = signed_degrees(vergence)
    )
    .into_owned()
}

/// The daemon's connection messages can run on with retry details; a tile
/// has room for the first sentence.
/// The daemon's account of the headset connection, short enough for a
/// tile's one line: its first sentence, with a lost connection said as just
/// that, since its cause and the address it retries don't fit.
fn brief_state(status: &str) -> Cow<'_, str> {
    let first = first_sentence(status);
    if first.starts_with("Lost the connection") {
        t!("summary.lost_connection")
    } else if first.starts_with("Can't reach the headset") {
        t!("summary.cant_reach_headset")
    } else {
        first.into()
    }
}

pub fn first_sentence(text: &str) -> &str {
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
        .filter(|output| tracking || output.source == TongueSource::EnhancedModel)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_readiness_names_what_is_missing() {
        let mut status = tracking_status();
        assert_eq!(recording_cameras(None).value, "Tracking isn't running");
        assert_eq!(
            recording_cameras(Some(&status)).value,
            "Headset not streaming"
        );
        status.source = Some("192.168.1.20:40000".into());
        assert_eq!(recording_cameras(Some(&status)).tone, Tone::Waiting);
        status.frame_age_ms = Some(20);
        assert_eq!(recording_cameras(Some(&status)).value, "Live");
    }
    use crate::daemon::{EyeStatus, Mismatch, ModelStatus, OutputStatus};

    fn tracking_status() -> Status {
        Status {
            status: "Streaming".into(),
            ..Status::default()
        }
    }

    const LIVE: Rates = Rates {
        tracking_fps: Some(60.0),
        camera_fps: Some(24.0),
    };

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
            output: Some(OutputStatus {
                source: TongueSource::EnhancedModel,
                visible: Some(true),
                values: [0.8, 0.5, 0.0, 0.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                ..OutputStatus::default()
            }),
            ..Status::default()
        };
        let reading = tongue(&status, false);
        assert_eq!(reading.state, TongueState::Out);
        assert_eq!(reading.vertical, 0.5);
        assert_eq!(reading.horizontal, -0.25);
    }

    #[test]
    fn tongue_falls_back_to_the_model_without_output() {
        let status = tracking_status();
        let status = Status {
            model: Some(ModelStatus {
                fresh: true,
                threshold: 0.5,
                values: [0.4, 0.9, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                ..ModelStatus::default()
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
            output: Some(OutputStatus {
                source: TongueSource::TrackingModule,
                ..OutputStatus::default()
            }),
            ..Status::default()
        };
        assert_eq!(tongue(&status, false).state, TongueState::NotTracked);
        assert_eq!(tongue(&status, true).state, TongueState::In);
    }

    #[test]
    fn headset_says_which_side_to_update_when_protocols_differ() {
        let status = Status {
            headset_mismatch: Some(Mismatch {
                protocol: 4,
                apk_version: Some("2027.1.0".into()),
                update: Update::Vrft,
            }),
            ..Status::default()
        };
        let reading = headset(Some(&status));
        assert_eq!(reading.tone, Tone::Problem);
        assert_eq!(reading.value, "Update VRFaceTracking");
        assert_eq!(
            reading.detail,
            "Headset app 2027.1.0 is newer than this app supports"
        );
        let status = Status {
            headset_mismatch: Some(Mismatch {
                protocol: 2,
                apk_version: None,
                update: Update::HeadsetApp,
            }),
            ..Status::default()
        };
        assert_eq!(headset(Some(&status)).value, "Update headset app");
    }

    #[test]
    fn a_lost_headset_is_said_briefly() {
        let status = Status {
            status: "Lost the connection to the headset (Camera stream closed). Retrying 192.168.1.20:27274".into(),
            ..Status::default()
        };
        assert_eq!(headset(Some(&status)).detail, "Lost connection");
    }

    #[test]
    fn headset_detail_keeps_the_first_sentence() {
        let status = Status {
            status: "Looking for the headset on the network. Start the stream on it".into(),
            ..Status::default()
        };
        assert_eq!(
            headset(Some(&status)).detail,
            "Looking for the headset on the network"
        );
    }

    #[test]
    fn combined_gaze_is_only_a_problem_when_it_wasnt_chosen() {
        let mut status = Status {
            eyes: EyeStatus {
                fresh: true,
                ..Default::default()
            },
            ..Status::default()
        };
        assert_eq!(eyes(Some(&status)).tone, Tone::Waiting);
        assert_eq!(eyes(Some(&status)).fix, Some(Fix::Open(pages::HEADSET)));
        status.settings.eye_gaze = false;
        assert_eq!(eyes(Some(&status)).tone, Tone::Off);
        assert_eq!(across(3.04), "3.0° right");
        assert_eq!(across(-2.0), "2.0° left");
        assert_eq!(across(0.01), "straight ahead");
    }

    #[test]
    fn eyes_meet_at_the_vergence_distance() {
        // 7.2 degrees of vergence puts the crossing about half a metre away.
        assert_eq!(eyes_meet(3.6, -3.6), "About 0.50 m (+7.2°)");
        assert_eq!(eyes_meet(0.1, 0.0), "Far away (+0.1°)");
        assert_eq!(eyes_meet(-1.0, 1.0), "Not meeting (-2.0°)");
    }

    #[test]
    fn eyes_without_data_point_at_the_headset() {
        let reading = eyes(Some(&Status::default()));
        assert_eq!(reading.value, "Off");
        assert_eq!(reading.fix, Some(Fix::Open(pages::HEADSET)));
        let status = Status {
            eye_output_deg: Some([[1.0, 0.0], [-1.0, 0.0]]),
            eyes: EyeStatus {
                rate_hz: 90.0,
                ..Default::default()
            },
            ..Status::default()
        };
        let reading = eyes(Some(&status));
        assert_eq!(
            (reading.value.as_str(), reading.detail.as_str()),
            ("Per-eye", "90 Hz")
        );
    }
}
