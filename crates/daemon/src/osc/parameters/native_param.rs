//! Native tracking parameters that bypass VRChat's parameter system.
//! These send directly to /tracking/ endpoints and are only relevant when
//! the avatar doesn't already have equivalent parameters.

use super::base_param::matches_address;
use super::{ParamType, Parameter};
use rosc::{OscMessage, OscType};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vrft_common::UnifiedTrackingData;

/// How often to resend a native value that has not changed.
///
/// VRChat drops back to its own simulated eye behaviour if a `/tracking/eye`
/// endpoint goes quiet for 10 seconds. Delta suppression alone would trip that
/// whenever gaze holds still, and a module reporting a constant is ordinary --
/// `vd_module` emits a fixed pose for an eye it cannot see. Well under the
/// timeout, and two messages a second costs nothing.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);

/// Type alias for value getter function to reduce type complexity
type ValueGetterFn = Arc<dyn Fn(&UnifiedTrackingData) -> Vec<f32> + Send + Sync>;

/// Type alias for condition function to reduce type complexity
type ConditionFn = Arc<dyn Fn(&HashSet<String>) -> bool + Send + Sync>;

/// Converts the horizontal gaze component to a yaw angle in degrees, which is
/// what VRChat's /tracking/eye endpoints expect.
fn gaze_to_yaw_degrees(x: f32) -> f32 {
    x.atan().to_degrees()
}

/// Converts the vertical gaze component to a pitch angle in degrees. The sign
/// is inverted because VRChat's pitch axis runs opposite to the gaze vector.
fn gaze_to_pitch_degrees(y: f32) -> f32 {
    -y.atan().to_degrees()
}

/// Native parameter with conditional relevancy.
/// Only activates when the condition function returns true (e.g., when avatar lacks eye params).
pub struct NativeParameter {
    address: String,
    get_value: ValueGetterFn,
    /// Returns true if this native param should be relevant given the avatar's params
    condition: ConditionFn,
    relevant: bool,
    last_values: Option<Vec<f32>>,
    last_sent: Option<Instant>,
}

impl NativeParameter {
    /// Create a native float parameter
    pub fn new_float(
        address: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> f32 + Send + Sync + 'static,
        condition: impl Fn(&HashSet<String>) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            address: address.to_string(),
            get_value: Arc::new(move |d| vec![get_value(d)]),
            condition: Arc::new(condition),
            relevant: false,
            last_values: None,
            last_sent: None,
        }
    }

    /// Create a native Vector4 parameter (sends as 4 floats)
    pub fn new_vector4(
        address: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> [f32; 4] + Send + Sync + 'static,
        condition: impl Fn(&HashSet<String>) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            address: address.to_string(),
            get_value: Arc::new(move |d| get_value(d).to_vec()),
            condition: Arc::new(condition),
            relevant: false,
            last_values: None,
            last_sent: None,
        }
    }
}

impl Parameter for NativeParameter {
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        _param_types: &HashMap<String, ParamType>,
    ) -> usize {
        self.last_values = None;
        self.last_sent = None;

        // Check if condition is met (e.g., avatar lacks eye params)
        self.relevant = (self.condition)(avatar_params);

        if self.relevant {
            log::debug!("NativeParam '{}' is relevant", self.address);
        }

        if self.relevant {
            1
        } else {
            0
        }
    }

    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage> {
        self.process_at(data, Instant::now())
    }
}

impl NativeParameter {
    /// [`Parameter::process`] with the clock passed in, so the keepalive can be
    /// tested without waiting on a real second to elapse.
    fn process_at(&mut self, data: &UnifiedTrackingData, now: Instant) -> Vec<OscMessage> {
        if !self.relevant {
            return vec![];
        }

        let values = (self.get_value)(data);

        // Delta check
        let changed = match &self.last_values {
            Some(last) => values
                .iter()
                .zip(last.iter())
                .any(|(a, b)| (a - b).abs() > 0.00001),
            None => true,
        };

        // Resend unchanged values periodically, or VRChat times the endpoint
        // out and takes the eyes back.
        let stale = self
            .last_sent
            .is_none_or(|sent| now.duration_since(sent) >= KEEPALIVE_INTERVAL);

        if !changed && !stale {
            return vec![];
        }

        self.last_values = Some(values.clone());
        self.last_sent = Some(now);

        // Send as array of floats
        vec![OscMessage {
            addr: self.address.clone(),
            args: values.iter().map(|v| OscType::Float(*v)).collect(),
        }]
    }
}

