use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STEP_SECONDS: f32 = 8.0;
const SETTLE_SECONDS: f32 = 4.0;
const TARGET_NAMES: [&str; 12] = [
    "visibility",
    "extension",
    "horizontal",
    "vertical",
    "curl_up",
    "bend_down",
    "roll",
    "flat",
    "squish",
    "twist",
    "cheek_puff_left",
    "cheek_puff_right",
];

#[derive(Clone, Copy)]
struct Pose {
    name: &'static str,
    instruction: &'static str,
    targets: [f32; 12],
}

const NEUTRAL: [f32; 12] = [0.0; 12];

/// A visible tongue with the given extension, horizontal and vertical
/// direction. Values between the extremes are graded prompts, as in the
/// reference's still-capture cards, so the model learns intermediate poses
/// rather than only endpoints.
const fn out(extension: f32, horizontal: f32, vertical: f32) -> [f32; 12] {
    [
        1.0, extension, horizontal, vertical, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ]
}

/// The tongue in, with the left and right cheeks puffed this much.
const fn puff(left: f32, right: f32) -> [f32; 12] {
    let mut targets = NEUTRAL;
    targets[10] = left;
    targets[11] = right;
    targets
}

const fn pose(name: &'static str, instruction: &'static str, targets: [f32; 12]) -> Pose {
    Pose {
        name,
        instruction,
        targets,
    }
}

/// More of the tongue's directions, halfway and diagonal, for the tongue
/// map training fits.
const DIRECTION_POSES: [Pose; 15] = [
    pose(
        "Relaxed, mouth slightly open",
        "Relax your tongue fully inside with your mouth slightly open.",
        NEUTRAL,
    ),
    pose(
        "Tongue half left",
        "Extend your tongue and point it halfway toward your left.",
        out(1.0, -0.5, 0.0),
    ),
    pose(
        "Tongue half right",
        "Extend your tongue and point it halfway toward your right.",
        out(1.0, 0.5, 0.0),
    ),
    pose(
        "Tongue half up",
        "Extend your tongue and tilt the tip halfway up.",
        out(1.0, 0.0, 0.5),
    ),
    pose(
        "Tongue half down",
        "Extend your tongue and tilt the tip halfway down.",
        out(1.0, 0.0, -0.5),
    ),
    pose(
        "Tongue up and left",
        "Extend your tongue diagonally toward your upper left.",
        out(1.0, -0.7, 0.7),
    ),
    pose(
        "Tongue up and right",
        "Extend your tongue diagonally toward your upper right.",
        out(1.0, 0.7, 0.7),
    ),
    pose(
        "Tongue down and left",
        "Extend your tongue diagonally toward your lower left.",
        out(1.0, -0.7, -0.7),
    ),
    pose(
        "Tongue down and right",
        "Extend your tongue diagonally toward your lower right.",
        out(1.0, 0.7, -0.7),
    ),
    pose(
        "Tongue left, jaw wide",
        "Open your jaw wide. Extend your tongue fully to your left and keep it visible.",
        out(1.0, -1.0, 0.0),
    ),
    pose(
        "Tongue right, jaw wide",
        "Keep your jaw wide. Extend your tongue fully to your right and keep it visible.",
        out(1.0, 1.0, 0.0),
    ),
    pose(
        "Tongue up, jaw wide",
        "Keep your jaw wide. Point the tip of your tongue toward your nose.",
        out(1.0, 0.0, 1.0),
    ),
    pose(
        "Tongue down, jaw wide",
        "Keep your jaw wide. Point the tip of your tongue toward your chin.",
        out(1.0, 0.0, -1.0),
    ),
    pose(
        "Jaw wide, no tongue",
        "Retract your tongue fully but keep your jaw wide open.",
        NEUTRAL,
    ),
    pose(
        "Final relaxed neutral",
        "Retract your tongue completely and relax your mouth.",
        NEUTRAL,
    ),
];

/// Follow the dot: the wearer tracks a dot gliding between these points with
/// the tongue, so every frame gets its own direction label. Diagonals stop at
/// 0.7 like the direction poses.
const FOLLOW_ROUNDS: usize = 12;
const FOLLOW_STOPS: usize = 3;
/// Time to put the tongue out before the dot moves; not saved.
const FOLLOW_READY: f32 = 1.5;
/// The dot rests at the centre this long before it moves and after it returns.
const FOLLOW_HOLD: f32 = 0.8;
const FOLLOW_MOVE: f32 = 1.0;
const FOLLOW_DWELL: f32 = 0.6;
/// Labels use the dot's position this long before each frame, since the
/// tongue trails the dot. `dot` in each sample keeps the unlagged position so
/// labels can be recomputed with another lag.
const FOLLOW_LAG: f32 = 0.35;
const FOLLOW_WAYPOINTS: [[f32; 2]; 16] = [
    [-1.0, 0.0],
    [1.0, 0.0],
    [0.0, 1.0],
    [0.0, -1.0],
    [-0.5, 0.0],
    [0.5, 0.0],
    [0.0, 0.5],
    [0.0, -0.5],
    [-0.7, 0.7],
    [0.7, 0.7],
    [-0.7, -0.7],
    [0.7, -0.7],
    [-0.35, 0.35],
    [0.35, 0.35],
    [-0.35, -0.35],
    [0.35, -0.35],
];
/// Tongue-in rests between rounds, recorded as extra hidden examples.
const FOLLOW_RESTS: [(&str, &str); 4] = [
    ("Tongue in, relax", "Tongue in. Relax your mouth."),
    ("Tongue in, talk", "Tongue in. Say a few words out loud."),
    ("Tongue in, smile", "Tongue in. Smile and show your teeth."),
    ("Tongue in, jaw open", "Tongue in. Open your mouth wide."),
];
const REST_SECONDS: f32 = 3.0;
const REST_SETTLE: f32 = 1.0;

const MODE_NAMES: &str = "face, direction, follow, or enrollment";

/// The face recording: QFT+'s own labelled prompts, the targets and
/// look-alikes of its short benchmark (`guided_session.py`), which personal
/// training fine-tunes the face model on. A relaxed face first, then each
/// target at three amounts with a rest after each, then look-alikes that
/// mustn't read as a target, and reading out loud. About five minutes.
const FACE_AMOUNTS: [(&str, f32); 3] = [("halfway", 0.6), ("most of the way", 0.8), ("full", 1.0)];
/// A target: `FACE_RAMP` seconds to get into it, then held, saved.
const FACE_HOLD: f32 = 2.5;
const FACE_RAMP: f32 = 1.0;
const FACE_REST: f32 = 1.5;
/// Frames a second each kind of step saves at most: near-identical
/// neighbours add disk, not information.
const TARGET_RATE: f32 = 12.0;
const LONG_RATE: f32 = 6.0;

/// What a face recording target asks for, at an amount.
#[derive(Clone, Copy)]
enum Target {
    /// Left and right cheeks.
    Puff(f32, f32),
    Suck,
    /// Horizontal and vertical, the tongue out.
    Tongue(f32, f32),
    /// The brows that move, and those that stay still; the rest go
    /// unlabelled, since not everyone can keep them still.
    Brows(&'static [&'static str], &'static [&'static str]),
}

const BROW_RAISES: [&str; 4] = [
    "brow_inner_up_left",
    "brow_inner_up_right",
    "brow_outer_up_left",
    "brow_outer_up_right",
];
const BROW_DOWN: [&str; 4] = [
    "brow_lowerer_left",
    "brow_lowerer_right",
    "brow_pinch_left",
    "brow_pinch_right",
];
const LIPS_SEALED: &str = "Fill with air and keep your lips sealed.";
const AS_YOU_FEEL_IT: &str = "Your left and right, as you feel it.";
const AS_FAR: &str = "Point it as far as feels comfortable.";
const ONE_BROW: &str = "Keep the other one still if you can. If you can't, just try.";
const FACE_TARGETS: [(&str, &str, Target); 16] = [
    ("Puff both cheeks", LIPS_SEALED, Target::Puff(1.0, 1.0)),
    ("Puff your left cheek", LIPS_SEALED, Target::Puff(1.0, 0.0)),
    ("Puff your right cheek", LIPS_SEALED, Target::Puff(0.0, 1.0)),
    (
        "Suck in your cheeks",
        "Pull both cheeks in between your teeth.",
        Target::Suck,
    ),
    ("Stick your tongue out", AS_FAR, Target::Tongue(0.0, 0.0)),
    (
        "Stick your tongue out, pointing left",
        AS_YOU_FEEL_IT,
        Target::Tongue(-1.0, 0.0),
    ),
    (
        "Stick your tongue out, pointing right",
        AS_YOU_FEEL_IT,
        Target::Tongue(1.0, 0.0),
    ),
    (
        "Stick your tongue out, pointing up",
        AS_FAR,
        Target::Tongue(0.0, 1.0),
    ),
    (
        "Stick your tongue out, pointing down",
        AS_FAR,
        Target::Tongue(0.0, -1.0),
    ),
    (
        "Stick your tongue out, pointing up and to the left",
        AS_YOU_FEEL_IT,
        Target::Tongue(-0.7, 0.7),
    ),
    (
        "Stick your tongue out, pointing up and to the right",
        AS_YOU_FEEL_IT,
        Target::Tongue(0.7, 0.7),
    ),
    (
        "Raise both eyebrows",
        "As if surprised.",
        Target::Brows(&BROW_RAISES, &BROW_DOWN),
    ),
    (
        "Raise only your left eyebrow",
        ONE_BROW,
        Target::Brows(&["brow_inner_up_left", "brow_outer_up_left"], &BROW_DOWN),
    ),
    (
        "Raise only your right eyebrow",
        ONE_BROW,
        Target::Brows(&["brow_inner_up_right", "brow_outer_up_right"], &BROW_DOWN),
    ),
    (
        "Look worried",
        "Lift the middle of your brows, as if worried or sad.",
        Target::Brows(&["brow_inner_up_left", "brow_inner_up_right"], &[]),
    ),
    (
        "Frown",
        "Pull your brows down and together, as if annoyed.",
        Target::Brows(&BROW_DOWN, &BROW_RAISES),
    ),
];
/// Look-alikes: the tongue in, the cheeks neither puffed nor sucked.
const FACE_LOOKALIKES: [(&str, &str); 7] = [
    (
        "Chew",
        "Chew slowly, as if you had gum, with your lips closed.",
    ),
    (
        "Push your tongue into your left cheek",
        "Lips closed; make a bump from the inside.",
    ),
    (
        "Push your tongue into your right cheek",
        "Lips closed; make a bump from the inside.",
    ),
    (
        "Suck your lips in",
        "Press both lips in between your teeth.",
    ),
    ("Pout", "Push your lips forward like a sulking child."),
    ("Smile and talk", "Big smiles, then a few words."),
    ("Squint", "As if looking into bright sun."),
];
const LOOKALIKE_SECONDS: f32 = 10.0;
const FACE_READING: [&str; 2] = [
    "The thick path through the thirty thin birch trees bent north, then south, then back again.",
    "Bob put the big map by the pump, but Pam mopped the mud before it dried on the mat.",
];
const READING_SECONDS: f32 = 15.0;
const RELAXED_SECONDS: f32 = 15.0;

