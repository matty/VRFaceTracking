//! Pupil size from the eye cameras' snapshots. Meta's eye tracking reports
//! no pupil size, so each snapshot's two views are searched for the dark
//! pupil the infrared cameras see, and its width is measured.
//!
//! Camera pixels are not millimetres, and how many a pupil spans depends on
//! the eye's distance from its camera, so each eye's sizes are mapped onto
//! the range that eye has shown this session, then onto a typical human
//! pupil range. That keeps dilation, which is what avatars use, correct
//! without knowing the camera's scale.
//!
//! Pupils widen and narrow together, so a size far from what an eye usually
//! shows, or a sudden change, counts only when the other eye shows it too.
//! Checks after Qpro-Enhanced-FT-GNimrodG's `pupil_tracking.py` (MIT).

use crate::settings::QuestProSettings;
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use vrft_api::UnifiedTrackingData;
use vrft_quest_pro_protocol::PupilStatus;

/// Snapshots come a few times a second; a measurement stays in use this long.
pub const PUPIL_FRESH_FOR: Duration = Duration::from_millis(1500);
/// The typical range of an adult pupil, which each eye's own range maps to.
pub const MIN_MM: f32 = 2.0;
pub const MAX_MM: f32 = 8.0;

/// One camera's view in the snapshot strip, and the side of the half
/// resolution it is searched at.
const VIEW: usize = vrft_quest_pro_protocol::FRAME_HEIGHT as usize;
const STRIP: usize = vrft_quest_pro_protocol::FRAME_WIDTH as usize;
const SCALE: usize = 2;
const SIZE: usize = VIEW / SCALE;
/// The pupil's darkest point is looked for this far inside the view.
const MARGIN: usize = SIZE / 10;
/// Blur radii: a wide one finds the largest dark area, which is the pupil
/// rather than a lash or a shadow's edge; a narrow one outlines it.
const SEED_BLUR: usize = 4;
const EDGE_BLUR: usize = 1;
/// The pupil must be this much darker than the view's middle brightness,
/// and at most [`MAX_RELATIVE_LEVEL`] of it: under infrared light the pupil
/// is near black, the iris only somewhat darker than skin.
const MIN_CONTRAST: u8 = 18;
const MAX_RELATIVE_LEVEL: f32 = 0.45;
/// How far from the pupil's darkest point towards the view's middle
/// brightness its edge is taken to be.
const EDGE_LEVEL: f32 = 0.4;
/// Plausible pupil areas at half resolution, in pixels.
const MIN_AREA: usize = 12;
const MAX_AREA: usize = SIZE * SIZE / 6;
/// An eyelid may hide part of the pupil, but not most of it.
const MIN_ASPECT: f32 = 0.45;
/// How much of the ellipse its moments describe the area must fill.
const FILL: std::ops::RangeInclusive<f32> = 0.6..=1.3;

/// Smoothing of each eye's dilation.
const SMOOTHING: Duration = Duration::from_millis(350);
/// Accepted measurements further apart than this start the eye over, all
/// but its range.
const GAP: Duration = Duration::from_secs(2);
/// Each eye's range is taken from its sizes over this long, between these
/// shares of them, so a few stray measurements cannot stretch it.
const RANGE_HISTORY: Duration = Duration::from_secs(600);
const RANGE_SHARES: [f32; 2] = [0.03, 0.97];
/// Until an eye has shown this much change, relative to its typical size,
/// its range is widened to it, so small wobbles do not read as full dilation.
const MIN_SPAN: f32 = 0.45;
/// Looking aside turns an eye from its camera, and lashes can then pass for
/// a small pupil. So a size outside this band around the eye's median over
/// the last minute counts only when the other eye has changed the same way.
const TYPICAL_WINDOW: Duration = Duration::from_secs(60);
const TYPICAL_BAND: [f32; 2] = [0.6, 1.7];
/// The band applies once the last minute holds this many sizes: two
/// seconds of snapshots at the default 5 a second.
const TYPICAL_MIN: usize = 10;
/// A size this far from the eye's current one is a jump. It counts only
/// when the other eye jumps the same way, and once it has lasted this long,
/// staying within `JUMP_STEADY` of itself.
const JUMP: f32 = 0.3;
const JUMP_HOLD: Duration = Duration::from_millis(600);
const JUMP_STEADY: f32 = 0.15;
/// The other eye agrees when it has changed the same way by at least this
/// share as much, in ratio terms.
const AGREE: f32 = 0.5;

