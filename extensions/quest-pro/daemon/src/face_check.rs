//! Checks each held pose of the face setup as it ends, as QFT+'s enrollment
//! does (`guided_session.py`, MIT): the cameras sent images that weren't
//! frozen, too dark or too bright; the face moved away from the relaxed one
//! and held still; and, with a tracking module sending, the headset's own
//! tracking saw the pose. A pose that fails is asked for once more.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use vrft_quest_pro_protocol::{CameraLayout, MOUTH_CAMERAS, VIEW};

/// Each mouth camera's view shrinks to this many blocks a side.
const BLOCKS: usize = 25;
const BLOCK: usize = VIEW as usize / BLOCKS;
/// A pose with fewer frames than this didn't get images.
const MIN_FRAMES: usize = 3;
/// More frames than this share repeating their predecessor: frozen.
const MAX_FROZEN: f32 = 0.3;
/// The mouth cameras' mean brightness must sit in this range.
const BRIGHTNESS: std::ops::RangeInclusive<f32> = 10.0..=230.0;
/// A pose closer than this to the relaxed face didn't change it.
const MIN_DISTANCE: f32 = 3.0;
/// No retries once a setup has run this long, so it stays short.
pub(crate) const RETRY_BEFORE_SECONDS: f32 = 80.0;

/// What a slot's hold must show the headset's own tracking: Meta's values
/// starting with `prefix` (the largest of them on each frame) at least
/// `minimum` on the median frame. Puffs and sucks aren't checked, as in
/// QFT+: Meta's own read them too weakly.
fn native_minimum(slot: &str) -> Option<(&'static str, f32)> {
    match slot {
        "jaw_open" => Some(("JawDrop", 0.4)),
        "pucker" => Some(("LipPucker", 0.3)),
        slot if slot.starts_with("tongue_") => Some(("TongueOut", 0.5)),
        _ => None,
    }
}

/// The headset's own values a hold is checked against, by Meta's names.
pub(crate) const NATIVE_PREFIXES: [&str; 3] = ["JawDrop", "LipPucker", "TongueOut"];

/// One frame of a hold: the mouth cameras shrunk to blocks, whether it
/// repeated the frame before, and the headset's own values if fresh.
pub(crate) struct Observation {
    blocks: Vec<f32>,
    frozen: bool,
    native: Option<Vec<(&'static str, f32)>>,
}

/// A hold's frames as they come, for checking when it ends.
pub(crate) struct Hold {
    pub step: usize,
    observations: Vec<Observation>,
}

/// The outcome of one hold's check.
pub(crate) struct Verdict {
    pub passed: bool,
    pub reason: Option<&'static str>,
    pub frames: usize,
    /// The hold's blocks on average, kept from the relaxed face to measure
    /// the others against.
    pub mean_blocks: Option<Vec<f32>>,
}

/// A frame's fingerprint, to spot a frozen image.
pub(crate) fn fingerprint(pixels: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    pixels.hash(&mut hasher);
    hasher.finish()
}

/// The mouth cameras of a frame in `layout`, each averaged into
/// `BLOCKS` x `BLOCKS` blocks, side by side.
fn mouth_blocks(layout: &CameraLayout, pixels: &[u8]) -> Option<Vec<f32>> {
    let mouth = layout.select(pixels, &MOUTH_CAMERAS)?;
    let view = VIEW as usize;
    let width = view * MOUTH_CAMERAS.len();
    let columns = BLOCKS * MOUTH_CAMERAS.len();
    let mut sums = vec![0u32; BLOCKS * columns];
    for row in 0..view {
        let line = &mouth[row * width..][..width];
        let out = &mut sums[(row / BLOCK) * columns..][..columns];
        for (column, &value) in line.iter().enumerate() {
            out[column / BLOCK] += u32::from(value);
        }
    }
    let area = (BLOCK * BLOCK) as f32;
    Some(sums.into_iter().map(|sum| sum as f32 / area).collect())
}

impl Hold {
    pub fn new(step: usize) -> Self {
        Self {
            step,
            observations: Vec::new(),
        }
    }

