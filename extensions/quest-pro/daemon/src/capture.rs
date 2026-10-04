use serde::Serialize;
use std::collections::HashSet;
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

const CORE_POSES: [Pose; 17] = [
    pose(
        "Neutral",
        "Keep the tongue fully inside and relax your mouth.",
        NEUTRAL,
    ),
    pose(
        "Natural speech",
        "Speak normally while keeping the tongue inside.",
        NEUTRAL,
    ),
    pose(
        "Smile, no tongue",
        "Smile and show teeth with the tongue inside.",
        NEUTRAL,
    ),
    pose(
        "Jaw open, no tongue",
        "Open your mouth wide with the tongue inside.",
        NEUTRAL,
    ),
    pose(
        "Tongue tip",
        "Show just the tip of your tongue past your lips and hold it still.",
        out(0.25, 0.0, 0.0),
    ),
    pose(
        "Tongue half out",
        "Extend your tongue about halfway and hold it.",
        out(0.5, 0.0, 0.0),
    ),
    pose(
        "Tongue straight out",
        "Extend your tongue fully straight out and hold it.",
        out(1.0, 0.0, 0.0),
    ),
    pose(
        "Tongue left",
        "Extend your tongue toward your left and hold it.",
        out(1.0, -1.0, 0.0),
    ),
    pose(
        "Tongue right",
        "Extend your tongue toward your right and hold it.",
        out(1.0, 1.0, 0.0),
    ),
    pose(
        "Tongue up",
        "Extend your tongue upward and hold it.",
        out(1.0, 0.0, 1.0),
    ),
    pose(
        "Tongue down",
        "Extend your tongue downward and hold it.",
        out(1.0, 0.0, -1.0),
    ),
    pose(
        "Tongue in cheek, hidden",
        "Push your tongue into the inside of your left cheek, lips closed.",
        NEUTRAL,
    ),
    pose(
        "Pucker, no tongue",
        "Pucker your lips with the tongue inside.",
        NEUTRAL,
    ),
    pose(
        "Left cheek puffed",
        "Puff out only your left cheek, lips closed.",
        puff(1.0, 0.0),
    ),
    pose(
        "Right cheek puffed",
        "Puff out only your right cheek, lips closed.",
        puff(0.0, 1.0),
    ),
    pose(
        "Both cheeks puffed",
        "Puff out both cheeks, lips closed.",
        puff(1.0, 1.0),
    ),
    pose(
        "Final neutral",
        "Retract your tongue completely and relax.",
        NEUTRAL,
    ),
];

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

/// Tongue-hidden poses that commonly trigger false detections, each paired
/// with a matching visible pose, as in the reference correction set.
const NEGATIVE_POSES: [Pose; 13] = [
    pose(
        "Slight smile, no tongue",
        "Give a slight, relaxed smile with the tongue inside.",
        NEUTRAL,
    ),
    pose(
        "Lower teeth showing",
        "Pull your lower lip down to show your lower teeth, tongue inside.",
        NEUTRAL,
    ),
    pose(
        "Vowels, no tongue",
        "Slowly say ee, ah, oh, repeating, with the tongue inside.",
        NEUTRAL,
    ),
    pose(
        "Cheeks puffed",
        "Puff out both cheeks with your lips closed.",
        puff(1.0, 1.0),
    ),
    pose(
        "Cheeks sucked in",
        "Suck in your cheeks with the tongue inside.",
        NEUTRAL,
    ),
    pose(
        "Tongue in right cheek",
        "Push your tongue into the inside of your right cheek, lips closed.",
        NEUTRAL,
    ),
    pose("Lips pressed", "Press your lips firmly together.", NEUTRAL),
    pose(
        "Chin tucked, no tongue",
        "Tuck your chin down toward your chest with the tongue inside.",
        NEUTRAL,
    ),
    pose(
        "Tongue out, smiling",
        "Smile and extend your tongue straight out.",
        out(1.0, 0.0, 0.0),
    ),
    pose(
        "Tongue tip, jaw wide",
        "Open your jaw wide and show only the tip of your tongue.",
        out(0.25, 0.0, 0.0),
    ),
    pose(
        "Tongue out, chin tucked",
        "Keep your chin tucked and extend your tongue straight out.",
        out(1.0, 0.0, 0.0),
    ),
    pose(
        "Tongue three-quarters out",
        "Extend your tongue about three-quarters of the way and hold it.",
        out(0.75, 0.0, 0.0),
    ),
    pose(
        "Final neutral",
        "Retract your tongue completely and relax.",
        NEUTRAL,
    ),
];