/// Box blur of a `SIZE` square image, `radius` pixels each way.
fn blur(image: &[f32], radius: usize) -> Vec<f32> {
    let pass = |source: &[f32], horizontal: bool| {
        let mut out = vec![0f32; SIZE * SIZE];
        for a in 0..SIZE {
            for b in 0..SIZE {
                let (low, high) = (b.saturating_sub(radius), (b + radius).min(SIZE - 1));
                let mut total = 0f32;
                for c in low..=high {
                    total += if horizontal {
                        source[a * SIZE + c]
                    } else {
                        source[c * SIZE + a]
                    };
                }
                let value = total / (high - low + 1) as f32;
                if horizontal {
                    out[a * SIZE + b] = value;
                } else {
                    out[b * SIZE + a] = value;
                }
            }
        }
        out
    };
    pass(&pass(image, true), false)
}

/// The pupil's width in snapshot pixels in `view` (0 for the left half of
/// the strip, 1 for the right), or `None` when no pupil shows, as when the
/// eye is closed.
pub fn measure(strip: &[u8], view: usize) -> Option<f32> {
    if strip.len() != STRIP * VIEW || view > 1 {
        return None;
    }
    let mut image = vec![0f32; SIZE * SIZE];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut total = 0u32;
            for dy in 0..SCALE {
                let row = (y * SCALE + dy) * STRIP + view * VIEW + x * SCALE;
                total += strip[row..row + SCALE]
                    .iter()
                    .map(|&p| u32::from(p))
                    .sum::<u32>();
            }
            image[y * SIZE + x] = total as f32 / (SCALE * SCALE) as f32;
        }
    }
    let wide = blur(&image, SEED_BLUR);
    let edge = blur(&image, EDGE_BLUR);
    let inner = MARGIN..SIZE - MARGIN;
    let (mut seed, mut darkest) = (0, f32::MAX);
    let mut levels = [0u32; 256];
    for y in inner.clone() {
        for x in inner.clone() {
            let index = y * SIZE + x;
            if wide[index] < darkest {
                (seed, darkest) = (index, wide[index]);
            }
            levels[edge[index].round().clamp(0.0, 255.0) as usize] += 1;
        }
    }
    let half = (inner.len() * inner.len()) as u32 / 2;
    let mut seen = 0;
    let middle = levels
        .iter()
        .position(|&count| {
            seen += count;
            seen >= half
        })
        .unwrap_or(255) as f32;
    // The widely blurred minimum can sit on a glint; start from the darkest
    // pixel near it instead.
    let (x, y) = (seed % SIZE, seed / SIZE);
    let near = |centre: usize| centre.saturating_sub(SEED_BLUR)..(centre + SEED_BLUR + 1).min(SIZE);
    let seed = near(y)
        .flat_map(|y| near(x).map(move |x| y * SIZE + x))
        .min_by(|a, b| edge[*a].total_cmp(&edge[*b]))
        .unwrap_or(seed);
    let dark = edge[seed];
    if middle - dark < f32::from(MIN_CONTRAST) || dark > middle * MAX_RELATIVE_LEVEL {
        return None;
    }
    // A first outline, then the edge halfway between the pupil and what
    // surrounds it, which is where a blurred edge really is.
    let rough = grow(&edge, seed, dark + (middle - dark) * EDGE_LEVEL)?;
    let (inside, around) = levels_around(&edge, &rough);
    let pupil = grow(&edge, seed, (inside + around) / 2.0)?;
    let (area, major, minor) = shape(&fill_holes(&pupil))?;
    let fill = area / (std::f32::consts::FRAC_PI_4 * major * minor);
    if area < MIN_AREA as f32 || minor / major < MIN_ASPECT || !FILL.contains(&fill) {
        return None;
    }
    Some(major * SCALE as f32)
}

