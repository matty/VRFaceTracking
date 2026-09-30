//! Training frames: which frames are used, how they are sampled, and the
//! augmentation applied to each batch.

use std::collections::{BTreeMap, HashMap};
use std::f32::consts::PI;

use anyhow::{bail, Result};
use rand::rngs::StdRng;
use rand::Rng;
use serde_json::{json, Value};

use crate::preprocess::{AreaResize, VIEW};
use crate::recordings::{Recording, Sample};
use crate::{CHEEK_COLUMNS, TARGETS, TONGUE_TARGETS};

/// Labels may be graded (extension 0.25-1, directions +/-0.5 or 1,
/// diagonals +/-0.7); any magnitude above this counts as an active label.
pub const ACTIVE: f32 = 0.1;
/// Heads with a signed range.
pub const SIGNED_COLUMNS: [usize; 3] = [2, 3, 9];
/// A cheek head trains once this many labelled frames puff that cheek, and
/// [`CHEEK_RELAXED`] frames leave it relaxed.
const CHEEK_PUFFED: usize = 8;
const CHEEK_RELAXED: usize = 20;
/// Directions a basic run must cover: (column, sign, name).
const CORE_DIRECTIONS: [(usize, f32, &str); 4] = [
    (2, -1.0, "left"),
    (2, 1.0, "right"),
    (3, 1.0, "up"),
    (3, -1.0, "down"),
];
/// Held poses keep at most this many evenly spaced frames per recording.
const LIMIT_PER_POSE: usize = 90;
/// Follow-the-dot frames are sampled by where the tongue points, so a route
/// that happened to linger on one side does not dominate: the centre, then
/// eight directions at half and full reach.
const DIRECTION_SECTORS: [&str; 8] = [
    "right",
    "up right",
    "up",
    "up left",
    "left",
    "down left",
    "down",
    "down right",
];

pub struct Record {
    pub targets: [f32; TARGETS.len()],
    pub cheeks_labelled: bool,
    pub native: Option<f32>,
    pub moving: bool,
    /// Frames sharing a key share one sampling weight.
    pub key: String,
    /// From a synthetic set rather than the user's own recordings.
    pub synthetic: bool,
}

/// Evenly subsampled frames of every usable pose.
pub struct Frames {
    pub records: Vec<Record>,
    /// `[2, size, size]` gray8 per record, or empty for a labels-only set.
    pub images: Vec<Vec<u8>>,
    pub size: usize,
}

/// `np.linspace(0, count - 1, keep, dtype=int)`.
fn spread(count: usize, keep: usize) -> Vec<usize> {
    if keep <= 1 {
        return vec![0];
    }
    let step = (count - 1) as f64 / (keep - 1) as f64;
    (0..keep).map(|i| (i as f64 * step) as usize).collect()
}

/// The pose, or for follow-the-dot frames the direction the label points.
fn sampling_key(sample: &Sample) -> String {
    if !sample.moving || sample.targets[0] < 0.5 {
        return sample.pose.clone();
    }
    let [horizontal, vertical] = [sample.targets[2], sample.targets[3]];
    let reach = horizontal.hypot(vertical);
    if reach < 0.25 {
        return "Follow the dot: centre".into();
    }
    let turn = (vertical.atan2(horizontal) / (PI / 4.0)).round_ties_even() as i32;
    let sector = DIRECTION_SECTORS[turn.rem_euclid(8) as usize];
    let half = if reach < 0.75 { "half " } else { "" };
    format!("Follow the dot: {half}{sector}")
}

/// Which samples of one recording are used.
fn select(samples: &[Sample]) -> Vec<&Sample> {
    let mut steps: Vec<(u64, Vec<&Sample>)> = vec![];
    for sample in samples {
        match steps.iter_mut().find(|(step, _)| *step == sample.step) {
            Some((_, group)) => group.push(sample),
            None => steps.push((sample.step, vec![sample])),
        }
    }
    let mut selected = vec![];
    for (_, group) in steps {
        if group[0].moving {
            // Every follow-the-dot frame differs.
            selected.extend(group);
        } else if group.len() >= 8 {
            let keep = LIMIT_PER_POSE.min(group.len());
            selected.extend(spread(group.len(), keep).into_iter().map(|i| group[i]));
        }
    }
    selected.sort_by_key(|sample| sample.index);
    selected
}