    pub fn observe(
        &mut self,
        layout: &CameraLayout,
        pixels: &[u8],
        frozen: bool,
        native: Option<Vec<(&'static str, f32)>>,
    ) {
        if let Some(blocks) = mouth_blocks(layout, pixels) {
            self.observations.push(Observation {
                blocks,
                frozen,
                native,
            });
        }
    }

    /// Checks the hold of `slot` against the relaxed face's blocks, once
    /// that passed.
    pub fn check(&self, slot: &str, neutral: Option<&[f32]>) -> Verdict {
        let observations = &self.observations;
        let fail = |reason| Verdict {
            passed: false,
            reason: Some(reason),
            frames: observations.len(),
            mean_blocks: None,
        };
        if observations.len() < MIN_FRAMES {
            return fail("The headset cameras didn't send images.");
        }
        let count = observations.len() as f32;
        let frozen = observations.iter().filter(|o| o.frozen).count() as f32 / count;
        if frozen > MAX_FROZEN {
            return fail("The camera image froze.");
        }
        let brightness = observations.iter().map(|o| mean(&o.blocks)).sum::<f32>() / count;
        if !BRIGHTNESS.contains(&brightness) {
            return fail("The mouth cameras are too dark or too bright.");
        }
        let mean_blocks: Vec<f32> = (0..observations[0].blocks.len())
            .map(|index| observations.iter().map(|o| o.blocks[index]).sum::<f32>() / count)
            .collect();
        let relaxed = slot == "neutral";
        let reference = if relaxed {
            Some(mean_blocks.as_slice())
        } else {
            neutral
        };
        if let Some(reference) = reference {
            let distances: Vec<f32> = observations
                .iter()
                .map(|o| {
                    o.blocks
                        .iter()
                        .zip(reference)
                        .map(|(a, b)| (a - b).abs())
                        .sum::<f32>()
                        / reference.len() as f32
                })
                .collect();
            let distance = median(&distances);
            let average = mean(&distances);
            let spread = (distances.iter().map(|d| (d - average).powi(2)).sum::<f32>()
                / distances.len() as f32)
                .sqrt();
            if !relaxed && distance < MIN_DISTANCE {
                return fail("Your face looked the same as when relaxed.");
            }
            if spread > (0.5 * distance).max(2.5) {
                return fail("Try to hold still.");
            }
        }
        if let Some((prefix, minimum)) = native_minimum(slot) {
            let values: Vec<f32> = observations
                .iter()
                .filter_map(|o| {
                    o.native
                        .as_ref()?
                        .iter()
                        .filter(|(name, _)| name.starts_with(prefix))
                        .map(|(_, value)| *value)
                        .reduce(f32::max)
                })
                .collect();
            if !values.is_empty() && median(&values) < minimum {
                return fail("Face tracking didn't see it clearly.");
            }
        }
        Verdict {
            passed: true,
            reason: None,
            frames: observations.len(),
            mean_blocks: relaxed.then_some(mean_blocks),
        }
    }
}

fn mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len().max(1) as f32
}

/// The middle value, or the mean of the middle two, as NumPy's.
fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrft_quest_pro_protocol::FRAME_BYTES;

    fn frame(brightness: u8, mouth_spot: u8) -> Vec<u8> {
        let mut pixels = vec![brightness; FRAME_BYTES];
        // A patch in the left camera standing in for a pose.
        for row in 100..200 {
            pixels[row * 800 + 100..row * 800 + 200].fill(mouth_spot);
        }
        pixels
    }

    fn hold(frames: &[(Vec<u8>, bool)], native: Option<Vec<(&'static str, f32)>>) -> Hold {
        let layout = CameraLayout::mouth();
        let mut hold = Hold::new(0);
        for (pixels, frozen) in frames {
            hold.observe(&layout, pixels, *frozen, native.clone());
        }
        hold
    }

    #[test]
    fn a_still_relaxed_face_passes_and_becomes_the_reference() {
        let frames: Vec<_> = (0..8).map(|_| (frame(100, 100), false)).collect();
        let verdict = hold(&frames, None).check("neutral", None);
        assert!(verdict.passed, "{:?}", verdict.reason);
        assert_eq!(verdict.mean_blocks.unwrap().len(), BLOCKS * BLOCKS * 2);
    }

    #[test]
    fn holds_fail_for_each_of_qftpluss_reasons() {
        let few = [(frame(100, 100), false), (frame(100, 100), false)];
        assert_eq!(
            hold(&few, None).check("neutral", None).reason,
            Some("The headset cameras didn't send images.")
        );
        let frozen: Vec<_> = (0..8).map(|i| (frame(100, 100), i > 2)).collect();
        assert_eq!(
            hold(&frozen, None).check("neutral", None).reason,
            Some("The camera image froze.")
        );
        let dark: Vec<_> = (0..8).map(|_| (frame(4, 4), false)).collect();
        assert_eq!(
            hold(&dark, None).check("neutral", None).reason,
            Some("The mouth cameras are too dark or too bright.")
        );

        let relaxed: Vec<_> = (0..8).map(|_| (frame(100, 100), false)).collect();
        let reference = hold(&relaxed, None)
            .check("neutral", None)
            .mean_blocks
            .unwrap();
        assert_eq!(
            hold(&relaxed, None)
                .check("pucker", Some(&reference))
                .reason,
            Some("Your face looked the same as when relaxed.")
        );
        let moving: Vec<_> = (0..8)
            .map(|i| (frame(if i % 2 == 0 { 100 } else { 160 }, 255), false))
            .collect();
        assert_eq!(
            hold(&moving, None).check("pucker", Some(&reference)).reason,
            Some("Try to hold still.")
        );
        let posed: Vec<_> = (0..8).map(|_| (frame(100, 255), false)).collect();
        assert!(hold(&posed, None).check("pucker", Some(&reference)).passed);
        let unseen = Some(vec![("TongueOut", 0.2)]);
        assert_eq!(
            hold(&posed, unseen)
                .check("tongue_up", Some(&reference))
                .reason,
            Some("Face tracking didn't see it clearly.")
        );
        let seen = Some(vec![("LipPuckerL", 0.1), ("LipPuckerR", 0.6)]);
        assert!(hold(&posed, seen).check("pucker", Some(&reference)).passed);
    }

    #[test]
    fn a_median_matches_numpys() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), 2.5);
    }
}