/// The pixels no brighter than `threshold` joined to `seed`, or `None` when
/// they touch the view's edge or grow too large to be a pupil.
fn grow(image: &[f32], seed: usize, threshold: f32) -> Option<Vec<usize>> {
    let mut inside = vec![false; SIZE * SIZE];
    let mut pixels = vec![seed];
    inside[seed] = true;
    let mut next = 0;
    while next < pixels.len() {
        let index = pixels[next];
        next += 1;
        let (x, y) = (index % SIZE, index / SIZE);
        if x == 0 || y == 0 || x == SIZE - 1 || y == SIZE - 1 {
            return None;
        }
        for neighbour in [index - 1, index + 1, index - SIZE, index + SIZE] {
            if !inside[neighbour] && image[neighbour] <= threshold {
                inside[neighbour] = true;
                pixels.push(neighbour);
                if pixels.len() > MAX_AREA {
                    return None;
                }
            }
        }
    }
    Some(pixels)
}

/// The median brightness of `pixels`, and of a ring a few pixels outside
/// them: the pupil's own level, glints aside, and its surround's.
fn levels_around(image: &[f32], pixels: &[usize]) -> (f32, f32) {
    const RING: usize = 3;
    let median = |mut values: Vec<f32>| {
        values.sort_by(f32::total_cmp);
        values.get(values.len() / 2).copied()
    };
    let mut member = vec![false; SIZE * SIZE];
    for &index in pixels {
        member[index] = true;
    }
    let mut ring = vec![false; SIZE * SIZE];
    for &index in pixels {
        let (x, y) = (index % SIZE, index / SIZE);
        let neighbours = [
            (x >= RING).then(|| index - RING),
            (x + RING < SIZE).then_some(index + RING),
            (y >= RING).then(|| index - RING * SIZE),
            (y + RING < SIZE).then_some(index + RING * SIZE),
        ];
        for neighbour in neighbours.into_iter().flatten() {
            ring[neighbour] |= !member[neighbour];
        }
    }
    let inside = median(pixels.iter().map(|&index| image[index]).collect());
    let around = median(
        (0..SIZE * SIZE)
            .filter(|&index| ring[index])
            .map(|index| image[index])
            .collect(),
    );
    let inside = inside.unwrap_or(0.0);
    (inside, around.unwrap_or(inside))
}

/// `pixels` with any holes in it filled, such as the bright glints of the
/// headset's lights on the pupil.
fn fill_holes(pixels: &[usize]) -> Vec<usize> {
    let (mut left, mut top, mut right, mut bottom) = (SIZE, SIZE, 0, 0);
    for &index in pixels {
        let (x, y) = (index % SIZE, index / SIZE);
        (left, top) = (left.min(x), top.min(y));
        (right, bottom) = (right.max(x), bottom.max(y));
    }
    // The bounding box with a one pixel border, all outside the shape.
    let (width, height) = (right - left + 3, bottom - top + 3);
    let local = |index: usize| (index / SIZE - top + 1) * width + (index % SIZE - left + 1);
    let mut shape = vec![false; width * height];
    for &index in pixels {
        shape[local(index)] = true;
    }
    let mut outside = vec![false; width * height];
    let mut stack = vec![0];
    outside[0] = true;
    while let Some(index) = stack.pop() {
        let (x, y) = (index % width, index / width);
        let neighbours = [
            (x > 0).then(|| index - 1),
            (x + 1 < width).then_some(index + 1),
            (y > 0).then(|| index - width),
            (y + 1 < height).then_some(index + width),
        ];
        for neighbour in neighbours.into_iter().flatten() {
            if !outside[neighbour] && !shape[neighbour] {
                outside[neighbour] = true;
                stack.push(neighbour);
            }
        }
    }
    (0..width * height)
        .filter(|&index| !outside[index])
        .map(|index| (index / width + top - 1) * SIZE + index % width + left - 1)
        .collect()
}

