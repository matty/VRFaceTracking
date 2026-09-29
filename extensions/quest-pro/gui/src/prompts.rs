//! What to say aloud as a guided recording moves on, so someone wearing the
//! headset can follow it without the screen. Short cues, because each new
//! one cuts off the last: the pose's name when it starts, "Now" when its
//! frames start counting, and a word when it pauses, resumes or skips.
use crate::daemon::{CaptureMode, CaptureStatus, PauseReason};
use rust_i18n::t;

/// A pose that ends with more than this left was skipped.
const SKIPPED_WITH_SECONDS_LEFT: f32 = 1.0;

/// The cue for the change from `before` to `now`, if there is one.
pub fn cue(before: Option<&CaptureStatus>, now: &CaptureStatus) -> Option<String> {
    if !now.active {
        return None;
    }
    let pose = now.pose.clone().unwrap_or_default();
    let Some(before) = before.filter(|before| before.active && before.directory == now.directory)
    else {
        // A new recording: its first pose. Following the dot needs the
        // screen, which someone in the headset may not expect.
        if now.mode == Some(CaptureMode::Follow) {
            return Some(t!("prompts.watch_screen", pose = pose).into());
        }
        return Some(pose);
    };
    if now.paused && !before.paused {
        return Some(match now.pause_reason {
            Some(reason) => pause_message(reason).into(),
            None => t!("prompts.paused").into(),
        });
    }
    let step_changed = now.step != before.step;
    let skipped = step_changed
        && before
            .seconds_remaining
            .is_some_and(|left| left > SKIPPED_WITH_SECONDS_LEFT);
    if before.paused && !now.paused {
        // After pausing by itself, the pose starts over, so name it again.
        let again = before.pause_reason.is_some() || step_changed;
        return Some(if again {
            t!("prompts.resuming_pose", pose = pose).into()
        } else {
            t!("prompts.resuming").into()
        });
    }
    if step_changed {
        return Some(if skipped {
            t!("prompts.skipped_pose", pose = pose).into()
        } else {
            pose
        });
    }
    if now.recording && !before.recording && !now.paused {
        return Some(t!("prompts.now").into());
    }
    None
}

/// What a pause is shown and said as, in the app's language. The
/// protocol's own `PauseReason::message` stays English for the daemon.
pub fn pause_message(reason: PauseReason) -> std::borrow::Cow<'static, str> {
    match reason {
        PauseReason::CamerasStopped => t!("prompts.paused_cameras_stopped"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::PauseReason;

    fn status(step: usize, pose: &str) -> CaptureStatus {
        CaptureStatus {
            active: true,
            pose: Some(pose.into()),
            step: Some(step),
            seconds_remaining: Some(8.0),
            directory: Some("rec".into()),
            ..CaptureStatus::default()
        }
    }

    #[test]
    fn a_pose_is_named_then_counted_in() {
        let first = status(1, "Neutral");
        assert_eq!(cue(None, &first).as_deref(), Some("Neutral"));
        assert_eq!(cue(Some(&first), &first), None, "no repeats");
        let recording = CaptureStatus {
            recording: true,
            seconds_remaining: Some(3.9),
            ..first.clone()
        };
        assert_eq!(cue(Some(&first), &recording).as_deref(), Some("Now"));
        let ending = CaptureStatus {
            seconds_remaining: Some(0.1),
            ..recording.clone()
        };
        let next = status(2, "Tongue tip");
        assert_eq!(cue(Some(&ending), &next).as_deref(), Some("Tongue tip"));
    }

    #[test]
    fn skipping_pausing_and_resuming_are_said() {
        let first = status(1, "Neutral");
        let next = status(2, "Tongue tip");
        assert_eq!(
            cue(Some(&first), &next).as_deref(),
            Some("Skipped. Tongue tip")
        );

        let paused = CaptureStatus {
            paused: true,
            ..first.clone()
        };
        assert_eq!(cue(Some(&first), &paused).as_deref(), Some("Paused"));
        assert_eq!(cue(Some(&paused), &first).as_deref(), Some("Resuming"));

        let lost = CaptureStatus {
            paused: true,
            pause_reason: Some(PauseReason::CamerasStopped),
            ..first.clone()
        };
        assert_eq!(
            cue(Some(&first), &lost).as_deref(),
            Some("Paused: the mouth cameras stopped")
        );
        assert_eq!(
            cue(Some(&lost), &first).as_deref(),
            Some("Resuming. Neutral")
        );
    }

    #[test]
    fn a_new_recording_starts_with_its_first_pose() {
        let old = status(5, "Tongue up");
        let new = CaptureStatus {
            directory: Some("other".into()),
            ..status(1, "Neutral")
        };
        assert_eq!(cue(Some(&old), &new).as_deref(), Some("Neutral"));
        let finished = CaptureStatus::default();
        assert_eq!(cue(Some(&old), &finished), None);
        let follow = CaptureStatus {
            mode: Some(CaptureMode::Follow),
            ..status(1, "Follow the dot")
        };
        assert!(cue(None, &follow).unwrap().contains("Watch the screen"));
    }
}
