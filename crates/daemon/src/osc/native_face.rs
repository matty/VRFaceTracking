//! VRChat's native face tracking input: one OSC blob holding every Unified
//! Expressions key, one byte each, at [`ADDRESS`].
//!
//! It drives avatars set up in the SDK's Face Tracking section with "Use All
//! Tracking Keys", which VRChat reports at [`AVATAR_FULL_FACE_TRACKING`]. VRChat
//! networks these values itself, so they spend none of the avatar's synced
//! parameters.
//!
//! Spec: <https://creators.vrchat.com/avatars/face-tracking/osc>

use super::parameters::native_param::KEEPALIVE_INTERVAL;
use rosc::{OscMessage, OscType};
use std::time::Instant;
use vrft_common::{UnifiedExpressions, UnifiedTrackingData};

pub const ADDRESS: &str = "/tracking/face/v1";

/// Where VRChat says whether the avatar has Face Tracking with Use All Tracking
/// Keys on. Sent after `/avatar/change` and also exposed through OSCQuery.
pub const AVATAR_FULL_FACE_TRACKING: &str = "/avatar/avatarfullfacetracking";

/// VRChat ignores a message whose payload is any other size.
pub const PAYLOAD_LEN: usize = 102;

/// The eye keys that lead the payload, derived from [`UnifiedEyeData`] because
/// they are not expression shapes. Everything after them is
/// [`UnifiedExpressions`] in declaration order, from `EyeSquintRight` on.
///
/// [`UnifiedEyeData`]: vrft_common::UnifiedEyeData
const EYE_KEYS: usize = 14;

const _: () = assert!(EYE_KEYS + UnifiedExpressions::Max as usize == PAYLOAD_LEN);