const MODES: [(&str, &[Pose]); 3] = [
    ("core", &CORE_POSES),
    ("direction", &DIRECTION_POSES),
    ("negatives", &NEGATIVE_POSES),
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

const MODE_NAMES: &str = "core, direction, negatives, or follow";

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
        });
        let (name, instruction) = FOLLOW_RESTS[round % FOLLOW_RESTS.len()];
        steps.push(Step {
            name: name.into(),
            instruction: instruction.into(),
            seconds: REST_SECONDS,
            settle: REST_SETTLE,
            targets: Some(NEUTRAL),
            path: None,
        });
    }
    steps
}

/// The static mode name and the steps of a guided recording.
fn steps_for(name: &str, seed: u64) -> Option<(&'static str, Vec<Step>)> {
    if name == "follow" {
        return Some(("follow", follow_steps(seed)));
    }
    MODES
        .into_iter()
        .find(|(mode, _)| *mode == name)
        .map(|(mode, poses)| (mode, poses.iter().map(Step::held).collect()))
}

use vrft_quest_pro_protocol::{CameraLayout, CaptureMode, PauseReason};

/// Missing input pauses a recording; this long without it ends it.
const GIVE_UP_AFTER: Duration = Duration::from_secs(10);
/// Camera frames this far apart mean the cameras stopped.
const CAMERA_GAP: Duration = Duration::from_secs(2);
/// A tracking module TongueOut this old isn't saved beside a frame.
const NATIVE_FRESH_FOR: Duration = Duration::from_millis(200);
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
    last_directory: Option<PathBuf>,
    last_samples: u64,
    message: String,
}