impl Target {
    /// The tongue and cheek labels, and the face labels, at `amount`.
    fn labels(self, amount: f32) -> ([f32; 12], BTreeMap<&'static str, f32>) {
        let mut face: BTreeMap<&'static str, f32> =
            [("cheek_suck_left", 0.0), ("cheek_suck_right", 0.0)].into();
        let targets = match self {
            Target::Puff(left, right) => puff(left * amount, right * amount),
            Target::Suck => {
                face.insert("cheek_suck_left", amount);
                face.insert("cheek_suck_right", amount);
                NEUTRAL
            }
            Target::Tongue(horizontal, vertical) => out(amount, horizontal, vertical),
            Target::Brows(moving, still) => {
                face.extend(moving.iter().map(|&name| (name, amount)));
                face.extend(still.iter().map(|&name| (name, 0.0)));
                NEUTRAL
            }
        };
        (targets, face)
    }
}

/// The face recording's steps.
fn face_steps() -> Vec<Step> {
    let unpuffed: BTreeMap<&'static str, f32> =
        [("cheek_suck_left", 0.0), ("cheek_suck_right", 0.0)].into();
    let long = |name: &str, instruction: &str, seconds: f32, face: BTreeMap<_, _>| Step {
        name: name.into(),
        instruction: instruction.into(),
        seconds,
        settle: 1.0,
        targets: Some(NEUTRAL),
        path: None,
        anchor: None,
        face: Some(face),
        rate: Some(LONG_RATE),
    };
    let mut steps = vec![long(
        "Relax and look around",
        "Keep your face relaxed and still; move your eyes and head normally.",
        RELAXED_SECONDS,
        NEUTRAL_FACE.into_iter().collect(),
    )];
    for (title, instruction, target) in FACE_TARGETS {
        for (amount_name, amount) in FACE_AMOUNTS {
            let (targets, face) = target.labels(amount);
            steps.push(Step {
                name: format!("{title}, {amount_name}"),
                instruction: instruction.into(),
                seconds: FACE_HOLD,
                settle: FACE_RAMP,
                targets: Some(targets),
                path: None,
                anchor: None,
                face: Some(face),
                rate: Some(TARGET_RATE),
            });
            steps.push(relax_step(FACE_REST));
        }
    }
    for (name, instruction) in FACE_LOOKALIKES {
        steps.push(long(name, instruction, LOOKALIKE_SECONDS, unpuffed.clone()));
    }
    for text in FACE_READING {
        steps.push(long(
            "Read this out loud, again and again",
            text,
            READING_SECONDS,
            unpuffed.clone(),
        ));
    }
    steps
}

/// The one-minute face setup, QFT+'s own (`guided_session.py`): a neutral
/// face, then each pose the universal face model reads frames against,
/// held for [`ENROLL_HOLD`] seconds after [`ENROLL_RAMP`] to get into it,
/// with a short rest between. The tongue's directions follow straight out
/// without a rest, since it stays out; together they fit how the wearer's
/// tongue reads each way. A jaw sweep and some reading out loud end it,
/// with the tongue in. Every pose's saved frames are labelled with its
/// slot (`anchor`).
const ENROLL_RAMP: f32 = 1.0;
const ENROLL_HOLD: f32 = 2.0;
const ENROLL_REST: f32 = 1.0;
/// The setup's poses: name, instruction, slot, the tongue and cheek
/// labels, face labels, and whether a rest follows.
type EnrollPose = (
    &'static str,
    &'static str,
    Option<&'static str>,
    [f32; 12],
    &'static [(&'static str, f32)],
    bool,
);
const NEUTRAL_FACE: [(&str, f32); 11] = [
    ("jaw_open", 0.0),
    ("cheek_suck_left", 0.0),
    ("cheek_suck_right", 0.0),
    ("brow_inner_up_left", 0.0),
    ("brow_inner_up_right", 0.0),
    ("brow_outer_up_left", 0.0),
    ("brow_outer_up_right", 0.0),
    ("brow_lowerer_left", 0.0),
    ("brow_lowerer_right", 0.0),
    ("brow_pinch_left", 0.0),
    ("brow_pinch_right", 0.0),
];
const ENROLL_POSES: [EnrollPose; 12] = [
    (
        "Relax and look ahead",
        "Keep your face still and relaxed.",
        Some("neutral"),
        NEUTRAL,
        &NEUTRAL_FACE,
        false,
    ),
    (
        "Open your mouth wide",
        "As wide as is comfortable.",
        Some("jaw_open"),
        NEUTRAL,
        &[("jaw_open", 1.0)],
        true,
    ),
    (
        "Kiss",
        "Push your lips forward into a kiss.",
        Some("pucker"),
        NEUTRAL,
        &[],
        true,
    ),
    (
        "Puff both cheeks",
        "Fill with air, lips sealed.",
        Some("puff"),
        puff(1.0, 1.0),
        &[],
        true,
    ),
    (
        "Puff only your left cheek",
        "Move the air into your left cheek, lips sealed.",
        Some("puff_left"),
        puff(1.0, 0.0),
        &[],
        true,
    ),
    (
        "Puff only your right cheek",
        "Move the air into your right cheek, lips sealed.",
        Some("puff_right"),
        puff(0.0, 1.0),
        &[],
        true,
    ),
    (
        "Stick your tongue out",
        "Straight out, as far as is comfortable.",
        Some("tongue_out"),
        out(1.0, 0.0, 0.0),
        &[],
        false,
    ),
    (
        "Point your tongue up",
        "Keep it out, tip up toward your nose.",
        Some("tongue_up"),
        out(1.0, 0.0, 1.0),
        &[],
        false,
    ),
    (
        "Point your tongue down",
        "Keep it out, tip down toward your chin.",
        Some("tongue_down"),
        out(1.0, 0.0, -1.0),
        &[],
        false,
    ),
    (
        "Point your tongue left",
        "Keep it out. Your left, as you feel it.",
        Some("tongue_left"),
        out(1.0, -1.0, 0.0),
        &[],
        false,
    ),
    (
        "Point your tongue right",
        "Keep it out. Your right, as you feel it.",
        Some("tongue_right"),
        out(1.0, 1.0, 0.0),
        &[],
        true,
    ),
    (
        "Suck in your cheeks",
        "Pull both cheeks in between your teeth.",
        Some("suck"),
        NEUTRAL,
        &[("cheek_suck_left", 1.0), ("cheek_suck_right", 1.0)],
        true,
    ),
];

/// The face setup's last steps, after its poses: name, instruction and
/// seconds. Nothing is checked; the tongue stays in.
const ENROLL_ENDING: [(&str, &str, f32); 2] = [
    ("Jaw side to side", "Slowly, teeth apart.", 6.0),
    (
        "Read this out loud",
        "The thick path through the thirty thin birch trees bent north, then south, then back again.",
        9.0,
    ),
];

/// The face setup's steps, about a minute in all.
fn enrollment_steps() -> Vec<Step> {
    let mut steps = vec![];
    for (name, instruction, slot, targets, face, rest) in ENROLL_POSES {
        steps.push(Step {
            name: name.into(),
            instruction: instruction.into(),
            seconds: ENROLL_RAMP + ENROLL_HOLD,
            // The relaxed face is already held as it starts.
            settle: if slot == Some("neutral") {
                0.0
            } else {
                ENROLL_RAMP
            },
            targets: Some(targets),
            path: None,
            anchor: slot,
            face: (!face.is_empty()).then(|| face.iter().copied().collect()),
            rate: None,
        });
        if rest {
            steps.push(relax_step(ENROLL_REST));
        }
    }
    steps.extend(ENROLL_ENDING.map(|(name, instruction, seconds)| Step {
        name: name.into(),
        instruction: instruction.into(),
        seconds,
        settle: 0.0,
        targets: Some(NEUTRAL),
        path: None,
        anchor: None,
        face: None,
        rate: None,
    }));
    steps
}

/// A rest between poses: all settle, so nothing is saved.
fn relax_step(seconds: f32) -> Step {
    Step {
        name: "Relax".into(),
        instruction: "Let your face go loose.".into(),
        seconds,
        settle: seconds,
        targets: Some(NEUTRAL),
        path: None,
        anchor: None,
        face: None,
        rate: None,
    }
}

/// One prompt of a guided recording. Held poses have fixed `targets`;
/// follow-the-dot rounds have a `path` of `[seconds, horizontal, vertical]`
/// keyframes timed from the end of `settle`.
#[derive(Clone, Serialize)]
struct Step {
    name: String,
    instruction: String,
    seconds: f32,
    settle: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    targets: Option<[f32; 12]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<Vec<[f32; 3]>>,
    /// The face setup slot this step shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor: Option<&'static str>,
    /// Labels for the universal face model's other outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    face: Option<BTreeMap<&'static str, f32>>,
    /// Frames a second saved at most; every frame when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    rate: Option<f32>,
}

