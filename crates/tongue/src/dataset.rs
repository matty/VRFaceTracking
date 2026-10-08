//! Which recorded frames training reads: each pose evenly subsampled, and
//! some held back to score it.

use std::f32::consts::PI;

use crate::recordings::Sample;

/// Labels may be graded (extension 0.25-1, directions +/-0.5 or 1,
/// diagonals +/-0.7); any magnitude above this counts as an active label.
pub const ACTIVE: f32 = 0.1;
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
/// One recorded frame in this many is held back from training to score it:
/// the last fifth of each held pose, and every fifth block of a recording's
/// follow-the-dot frames. Contiguous blocks keep near-identical neighbours
/// of a held-back frame out of training.
const HOLD_OUT_EVERY: usize = 5;
/// Held poses with fewer frames than this all train.
const HOLD_OUT_MIN_POSE: usize = 10;
/// Follow-the-dot frames are held back in blocks this long.
const HOLD_OUT_BLOCK: usize = 36;
/// `np.linspace(0, count - 1, keep, dtype=int)`.
fn spread(count: usize, keep: usize) -> Vec<usize> {
    if keep <= 1 {
        return vec![0];
    }
    let step = (count - 1) as f64 / (keep - 1) as f64;
    (0..keep).map(|i| (i as f64 * step) as usize).collect()
}

/// The pose, or for follow-the-dot frames the direction the label points.
pub(crate) fn sampling_key(sample: &Sample) -> String {
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
pub(crate) fn select(samples: &[Sample]) -> Vec<&Sample> {
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

/// Which of one recording's `selected` samples (in frame order) are held
/// back: the last fifth of each held pose large enough to split, and every
/// fifth block of its follow-the-dot frames taken as one sequence.
pub(crate) fn held_back(selected: &[&Sample]) -> Vec<bool> {
    let mut held = vec![false; selected.len()];
    let mut poses: Vec<(u64, Vec<usize>)> = vec![];
    let mut follow = vec![];
    for (position, sample) in selected.iter().enumerate() {
        if sample.moving {
            follow.push(position);
            continue;
        }
        match poses.iter_mut().find(|(step, _)| *step == sample.step) {
            Some((_, group)) => group.push(position),
            None => poses.push((sample.step, vec![position])),
        }
    }
    for (_, group) in &poses {
        if group.len() >= HOLD_OUT_MIN_POSE {
            let kept = group.len() - group.len().div_ceil(HOLD_OUT_EVERY);
            for &position in &group[kept..] {
                held[position] = true;
            }
        }
    }
    for (order, &position) in follow.iter().enumerate() {
        held[position] = (order / HOLD_OUT_BLOCK) % HOLD_OUT_EVERY == HOLD_OUT_EVERY - 1;
    }
    held
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pose: &str, step: u64, index: usize, targets: [f32; 12], moving: bool) -> Sample {
        Sample {
            index,
            step,
            pose: pose.into(),
            targets,
            cheeks_labelled: true,
            moving,
            face: Default::default(),
            anchor: None,
            identity: None,
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
    fn the_end_of_each_pose_and_every_fifth_follow_block_are_held_back() {
        let out = [1., 1., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.];
        let mut samples = vec![];
        for index in 0..20 {
            samples.push(sample("Straight", 0, index, out, false));
        }
        for index in 20..29 {
            samples.push(sample("Short", 1, index, out, false));
        }
        // Two follow-the-dot rounds, taken as one sequence.
        for index in 29..229 {
            samples.push(sample(
                "Follow the dot",
                2 + index as u64 / 100,
                index,
                out,
                true,
            ));
        }
        let selected = select(&samples);
        let held = held_back(&selected);
        let held_indices = |pose: &str| -> Vec<usize> {
            selected
                .iter()
                .zip(&held)
                .filter(|(sample, held)| **held && sample.pose == pose)
                .map(|(sample, _)| sample.index)
                .collect()
        };
        assert_eq!(held_indices("Straight"), [16, 17, 18, 19]);
        assert!(held_indices("Short").is_empty(), "too short to split");
        assert_eq!(
            held_indices("Follow the dot"),
            (29 + 4 * 36..29 + 5 * 36).collect::<Vec<_>>()
        );
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
}