impl Frames {
    /// With `size` the images are resized and kept; without, only labels
    /// are read, which is enough for coverage checks.
    pub fn load(recordings: &[Recording], size: Option<usize>) -> Result<Self> {
        let resize = size.map(AreaResize::new);
        let mut records = vec![];
        let mut images = vec![];
        for recording in recordings {
            let selected = select(&recording.samples);
            if let Some(resize) = &resize {
                let indices: Vec<usize> = selected.iter().map(|sample| sample.index).collect();
                let size = resize.size();
                let stored = recording.view;
                if stored != VIEW && stored != size {
                    bail!(
                        "{} holds {stored} px views; this model reads {size} px",
                        recording.dir.display()
                    );
                }
                recording.read_frames(&indices, |strip| {
                    let mut image = vec![0u8; 2 * size * size];
                    for (view, out) in image.chunks_mut(size * size).enumerate() {
                        if stored == VIEW {
                            resize.view(strip, view, out);
                        } else {
                            // Already at the model's size: rows of both views side by side.
                            for (row, line) in out.chunks_mut(size).enumerate() {
                                line.copy_from_slice(&strip[(2 * row + view) * size..][..size]);
                            }
                        }
                    }
                    images.push(image);
                })?;
            }
            records.extend(selected.into_iter().map(|sample| Record {
                targets: sample.targets,
                cheeks_labelled: sample.cheeks_labelled,
                native: sample.native,
                moving: sample.moving,
                key: sampling_key(sample),
                synthetic: recording.synthetic,
            }));
        }
        if records.is_empty() {
            bail!("No usable poses in these recordings. Record a full basic run, then train again");
        }
        Ok(Self {
            records,
            images,
            size: size.unwrap_or(0),
        })
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn visible(&self) -> impl Iterator<Item = &Record> {
        self.records
            .iter()
            .filter(|record| record.targets[0] >= 0.5)
    }

    /// Frames whose cheek puffs were labelled, tongue out or not.
    fn cheeks_labelled(&self) -> impl Iterator<Item = &Record> {
        self.records.iter().filter(|record| record.cheeks_labelled)
    }

    /// Frame, pose and per-head label counts for the report.
    pub fn coverage(&self) -> Value {
        let positive = self.visible().count();
        let follow = self.records.iter().filter(|record| record.moving).count();
        let mut poses = BTreeMap::new();
        for record in &self.records {
            *poses.entry(record.key.clone()).or_insert(0usize) += 1;
        }
        let mut targets = serde_json::Map::new();
        for (column, name) in TARGETS.iter().enumerate().skip(1) {
            let labelled: Vec<&Record> = if CHEEK_COLUMNS.contains(&column) {
                self.cheeks_labelled().collect()
            } else {
                self.visible().collect()
            };
            let values: Vec<f32> = labelled
                .iter()
                .map(|record| record.targets[column])
                .collect();
            let mut levels: Vec<i64> = values
                .iter()
                .filter(|value| value.abs() > ACTIVE)
                .map(|value| (*value as f64 * 100.0).round() as i64)
                .collect();
            levels.sort_unstable();
            levels.dedup();
            targets.insert(
                name.to_string(),
                json!({
                    "positive": values.iter().filter(|v| **v > ACTIVE).count(),
                    "negative": values.iter().filter(|v| **v < -ACTIVE).count(),
                    "levels": levels.iter().map(|level| *level as f64 / 100.0).collect::<Vec<_>>(),
                }),
            );
        }
        json!({
            "frames": self.len(),
            "negative": self.len() - positive,
            "positive": positive,
            "sources": {"follow": follow, "poses": self.len() - follow},
            "synthetic": self.records.iter().filter(|record| record.synthetic).count(),
            "poses": poses,
            "targets": targets,
        })
    }

    /// Heads with enough examples to train; the basic directions and both
    /// tongue out and in are required, the cheek puffs are not.
    pub fn trainable_targets(&self) -> Result<[bool; TARGETS.len()]> {
        let positive = self.visible().count();
        if positive < 20 || self.len() - positive < 20 {
            bail!(
                "Training needs at least 20 tongue-out and 20 tongue-in frames. \
                 Record a full basic run, then train again"
            );
        }
        let count = |column: usize, sign: f32| {
            self.visible()
                .filter(|record| record.targets[column] * sign > ACTIVE)
                .count()
        };
        let mut enabled = [true; TARGETS.len()];
        for (column, enabled) in enabled.iter_mut().enumerate().take(TONGUE_TARGETS).skip(2) {
            *enabled = count(column, 1.0) >= 8
                && (!SIGNED_COLUMNS.contains(&column) || count(column, -1.0) >= 8);
        }
        for column in CHEEK_COLUMNS {
            let puffed = self
                .cheeks_labelled()
                .filter(|record| record.targets[column] > ACTIVE)
                .count();
            let relaxed = self
                .cheeks_labelled()
                .filter(|record| record.targets[column] <= ACTIVE)
                .count();
            enabled[column] = puffed >= CHEEK_PUFFED && relaxed >= CHEEK_RELAXED;
        }
        let missing: Vec<&str> = CORE_DIRECTIONS
            .iter()
            .filter(|(column, sign, _)| count(*column, *sign) < 8)
            .map(|(.., name)| *name)
            .collect();
        if !missing.is_empty() {
            bail!(
                "No usable tongue {} poses. Record a full basic run, then train again",
                missing.join(", ")
            );
        }
        Ok(enabled)
    }

    /// One pass of sample indices, each pose (key) drawn equally often,
    /// with replacement.
    pub fn balanced_order(&self, rng: &mut StdRng) -> Vec<usize> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for record in &self.records {
            *counts.entry(&record.key).or_default() += 1;
        }
        let mut cumulative = Vec::with_capacity(self.len());
        let mut total = 0f64;
        for record in &self.records {
            total += 1.0 / counts[record.key.as_str()] as f64;
            cumulative.push(total);
        }
        (0..self.len())
            .map(|_| {
                let draw = rng.random::<f64>() * total;
                cumulative
                    .partition_point(|&value| value <= draw)
                    .min(self.len() - 1)
            })
            .collect()
    }
}