impl Step {
    fn held(pose: &Pose) -> Self {
        Self {
            name: pose.name.into(),
            instruction: pose.instruction.into(),
            seconds: STEP_SECONDS,
            settle: SETTLE_SECONDS,
            targets: Some(pose.targets),
            path: None,
            anchor: None,
            face: None,
            rate: None,
        }
    }

    /// Labels `seconds` after settling, and the unlagged dot position.
    fn label(&self, seconds: f32) -> ([f32; 12], Option<[f32; 2]>) {
        match &self.path {
            Some(path) => {
                let [horizontal, vertical] = path_position(path, (seconds - FOLLOW_LAG).max(0.0));
                (
                    out(1.0, horizontal, vertical),
                    Some(path_position(path, seconds)),
                )
            }
            None => (self.targets.unwrap_or(NEUTRAL), None),
        }
    }
}

/// Linear interpolation along dot keyframes, held at both ends.
fn path_position(path: &[[f32; 3]], seconds: f32) -> [f32; 2] {
    let Some(first) = path.first() else {
        return [0.0, 0.0];
    };
    if seconds <= first[0] {
        return [first[1], first[2]];
    }
    for pair in path.windows(2) {
        let ([t0, h0, v0], [t1, h1, v1]) = (pair[0], pair[1]);
        if seconds <= t1 {
            let blend = if t1 > t0 {
                (seconds - t0) / (t1 - t0)
            } else {
                1.0
            };
            return [h0 + (h1 - h0) * blend, v0 + (v1 - v0) * blend];
        }
    }
    let last = path[path.len() - 1];
    [last[1], last[2]]
}

/// Small xorshift generator; dot paths only need variety between sessions.
struct Shuffle(u64);

impl Shuffle {
    fn below(&mut self, count: usize) -> usize {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x % count as u64) as usize
    }
}

/// Rounds of dot-following, each visiting three waypoints from a shuffled
/// bag so every waypoint comes up about equally, then a tongue-in rest.
fn follow_steps(seed: u64) -> Vec<Step> {
    let mut random = Shuffle(seed | 1);
    let mut bag: Vec<[f32; 2]> = vec![];
    let mut steps = vec![];
    for round in 0..FOLLOW_ROUNDS {
        let mut time = FOLLOW_HOLD;
        let mut path = vec![[0.0, 0.0, 0.0], [time, 0.0, 0.0]];
        let mut previous = [0.0, 0.0];
        for _ in 0..FOLLOW_STOPS {
            if bag.is_empty() {
                bag = FOLLOW_WAYPOINTS.to_vec();
                for index in (1..bag.len()).rev() {
                    bag.swap(index, random.below(index + 1));
                }
            }
            if bag.len() > 1 && bag[bag.len() - 1] == previous {
                let last = bag.len() - 1;
                bag.swap(0, last);
            }
            let [horizontal, vertical] = bag.pop().unwrap();
            time += FOLLOW_MOVE;
            path.push([time, horizontal, vertical]);
            time += FOLLOW_DWELL;
            path.push([time, horizontal, vertical]);
            previous = [horizontal, vertical];
        }
        time += FOLLOW_MOVE;
        path.push([time, 0.0, 0.0]);
        time += FOLLOW_HOLD;
        path.push([time, 0.0, 0.0]);
        steps.push(Step {
            name: "Follow the dot".into(),
            instruction: "Tongue out straight, then follow the dot with the tip of your tongue."
                .into(),
            seconds: FOLLOW_READY + time,
            settle: FOLLOW_READY,
            targets: None,
            path: Some(path),
            anchor: None,
            face: None,
            rate: None,
        });
        let (name, instruction) = FOLLOW_RESTS[round % FOLLOW_RESTS.len()];
        steps.push(Step {
            name: name.into(),
            instruction: instruction.into(),
            seconds: REST_SECONDS,
            settle: REST_SETTLE,
            targets: Some(NEUTRAL),
            path: None,
            anchor: None,
            face: None,
            rate: None,
        });
    }
    steps
}

/// The static mode name and the steps of a guided recording. Recordings
/// from before the face recording, basic and expression poses, can't be
/// made any more.
fn steps_for(name: &str, seed: u64) -> Option<(&'static str, Vec<Step>)> {
    if name == "follow" {
        return Some(("follow", follow_steps(seed)));
    }
    match name {
        "face" => Some(("face", face_steps())),
        "enrollment" => Some(("enrollment", enrollment_steps())),
        "direction" => Some((
            "direction",
            DIRECTION_POSES.iter().map(Step::held).collect(),
        )),
        _ => None,
    }
}

use crate::face_check::{self, Hold, RETRY_BEFORE_SECONDS};
use vrft_quest_pro_protocol::{
    CameraLayout, CaptureMode, FacePoseCheck, FaceSetupReport, PauseReason,
};

/// Missing input pauses a recording; this long without it ends it.
const GIVE_UP_AFTER: Duration = Duration::from_secs(10);
/// Camera frames this far apart mean the cameras stopped.
const CAMERA_GAP: Duration = Duration::from_secs(2);
/// A tracking module TongueOut this old isn't saved beside a frame.
const NATIVE_FRESH_FOR: Duration = Duration::from_millis(200);
/// The headset's own values this old don't count in a face setup check, as
/// in QFT+ (`label_capture.FRESH_NS`).
const NATIVE_FACE_FRESH_FOR: Duration = Duration::from_millis(100);
pub use vrft_quest_pro_protocol::CaptureStatus;

#[derive(Serialize)]
struct Sample<'a> {
    index: u64,
    sequence: u64,
    pose: &'a str,
    step: usize,
    round: usize,
    targets: [f32; 12],
    native_tongue_out: Option<f32>,
    captured_unix_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    dot: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    face: Option<&'a BTreeMap<&'static str, f32>>,
}

struct Session {
    mode: &'static str,
    /// The cameras each saved frame holds: the mouth pair, or all five while
    /// the headset sends them.
    layout: CameraLayout,
    steps: Vec<Step>,
    started: Instant,
    directory: PathBuf,
    frames: BufWriter<File>,
    labels: BufWriter<File>,
    samples: u64,
    skipped_steps: HashSet<usize>,
    paused_at: Option<Instant>,
    /// Why the recording paused by itself, if it did.
    pause_reason: Option<PauseReason>,
    last_frame_at: Instant,
    /// Where a finished face setup is recorded as the one in use.
    enrollment_file: Option<PathBuf>,
    /// A face setup's checks of each held pose.
    checks: Option<FaceChecks>,
    /// Holds that failed their check, left out like skipped poses.
    failed_steps: HashSet<usize>,
    /// The step and the seconds into its hold of the last frame saved, for
    /// steps that save fewer.
    last_saved: Option<(usize, f32)>,
}

/// A face setup's checks, as its poses end.
#[derive(Default)]
struct FaceChecks {
    /// The hold being watched.
    hold: Option<Hold>,
    /// The steps before this one are checked.
    checked: usize,
    /// The relaxed face's blocks, once its hold passed.
    neutral: Option<Vec<f32>>,
    last_fingerprint: Option<u64>,
    /// Slots already asked for once more.
    retried: HashSet<&'static str>,
    /// Every hold checked, in order.
    attempts: Vec<FacePoseCheck>,
}

impl Session {
    fn total_seconds(&self) -> f32 {
        self.steps.iter().map(|step| step.seconds).sum()
    }

    /// The step at `elapsed` seconds and the seconds into it.
    fn locate(&self, elapsed: f32) -> Option<(usize, f32)> {
        let mut start = 0.0;
        for (index, step) in self.steps.iter().enumerate() {
            if elapsed < start + step.seconds {
                return Some((index, elapsed - start));
            }
            start += step.seconds;
        }
        None
    }
}