/// Area, and the widths along the longest and shortest axes of the ellipse
/// with the same second moments.
fn shape(pixels: &[usize]) -> Option<(f32, f32, f32)> {
    let count = pixels.len() as f64;
    if count == 0.0 {
        return None;
    }
    let point = |index: &usize| ((index % SIZE) as f64, (index / SIZE) as f64);
    let (sum_x, sum_y) = pixels
        .iter()
        .map(point)
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let (mean_x, mean_y) = (sum_x / count, sum_y / count);
    let (mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0);
    for (x, y) in pixels.iter().map(point) {
        let (dx, dy) = (x - mean_x, y - mean_y);
        xx += dx * dx;
        yy += dy * dy;
        xy += dx * dy;
    }
    // A pixel is a unit square, which adds 1/12 to each axis's variance.
    let (xx, yy, xy) = (xx / count + 1.0 / 12.0, yy / count + 1.0 / 12.0, xy / count);
    let spread = ((xx - yy).powi(2) / 4.0 + xy * xy).sqrt();
    let (major, minor) = ((xx + yy) / 2.0 + spread, (xx + yy) / 2.0 - spread);
    // A filled ellipse's variance along an axis is a quarter of its semi-axis squared.
    Some((
        count as f32,
        (4.0 * major.sqrt()) as f32,
        (4.0 * minor.max(0.0).sqrt()) as f32,
    ))
}

/// The middle of `values`, the upper one of an even count.
fn median(values: impl IntoIterator<Item = f32>) -> Option<f32> {
    let mut values: Vec<f32> = values.into_iter().collect();
    values.sort_by(f32::total_cmp);
    values.get(values.len() / 2).copied()
}

/// How a width compares with the eye's current and typical widths, as
/// ratios, where the eye has them.
#[derive(Clone, Copy)]
struct Change {
    current: Option<f32>,
    typical: Option<f32>,
}

/// A jump waiting to last: when it started, which way it went, and its
/// widths over the last `JUMP_HOLD`.
struct Jump {
    since: Instant,
    wider: bool,
    widths: VecDeque<(Instant, f32)>,
}

/// One eye's measurements over time.
#[derive(Default)]
struct EyeTrack {
    /// The last three accepted widths, whose median is used, so one bad
    /// snapshot, as in a blink, does not show.
    recent: VecDeque<f32>,
    /// Accepted widths over `RANGE_HISTORY`, for the range and the
    /// typical width.
    history: VecDeque<(Instant, f32)>,
    jump: Option<Jump>,
    /// Where the eye sits in its range, 0 to 1, smoothed.
    dilation: Option<f32>,
    /// When a width was last accepted.
    last: Option<Instant>,
}

impl EyeTrack {
    /// Forgets what a long gap or old age has made stale.
    fn age(&mut self, at: Instant) {
        if self
            .last
            .is_none_or(|last| at.saturating_duration_since(last) > GAP)
        {
            self.recent.clear();
            self.jump = None;
            self.dilation = None;
        }
        while self
            .history
            .front()
            .is_some_and(|(when, _)| at.saturating_duration_since(*when) > RANGE_HISTORY)
        {
            self.history.pop_front();
        }
    }

    fn current(&self) -> Option<f32> {
        median(self.recent.iter().copied())
    }

    /// The median width over the last minute, once there are enough.
    fn typical(&self, at: Instant) -> Option<f32> {
        let widths: Vec<f32> = self
            .history
            .iter()
            .filter(|(when, _)| at.saturating_duration_since(*when) <= TYPICAL_WINDOW)
            .map(|&(_, width)| width)
            .collect();
        if widths.len() < TYPICAL_MIN {
            return None;
        }
        median(widths)
    }