/// A random geometry and exposure change per frame. Both views of a frame
/// get the same change, which preserves stereo correspondence: a small
/// rotation, zoom and shift as from a refitted headset, then gamma, gain and
/// offset, sometimes blur, sometimes sensor noise (independent per view,
/// like a real sensor). Matches the reference trainer's `augment_batch`.
pub fn augment(image: &[u8], size: usize, rng: &mut StdRng, out: &mut [f32]) {
    let angle = rng.random_range(-6.0f32..6.0).to_radians();
    let scale = rng.random_range(0.92f32..1.08);
    let (cos, sin) = (angle.cos() / scale, angle.sin() / scale);
    // Normalised coordinates span 2, so 1/12 shifts by 1/24 of the image.
    let shift_x = rng.random_range(-1.0f32 / 12.0..1.0 / 12.0);
    let shift_y = rng.random_range(-1.0f32 / 12.0..1.0 / 12.0);
    let gamma = rng.random_range(0.8f32..1.25);
    let gain = rng.random_range(0.88f32..1.12);
    let offset = rng.random_range(-0.04f32..0.04);
    let blur = rng.random::<f32>() < 0.15;
    let noise = rng.random::<f32>() < 0.2;
    let plane = size * size;
    let last = (size - 1) as f32;
    let mut sampled = vec![0f32; plane];
    for (view, out) in out.chunks_mut(plane).enumerate() {
        let source = &image[view * plane..(view + 1) * plane];
        let pixel = |x: usize, y: usize| source[y * size + x] as f32 / 255.0;
        // affine_grid + grid_sample (bilinear, border padding,
        // align_corners=False).
        for y in 0..size {
            let ny = (2 * y + 1) as f32 / size as f32 - 1.0;
            for x in 0..size {
                let nx = (2 * x + 1) as f32 / size as f32 - 1.0;
                let gx = cos * nx - sin * ny + shift_x;
                let gy = sin * nx + cos * ny + shift_y;
                let ix = (((gx + 1.0) * size as f32 - 1.0) / 2.0).clamp(0.0, last);
                let iy = (((gy + 1.0) * size as f32 - 1.0) / 2.0).clamp(0.0, last);
                let (x0, y0) = (ix.floor() as usize, iy.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(size - 1), (y0 + 1).min(size - 1));
                let (fx, fy) = (ix - x0 as f32, iy - y0 as f32);
                let top = pixel(x0, y0) * (1.0 - fx) + pixel(x1, y0) * fx;
                let bottom = pixel(x0, y1) * (1.0 - fx) + pixel(x1, y1) * fx;
                let value = (top * (1.0 - fy) + bottom * fy).clamp(0.0, 1.0);
                sampled[y * size + x] = (value.powf(gamma) * gain + offset).clamp(0.0, 1.0);
            }
        }
        if blur {
            // 3x3 mean over the neighbours that exist.
            for y in 0..size {
                for x in 0..size {
                    let (mut total, mut count) = (0f32, 0f32);
                    for yy in y.saturating_sub(1)..=(y + 1).min(size - 1) {
                        for xx in x.saturating_sub(1)..=(x + 1).min(size - 1) {
                            total += sampled[yy * size + xx];
                            count += 1.0;
                        }
                    }
                    out[y * size + x] = total / count;
                }
            }
        } else {
            out.copy_from_slice(&sampled);
        }
        if noise {
            for value in out.iter_mut() {
                *value = (*value + gaussian(rng) * 0.008).clamp(0.0, 1.0);
            }
        }
    }
}