/// The gaze parameters this application drives, from `legacy_eye.rs` and the
/// v2 block of `registry.rs`.
///
/// Membership in this list is the question that matters: the native endpoint
/// exists to cover avatars whose gaze we are *not* already driving, so an
/// avatar parameter we never send is not a reason to stay quiet.
const GAZE_PARAM_NAMES: &[&str] = &[
    "EyesX",
    "EyesY",
    "LeftEyeX",
    "LeftEyeY",
    "RightEyeX",
    "RightEyeY",
    "v2/EyeX",
    "v2/EyeY",
    "v2/EyeLeftX",
    "v2/EyeLeftY",
    "v2/EyeRightX",
    "v2/EyeRightY",
];

/// Checks whether the avatar carries any of the gaze parameters we drive.
///
/// Matched by name through [`matches_address`], the same way every other
/// parameter resolves an avatar address, so custom prefixes work here too.
///
/// This used to pattern-match instead, on "contains eye and ends with x or y".
/// The word "eyebrow" contains "eye", so an avatar exposing `EyebrowY` and no
/// gaze parameters at all looked like it had gaze covered, and the native
/// endpoint went silent -- eyes that never moved.
pub fn has_eye_xy_params(avatar_params: &HashSet<String>) -> bool {
    GAZE_PARAM_NAMES
        .iter()
        .any(|name| avatar_params.iter().any(|addr| matches_address(name, addr)))
}

/// Checks if avatar has any eye openness/lid parameters
pub fn has_eye_lid_params(avatar_params: &HashSet<String>) -> bool {
    avatar_params.iter().any(|p| {
        let lower = p.to_lowercase();
        lower.contains("eye") && (lower.contains("open") || lower.contains("lid"))
    })
}