    fn change(&self, width: f32, at: Instant) -> Change {
        Change {
            current: self.current().map(|current| width / current),
            typical: self.typical(at).map(|typical| width / typical),
        }
    }

    /// Takes one width measured at `at`, how it compares with this eye's
    /// widths, and how the other eye's width at the same moment compares
    /// with its own, if it was measured. Returns the eye's dilation, which
    /// stays where it was while a width is held back.
    fn update(
        &mut self,
        width: f32,
        change: Change,
        other: Option<Change>,
        at: Instant,
    ) -> Option<f32> {
        let agrees = |own: f32, theirs: Option<f32>| {
            theirs.is_some_and(|theirs| {
                let (own, theirs) = (own.ln(), theirs.ln());
                own * theirs > 0.0 && theirs.abs() >= AGREE * own.abs()
            })
        };
        if let Some(ratio) = change.current.filter(|ratio| (ratio - 1.0).abs() > JUMP) {
            if !agrees(
                ratio,
                other.and_then(|other| other.current.or(other.typical)),
            ) {
                self.jump = None;
                return self.held(at);
            }
            return self.hold_jump(width, ratio > 1.0, at);
        }
        self.jump = None;
        let [low, high] = TYPICAL_BAND;
        if let Some(ratio) = change.typical.filter(|ratio| !(low..=high).contains(ratio)) {
            if !agrees(
                ratio,
                other.and_then(|other| other.typical.or(other.current)),
            ) {
                return self.held(at);
            }
        }
        self.recent.push_back(width);
        if self.recent.len() > 3 {
            self.recent.pop_front();
        }
        Some(self.settle(at))
    }

    /// Counts a jump once it has lasted and stayed steady.
    fn hold_jump(&mut self, width: f32, wider: bool, at: Instant) -> Option<f32> {
        if self.jump.as_ref().is_some_and(|jump| jump.wider != wider) {
            self.jump = None;
        }
        let jump = self.jump.get_or_insert_with(|| Jump {
            since: at,
            wider,
            widths: VecDeque::new(),
        });
        jump.widths.push_back((at, width));
        while jump
            .widths
            .front()
            .is_some_and(|(when, _)| at.saturating_duration_since(*when) > JUMP_HOLD)
        {
            jump.widths.pop_front();
        }
        let (low, high) = jump
            .widths
            .iter()
            .fold((f32::MAX, f32::MIN), |(low, high), &(_, width)| {
                (low.min(width), high.max(width))
            });
        if at.saturating_duration_since(jump.since) < JUMP_HOLD || high - low > JUMP_STEADY * width
        {
            return self.held(at);
        }
        // It lasted: the eye's current width moves to it at once.
        self.recent = jump
            .widths
            .iter()
            .rev()
            .take(3)
            .map(|&(_, width)| width)
            .collect();
        self.jump = None;
        Some(self.settle(at))
    }

    /// The dilation while a width is held back, as long as it is recent.
    fn held(&self, at: Instant) -> Option<f32> {
        self.dilation.filter(|_| {
            self.last
                .is_some_and(|last| at.saturating_duration_since(last) <= PUPIL_FRESH_FOR)
        })
    }

    /// Records the current width and returns where it sits in the range.
    fn settle(&mut self, at: Instant) -> f32 {
        let width = self.current().unwrap_or_default();
        self.history.push_back((at, width));
        let mut sorted: Vec<f32> = self.history.iter().map(|&(_, width)| width).collect();
        sorted.sort_by(f32::total_cmp);
        let share = |share: f32| sorted[((sorted.len() - 1) as f32 * share).round() as usize];
        let [low, high] = RANGE_SHARES.map(share);
        let middle = (low + high) / 2.0;
        let span = (high - low).max(MIN_SPAN * share(0.5)).max(f32::EPSILON);
        let target = ((width - middle) / span + 0.5).clamp(0.0, 1.0);
        let elapsed = self.last.map(|last| at.saturating_duration_since(last));
        self.last = Some(at);
        let dilation = match (self.dilation, elapsed) {
            (Some(previous), Some(elapsed)) => {
                let alpha = 1.0 - (-elapsed.as_secs_f32() / SMOOTHING.as_secs_f32()).exp();
                previous + (target - previous) * alpha
            }
            _ => target,
        };
        self.dilation = Some(dilation);
        dilation
    }
}

