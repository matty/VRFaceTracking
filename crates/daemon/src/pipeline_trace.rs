//! Records what each stage of the pipeline does to a frame, for the desktop
//! app's Debug page. Recording copies every value once per stage, so the
//! output thread only records while someone has asked for a trace lately.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use vrft_api::{UnifiedExpressions, UnifiedSingleEyeData, UnifiedTrackingData};
use vrft_protocol::{PipelineTrace, StageKind, TraceParam, TraceStage};

/// How long recording goes on after the last request, and how old a trace
/// can be and still be served.
const KEEP_RECORDING: Duration = Duration::from_secs(2);

/// Shared by the output thread, which records, and the API, which asks.
pub struct PipelineTracer {
    started: Instant,
    /// Milliseconds after `started` until which frames are recorded.
    wanted_until: AtomicU64,
    latest: Mutex<Option<(Instant, PipelineTrace)>>,
}

impl Default for PipelineTracer {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            wanted_until: AtomicU64::new(0),
            latest: Mutex::new(None),
        }
    }
}

impl PipelineTracer {
    fn millis(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Whether this frame should be recorded.
    pub fn recording(&self) -> bool {
        self.millis() < self.wanted_until.load(Ordering::Relaxed)
    }

    /// The latest trace, unless it's old, and keeps recording for a while.
    pub fn request(&self) -> Option<PipelineTrace> {
        let until = self.millis() + KEEP_RECORDING.as_millis() as u64;
        self.wanted_until.fetch_max(until, Ordering::Relaxed);
        let latest = self.latest.lock().ok()?;
        let (at, trace) = latest.as_ref()?;
        (at.elapsed() < KEEP_RECORDING).then(|| PipelineTrace {
            params: params().to_vec(),
            ..trace.clone()
        })
    }

    /// Keeps `trace` for the next request. It never waits: while the API is
    /// reading the last trace, this one is dropped.
    pub fn publish(&self, trace: PipelineTrace) {
        if let Ok(mut latest) = self.latest.try_lock() {
            *latest = Some((Instant::now(), trace));
        }
    }
}

/// One frame's stages, as they run.
pub struct FrameRecorder {
    trace: PipelineTrace,
}

impl FrameRecorder {
    /// `fresh`: the tracking module produced the frame.
    pub fn new(fresh: bool) -> Self {
        Self {
            trace: PipelineTrace {
                fresh,
                ..PipelineTrace::default()
            },
        }
    }

    /// A stage that ran, and left `data`.
    pub fn record(&mut self, kind: StageKind, name: &str, data: &UnifiedTrackingData) {
        self.push(kind, name, Some(values(data)));
    }

    /// A stage that's turned off.
    pub fn skip(&mut self, kind: StageKind, name: &str) {
        self.push(kind, name, None);
    }

    /// The mutator's `steps`, and the values each one that ran left, in
    /// order.
    pub fn mutations(&mut self, steps: Vec<(String, bool)>, ran: Vec<Vec<f32>>) {
        let mut ran = ran.into_iter();
        for (name, runs) in steps {
            let values = if runs { ran.next() } else { None };
            self.push(mutation_kind(&name), &name, values);
        }
    }

    fn push(&mut self, kind: StageKind, name: &str, values: Option<Vec<f32>>) {
        self.trace.stages.push(TraceStage {
            kind,
            name: name.to_string(),
            active: values.is_some(),
            values: values.unwrap_or_default(),
        });
    }