fn captures_root() -> Result<PathBuf, String> {
    let root = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(root.join(".local/tongue-captures"))
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
            let result = fs::write(
                session.directory.join("excluded_steps.json"),
                serde_json::to_vec(&serde_json::json!({"excluded_steps": session.skipped_steps}))
                    .unwrap(),
            );
            let finished = step == last;
            inner.message = match result {
                Ok(()) if finished => {
                    log::info!("Tongue capture skipped the last pose step={step}");
                    finish(&mut inner, "Skipped the last pose. Recording complete.");
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
            if session_elapsed(session).as_secs_f32() >= session.total_seconds() {
                finish(&mut inner, "Recording complete.");
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
        let Some((step, offset)) = session.locate(elapsed) else {
            finish(&mut inner, "Recording complete.");
            return;
        };
        let prompt = &session.steps[step];
        if offset < prompt.settle || session.skipped_steps.contains(&step) {
            return;
        }
        let (targets, dot) = prompt.label(offset - prompt.settle);
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
        if let Some((_, offset)) = session.locate(elapsed) {
            session.started += Duration::from_secs_f32(offset);
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

fn finish(inner: &mut Inner, message: &str) {
    if let Some(mut session) = inner.session.take() {
        if let Err(error) = session.frames.flush().and_then(|_| session.labels.flush()) {
            inner.message = format!("Recording stopped: saving failed ({error}).");
        } else {
            inner.message = format!("{message} {} frames saved.", session.samples);
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
        }
    }
}

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
        });
        manager.set_module_loaded(true);
        manager.update_native(0.);
        (manager, directory)
    }

    fn test_manager() -> (CaptureManager, PathBuf) {
        manager_with("core")
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
        assert_eq!(status.pose.as_deref(), Some(CORE_POSES[1].name));
        assert!(!status.skipped, "the next pose isn't skipped");
        let remaining = status.seconds_remaining.unwrap();
        assert!(remaining > STEP_SECONDS - 0.5, "{remaining}");

        let last = STEP_SECONDS * (CORE_POSES.len() as f32 - 1.0) + 1.0;
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
            "core",
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
    fn a_recording_can_be_just_some_poses() {
        let manager = CaptureManager::default();
        assert!(manager
            .start("core", &["Not a pose".into()], mouth())
            .unwrap_err()
            .contains("no pose called Not a pose"));
        assert!(manager
            .start("follow", &["Tongue left".into()], mouth())
            .is_err());
        let (_, steps) = steps_for("core", 1).unwrap();
        let chosen = ["Tongue left".to_string(), "Tongue up".to_string()];
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
        assert_eq!(status.pose.as_deref(), Some(CORE_POSES[0].name));
        assert_eq!(status.next_pose.as_deref(), Some(CORE_POSES[1].name));
        assert_eq!(
            (status.step_seconds, status.settle_seconds),
            (STEP_SECONDS, SETTLE_SECONDS)
        );
        assert!(status.path.is_none());
        let last = STEP_SECONDS * (CORE_POSES.len() as f32 - 1.0) + 1.0;
        manager.0.lock().unwrap().session.as_mut().unwrap().started =
            Instant::now() - Duration::from_secs_f32(last);
        let status = manager.status();
        assert_eq!(
            status.pose.as_deref(),
            Some(CORE_POSES.last().unwrap().name)
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

    #[test]
    fn capture_modes_have_negatives_and_directions() {
        let hidden = |poses: &[Pose]| poses.iter().filter(|pose| pose.targets[0] == 0.0).count();
        assert!(hidden(&CORE_POSES) >= 6);
        for column in [2, 3] {
            assert!(CORE_POSES.iter().any(|pose| pose.targets[column] < 0.0));
            assert!(CORE_POSES.iter().any(|pose| pose.targets[column] > 0.0));
        }
        assert!(hidden(&NEGATIVE_POSES) >= 8);
        assert!(NEGATIVE_POSES.iter().any(|pose| pose.targets[0] == 1.0));
    }

    #[test]
    fn prompts_are_graded_and_include_diagonals() {
        let extensions: HashSet<u32> = CORE_POSES
            .iter()
            .chain(NEGATIVE_POSES.iter())
            .filter(|pose| pose.targets[0] == 1.0)
            .map(|pose| (pose.targets[1] * 100.0) as u32)
            .collect();
        assert!([25, 50, 75, 100]
            .iter()
            .all(|value| extensions.contains(value)));
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
        for (_, poses) in MODES {
            for pose in poses {
                assert!(
                    pose.targets[6..10].iter().all(|value| *value == 0.0),
                    "{}",
                    pose.name
                );
                assert!(pose
                    .targets
                    .iter()
                    .all(|value| (-1.0..=1.0).contains(value)));
            }
        }
    }

    #[test]
    fn the_basic_run_puffs_each_cheek_alone_with_the_tongue_in() {
        use vrft_quest_pro_protocol::CHEEK_POSES;
        let puffs = |left: f32, right: f32| {
            CORE_POSES
                .iter()
                .filter(|pose| pose.targets[10] == left && pose.targets[11] == right)
                .inspect(|pose| assert_eq!(pose.targets[..10], [0.0; 10], "{}", pose.name))
                .count()
        };
        assert_eq!(
            (puffs(1.0, 0.0), puffs(0.0, 1.0), puffs(1.0, 1.0)),
            (1, 1, 1)
        );
        for name in CHEEK_POSES {
            assert!(CORE_POSES.iter().any(|pose| pose.name == name), "{name}");
        }
        // Tongue-in-cheek bulges are labelled unpuffed, so they are told apart.
        assert!(CORE_POSES
            .iter()
            .chain(&NEGATIVE_POSES)
            .filter(|pose| pose.name.contains("cheek") && pose.name.starts_with("Tongue"))
            .all(|pose| pose.targets[10..] == [0.0, 0.0]));
    }

    #[test]
    fn each_capture_mode_visits_its_poses_once() {
        for (name, poses) in MODES {
            let (_, steps) = steps_for(name, 1).unwrap();
            assert_eq!(steps.len(), poses.len());
            assert!(steps
                .iter()
                .all(|step| step.seconds == STEP_SECONDS && step.settle == SETTLE_SECONDS));
        }
        assert!(steps_for("roll", 1).is_none());
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