/// Takes one snapshot's widths, `None` where no pupil showed, and returns
/// each eye's dilation.
fn update_eyes(
    eyes: &mut [EyeTrack; 2],
    widths: [Option<f32>; 2],
    at: Instant,
) -> [Option<f32>; 2] {
    for eye in eyes.iter_mut() {
        eye.age(at);
    }
    let changes = [0, 1].map(|index| widths[index].map(|width| eyes[index].change(width, at)));
    [0, 1].map(|index| {
        let (width, change) = (widths[index]?, changes[index]?);
        eyes[index].update(width, change, changes[1 - index], at)
    })
}

fn millimetres(dilation: f32) -> f32 {
    MIN_MM + dilation * (MAX_MM - MIN_MM)
}

#[derive(Clone, Copy)]
struct PupilSample {
    /// Each eye's dilation, from the left then the right half of the strip.
    dilation: [Option<f32>; 2],
    at: Instant,
}

#[derive(Default)]
struct Shared {
    latest: Option<PupilSample>,
    /// When recent snapshots were measured, for the rate.
    measured: VecDeque<Instant>,
    missed: u64,
}

/// Pupil sizes as the stream measures them, shared with the overlay and
/// the status.
#[derive(Clone, Default)]
pub struct PupilState(Arc<RwLock<Shared>>);

impl PupilState {
    pub fn processor(&self) -> PupilProcessor {
        PupilProcessor {
            state: self.clone(),
            eyes: Default::default(),
        }
    }

    fn fresh(&self) -> Option<PupilSample> {
        self.0
            .read()
            .unwrap()
            .latest
            .filter(|sample| sample.at.elapsed() <= PUPIL_FRESH_FOR)
    }

    pub fn status(&self, settings: &QuestProSettings) -> PupilStatus {
        let shared = self.0.read().unwrap();
        let cutoff = Instant::now().checked_sub(Duration::from_secs(5));
        let recent = shared
            .measured
            .iter()
            .filter(|at| cutoff.is_none_or(|cutoff| **at >= cutoff))
            .count();
        let sample = shared
            .latest
            .filter(|sample| settings.pupils && sample.at.elapsed() <= PUPIL_FRESH_FOR);
        PupilStatus {
            fresh: sample.is_some(),
            rate_hz: recent as f32 / 5.0,
            missed: shared.missed,
            diameter_mm: sample.map_or([None; 2], |sample| {
                let [left, right] = both(sample.dilation);
                [left.map(millimetres), right.map(millimetres)]
            }),
            dilation: sample.and_then(|sample| mean(both(sample.dilation))),
        }
    }
}

/// Each eye's value, the other eye's where one is missing: pupils widen and
/// narrow together.
fn both([left, right]: [Option<f32>; 2]) -> [Option<f32>; 2] {
    [left.or(right), right.or(left)]
}

fn mean(values: [Option<f32>; 2]) -> Option<f32> {
    match values {
        [Some(a), Some(b)] => Some((a + b) / 2.0),
        [a, b] => a.or(b),
    }
}

/// Measures each eye snapshot on the stream's thread.
pub struct PupilProcessor {
    state: PupilState,
    eyes: [EyeTrack; 2],
}

impl PupilProcessor {
    pub fn process(&mut self, strip: &[u8], received_at: Instant) {
        let widths = [measure(strip, 0), measure(strip, 1)];
        let mut shared = self.state.0.write().unwrap();
        shared.measured.push_back(received_at);
        while shared.measured.len() > 64 {
            shared.measured.pop_front();
        }
        if widths.iter().all(Option::is_none) {
            shared.missed += 1;
            return;
        }
        let dilation = update_eyes(&mut self.eyes, widths, received_at);
        if dilation.iter().any(Option::is_some) {
            shared.latest = Some(PupilSample {
                dilation,
                at: received_at,
            });
        }
    }
}

