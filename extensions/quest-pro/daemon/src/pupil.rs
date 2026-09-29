//! Pupil size from the eye cameras' snapshots. Meta's eye tracking reports
//! no pupil size, so each snapshot's two views are searched for the dark
//! pupil the infrared cameras see, and its width is measured.
//!
//! Camera pixels are not millimetres, and how many a pupil spans depends on
//! the eye's distance from its camera, so each eye's sizes are mapped onto
//! the range that eye has shown this session, then onto a typical human
//! pupil range. That keeps dilation, which is what avatars use, correct
//! without knowing the camera's scale.

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

/// Smoothing of each eye's millimetres, and how fast the range it has shown
/// forgets an extreme.
const SMOOTHING: Duration = Duration::from_millis(350);
const RANGE_MEMORY: Duration = Duration::from_secs(120);
/// Until an eye has shown this much change, relative to its size, its range
/// is widened to it, so small wobbles do not read as full dilation.
const MIN_SPAN: f32 = 0.3;
/// Measurements further apart than this start the smoothing over.
const GAP: Duration = Duration::from_secs(2);

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

/// One eye's measurements over time.
#[derive(Default)]
struct EyeTrack {
    /// The last three widths, whose median is used, so one bad
    /// snapshot, as in a blink, does not show.
    recent: VecDeque<f32>,
    /// The smallest and largest widths this eye has shown, forgetting slowly.
    range: Option<(f32, f32)>,
    /// Where the eye sits in its range, 0 to 1, smoothed.
    dilation: Option<f32>,
    last: Option<Instant>,
}

impl EyeTrack {
    /// Takes one width measured at `at` and returns the eye's dilation.
    fn update(&mut self, width: f32, at: Instant) -> f32 {
        let elapsed = self.last.map(|last| at.saturating_duration_since(last));
        self.last = Some(at);
        if elapsed.is_none_or(|elapsed| elapsed > GAP) {
            self.recent.clear();
            self.dilation = None;
        }
        self.recent.push_back(width);
        if self.recent.len() > 3 {
            self.recent.pop_front();
        }
        let mut sorted: Vec<f32> = self.recent.iter().copied().collect();
        sorted.sort_by(f32::total_cmp);
        let width = sorted[sorted.len() / 2];
        let (mut low, mut high) = self.range.unwrap_or((width, width));
        let forget = elapsed.map_or(0.0, |elapsed| {
            1.0 - (-elapsed.as_secs_f32() / RANGE_MEMORY.as_secs_f32()).exp()
        });
        low = if width < low {
            width
        } else {
            low + (width - low) * forget
        };
        high = if width > high {
            width
        } else {
            high + (width - high) * forget
        };
        self.range = Some((low, high));
        let middle = (low + high) / 2.0;
        let span = (high - low).max(MIN_SPAN * middle).max(f32::EPSILON);
        let target = ((width - middle) / span + 0.5).clamp(0.0, 1.0);
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
        let mut dilation = [None; 2];
        for ((value, eye), width) in dilation.iter_mut().zip(&mut self.eyes).zip(widths) {
            *value = width.map(|width| eye.update(width, received_at));
        }
        shared.latest = Some(PupilSample {
            dilation,
            at: received_at,
        });
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

    #[test]
    fn each_eye_spans_the_range_it_has_shown() {
        let mut eye = EyeTrack::default();
        let start = Instant::now();
        let at = |seconds: f32| start + Duration::from_secs_f32(seconds);
        assert_eq!(eye.update(40.0, at(0.0)), 0.5, "one size is the middle");
        // Too small a change is not full dilation.
        let slight = eye.update(41.0, at(0.2));
        assert!(slight > 0.5 && slight < 0.6, "{slight}");
        for step in 0..20 {
            eye.update(20.0, at(0.4 + step as f32 * 0.2));
        }
        let narrow = eye.dilation.unwrap();
        for step in 0..20 {
            eye.update(60.0, at(5.0 + step as f32 * 0.2));
        }
        let wide = eye.dilation.unwrap();
        assert!(narrow < 0.05 && wide > 0.95, "{narrow} {wide}");
        // A single outlier, as in a blink, is ignored.
        let blink = eye.update(15.0, at(9.2));
        assert!(blink > 0.9, "{blink}");
    }

    #[test]
    fn a_missing_eye_borrows_the_other() {
        assert_eq!(both([Some(0.2), None]), [Some(0.2), Some(0.2)]);
        assert!((mean([Some(0.2), Some(0.4)]).unwrap() - 0.3).abs() < 1e-6);
        assert_eq!(mean([None, None]), None);
    }
}