#[derive(Default)]
struct Inner {
    session: Option<Session>,
    module_loaded: bool,
    native: Option<(f32, Instant)>,
    /// The headset's own values a face setup checks its poses against.
    native_face: Option<(Vec<(&'static str, f32)>, Instant)>,
    last_directory: Option<PathBuf>,
    last_samples: u64,
    message: String,
    face_setup: Option<FaceSetupReport>,
}

fn captures_root() -> Result<PathBuf, String> {
    let root = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(root.join(".local/tongue-captures"))
}

/// Names the face setup in use: `{"recording": "<id>"}`, the id being its
/// folder under `.local/tongue-captures`.
pub const ENROLLMENT_FILE: &str = ".local/face-enrollment.json";

/// The face setup in use's recording id, as [`ENROLLMENT_FILE`] names it.
pub fn face_setup_in_use(root: &Path) -> Option<String> {
    let bytes = fs::read(root.join(ENROLLMENT_FILE)).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value["recording"].as_str().map(str::to_owned)
}

#[derive(Clone, Default)]
pub struct CaptureManager(Arc<Mutex<Inner>>);

impl CaptureManager {
    pub fn set_module_loaded(&self, loaded: bool) {
        self.0.lock().unwrap().module_loaded = loaded;
    }

    pub fn update_native(&self, value: f32) {
        let mut inner = self.0.lock().unwrap();
        if inner.module_loaded {
            inner.native = Some((value.clamp(0.0, 1.0), Instant::now()));
        }
    }

    /// The headset's own values a face setup's checks read, by Meta's names
    /// ([`face_check::NATIVE_PREFIXES`]).
    pub fn update_native_face(&self, values: Vec<(&'static str, f32)>) {
        let mut inner = self.0.lock().unwrap();
        if inner.module_loaded {
            inner.native_face = Some((values, Instant::now()));
        }
    }

    /// Starts a guided recording of `mode`, of only `poses` when there are
    /// any, saving the cameras in `layout`.
    pub fn start(
        &self,
        mode: &str,
        poses: &[String],
        layout: CameraLayout,
    ) -> Result<CaptureStatus, String> {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos() as u64;
        let Some((mode, mut steps)) = steps_for(mode, seed) else {
            return Err(format!("capture mode must be {MODE_NAMES}"));
        };
        if !poses.is_empty() {
            if mode == "follow" {
                return Err("Following the dot can't be recorded in parts".into());
            }
            let unknown: Vec<&str> = poses
                .iter()
                .filter(|pose| !steps.iter().any(|step| step.name == **pose))
                .map(String::as_str)
                .collect();
            if !unknown.is_empty() {
                return Err(format!("{mode} has no pose called {}", unknown.join(", ")));
            }
            steps.retain(|step| poses.contains(&step.name));
        }
        let mut inner = self.0.lock().unwrap();
        if inner.session.is_some() {
            return Err("a capture is already running".into());
        }
        let metadata = serde_json::json!({
            "stepSeconds": STEP_SECONDS, "settleSeconds": SETTLE_SECONDS, "rounds": 1,
            "followLagSeconds": FOLLOW_LAG, "steps": steps,
        });
        let (directory, frames, labels) =
            create_capture(&captures_root()?, mode, &layout, metadata)
                .map_err(|e| e.to_string())?;
        log::info!("Tongue capture keeps cameras {:?}", layout.cameras);
        inner.session = Some(Session {
            mode,
            layout,
            steps,
            started: Instant::now(),
            directory,
            frames,
            labels,
            samples: 0,
            skipped_steps: HashSet::new(),
            paused_at: None,
            pause_reason: None,
            last_frame_at: Instant::now(),
            enrollment_file: (mode == "enrollment")
                .then(|| {
                    std::env::current_dir()
                        .map(|root| root.join(ENROLLMENT_FILE))
                        .ok()
                })
                .flatten(),
            checks: (mode == "enrollment").then(FaceChecks::default),
            failed_steps: HashSet::new(),
            last_saved: None,
        });
        inner.message = "Recording started. Follow each prompt.".into();
        log::info!("Tongue capture started: mode={mode}");
        Ok(status_locked(&mut inner))
    }

    pub fn stop(&self) -> CaptureStatus {
        let mut inner = self.0.lock().unwrap();
        finish(
            &mut inner,
            "Recording stopped. Everything recorded so far is kept.",
            false,
        );
        status_locked(&mut inner)
    }

    /// Leaves the current pose out and moves straight on to the next, or
    /// ends the recording after the last.
    pub fn skip_current(&self) -> CaptureStatus {
        let mut inner = self.0.lock().unwrap();
        if let Some(session) = inner.session.as_mut() {
            let elapsed = session_elapsed(session).as_secs_f32();
            let last = session.steps.len() - 1;
            let (step, offset) = session
                .locate(elapsed)
                .unwrap_or((last, session.steps[last].seconds));
            session.skipped_steps.insert(step);
            // Start the next pose now. Frames already on their way land in
            // its settle time, which records nothing.
            let remaining = (session.steps[step].seconds - offset).max(0.0);
            if let Some(started) = session
                .started
                .checked_sub(Duration::from_secs_f32(remaining))
            {
                session.started = started;
            }
            let result = save_exclusions(session);
            let finished = step == last;
            inner.message = match result {
                Ok(()) if finished => {
                    log::info!("Tongue capture skipped the last pose step={step}");
                    finish(
                        &mut inner,
                        "Skipped the last pose. Recording complete.",
                        true,
                    );
                    return status_locked(&mut inner);
                }
                Ok(()) => format!(
                    "Skipped pose {}. None of its frames will be used.",
                    step + 1
                ),
                Err(error) => {
                    finish(
                        &mut inner,
                        &format!("Couldn't skip the pose ({error}). Delete this recording."),
                        false,
                    );
                    return status_locked(&mut inner);
                }
            };
            log::info!("Tongue capture skipped pose step={step}");
        }
        status_locked(&mut inner)
    }

    /// Pauses, or resumes when paused, including after pausing by itself.
    pub fn pause(&self) -> CaptureStatus {
        let mut inner = self.0.lock().unwrap();
        if let Some(session) = inner.session.as_mut() {
            if session.paused_at.is_some() {
                resume(session, false);
            } else {
                session.paused_at = Some(Instant::now());
            }
        }
        status_locked(&mut inner)
    }

    pub fn status(&self) -> CaptureStatus {
        let mut inner = self.0.lock().unwrap();
        if let Some(session) = inner.session.as_mut() {
            let elapsed = session_elapsed(session).as_secs_f32();
            check_holds(session, elapsed);
            if elapsed >= session.total_seconds() {
                finish(&mut inner, "Recording complete.", true);
            } else if session.paused_at.is_none() && session.last_frame_at.elapsed() > CAMERA_GAP {
                auto_pause(session, PauseReason::CamerasStopped);
            } else if let (Some(reason), Some(paused_at)) =
                (session.pause_reason, session.paused_at)
            {
                if paused_at.elapsed() > GIVE_UP_AFTER {
                    let why = match reason {
                        PauseReason::CamerasStopped => "the mouth cameras stopped sending",
                    };
                    finish(
                        &mut inner,
                        &format!(
                            "Recording stopped: {why} for {} seconds. Everything recorded so far is kept.",
                            GIVE_UP_AFTER.as_secs()
                        ),
                        false,
                    );
                }
            }
        }
        status_locked(&mut inner)
    }

    /// Saves a camera frame holding the cameras in `layout`. A recording of
    /// the mouth pair takes it from a five-camera frame; one of all five
    /// cameras skips mouth frames, and pauses by itself if only those come.
    pub fn record(
        &self,
        sequence: u64,
        received_at: Instant,
        layout: &CameraLayout,
        pixels: &[u8],
    ) {
        let mut inner = self.0.lock().unwrap();
        let native = inner
            .native
            .and_then(|(value, at)| (at.elapsed() <= NATIVE_FRESH_FOR).then_some(value));
        // Only a face setup's checks read them.
        let native_face = inner
            .native_face
            .as_ref()
            .filter(|(_, at)| at.elapsed() <= NATIVE_FACE_FRESH_FOR)
            .map(|(values, _)| values.clone());
        let Some(session) = inner.session.as_mut() else {
            return;
        };
        if pixels.len() != layout.frame_bytes() {
            return;
        }
        let pixels = if *layout == session.layout {
            std::borrow::Cow::Borrowed(pixels)
        } else {
            match layout.select(pixels, &session.layout.cameras) {
                Some(selected) if selected.len() == session.layout.frame_bytes() => {
                    std::borrow::Cow::Owned(selected)
                }
                _ => return,
            }
        };
        session.last_frame_at = received_at;
        // A camera frame means whatever paused the recording by itself has
        // come back.
        if session.pause_reason.is_some() {
            resume(session, true);
            return;
        }
        if session.paused_at.is_some() {
            return;
        }
        let elapsed = received_at
            .saturating_duration_since(session.started)
            .as_secs_f32();
        check_holds(session, elapsed);
        let Some((step, offset)) = session.locate(elapsed) else {
            finish(&mut inner, "Recording complete.", true);
            return;
        };
        let prompt = &session.steps[step];
        let holding = offset >= prompt.settle && !session.skipped_steps.contains(&step);
        if let Some(checks) = session.checks.as_mut() {
            let fingerprint = face_check::fingerprint(&pixels);
            let frozen = checks.last_fingerprint == Some(fingerprint);
            checks.last_fingerprint = Some(fingerprint);
            if holding && prompt.anchor.is_some() {
                checks.hold.get_or_insert_with(|| Hold::new(step)).observe(
                    &session.layout,
                    &pixels,
                    frozen,
                    native_face,
                );
            }
        }
        if !holding {
            return;
        }
        let held = offset - prompt.settle;
        if let Some(rate) = prompt.rate {
            if session
                .last_saved
                .is_some_and(|(saved, at)| saved == step && held - at < 1.0 / rate)
            {
                return;
            }
        }
        session.last_saved = Some((step, held));
        let (targets, dot) = prompt.label(held);
        let sample = Sample {
            index: session.samples,
            sequence,
            pose: &prompt.name,
            step,
            round: 1,
            targets,
            native_tongue_out: native,
            captured_unix_ms: unix_ms(received_at),
            dot,
            anchor: prompt.anchor,
            face: prompt.face.as_ref(),
        };
        let result = (|| -> std::io::Result<()> {
            session.frames.write_all(&pixels)?;
            serde_json::to_writer(&mut session.labels, &sample)?;
            session.labels.write_all(b"\n")?;
            session.samples += 1;
            Ok(())
        })();
        if let Err(error) = result {
            finish(
                &mut inner,
                &format!("Recording stopped: couldn't write to disk ({error})."),
                false,
            );
        } else if session.samples % 500 == 0 {
            log::info!("Tongue capture: {} frames saved", session.samples);
        }
    }
}

fn create_capture(
    root: &Path,
    mode: &str,
    layout: &CameraLayout,
    extra: serde_json::Value,
) -> std::io::Result<(PathBuf, BufWriter<File>, BufWriter<File>)> {
    fs::create_dir_all(root)?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let directory = root.join(format!("{millis}-{mode}-{}", std::process::id()));
    fs::create_dir(&directory)?;
    let frames = BufWriter::new(File::create(directory.join("frames.gray8"))?);
    let labels = BufWriter::new(File::create(directory.join("samples.jsonl"))?);
    // `cameras` says which views each frame holds side by side; readers from
    // before it see `width` and `bytesPerFrame` they don't support and leave
    // a five-camera recording alone.
    let mut metadata = serde_json::json!({
        "format": "vrft-tongue-capture-v1", "mode": mode, "targets": TARGET_NAMES,
    });
    for fields in [layout.metadata(), extra] {
        if let (Some(metadata), serde_json::Value::Object(fields)) =
            (metadata.as_object_mut(), fields)
        {
            metadata.extend(fields);
        }
    }
    fs::write(
        directory.join("metadata.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )?;
    Ok((directory, frames, labels))
}

fn unix_ms(at: Instant) -> u128 {
    SystemTime::now()
        .checked_sub(at.elapsed())
        .unwrap_or_else(SystemTime::now)
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Pauses by itself for `reason`, saving nothing until it comes back.
fn auto_pause(session: &mut Session, reason: PauseReason) {
    log::info!("Tongue capture paused by itself: {reason:?}");
    session.paused_at = Some(Instant::now());
    session.pause_reason = Some(reason);
}

/// Carries on after a pause. After pausing by itself, the interrupted pose
/// starts again from its beginning, with time to get back into it.
fn resume(session: &mut Session, from_pose_start: bool) {
    let Some(paused_at) = session.paused_at.take() else {
        return;
    };
    let automatic = session.pause_reason.take().is_some();
    session.started += paused_at.elapsed();
    session.last_frame_at = Instant::now();
    if from_pose_start || automatic {
        let elapsed = session_elapsed(session).as_secs_f32();
        if let Some((step, offset)) = session.locate(elapsed) {
            session.started += Duration::from_secs_f32(offset);
            if let Some(checks) = session.checks.as_mut() {
                checks.hold = checks.hold.take().filter(|hold| hold.step != step);
            }
        }
        log::info!("Tongue capture resumed from the start of the interrupted pose");
    }
}

fn session_elapsed(session: &Session) -> Duration {
    session
        .paused_at
        .unwrap_or_else(Instant::now)
        .saturating_duration_since(session.started)
}

/// Ends the recording. `completed`: it reached the end, rather than being
/// stopped part-way.
fn finish(inner: &mut Inner, message: &str, completed: bool) {
    if let Some(mut session) = inner.session.take() {
        if let Err(error) = session.frames.flush().and_then(|_| session.labels.flush()) {
            inner.message = format!("Recording stopped: saving failed ({error}).");
        } else {
            inner.message = format!("{message} {} frames saved.", session.samples);
        }
        if session.checks.is_some() {
            let report = finish_face_setup(&mut session, completed);
            inner.message = match &report.problem {
                Some(problem) => format!("{} {problem}", inner.message),
                None => {
                    let (passed, total) = report.passed();
                    format!(
                        "{} Face setup in use: {passed} of {total} poses passed.",
                        inner.message
                    )
                }
            };
            inner.face_setup = Some(report);
        }
        inner.last_directory = Some(session.directory);
        inner.last_samples = session.samples;
        log::info!("Tongue capture ended: {}", inner.message);
    }
}

fn status_locked(inner: &mut Inner) -> CaptureStatus {
    let native_recent = inner.module_loaded
        && inner
            .native
            .is_some_and(|(_, at)| at.elapsed() <= NATIVE_FRESH_FOR);
    if let Some(session) = &inner.session {
        let elapsed = session_elapsed(session).as_secs_f32();
        let last = session.steps.len() - 1;
        let (step, offset) = session
            .locate(elapsed)
            .unwrap_or((last, session.steps[last].seconds));
        let prompt = &session.steps[step];
        CaptureStatus {
            active: true,
            mode: CaptureMode::from_name(session.mode),
            pose: Some(prompt.name.clone()),
            instruction: Some(prompt.instruction.clone()),
            next_pose: session.steps.get(step + 1).map(|next| next.name.clone()),
            seconds_remaining: Some((prompt.seconds - offset).max(0.0)),
            step_seconds: prompt.seconds,
            settle_seconds: prompt.settle,
            path: prompt.path.clone(),
            path_elapsed: prompt.path.as_ref().map(|_| offset - prompt.settle),
            recording: session.paused_at.is_none() && offset >= prompt.settle,
            skipped: session.skipped_steps.contains(&step),
            paused: session.paused_at.is_some(),
            pause_reason: session.pause_reason,
            step: Some(step + 1),
            total_steps: Some(session.steps.len()),
            samples: session.samples,
            directory: Some(session.directory.display().to_string()),
            native_recent,
            message: inner.message.clone(),
            face_setup: None,
        }
    } else {
        CaptureStatus {
            active: false,
            mode: None,
            pose: None,
            instruction: None,
            next_pose: None,
            seconds_remaining: None,
            step_seconds: STEP_SECONDS,
            settle_seconds: SETTLE_SECONDS,
            path: None,
            path_elapsed: None,
            recording: false,
            skipped: false,
            paused: false,
            pause_reason: None,
            step: None,
            total_steps: None,
            samples: inner.last_samples,
            directory: inner
                .last_directory
                .as_ref()
                .map(|p| p.display().to_string()),
            native_recent,
            message: inner.message.clone(),
            face_setup: inner.face_setup.clone(),
        }
    }
}

/// Saves the poses left out: skipped, or failing their check.
fn save_exclusions(session: &Session) -> std::io::Result<()> {
    let excluded: std::collections::BTreeSet<usize> = session
        .skipped_steps
        .union(&session.failed_steps)
        .copied()
        .collect();
    fs::write(
        session.directory.join("excluded_steps.json"),
        serde_json::to_vec(&serde_json::json!({ "excluded_steps": excluded })).unwrap(),
    )
}

/// Rewrites the recording's steps, after asking for a pose once more.
fn save_steps(session: &Session) -> Result<(), String> {
    let path = session.directory.join("metadata.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    metadata["steps"] = serde_json::to_value(&session.steps).map_err(|e| e.to_string())?;
    fs::write(
        path,
        serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Checks each face setup hold that ended by `elapsed` seconds in, asking
/// once more for a pose that failed while there's time, as QFT+ does.
fn check_holds(session: &mut Session, elapsed: f32) {
    loop {
        let current = session
            .locate(elapsed)
            .map_or(session.steps.len(), |(step, _)| step);
        let Some(checks) = session.checks.as_mut() else {
            return;
        };
        if checks.checked >= current {
            return;
        }
        let step = checks.checked;
        checks.checked += 1;
        let prompt = &session.steps[step];
        let Some(slot) = prompt.anchor else {
            continue;
        };
        let hold = checks.hold.take().filter(|hold| hold.step == step);
        let retry = prompt.name.starts_with(ONCE_MORE);
        let pose = prompt.name.trim_start_matches(ONCE_MORE).to_string();
        if session.skipped_steps.contains(&step) {
            checks.attempts.push(FacePoseCheck {
                slot: slot.into(),
                pose,
                retried: retry,
                skipped: true,
                ..FacePoseCheck::default()
            });
            continue;
        }
        let verdict = hold
            .unwrap_or_else(|| Hold::new(step))
            .check(slot, checks.neutral.as_deref());
        log::info!(
            "Face setup check: {slot} {} ({} frames){}",
            if verdict.passed { "passed" } else { "failed" },
            verdict.frames,
            verdict
                .reason
                .map(|reason| format!(": {reason}"))
                .unwrap_or_default()
        );
        checks.attempts.push(FacePoseCheck {
            slot: slot.into(),
            pose,
            passed: verdict.passed,
            retried: retry,
            skipped: false,
            reason: verdict.reason.map(str::to_owned),
        });
        if verdict.passed {
            if verdict.mean_blocks.is_some() {
                checks.neutral = verdict.mean_blocks;
            }
            continue;
        }
        session.failed_steps.insert(step);
        if !checks.retried.contains(slot) && elapsed < RETRY_BEFORE_SECONDS {
            checks.retried.insert(slot);
            let mut again = prompt.clone();
            again.name = format!("{ONCE_MORE}{}", prompt.name);
            again.instruction = format!(
                "{} {}",
                verdict.reason.unwrap_or_default(),
                prompt.instruction
            );
            // Poses after it move along, and so do their skips.
            let at = step + 1;
            session
                .steps
                .splice(at..at, [again, relax_step(ENROLL_REST)]);
            for steps in [&mut session.skipped_steps, &mut session.failed_steps] {
                *steps = steps
                    .iter()
                    .map(|&index| if index >= at { index + 2 } else { index })
                    .collect();
            }
            if let Err(error) = save_steps(session) {
                log::warn!("Face setup: couldn't save the retried pose ({error})");
            }
        }
        if let Err(error) = save_exclusions(session) {
            log::warn!("Face setup: couldn't leave out a failed pose ({error})");
        }
    }
}

/// What a pose asked for once more is called.
const ONCE_MORE: &str = "Once more: ";

/// Sums up a face setup as it ends, and puts it in use when it reached the
/// end with a clean relaxed face, as QFT+ does. Otherwise the face setup
/// in use stays.
fn finish_face_setup(session: &mut Session, completed: bool) -> FaceSetupReport {
    let elapsed = if completed {
        f32::INFINITY
    } else {
        session_elapsed(session).as_secs_f32()
    };
    check_holds(session, elapsed);
    // Stopping after the last pose, in the jaw sweep or the reading, still
    // completes it.
    let completed = completed
        || session
            .steps
            .iter()
            .rposition(|step| step.anchor.is_some())
            .is_none_or(|last| session.checks.as_ref().is_some_and(|c| c.checked > last));
    let id = session
        .directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let checks = session.checks.as_ref().expect("a face setup");
    // Each slot as it ended: its passing attempt, else its last.
    let mut poses: Vec<FacePoseCheck> = Vec::new();
    for attempt in &checks.attempts {
        match poses.iter_mut().find(|pose| pose.slot == attempt.slot) {
            Some(pose) if pose.passed => {}
            Some(pose) => *pose = attempt.clone(),
            None => poses.push(attempt.clone()),
        }
    }
    let previous = session
        .enrollment_file
        .as_ref()
        .is_some_and(|file| file.is_file());
    let keeping = if previous {
        " Your previous face setup is still in use."
    } else {
        ""
    };
    let problem = if !completed {
        Some(format!(
            "The face setup was stopped before its last pose, so it isn't used.{keeping}"
        ))
    } else if checks.neutral.is_none() {
        Some(format!(
            "The relaxed face didn't record cleanly, so this face setup isn't used.{keeping} Run the face setup again."
        ))
    } else {
        None
    };
    let mut report = FaceSetupReport {
        recording: id.clone(),
        in_use: false,
        problem,
        poses,
    };
    if report.problem.is_none() {
        if let Some(file) = &session.enrollment_file {
            let written = fs::write(
                file,
                serde_json::to_vec_pretty(&serde_json::json!({"recording": id})).unwrap(),
            );
            match written {
                Ok(()) => {
                    log::info!("Face setup {id} is now in use");
                    report.in_use = true;
                }
                Err(error) => {
                    report.problem =
                        Some(format!("The face setup couldn't be put in use ({error})."))
                }
            }
        }
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&report) {
        if let Err(error) = fs::write(session.directory.join(FACE_SETUP_REPORT), bytes) {
            log::warn!("Face setup: couldn't save its report ({error})");
        }
    }
    report
}

/// A face setup recording's report of its checks.
pub const FACE_SETUP_REPORT: &str = "face_setup.json";

#[cfg(test)]
mod tests {
    use super::*;
    use vrft_quest_pro_protocol::{FRAME_BYTES, STRIP_BYTES};

    fn mouth() -> CameraLayout {
        CameraLayout::mouth()
    }

    fn test_directory() -> PathBuf {
        // Tests run in parallel, and Windows' coarse clock can give two of
        // them the same timestamp, so a counter keeps the names unique.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.local/tongue-tests-rust");
        fs::create_dir_all(&root).unwrap();
        let directory = root.join(format!(
            "{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        directory
    }

    fn manager_with(mode: &str) -> (CaptureManager, PathBuf) {
        let directory = test_directory();
        let manager = CaptureManager::default();
        let (mode, steps) = steps_for(mode, 7).unwrap();
        manager.0.lock().unwrap().session = Some(Session {
            mode,
            layout: CameraLayout::mouth(),
            started: Instant::now() - Duration::from_secs_f32(steps[0].settle),
            steps,
            directory: directory.clone(),
            frames: BufWriter::new(File::create(directory.join("frames.gray8")).unwrap()),
            labels: BufWriter::new(File::create(directory.join("samples.jsonl")).unwrap()),
            samples: 0,
            skipped_steps: HashSet::new(),
            paused_at: None,
            pause_reason: None,
            last_frame_at: Instant::now(),
            enrollment_file: None,
            checks: (mode == "enrollment").then(FaceChecks::default),
            failed_steps: HashSet::new(),
            last_saved: None,
        });
        manager.set_module_loaded(true);
        manager.update_native(0.);
        (manager, directory)
    }

    fn test_manager() -> (CaptureManager, PathBuf) {
        manager_with("direction")
    }

    fn remove_test_directory(directory: PathBuf) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../.local/tongue-tests-rust")
            .canonicalize()
            .unwrap();
        let target = directory.canonicalize().unwrap();
        assert_eq!(target.parent(), Some(root.as_path()));
        fs::remove_dir_all(target).unwrap();
    }

    fn saved_labels(directory: &Path) -> Vec<serde_json::Value> {
        fs::read_to_string(directory.join("samples.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn skipping_excludes_frames_recorded_before_the_click() {
        let (manager, directory) = test_manager();
        manager.record(1, Instant::now(), &mouth(), &vec![0; FRAME_BYTES]);
        assert_eq!(manager.status().samples, 1);
        manager.skip_current();
        manager.record(2, Instant::now(), &mouth(), &vec![0; FRAME_BYTES]);
        assert_eq!(manager.stop().samples, 1);
        let exclusions: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("excluded_steps.json")).unwrap())
                .unwrap();
        assert_eq!(exclusions["excluded_steps"], serde_json::json!([0]));
        remove_test_directory(directory);
    }

    #[test]
    fn skipping_moves_straight_on_and_ends_after_the_last_pose() {
        let (manager, directory) = test_manager();
        assert_eq!(manager.status().step, Some(1));
        let status = manager.skip_current();
        assert_eq!(status.step, Some(2));
        assert_eq!(status.pose.as_deref(), Some(DIRECTION_POSES[1].name));
        assert!(!status.skipped, "the next pose isn't skipped");
        let remaining = status.seconds_remaining.unwrap();
        assert!(remaining > STEP_SECONDS - 0.5, "{remaining}");

        let last = STEP_SECONDS * (DIRECTION_POSES.len() as f32 - 1.0) + 1.0;
        manager.0.lock().unwrap().session.as_mut().unwrap().started =
            Instant::now() - Duration::from_secs_f32(last);
        let status = manager.skip_current();
        assert!(!status.active);
        assert!(status.message.starts_with("Skipped the last pose"));
        remove_test_directory(directory);
    }

    #[test]
    fn skipping_while_paused_moves_on_and_stays_paused() {
        let (manager, directory) = test_manager();
        manager.pause();
        let status = manager.skip_current();
        assert!(status.paused);
        assert_eq!(status.step, Some(2));
        manager.stop();
        remove_test_directory(directory);
    }

    #[test]
    fn five_camera_recordings_keep_every_camera_and_mouth_ones_cut_the_pair() {
        let strip: Vec<u8> = (0..STRIP_BYTES).map(|i| ((i % 2000) / 400) as u8).collect();
        let (manager, directory) = test_manager();
        manager.record(1, Instant::now(), &CameraLayout::all(), &strip);
        manager.stop();
        let frames = fs::read(directory.join("frames.gray8")).unwrap();
        assert_eq!(
            frames.len(),
            FRAME_BYTES,
            "a mouth recording keeps the pair"
        );
        assert_eq!((frames[0], frames[400]), (2, 3));
        remove_test_directory(directory);

        let (manager, directory) = test_manager();
        manager.0.lock().unwrap().session.as_mut().unwrap().layout = CameraLayout::all();
        manager.record(1, Instant::now(), &mouth(), &vec![0; FRAME_BYTES]);
        assert_eq!(manager.status().samples, 0, "mouth frames lack the brow");
        manager.record(2, Instant::now(), &CameraLayout::all(), &strip);
        manager.stop();
        assert_eq!(
            fs::read(directory.join("frames.gray8")).unwrap(),
            strip,
            "a five-camera recording keeps the strip"
        );
        remove_test_directory(directory);
    }

    #[test]
    fn a_recordings_metadata_says_which_cameras_it_holds() {
        let root = test_directory();
        let (directory, ..) = create_capture(
            &root,
            "face",
            &CameraLayout::all(),
            serde_json::json!({"rounds": 1}),
        )
        .unwrap();
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("metadata.json")).unwrap()).unwrap();
        assert_eq!(metadata["cameras"], serde_json::json!([0, 1, 2, 3, 4]));
        assert_eq!(metadata["bytesPerFrame"], STRIP_BYTES);
        assert_eq!(metadata["rounds"], 1);
        assert_eq!(
            CameraLayout::from_metadata(&metadata).unwrap(),
            CameraLayout::all()
        );
        remove_test_directory(root);
    }

    #[test]
    fn the_face_setup_takes_about_a_minute_and_labels_its_slots() {
        let steps = enrollment_steps();
        let seconds: f32 = steps.iter().map(|step| step.seconds).sum();
        assert!((50.0..=70.0).contains(&seconds), "{seconds}");
        let slots: Vec<&str> = steps.iter().filter_map(|step| step.anchor).collect();
        for slot in [
            "neutral",
            "jaw_open",
            "pucker",
            "puff",
            "tongue_out",
            "suck",
        ] {
            assert!(slots.contains(&slot), "{slot}");
        }
        for slot in ["tongue_up", "tongue_down", "tongue_left", "tongue_right"] {
            assert!(slots.contains(&slot), "{slot}");
        }
        // Rests record nothing.
        assert!(steps
            .iter()
            .filter(|step| step.name == "Relax")
            .all(|step| step.settle >= step.seconds));
        // QFT+'s poses, and its jaw sweep and reading to end.
        assert_eq!(slots.len(), 12);
        let names: Vec<&str> = steps.iter().map(|step| step.name.as_str()).collect();
        assert_eq!(
            names[names.len() - 2..],
            ["Jaw side to side", "Read this out loud"]
        );

        let (manager, directory) = manager_with("enrollment");
        let pointer = directory.join("face-enrollment.json");
        manager
            .0
            .lock()
            .unwrap()
            .session
            .as_mut()
            .unwrap()
            .enrollment_file = Some(pointer.clone());
        manager.record(1, Instant::now(), &mouth(), &vec![0; FRAME_BYTES]);
        let status = manager.stop();
        let labels = saved_labels(&directory);
        assert_eq!(labels[0]["anchor"], "neutral");
        assert_eq!(labels[0]["face"]["jaw_open"], 0.0);
        // Stopped part-way, it isn't put in use.
        assert!(!pointer.exists());
        let report = status.face_setup.unwrap();
        assert!(!report.in_use);
        assert!(report
            .problem
            .unwrap()
            .contains("stopped before its last pose"));
        remove_test_directory(directory);
    }

    /// A face setup session `seconds` in, with its pointer in its folder.
    fn face_setup() -> (CaptureManager, PathBuf, PathBuf) {
        let (manager, directory) = manager_with("enrollment");
        let pointer = directory.join("face-enrollment.json");
        manager
            .0
            .lock()
            .unwrap()
            .session
            .as_mut()
            .unwrap()
            .enrollment_file = Some(pointer.clone());
        fs::write(directory.join("metadata.json"), br#"{"steps": []}"#).unwrap();
        (manager, directory, pointer)
    }

    fn at(manager: &CaptureManager, seconds: f32) {
        manager.0.lock().unwrap().session.as_mut().unwrap().started =
            Instant::now() - Duration::from_secs_f32(seconds);
    }

    /// A still face, its first pixel counting frames so none repeats.
    fn face(level: u8, sequence: u64) -> Vec<u8> {
        let mut pixels = vec![level; FRAME_BYTES];
        pixels[0] = sequence as u8;
        pixels
    }

    #[test]
    fn a_face_setup_pose_that_fails_its_check_is_asked_for_once_more() {
        let (manager, directory, pointer) = face_setup();
        // The relaxed face, held from the start to 3 s in.
        at(&manager, 2.0);
        for sequence in 0..8 {
            manager.record(sequence, Instant::now(), &mouth(), &face(100, sequence));
        }
        // The open mouth, 4 to 6 s in, looking just the same.
        at(&manager, 5.0);
        for sequence in 8..16 {
            manager.record(sequence, Instant::now(), &mouth(), &face(100, sequence));
        }
        at(&manager, 6.2);
        let status = manager.status();
        assert_eq!(
            status.pose.as_deref(),
            Some("Once more: Open your mouth wide")
        );
        let instruction = status.instruction.unwrap();
        assert!(
            instruction.starts_with("Your face looked the same as when relaxed."),
            "{instruction}"
        );
        let exclusions: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("excluded_steps.json")).unwrap())
                .unwrap();
        assert_eq!(exclusions["excluded_steps"], serde_json::json!([1]));
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("metadata.json")).unwrap()).unwrap();
        assert_eq!(
            metadata["steps"][2]["name"],
            "Once more: Open your mouth wide"
        );

        // To the end: the relaxed face passed, so it's put in use.
        at(&manager, 1000.0);
        let status = manager.status();
        assert!(!status.active);
        let report = status.face_setup.unwrap();
        assert!(report.in_use, "{:?}", report.problem);
        assert!(pointer.is_file());
        assert_eq!(report.poses[0].slot, "neutral");
        assert!(report.poses[0].passed);
        let jaw = &report.poses[1];
        assert_eq!(
            (
                jaw.slot.as_str(),
                jaw.pose.as_str(),
                jaw.passed,
                jaw.retried
            ),
            ("jaw_open", "Open your mouth wide", false, true)
        );
        assert_eq!(report.passed(), (1, 12));
        assert!(directory.join(FACE_SETUP_REPORT).is_file());
        assert!(status
            .message
            .contains("Face setup in use: 1 of 12 poses passed"));
        remove_test_directory(directory);
    }

    #[test]
    fn a_face_setup_stopped_in_its_reading_is_still_used() {
        let (manager, directory, pointer) = face_setup();
        at(&manager, 2.0);
        for sequence in 0..8 {
            manager.record(sequence, Instant::now(), &mouth(), &face(100, sequence));
        }
        // On past every pose, and any asked for once more, to the reading.
        let mut seconds = 3.0;
        while manager.status().pose.as_deref() != Some("Read this out loud") {
            assert!(seconds < 200.0);
            seconds += 0.5;
            at(&manager, seconds);
        }
        let report = manager.stop().face_setup.unwrap();
        assert!(report.in_use, "{:?}", report.problem);
        assert!(pointer.is_file());
        remove_test_directory(directory);
    }

    #[test]
    fn a_face_setup_without_a_clean_relaxed_face_keeps_the_one_in_use() {
        let (manager, directory, pointer) = face_setup();
        fs::write(&pointer, br#"{"recording":"before"}"#).unwrap();
        at(&manager, 1000.0);
        let report = manager.status().face_setup.unwrap();
        assert!(!report.in_use);
        let problem = report.problem.unwrap();
        assert!(
            problem.contains("relaxed face didn't record cleanly"),
            "{problem}"
        );
        assert!(
            problem.contains("previous face setup is still in use"),
            "{problem}"
        );
        assert_eq!(fs::read(&pointer).unwrap(), br#"{"recording":"before"}"#);
        remove_test_directory(directory);
    }

    #[test]
    fn a_recording_can_be_just_some_poses() {
        let manager = CaptureManager::default();
        assert!(manager
            .start("face", &["Not a pose".into()], mouth())
            .unwrap_err()
            .contains("no pose called Not a pose"));
        assert!(manager
            .start("follow", &["Tongue left".into()], mouth())
            .is_err());
        let (_, steps) = steps_for("face", 1).unwrap();
        let chosen = [
            "Puff your left cheek, full".to_string(),
            "Frown, halfway".to_string(),
        ];
        let kept: Vec<_> = steps
            .into_iter()
            .filter(|step| chosen.contains(&step.name))
            .map(|step| step.name)
            .collect();
        assert_eq!(kept, chosen);
    }

    #[test]
    fn status_announces_the_next_pose_until_the_last() {
        let (manager, directory) = test_manager();
        let status = manager.status();
        assert_eq!(status.pose.as_deref(), Some(DIRECTION_POSES[0].name));
        assert_eq!(status.next_pose.as_deref(), Some(DIRECTION_POSES[1].name));
        assert_eq!(
            (status.step_seconds, status.settle_seconds),
            (STEP_SECONDS, SETTLE_SECONDS)
        );
        assert!(status.path.is_none());
        let last = STEP_SECONDS * (DIRECTION_POSES.len() as f32 - 1.0) + 1.0;
        manager.0.lock().unwrap().session.as_mut().unwrap().started =
            Instant::now() - Duration::from_secs_f32(last);
        let status = manager.status();
        assert_eq!(
            status.pose.as_deref(),
            Some(DIRECTION_POSES.last().unwrap().name)
        );
        assert!(status.next_pose.is_none());
        manager.stop();
        remove_test_directory(directory);
    }

    #[test]
    fn paused_capture_saves_no_frames_and_preserves_pose_clock() {
        let (manager, directory) = test_manager();
        manager.pause();
        let before = manager.status();
        manager.record(1, Instant::now(), &mouth(), &vec![0; FRAME_BYTES]);
        let after = manager.status();
        assert!(after.paused);
        assert!(!after.recording);
        assert_eq!(after.samples, 0);
        assert_eq!(before.seconds_remaining, after.seconds_remaining);
        assert!(!manager.pause().paused);
        manager.record(2, Instant::now(), &mouth(), &vec![0; FRAME_BYTES]);
        assert_eq!(manager.stop().samples, 1);
        remove_test_directory(directory);
    }

    fn stall_cameras(manager: &CaptureManager) {
        manager
            .0
            .lock()
            .unwrap()
            .session
            .as_mut()
            .unwrap()
            .last_frame_at = Instant::now() - CAMERA_GAP - Duration::from_millis(100);
    }

    #[test]
    fn frames_record_without_a_tracking_module() {
        let (manager, directory) = test_manager();
        manager.0.lock().unwrap().session.as_mut().unwrap().started =
            Instant::now() - Duration::from_secs_f32(SETTLE_SECONDS + 1.0);
        manager.0.lock().unwrap().native = None;
        manager.record(1, Instant::now(), &mouth(), &vec![0; FRAME_BYTES]);
        let status = manager.status();
        assert!(!status.paused);
        assert!(!status.native_recent);
        assert_eq!(status.samples, 1);
        manager.stop();
        let labels = fs::read_to_string(directory.join("samples.jsonl")).unwrap();
        assert!(labels.contains("\"native_tongue_out\":null"), "{labels}");
        remove_test_directory(directory);
    }

    #[test]
    fn cameras_returning_resume_the_pose_from_its_start() {
        let (manager, directory) = test_manager();
        // Two seconds into the first pose's recording time.
        manager.0.lock().unwrap().session.as_mut().unwrap().started =
            Instant::now() - Duration::from_secs_f32(SETTLE_SECONDS + 2.0);
        stall_cameras(&manager);
        let status = manager.status();
        assert!(status.active && status.paused);
        assert_eq!(status.pause_reason, Some(PauseReason::CamerasStopped));

        manager.record(2, Instant::now(), &mouth(), &vec![0; FRAME_BYTES]);
        let status = manager.status();
        assert!(!status.paused);
        assert_eq!(status.pause_reason, None);
        assert_eq!(status.step, Some(1));
        assert!(
            !status.recording,
            "the pose starts over with time to settle"
        );
        let remaining = status.seconds_remaining.unwrap();
        assert!(remaining > STEP_SECONDS - 0.5, "{remaining}");
        manager.stop();
        remove_test_directory(directory);
    }

    #[test]
    fn stopped_cameras_pause_and_give_up_after_a_while() {
        let (manager, directory) = test_manager();
        stall_cameras(&manager);
        let status = manager.status();
        assert!(status.active && status.paused);
        assert_eq!(status.pause_reason, Some(PauseReason::CamerasStopped));

        manager
            .0
            .lock()
            .unwrap()
            .session
            .as_mut()
            .unwrap()
            .paused_at = Some(Instant::now() - GIVE_UP_AFTER - Duration::from_millis(100));
        let status = manager.status();
        assert!(!status.active);
        assert!(status.message.contains("mouth cameras stopped sending"));
        remove_test_directory(directory);
    }

    #[test]
    fn resuming_by_hand_after_an_automatic_pause_restarts_the_pose() {
        let (manager, directory) = test_manager();
        stall_cameras(&manager);
        assert!(manager.status().paused);
        let status = manager.pause();
        assert!(!status.paused);
        assert_eq!(status.pause_reason, None);
        manager.stop();
        remove_test_directory(directory);
    }

    /// The face recording's held steps: name, targets and face labels.
    fn face_holds() -> Vec<Step> {
        face_steps()
            .into_iter()
            .filter(|step| step.settle < step.seconds)
            .collect()
    }

    #[test]
    fn the_face_recording_is_qfts_short_benchmark() {
        let steps = face_steps();
        let seconds: f32 = steps.iter().map(|step| step.seconds).sum();
        assert!((270.0..=330.0).contains(&seconds), "{seconds}");
        let holds = face_holds();
        // Each target at three amounts, then the relaxed face, look-alikes
        // and reading.
        assert_eq!(holds.len(), FACE_TARGETS.len() * 3 + 1 + 7 + 2);
        for step in &holds {
            let face = step.face.as_ref().unwrap();
            let targets = step.targets.unwrap();
            // The cheeks are labelled throughout, with what's still unknown
            // left out rather than guessed.
            assert!(face.contains_key("cheek_suck_left"), "{}", step.name);
            assert!(targets[4..10].iter().all(|value| *value == 0.0));
            assert!(step.rate.is_some());
        }
        let find = |name: &str| holds.iter().find(|step| step.name == name).unwrap();
        let worried = find("Look worried, most of the way").face.clone().unwrap();
        assert_eq!(worried.get("brow_inner_up_left"), Some(&0.8));
        assert_eq!(worried.get("brow_outer_up_left"), None);
        let left = find("Raise only your left eyebrow, full")
            .face
            .clone()
            .unwrap();
        assert_eq!(left.get("brow_outer_up_left"), Some(&1.0));
        assert_eq!(left.get("brow_outer_up_right"), None, "not everyone can");
        assert_eq!(left.get("brow_pinch_right"), Some(&0.0));
        let tongue = find("Stick your tongue out, pointing up and to the left, halfway");
        assert_eq!(tongue.targets.unwrap()[..4], [1.0, 0.6, -0.7, 0.7]);
        let relaxed = find("Relax and look around").face.clone().unwrap();
        assert!(BROW_RAISES
            .iter()
            .chain(&BROW_DOWN)
            .all(|brow| relaxed[brow] == 0.0));
    }

    #[test]
    fn the_apps_top_ups_are_face_recording_poses() {
        let (_, steps) = steps_for("face", 1).unwrap();
        for (_, poses) in vrft_quest_pro_protocol::FACE_TOP_UPS {
            for pose in poses {
                assert!(steps.iter().any(|step| step.name == *pose), "{pose}");
            }
        }
    }

    #[test]
    fn puffs_are_one_cheek_at_a_time_and_bulges_are_not_puffs() {
        let holds = face_holds();
        let puffs = |left: f32, right: f32| {
            holds
                .iter()
                .filter(|step| {
                    let targets = step.targets.unwrap();
                    targets[10] == left && targets[11] == right
                })
                .inspect(|step| assert_eq!(step.targets.unwrap()[..10], [0.0; 10], "{}", step.name))
                .count()
        };
        assert_eq!(
            (puffs(1.0, 0.0), puffs(0.0, 1.0), puffs(1.0, 1.0)),
            (1, 1, 1)
        );
        let bulges: Vec<&Step> = holds
            .iter()
            .filter(|step| step.name.starts_with("Push your tongue into"))
            .collect();
        assert_eq!(bulges.len(), 2);
        for step in bulges {
            assert_eq!(step.targets.unwrap(), NEUTRAL);
            assert_eq!(step.face.as_ref().unwrap()["cheek_suck_left"], 0.0);
        }
    }

    #[test]
    fn the_face_recording_saves_at_most_its_rate() {
        let (manager, directory) = manager_with("face");
        let started = Instant::now() - Duration::from_secs_f32(2.0);
        manager.0.lock().unwrap().session.as_mut().unwrap().started = started;
        let mut saved = 0;
        for frame in 0..24u64 {
            let at = started + Duration::from_secs_f32(1.5 + frame as f32 / 24.0);
            manager.record(frame, at, &mouth(), &vec![0; FRAME_BYTES]);
            saved = manager.status().samples;
        }
        manager.stop();
        assert!((5..=7).contains(&saved), "{saved} frames in a second");
        remove_test_directory(directory);
    }

    #[test]
    fn prompts_are_graded_and_include_diagonals() {
        let extensions: HashSet<u32> = face_holds()
            .iter()
            .filter_map(|step| step.targets)
            .filter(|targets| targets[0] == 1.0)
            .map(|targets| (targets[1] * 100.0).round() as u32)
            .collect();
        assert_eq!(extensions, HashSet::from([60, 80, 100]));
        assert!(DIRECTION_POSES
            .iter()
            .any(|pose| pose.targets[2].abs() == 0.5 || pose.targets[3].abs() == 0.5));
        assert_eq!(
            DIRECTION_POSES
                .iter()
                .filter(|pose| pose.targets[2] != 0.0 && pose.targets[3] != 0.0)
                .count(),
            4
        );
    }

    #[test]
    fn unsupervisable_shapes_are_never_prompted() {
        for mode in ["face", "direction", "enrollment", "follow"] {
            let (_, steps) = steps_for(mode, 1).unwrap();
            for step in steps {
                let targets = step.label(0.0).0;
                assert!(
                    targets[4..10].iter().all(|value| *value == 0.0),
                    "{}",
                    step.name
                );
                assert!(targets.iter().all(|value| (-1.0..=1.0).contains(value)));
            }
        }
    }

    #[test]
    fn only_the_recordings_training_uses_can_be_made() {
        let (_, steps) = steps_for("direction", 1).unwrap();
        assert_eq!(steps.len(), DIRECTION_POSES.len());
        assert!(steps
            .iter()
            .all(|step| step.seconds == STEP_SECONDS && step.settle == SETTLE_SECONDS));
        for old in ["core", "negatives", "roll"] {
            assert!(steps_for(old, 1).is_none(), "{old}");
        }
    }

    #[test]
    fn follow_paths_cover_every_waypoint_and_start_and_end_centred() {
        let steps = follow_steps(12345);
        assert_eq!(steps.len(), FOLLOW_ROUNDS * 2);
        let mut visited = HashSet::new();
        for pair in steps.chunks(2) {
            let path = pair[0].path.as_ref().unwrap();
            assert_eq!(path_position(path, 0.0), [0.0, 0.0]);
            assert_eq!(path_position(path, f32::MAX), [0.0, 0.0]);
            assert!(path.windows(2).all(|keys| keys[1][0] >= keys[0][0]));
            let seconds = path.last().unwrap()[0];
            assert!((pair[0].seconds - FOLLOW_READY - seconds).abs() < 1e-4);
            let stops: Vec<[u32; 2]> = path
                .iter()
                .filter(|key| key[1] != 0.0 || key[2] != 0.0)
                .map(|key| [key[1].to_bits(), key[2].to_bits()])
                .collect();
            // Each stop is held, so it appears as two consecutive keyframes.
            assert_eq!(stops.len(), FOLLOW_STOPS * 2);
            assert!(stops
                .chunks(2)
                .zip(stops.chunks(2).skip(1))
                .all(|(a, b)| a[0] != b[0]));
            visited.extend(stops);
            assert_eq!(pair[1].targets, Some(NEUTRAL));
            assert!(pair[1].path.is_none());
        }
        assert_eq!(visited.len(), FOLLOW_WAYPOINTS.len());
        assert_ne!(
            follow_steps(1)[0].path,
            follow_steps(2)[0].path,
            "each session gets its own route"
        );
    }

    #[test]
    fn follow_labels_trail_the_dot_by_the_reaction_lag() {
        let path = vec![[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 1.0, 0.0]];
        let step = Step {
            name: "Follow the dot".into(),
            instruction: String::new(),
            seconds: 3.5,
            settle: 1.5,
            targets: None,
            path: Some(path),
            anchor: None,
            face: None,
            rate: None,
        };
        let (targets, dot) = step.label(0.5 + FOLLOW_LAG);
        assert!((targets[2] - 0.5).abs() < 1e-5);
        assert_eq!((targets[0], targets[1], targets[3]), (1.0, 1.0, 0.0));
        assert!((dot.unwrap()[0] - (0.5 + FOLLOW_LAG)).abs() < 1e-5);
        assert_eq!(step.label(0.1).0[2], 0.0, "lag never looks before the path");
    }

    #[test]
    fn follow_recording_saves_moving_labels_and_skips_getting_ready() {
        let (manager, directory) = manager_with("follow");
        let started = Instant::now() - Duration::from_secs_f32(FOLLOW_READY + FOLLOW_HOLD + 1.2);
        manager.0.lock().unwrap().session.as_mut().unwrap().started = started;
        let status = manager.status();
        assert_eq!(status.pose.as_deref(), Some("Follow the dot"));
        assert!(status.path.is_some());
        assert!((status.path_elapsed.unwrap() - (FOLLOW_HOLD + 1.2)).abs() < 0.1);
        manager.record(1, Instant::now(), &mouth(), &vec![0; FRAME_BYTES]);
        manager.record(
            2,
            started + Duration::from_secs_f32(0.5),
            &mouth(),
            &vec![0; FRAME_BYTES],
        );
        manager.stop();
        let labels = saved_labels(&directory);
        assert_eq!(labels.len(), 1, "frames while getting ready are not saved");
        let dot = labels[0]["dot"].as_array().unwrap();
        assert!(dot[0].as_f64().unwrap() != 0.0 || dot[1].as_f64().unwrap() != 0.0);
        assert_eq!(labels[0]["targets"][0], 1.0);
        remove_test_directory(directory);
    }

    #[test]
    fn follow_rests_are_saved_as_hidden_frames() {
        let (manager, directory) = manager_with("follow");
        let first = manager.0.lock().unwrap().session.as_ref().unwrap().steps[0].seconds;
        manager.0.lock().unwrap().session.as_mut().unwrap().started =
            Instant::now() - Duration::from_secs_f32(first + REST_SETTLE + 0.5);
        let status = manager.status();
        assert_eq!(status.pose.as_deref(), Some(FOLLOW_RESTS[0].0));
        assert!(status.path.is_none() && status.recording);
        manager.record(1, Instant::now(), &mouth(), &vec![0; FRAME_BYTES]);
        manager.stop();
        let labels = saved_labels(&directory);
        assert_eq!(labels[0]["targets"][0], 0.0);
        assert_eq!(labels[0]["step"], 1);
        remove_test_directory(directory);
    }
}