/// VRChat decodes each byte as `byte / 255.0`.
fn to_byte(value: f32) -> u8 {
    // A NaN survives the clamp and then casts to 0.
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// `value` as one of a pair of opposed keys: the part above 0 and the part
/// below it, each as a positive amount.
fn split(value: f32) -> (f32, f32) {
    (value.max(0.0), (-value).max(0.0))
}

/// The complete payload for `data`.
pub fn payload(data: &UnifiedTrackingData) -> [u8; PAYLOAD_LEN] {
    let (left, right) = (&data.eye.left, &data.eye.right);

    // The same gaze values as `v2/EyeLeftX` and friends. X is positive to the
    // wearer's right, which is outward for the right eye and inward for the
    // left; Y is positive upward.
    let (out_right, in_right) = split(right.gaze.x);
    let (in_left, out_left) = split(left.gaze.x);
    let (up_right, down_right) = split(right.gaze.y);
    let (up_left, down_left) = split(left.gaze.y);

    // `dilation` is neutral at 0.5, including when the module gives no range,
    // so an unknown pupil size sets neither key.
    let (dilate, constrict) = split(data.eye.dilation() * 2.0 - 1.0);

    let eye_keys: [f32; EYE_KEYS] = [
        out_right,
        in_right,
        up_right,
        down_right,
        out_left,
        in_left,
        up_left,
        down_left,
        1.0 - right.openness,
        1.0 - left.openness,
        dilate,
        dilate,
        constrict,
        constrict,
    ];

    let mut payload = [0; PAYLOAD_LEN];
    let (eyes, shapes) = payload.split_at_mut(EYE_KEYS);
    for (byte, value) in eyes.iter_mut().zip(eye_keys) {
        *byte = to_byte(value);
    }
    // Keys the module doesn't reach stay 0, as the spec asks.
    for (byte, shape) in shapes.iter_mut().zip(&data.shapes) {
        *byte = to_byte(shape.weight);
    }
    payload
}

fn message(payload: [u8; PAYLOAD_LEN]) -> OscMessage {
    OscMessage {
        addr: ADDRESS.to_string(),
        args: vec![OscType::Blob(payload.to_vec())],
    }
}

/// Every key at rest: eyes open and looking ahead, face relaxed.
pub fn neutral_message() -> OscMessage {
    message([0; PAYLOAD_LEN])
}

/// Decides which frames become a message.
#[derive(Default)]
pub struct NativeFaceSender {
    last: Option<([u8; PAYLOAD_LEN], Instant)>,
}

impl NativeFaceSender {
    /// The message to send for `data` at `now`, if any: when the payload
    /// changes, and at the keepalive interval when it doesn't. VRChat documents
    /// no timeout for this endpoint, so this holds it the way the
    /// `/tracking/eye` endpoints are held. Nothing while not `active`.
    pub fn next(
        &mut self,
        data: &UnifiedTrackingData,
        active: bool,
        now: Instant,
    ) -> Option<OscMessage> {
        if !active {
            // The next avatar with face tracking gets a message straight away.
            self.last = None;
            return None;
        }
        let payload = payload(data);
        let due = self.last.is_none_or(|(sent, at)| {
            sent != payload || now.duration_since(at) >= KEEPALIVE_INTERVAL
        });
        if !due {
            return None;
        }
        self.last = Some((payload, now));
        Some(message(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use UnifiedExpressions as E;

    /// The key table from VRChat's OSC Face Tracking page, in payload order.
    #[rustfmt::skip]
    const SPEC_KEYS: [&str; PAYLOAD_LEN] = [
        "EyeLookOutRight", "EyeLookInRight", "EyeLookUpRight", "EyeLookDownRight",
        "EyeLookOutLeft", "EyeLookInLeft", "EyeLookUpLeft", "EyeLookDownLeft",
        "EyeClosedRight", "EyeClosedLeft", "EyeDilationRight", "EyeDilationLeft",
        "EyeConstrictRight", "EyeConstrictLeft", "EyeSquintRight", "EyeSquintLeft",
        "EyeWideRight", "EyeWideLeft", "BrowPinchRight", "BrowPinchLeft",
        "BrowLowererRight", "BrowLowererLeft", "BrowInnerUpRight", "BrowInnerUpLeft",
        "BrowOuterUpRight", "BrowOuterUpLeft", "NasalDilationRight", "NasalDilationLeft",
        "NasalConstrictRight", "NasalConstrictLeft", "CheekSquintRight", "CheekSquintLeft",
        "CheekPuffRight", "CheekPuffLeft", "CheekSuckRight", "CheekSuckLeft",
        "JawOpen", "JawRight", "JawLeft", "JawForward", "JawBackward", "JawClench",
        "JawMandibleRaise", "MouthClosed", "LipSuckUpperRight", "LipSuckUpperLeft",
        "LipSuckLowerRight", "LipSuckLowerLeft", "LipSuckCornerRight", "LipSuckCornerLeft",
        "LipFunnelUpperRight", "LipFunnelUpperLeft", "LipFunnelLowerRight", "LipFunnelLowerLeft",
        "LipPuckerUpperRight", "LipPuckerUpperLeft", "LipPuckerLowerRight", "LipPuckerLowerLeft",
        "MouthUpperUpRight", "MouthUpperUpLeft", "MouthUpperDeepenRight", "MouthUpperDeepenLeft",
        "NoseSneerRight", "NoseSneerLeft", "MouthLowerDownRight", "MouthLowerDownLeft",
        "MouthUpperRight", "MouthUpperLeft", "MouthLowerRight", "MouthLowerLeft",
        "MouthCornerPullRight", "MouthCornerPullLeft", "MouthCornerSlantRight", "MouthCornerSlantLeft",
        "MouthFrownRight", "MouthFrownLeft", "MouthStretchRight", "MouthStretchLeft",
        "MouthDimpleRight", "MouthDimpleLeft", "MouthRaiserUpper", "MouthRaiserLower",
        "MouthPressRight", "MouthPressLeft", "MouthTightenerRight", "MouthTightenerLeft",
        "TongueOut", "TongueUp", "TongueDown", "TongueRight", "TongueLeft", "TongueRoll",
        "TongueBendDown", "TongueCurlUp", "TongueSquish", "TongueFlat", "TongueTwistRight",
        "TongueTwistLeft", "SoftPalateClose", "ThroatSwallow", "NeckFlexRight", "NeckFlexLeft",
    ];

    fn key(name: &str) -> usize {
        SPEC_KEYS.iter().position(|k| *k == name).unwrap()
    }

    /// Open eyes looking ahead, so anything non-zero is what the test set.
    fn neutral() -> UnifiedTrackingData {
        let mut data = UnifiedTrackingData::default();
        data.eye.left.openness = 1.0;
        data.eye.right.openness = 1.0;
        data
    }

    /// Guards the assumption the encoder rests on: every expression's name
    /// sits at its enum position after the eye keys. Adding or reordering an
    /// expression, or a change to VRChat's table, fails here by name.
    #[test]
    fn every_expression_lands_on_its_key_in_the_spec() {
        for index in 0..E::Max as usize {
            let expression = E::try_from(index).unwrap();
            let name = format!("{expression:?}");
            assert_eq!(SPEC_KEYS[EYE_KEYS + index], name);

            let mut data = neutral();
            *data.weight_mut(expression).unwrap() = 1.0;
            let payload = payload(&data);
            assert_eq!(payload[key(&name)], 255, "{name}");
            assert_eq!(payload.iter().filter(|b| **b != 0).count(), 1, "{name}");
        }
    }

    /// The worked example on VRChat's page.
    #[test]
    fn jaw_open_is_byte_36() {
        let mut data = neutral();
        *data.weight_mut(E::JawOpen).unwrap() = 1.0;
        let mut expected = [0; PAYLOAD_LEN];
        expected[36] = 255;
        assert_eq!(payload(&data), expected);
    }

    #[test]
    fn looking_right_is_out_for_the_right_eye_and_in_for_the_left() {
        let mut data = neutral();
        data.eye.right.gaze.x = 0.5;
        data.eye.left.gaze.x = 0.25;
        let payload = payload(&data);
        assert_eq!(payload[key("EyeLookOutRight")], 128);
        assert_eq!(payload[key("EyeLookInLeft")], 64);
        assert_eq!(payload[key("EyeLookInRight")], 0);
        assert_eq!(payload[key("EyeLookOutLeft")], 0);
    }

    #[test]
    fn looking_left_is_in_for_the_right_eye_and_out_for_the_left() {
        let mut data = neutral();
        data.eye.right.gaze.x = -0.5;
        data.eye.left.gaze.x = -0.25;
        let payload = payload(&data);
        assert_eq!(payload[key("EyeLookInRight")], 128);
        assert_eq!(payload[key("EyeLookOutLeft")], 64);
        assert_eq!(payload[key("EyeLookOutRight")], 0);
        assert_eq!(payload[key("EyeLookInLeft")], 0);
    }

    #[test]
    fn gaze_y_is_positive_upward() {
        let mut data = neutral();
        data.eye.right.gaze.y = 1.0;
        data.eye.left.gaze.y = -1.0;
        let payload = payload(&data);
        assert_eq!(payload[key("EyeLookUpRight")], 255);
        assert_eq!(payload[key("EyeLookDownLeft")], 255);
        assert_eq!(payload[key("EyeLookDownRight")], 0);
        assert_eq!(payload[key("EyeLookUpLeft")], 0);
    }

    #[test]
    fn each_eye_closes_on_its_own() {
        let mut data = neutral();
        data.eye.right.openness = 0.0;
        let payload = payload(&data);
        assert_eq!(payload[key("EyeClosedRight")], 255);
        assert_eq!(payload[key("EyeClosedLeft")], 0);
    }

    #[test]
    fn pupils_split_either_side_of_the_midpoint_of_their_range() {
        let mut data = neutral();
        (data.eye.min_dilation, data.eye.max_dilation) = (2.0, 8.0);

        data.eye.left.pupil_diameter_mm = 8.0;
        data.eye.right.pupil_diameter_mm = 8.0;
        let dilated = payload(&data);
        assert_eq!(dilated[key("EyeDilationLeft")], 255);
        assert_eq!(dilated[key("EyeDilationRight")], 255);
        assert_eq!(dilated[key("EyeConstrictLeft")], 0);

        data.eye.left.pupil_diameter_mm = 2.0;
        data.eye.right.pupil_diameter_mm = 2.0;
        let constricted = payload(&data);
        assert_eq!(constricted[key("EyeConstrictLeft")], 255);
        assert_eq!(constricted[key("EyeConstrictRight")], 255);
        assert_eq!(constricted[key("EyeDilationLeft")], 0);
    }

    #[test]
    fn pupils_without_a_range_set_neither_key() {
        assert_eq!(payload(&neutral()), [0; PAYLOAD_LEN]);
    }

    #[test]
    fn out_of_range_and_nan_values_stay_in_a_byte() {
        assert_eq!(to_byte(1.8), 255);
        assert_eq!(to_byte(-0.5), 0);
        assert_eq!(to_byte(f32::NAN), 0);
        assert_eq!(to_byte(f32::INFINITY), 255);
        assert_eq!(to_byte(0.5), 128);
    }

    /// A module with too few shapes is still a full payload, or VRChat would
    /// ignore the message.
    #[test]
    fn short_shape_lists_still_make_a_full_payload() {
        let mut data = neutral();
        data.shapes.truncate(1);
        data.shapes[0].weight = 1.0;
        let payload = payload(&data);
        assert_eq!(payload.len(), PAYLOAD_LEN);
        assert_eq!(payload[key("EyeSquintRight")], 255);
        assert_eq!(payload[key("NeckFlexLeft")], 0);
    }

    #[test]
    fn the_message_is_one_blob_of_the_payload() {
        let mut sender = NativeFaceSender::default();
        let message = sender.next(&neutral(), true, Instant::now()).unwrap();
        assert_eq!(message.addr, "/tracking/face/v1");
        assert_eq!(message.args, vec![OscType::Blob(vec![0; PAYLOAD_LEN])]);
    }

    #[test]
    fn unchanged_frames_wait_for_the_keepalive() {
        let mut sender = NativeFaceSender::default();
        let data = neutral();
        let start = Instant::now();
        assert!(sender.next(&data, true, start).is_some());
        assert!(sender
            .next(&data, true, start + KEEPALIVE_INTERVAL / 2)
            .is_none());
        assert!(sender
            .next(&data, true, start + KEEPALIVE_INTERVAL)
            .is_some());
    }

    #[test]
    fn a_change_sends_straight_away() {
        let mut sender = NativeFaceSender::default();
        let mut data = neutral();
        let start = Instant::now();
        sender.next(&data, true, start);
        *data.weight_mut(E::JawOpen).unwrap() = 1.0;
        assert!(sender.next(&data, true, start).is_some());
    }

    /// A change too small to move a byte is the same message.
    #[test]
    fn a_change_below_one_byte_is_not_a_change() {
        let mut sender = NativeFaceSender::default();
        let mut data = neutral();
        let start = Instant::now();
        sender.next(&data, true, start);
        *data.weight_mut(E::JawOpen).unwrap() = 0.001;
        assert!(sender
            .next(&data, true, start + Duration::from_millis(1))
            .is_none());
    }

    #[test]
    fn nothing_is_sent_while_inactive_and_the_next_active_frame_sends() {
        let mut sender = NativeFaceSender::default();
        let data = neutral();
        let start = Instant::now();
        assert!(sender.next(&data, false, start).is_none());
        assert!(sender.next(&data, true, start).is_some());
        assert!(sender.next(&data, false, start).is_none());
        assert!(sender.next(&data, true, start).is_some());
    }
}