/// Puts measured pupil sizes into the frames VRFT sends.
pub struct PupilOverlay {
    state: PupilState,
}

impl PupilOverlay {
    pub fn new(state: PupilState) -> Self {
        Self { state }
    }

    pub fn is_live(&self) -> bool {
        self.state.fresh().is_some()
    }

    pub fn apply(&self, data: &mut UnifiedTrackingData, settings: &QuestProSettings) {
        if !settings.pupils {
            return;
        }
        let Some(sample) = self.state.fresh() else {
            return;
        };
        let [Some(left), Some(right)] = both(sample.dilation) else {
            return;
        };
        data.eye.left.pupil_diameter_mm = millimetres(left);
        data.eye.right.pupil_diameter_mm = millimetres(right);
        data.eye.min_dilation = MIN_MM;
        data.eye.max_dilation = MAX_MM;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic eye snapshot: skin, a darker iris and a pupil of
    /// `pupil` pixels across with a bright glint, in each view.
    fn eye_strip(pupil: [f32; 2]) -> Vec<u8> {
        let mut strip = vec![0u8; STRIP * VIEW];
        for (view, diameter) in pupil.into_iter().enumerate() {
            let (cx, cy) = (190.0 + 20.0 * view as f32, 210.0);
            for y in 0..VIEW {
                for x in 0..VIEW {
                    let distance = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                    let glint =
                        (x as f32 - cx - 4.0).abs() < 3.0 && (y as f32 - cy + 3.0).abs() < 3.0;
                    let value = if glint && diameter > 0.0 {
                        250
                    } else if distance < diameter / 2.0 {
                        25
                    } else if distance < 70.0 {
                        95
                    } else {
                        160 + ((x * 7 + y * 13) % 11) as u8
                    };
                    strip[y * STRIP + view * VIEW + x] = value;
                }
            }
        }
        strip
    }

    #[test]
    fn measures_the_pupil_despite_its_glint() {
        for diameter in [24.0, 40.0, 64.0] {
            let strip = eye_strip([diameter, diameter]);
            for view in 0..2 {
                let measured = measure(&strip, view).unwrap();
                assert!(
                    (measured - diameter).abs() < diameter * 0.08,
                    "{diameter} measured as {measured}"
                );
            }
        }
    }

    #[test]
    fn no_pupil_is_found_in_a_closed_eye() {
        let strip = eye_strip([0.0, 0.0]);
        // The iris alone is too large and too faint to be a pupil.
        assert_eq!(measure(&strip, 0), None);
        let flat = vec![120u8; STRIP * VIEW];
        assert_eq!(measure(&flat, 1), None);
    }

    #[test]
    fn a_pupil_half_under_the_eyelid_keeps_its_width() {
        let mut strip = eye_strip([60.0, 60.0]);
        // An eyelid down to the pupil's middle.
        for y in 0..210 {
            for x in 0..VIEW {
                strip[y * STRIP + x] = 150;
            }
        }
        let measured = measure(&strip, 0).unwrap();
        assert!((measured - 60.0).abs() < 7.0, "{measured}");
        // Down to a sliver, it is no longer measured.
        for y in 210..232 {
            for x in 0..VIEW {
                strip[y * STRIP + x] = 150;
            }
        }
        assert_eq!(measure(&strip, 0), None);
    }

    /// Both eyes, fed snapshots five times a second.
    struct Eyes {
        tracks: [EyeTrack; 2],
        start: Instant,
        millis: u64,
    }

    impl Eyes {
        fn new() -> Self {
            Self {
                tracks: Default::default(),
                start: Instant::now(),
                millis: 0,
            }
        }

        /// These widths for `seconds`; the dilations after the last.
        fn run(&mut self, widths: [Option<f32>; 2], seconds: f32) -> [Option<f32>; 2] {
            let mut dilation = [None; 2];
            for _ in 0..(seconds * 5.0).round() as usize {
                self.millis += 200;
                let at = self.start + Duration::from_millis(self.millis);
                dilation = update_eyes(&mut self.tracks, widths, at);
            }
            dilation
        }
    }

    #[test]
    fn each_eye_spans_the_range_it_has_shown() {
        let mut eyes = Eyes::new();
        assert_eq!(
            eyes.run([Some(40.0); 2], 0.2),
            [Some(0.5); 2],
            "one size is the middle"
        );
        // Too small a change is not full dilation.
        let slight = eyes.run([Some(41.0); 2], 0.2)[0].unwrap();
        assert!(slight > 0.5 && slight < 0.6, "{slight}");
        eyes.run([Some(40.0); 2], 20.0);
        let narrow = eyes.run([Some(20.0); 2], 20.0)[0].unwrap();
        let wide = eyes.run([Some(60.0); 2], 20.0)[0].unwrap();
        assert!(narrow < 0.05 && wide > 0.95, "{narrow} {wide}");
    }

    #[test]
    fn a_few_stray_sizes_do_not_stretch_the_range() {
        let mut eye = EyeTrack::default();
        let start = Instant::now();
        let at = |step: u64| start + Duration::from_millis(step * 200);
        for step in 0..200 {
            eye.recent = [30.0 + (step % 21) as f32].into();
            eye.settle(at(step));
        }
        for step in 200..203 {
            eye.recent = [90.0].into();
            eye.settle(at(step));
        }
        eye.recent = [50.0].into();
        eye.dilation = None;
        let wide = eye.settle(at(203));
        assert!(wide > 0.95, "{wide}");
    }

    #[test]
    fn a_jump_in_one_eye_alone_is_held_back() {
        let mut eyes = Eyes::new();
        eyes.run([Some(40.0); 2], 10.0);
        // Lashes passing for a small pupil as an eye turns from its camera.
        assert_eq!(eyes.run([Some(15.0), Some(40.0)], 1.0), [Some(0.5); 2]);
        // Held back for long, the eye has no value, and borrows the other's.
        assert_eq!(eyes.run([Some(15.0), Some(40.0)], 1.0), [None, Some(0.5)]);
    }

    #[test]
    fn a_jump_in_both_eyes_counts_once_it_lasts() {
        let mut eyes = Eyes::new();
        eyes.run([Some(40.0); 2], 10.0);
        // One snapshot, as in a blink, is ignored.
        assert_eq!(eyes.run([Some(26.0); 2], 0.2), [Some(0.5); 2]);
        assert_eq!(eyes.run([Some(40.0); 2], 0.2), [Some(0.5); 2]);
        // A real change lasts, and comes through.
        assert_eq!(eyes.run([Some(26.0); 2], 0.4), [Some(0.5); 2]);
        let narrowed = eyes.run([Some(26.0); 2], 1.0)[0].unwrap();
        assert!(narrowed < 0.4, "{narrowed}");
    }

    #[test]
    fn a_slow_drift_counts_beyond_the_usual_band_only_in_both_eyes() {
        let drift = |both: bool| {
            let mut eyes = Eyes::new();
            eyes.run([Some(40.0); 2], 10.0);
            let mut width: f32 = 40.0;
            while width > 16.0 {
                // Too slow a change for a jump.
                width *= 0.9;
                eyes.run([Some(width), Some(if both { width } else { 40.0 })], 0.2);
            }
            eyes.tracks[0].current().unwrap()
        };
        let alone = drift(false);
        assert!(alone >= 0.6 * 40.0, "{alone}");
        let together = drift(true);
        assert!(together < 20.0, "{together}");
    }

    #[test]
    fn a_missing_eye_borrows_the_other() {
        assert_eq!(both([Some(0.2), None]), [Some(0.2), Some(0.2)]);
        assert!((mean([Some(0.2), Some(0.4)]).unwrap() - 0.3).abs() < 1e-6);
        assert_eq!(mean([None, None]), None);
    }
}