/// Creates all native tracking parameters
pub fn create_native_parameters() -> Vec<Box<dyn Parameter>> {
    vec![
        // Vector4: Left Pitch/Yaw, Right Pitch/Yaw
        // Only relevant if avatar lacks EyeX/EyeY params
        Box::new(NativeParameter::new_vector4(
            "/tracking/eye/LeftRightPitchYaw",
            |d| {
                // Gaze convention: X = yaw (left-right), Y = pitch (up-down).
                // The endpoint takes angles in degrees, not the raw gaze vector.
                [
                    gaze_to_pitch_degrees(d.eye.left.gaze.y),
                    gaze_to_yaw_degrees(d.eye.left.gaze.x),
                    gaze_to_pitch_degrees(d.eye.right.gaze.y),
                    gaze_to_yaw_degrees(d.eye.right.gaze.x),
                ]
            },
            |params| !has_eye_xy_params(params),
        )),
        // Float: Combined eye closed amount
        // Only relevant if avatar lacks eye open/lid params
        Box::new(NativeParameter::new_float(
            "/tracking/eye/EyesClosedAmount",
            // The endpoint is specified as 0-1; a module reporting openness
            // outside that range must not push it out of spec.
            |d| (1.0 - (d.eye.left.openness + d.eye.right.openness) / 2.0).clamp(0.0, 1.0),
            |params| !has_eye_lid_params(params),
        )),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avatar(params: &[&str]) -> HashSet<String> {
        params
            .iter()
            .map(|p| format!("/avatar/parameters/{p}"))
            .collect()
    }

    #[test]
    fn gaze_parameters_are_detected() {
        for p in ["EyesX", "LeftEyeY", "v2/EyeLeftX", "FT/v2/EyeRightY"] {
            assert!(has_eye_xy_params(&avatar(&[p])), "{p} should count as gaze");
        }
    }

    /// The regression: "eyebrow" contains "eye", and the old pattern match read
    /// `EyebrowY` as gaze coverage, silencing the native endpoint on an avatar
    /// with no gaze parameters at all.
    #[test]
    fn eyebrow_parameter_is_not_mistaken_for_gaze() {
        assert!(!has_eye_xy_params(&avatar(&["EyebrowY", "JawOpen"])));
    }

    #[test]
    fn unrelated_eye_parameters_are_not_mistaken_for_gaze() {
        for p in ["v2/EyeSquintLeft", "EyeLidLeft", "v2/EyeWideRight"] {
            assert!(
                !has_eye_xy_params(&avatar(&[p])),
                "{p} is not a gaze parameter"
            );
        }
    }

    #[test]
    fn avatar_without_eye_parameters_gets_the_native_endpoint() {
        assert!(!has_eye_xy_params(&avatar(&["JawOpen", "MouthSmile"])));
    }

    fn probe_param() -> NativeParameter {
        let mut p = NativeParameter::new_float(
            "/tracking/eye/Probe",
            |d| d.eye.left.gaze.x,
            |params| !has_eye_xy_params(params),
        );
        p.reset(&avatar(&["JawOpen"]), &HashMap::new());
        p
    }

    /// VRChat reverts to simulated eyes if the endpoint goes quiet for 10s, so
    /// an unchanging gaze still has to be resent.
    #[test]
    fn unchanged_values_are_resent_before_vrchat_times_out() {
        let mut p = probe_param();
        let data = UnifiedTrackingData::default();

        let start = Instant::now();
        assert_eq!(p.process_at(&data, start).len(), 1, "initial send");

        // Quiet between keepalives, so we are not shouting every frame.
        let mid = start + KEEPALIVE_INTERVAL / 2;
        assert!(p.process_at(&data, mid).is_empty());

        // ...but it must speak again well inside VRChat's 10 second timeout.
        let due = start + KEEPALIVE_INTERVAL;
        assert_eq!(p.process_at(&data, due).len(), 1, "keepalive");

        assert!(
            KEEPALIVE_INTERVAL < Duration::from_secs(10),
            "keepalive must beat the documented timeout"
        );
    }

    #[test]
    fn a_change_still_sends_immediately() {
        let mut p = probe_param();
        let mut data = UnifiedTrackingData::default();

        let start = Instant::now();
        p.process_at(&data, start);

        data.eye.left.gaze.x = 0.5;
        assert_eq!(
            p.process_at(&data, start).len(),
            1,
            "a moved eye must not wait for the keepalive"
        );
    }

    #[test]
    fn eyes_closed_amount_stays_in_range() {
        let mut data = UnifiedTrackingData::default();
        // A module reporting out-of-range openness must not push us out of spec.
        data.eye.left.openness = 1.8;
        data.eye.right.openness = 1.8;

        let messages: Vec<_> = create_native_parameters()
            .iter_mut()
            .flat_map(|p| {
                p.reset(&avatar(&["JawOpen"]), &HashMap::new());
                p.process(&data)
            })
            .filter(|m| m.addr.ends_with("EyesClosedAmount"))
            .collect();

        let [message] = messages.as_slice() else {
            panic!("expected exactly one EyesClosedAmount message, got {messages:?}")
        };
        let OscType::Float(v) = message.args[0] else {
            panic!("expected a float")
        };
        assert!((0.0..=1.0).contains(&v), "out of spec: {v}");
    }

    #[test]
    fn forward_gaze_is_zero_degrees() {
        assert_eq!(gaze_to_yaw_degrees(0.0), 0.0);
        assert_eq!(gaze_to_pitch_degrees(0.0), 0.0);
    }

    #[test]
    fn gaze_converts_to_degrees_not_raw_units() {
        // tan(30 deg) as the horizontal component must come back as 30 degrees.
        let x = 30.0_f32.to_radians().tan();
        assert!((gaze_to_yaw_degrees(x) - 30.0).abs() < 1e-3);
    }

    #[test]
    fn pitch_is_inverted_relative_to_the_gaze_component() {
        let y = 20.0_f32.to_radians().tan();
        assert!((gaze_to_pitch_degrees(y) + 20.0).abs() < 1e-3);
    }

    #[test]
    fn extreme_gaze_stays_within_a_quarter_turn() {
        // atan is bounded, so no input can produce an out-of-range angle.
        for v in [f32::MAX, -f32::MAX, 1e12, -1e12] {
            assert!(gaze_to_yaw_degrees(v).abs() <= 90.0);
            assert!(gaze_to_pitch_degrees(v).abs() <= 90.0);
        }
    }
}