/// A standard normal draw (Box-Muller).
fn gaussian(rng: &mut StdRng) -> f32 {
    let u = rng.random::<f32>().max(f32::MIN_POSITIVE);
    let v = rng.random::<f32>();
    (-2.0 * u.ln()).sqrt() * (2.0 * PI * v).cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn sample(pose: &str, step: u64, index: usize, targets: [f32; 12], moving: bool) -> Sample {
        Sample {
            index,
            step,
            pose: pose.into(),
            targets,
            cheeks_labelled: true,
            native: Some(targets[0]),
            moving,
        }
    }

    #[test]
    fn spread_matches_numpy_linspace() {
        assert_eq!(spread(10, 4), vec![0, 3, 6, 9]);
        assert_eq!(spread(200, 90).len(), 90);
        assert_eq!(*spread(200, 90).last().unwrap(), 199);
        assert_eq!(spread(5, 1), vec![0]);
    }

    #[test]
    fn held_poses_are_subsampled_and_short_ones_dropped() {
        let out = [1., 1., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.];
        let mut samples = vec![];
        for index in 0..200 {
            samples.push(sample("Straight", 0, index, out, false));
        }
        for index in 200..205 {
            samples.push(sample("Short", 1, index, out, false));
        }
        for index in 205..210 {
            samples.push(sample("Follow the dot", 2, index, out, true));
        }
        let selected = select(&samples);
        assert_eq!(selected.iter().filter(|s| s.pose == "Straight").count(), 90);
        assert_eq!(selected.iter().filter(|s| s.pose == "Short").count(), 0);
        assert_eq!(selected.iter().filter(|s| s.moving).count(), 5);
    }

    #[test]
    fn follow_frames_are_keyed_by_direction() {
        let mut targets = [1., 1., 1., 0., 0., 0., 0., 0., 0., 0., 0., 0.];
        let key = |targets| sampling_key(&sample("Follow the dot", 0, 0, targets, true));
        assert_eq!(key(targets), "Follow the dot: right");
        targets[2] = -0.5;
        targets[3] = 0.5;
        assert_eq!(key(targets), "Follow the dot: half up left");
        targets[2] = 0.1;
        targets[3] = 0.0;
        assert_eq!(key(targets), "Follow the dot: centre");
        targets[0] = 0.0;
        assert_eq!(key(targets), "Follow the dot");
    }

    #[test]
    fn augmentation_stays_in_range_and_keeps_a_flat_image_flat() {
        let size = 32;
        let image = vec![128u8; 2 * size * size];
        let mut out = vec![0f32; 2 * size * size];
        let mut rng = StdRng::seed_from_u64(1);
        for _ in 0..20 {
            augment(&image, size, &mut rng, &mut out);
            assert!(out.iter().all(|v| (0.0..=1.0).contains(v)));
            let (low, high) = out
                .iter()
                .fold((1f32, 0f32), |(l, h), v| (l.min(*v), h.max(*v)));
            assert!(high - low < 0.1, "a flat image only gains sensor noise");
        }
    }
}