    pub fn finish(self) -> PipelineTrace {
        self.trace
    }
}

fn mutation_kind(name: &str) -> StageKind {
    match name {
        "Adjustment" => StageKind::Adjustment,
        "Correctors" => StageKind::Correctors,
        "Smoothing" => StageKind::Smoothing,
        _ => StageKind::Other,
    }
}

/// An eye value, as the debug API names it after `EyeLeft`/`EyeRight`, its
/// range and how to read it.
type EyeValue = (&'static str, f32, f32, fn(&UnifiedSingleEyeData) -> f32);

const EYE_VALUES: [EyeValue; 4] = [
    ("Openness", 0., 1., |eye| eye.openness),
    ("GazeX", -1., 1., |eye| eye.gaze.x),
    ("GazeY", -1., 1., |eye| eye.gaze.y),
    ("Pupil", 0., 10., |eye| eye.pupil_diameter_mm),
];

const HEAD_VALUES: [&str; 6] = [
    "HeadYaw",
    "HeadPitch",
    "HeadRoll",
    "HeadPosX",
    "HeadPosY",
    "HeadPosZ",
];

/// Every value [`values`] lists, in its order: both eyes, the expressions,
/// then the head.
pub fn params() -> &'static [TraceParam] {
    static PARAMS: OnceLock<Vec<TraceParam>> = OnceLock::new();
    PARAMS.get_or_init(|| {
        let param = |name: String, group: &str, min: f32, max: f32| TraceParam {
            name,
            group: group.to_string(),
            min,
            max,
        };
        let eyes = ["Left", "Right"].into_iter().flat_map(|side| {
            EYE_VALUES.iter().map(move |(name, min, max, _)| {
                param(format!("Eye{side}{name}"), "eyes", *min, *max)
            })
        });
        let shapes = (0..UnifiedExpressions::Max as usize)
            .filter_map(|index| UnifiedExpressions::try_from(index).ok())
            .map(|expression| {
                let name = format!("{expression:?}");
                let group = expression_group(&name);
                param(name, group, 0., 1.)
            });
        let head = HEAD_VALUES
            .iter()
            .map(|name| param(name.to_string(), "head", -1., 1.));
        eyes.chain(shapes).chain(head).collect()
    })
}

/// Where an expression belongs, by its name.
fn expression_group(name: &str) -> &'static str {
    const PREFIXES: [(&str, &str); 12] = [
        ("Eye", "eyes"),
        ("Brow", "brows"),
        ("Nasal", "nose"),
        ("Nose", "nose"),
        ("Cheek", "cheeks"),
        ("Jaw", "jaw"),
        ("Mouth", "mouth"),
        ("Lip", "lips"),
        ("Tongue", "tongue"),
        ("SoftPalate", "throat"),
        ("Throat", "throat"),
        ("Neck", "throat"),
    ];
    PREFIXES
        .iter()
        .find(|(prefix, _)| name.starts_with(prefix))
        .map_or("other", |(_, group)| group)
}

/// `data`'s values, in [`params`] order.
pub fn values(data: &UnifiedTrackingData) -> Vec<f32> {
    let shapes = UnifiedExpressions::Max as usize;
    let mut values = Vec::with_capacity(EYE_VALUES.len() * 2 + shapes + HEAD_VALUES.len());
    for eye in [&data.eye.left, &data.eye.right] {
        values.extend(EYE_VALUES.iter().map(|(_, _, _, value)| value(eye)));
    }
    values.extend((0..shapes).map(|index| data.shapes.get(index).map_or(0., |shape| shape.weight)));
    let head = &data.head;
    values.extend([
        head.head_yaw,
        head.head_pitch,
        head.head_roll,
        head.head_pos_x,
        head.head_pos_y,
        head.head_pos_z,
    ]);
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_line_up_with_their_names() {
        let mut data = UnifiedTrackingData::default();
        data.eye.right.gaze.y = 0.25;
        *data.weight_mut(UnifiedExpressions::JawOpen).unwrap() = 0.75;
        data.head.head_roll = -0.5;
        let values = values(&data);
        assert_eq!(values.len(), params().len());
        let value = |name: &str| values[params().iter().position(|p| p.name == name).unwrap()];
        assert_eq!(value("EyeRightGazeY"), 0.25);
        assert_eq!(value("JawOpen"), 0.75);
        assert_eq!(value("HeadRoll"), -0.5);
    }

    #[test]
    fn every_expression_has_a_group() {
        let other: Vec<_> = params()
            .iter()
            .filter(|param| param.group == "other")
            .map(|param| param.name.as_str())
            .collect();
        assert!(other.is_empty(), "ungrouped: {other:?}");
    }

    #[test]
    fn steps_that_are_off_hold_their_place() {
        let mut recorder = FrameRecorder::new(true);
        recorder.mutations(
            vec![
                ("Adjustment".into(), false),
                ("Correctors".into(), true),
                ("Smoothing".into(), true),
            ],
            vec![vec![0.1], vec![0.2]],
        );
        let summary: Vec<_> = recorder
            .finish()
            .stages
            .into_iter()
            .map(|stage| (stage.kind, stage.active, stage.values))
            .collect();
        assert_eq!(
            summary,
            [
                (StageKind::Adjustment, false, vec![]),
                (StageKind::Correctors, true, vec![0.1]),
                (StageKind::Smoothing, true, vec![0.2]),
            ]
        );
    }

    #[test]
    fn traces_are_only_recorded_once_asked_for() {
        let tracer = PipelineTracer::default();
        assert!(!tracer.recording());
        assert_eq!(tracer.request(), None);
        assert!(tracer.recording());
        tracer.publish(FrameRecorder::new(true).finish());
        let trace = tracer.request().unwrap();
        assert!(trace.fresh);
        assert_eq!(trace.params.len(), params().len());
    }
}
