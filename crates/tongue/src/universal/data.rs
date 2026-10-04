//! Training frames for the universal face model: all five camera views of
//! each frame at the model's size, its labels and which of them are known,
//! and who it shows, so anchors can be drawn from the same face.
//!
//! Recordings of the mouth pair alone train it too: their eye and brow views
//! are blank, and nothing they lack a label for is learned from them.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use rand::rngs::StdRng;
use rand::Rng;

use super::{slot_for_pose, slot_index, target_index, ANCHOR_SLOTS, CAMERAS, FACE_TARGETS};
use crate::dataset::{sampling_key, select, ACTIVE};
use crate::preprocess::{AreaResize, VIEW};
use crate::recordings::{Recording, Sample};
use crate::CHEEK_COLUMNS;
use vrft_quest_pro_protocol::{BROW_CAMERA, EYE_CAMERAS};

const OUTPUTS: usize = FACE_TARGETS.len();
/// The recorded pose that labels both cheeks sucked in.
const SUCK_POSE: &str = "Cheeks sucked in";

pub struct FaceRecord {
    pub labels: [f32; OUTPUTS],
    /// Which labels are known.
    pub labelled: [bool; OUTPUTS],
    /// Index into [`FaceFrames::groups`]: whose face this is.
    pub group: usize,
    /// The anchor slot this frame shows, if it shows one.
    pub slot: Option<usize>,
    /// Frames sharing a key share one sampling weight within their group.
    pub key: String,
    pub synthetic: bool,
    /// The tracking module's TongueOut, for calibrating the visibility gate.
    pub native: Option<f32>,
    /// Whether the eye and brow cameras were recorded.
    pub upper_face: bool,
}

/// One face: a person's recording session, or a rendered identity.
pub struct Group {
    pub name: String,
    pub members: Vec<usize>,
    /// Each slot's frames, by record index.
    pub anchors: [Vec<usize>; ANCHOR_SLOTS.len()],
}

pub struct FaceFrames {
    pub records: Vec<FaceRecord>,
    /// `[5, size, size]` gray8 per record, or empty for a labels-only set.
    pub images: Vec<Vec<u8>>,
    pub size: usize,
    pub groups: Vec<Group>,
}

/// A frame's labels: the tongue's when it was labelled, the cheek puffs
/// (and, from the pose, the cheeks sucked in) when the cheeks were, then
/// anything named in its `face` labels.
pub fn labels(sample: &Sample) -> ([f32; OUTPUTS], [bool; OUTPUTS]) {
    let mut labels = [0.0; OUTPUTS];
    let mut labelled = [false; OUTPUTS];
    let visible = sample.targets[0] >= 0.5;
    labels[0] = sample.targets[0];
    labelled[0] = true;
    for column in 1..4 {
        labels[column] = sample.targets[column];
        labelled[column] = visible;
    }
    if sample.cheeks_labelled {
        for (output, column) in ["cheek_puff_left", "cheek_puff_right"]
            .into_iter()
            .zip(CHEEK_COLUMNS)
        {
            let index = target_index(output).expect("a face output");
            labels[index] = sample.targets[column];
            labelled[index] = true;
        }
        let sucked = if sample.pose == SUCK_POSE { 1.0 } else { 0.0 };
        for output in ["cheek_suck_left", "cheek_suck_right"] {
            let index = target_index(output).expect("a face output");
            labels[index] = sucked;
            labelled[index] = true;
        }
    }
    for (name, value) in &sample.face {
        if let Some(index) = target_index(name) {
            labels[index] = *value;
            labelled[index] = true;
        }
    }
    (labels, labelled)
}

impl FaceFrames {
    /// With `size` the views are resized and kept; without, only labels are
    /// read.
    pub fn load(recordings: &[Recording], size: Option<usize>) -> Result<Self> {
        let resize = size.filter(|size| *size < VIEW).map(AreaResize::new);
        let mut records = vec![];
        let mut images = vec![];
        let mut groups: Vec<Group> = vec![];
        let mut group_of: BTreeMap<String, usize> = BTreeMap::new();
        for recording in recordings {
            let layout = &recording.layout;
            let upper_face = EYE_CAMERAS
                .iter()
                .chain([&BROW_CAMERA])
                .all(|&camera| layout.has(camera));
            let selected = select(&recording.samples);
            if let Some(size) = size {
                if layout.view != VIEW && layout.view != size {
                    bail!(
                        "{} holds {} px views; this model reads {size} px",
                        recording.dir.display(),
                        layout.view
                    );
                }
                let indices: Vec<usize> = selected.iter().map(|sample| sample.index).collect();
                let plane = size * size;
                recording.read_whole(&indices, |frame| {
                    let mut image = vec![0u8; CAMERAS * plane];
                    for (camera, out) in image.chunks_mut(plane).enumerate() {
                        let Some(position) =
                            layout.cameras.iter().position(|&c| c as usize == camera)
                        else {
                            continue;
                        };
                        match &resize {
                            Some(resize) if layout.view == VIEW => {
                                resize.view_of(frame, layout.width(), position, out)
                            }
                            _ => {
                                layout.cut(frame, camera as u8, out);
                            }
                        }
                    }
                    images.push(image);
                })?;
            }
            for sample in selected {
                let name = sample.identity.clone().unwrap_or_else(|| recording.name());
                let group = *group_of.entry(name.clone()).or_insert_with(|| {
                    groups.push(Group {
                        name,
                        members: vec![],
                        anchors: Default::default(),
                    });
                    groups.len() - 1
                });
                let slot = match &sample.anchor {
                    Some(name) => slot_index(name),
                    None => slot_for_pose(&sample.pose),
                };
                let index = records.len();
                groups[group].members.push(index);
                if let Some(slot) = slot {
                    groups[group].anchors[slot].push(index);
                }
                let (labels, labelled) = labels(sample);
                records.push(FaceRecord {
                    labels,
                    labelled,
                    group,
                    slot,
                    key: sampling_key(sample),
                    synthetic: recording.synthetic,
                    native: sample.native,
                    upper_face,
                });
            }
        }
        if records.is_empty() {
            bail!("No usable poses in these recordings. Record a full basic run, then train again");
        }
        Ok(Self {
            records,
            images,
            size: size.unwrap_or(0),
            groups,
        })
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Outputs with enough labels to train. Visibility needs 20 frames each
    /// way, the directions 8 each way, everything else 8 active and 20
    /// resting frames.
    pub fn trainable(&self) -> Result<[bool; OUTPUTS]> {
        let count = |output: usize, test: &dyn Fn(f32) -> bool| {
            self.records
                .iter()
                .filter(|record| record.labelled[output] && test(record.labels[output]))
                .count()
        };
        let out = count(0, &|value| value >= 0.5);
        let inside = count(0, &|value| value < 0.5);
        if out < 20 || inside < 20 {
            bail!(
                "Training needs at least 20 tongue-out and 20 tongue-in frames. \
                 Record a full basic run, then train again"
            );
        }
        let mut enabled = [true; OUTPUTS];
        for (output, enabled) in enabled.iter_mut().enumerate().skip(1) {
            *enabled = if super::SIGNED.contains(&output) {
                count(output, &|value| value > ACTIVE) >= 8
                    && count(output, &|value| value < -ACTIVE) >= 8
            } else if output == 1 {
                count(output, &|value| value > ACTIVE) >= 8
            } else {
                count(output, &|value| value > ACTIVE) >= 8
                    && count(output, &|value| value <= ACTIVE) >= 20
            };
        }
        Ok(enabled)
    }

    /// Batches of up to `size` frames, each from at most two faces so their
    /// anchors are encoded once per batch. Faces are drawn by how many frames
    /// they have, and frames within a face so that each pose is equally
    /// likely. Covers about as many frames as there are.
    pub fn batches(&self, size: usize, rng: &mut StdRng) -> Vec<Vec<usize>> {
        let groups: Vec<&Group> = self
            .groups
            .iter()
            .filter(|g| !g.members.is_empty())
            .collect();
        let total: usize = groups.iter().map(|group| group.members.len()).sum();
        let per_group = size.div_ceil(2).max(1);
        let mut batches = vec![];
        let mut drawn = 0;
        while drawn < total {
            let mut batch = vec![];
            for _ in 0..2 {
                let mut pick = rng.random_range(0..total);
                let group = groups
                    .iter()
                    .find(|group| {
                        if pick < group.members.len() {
                            true
                        } else {
                            pick -= group.members.len();
                            false
                        }
                    })
                    .expect("a face is drawn");
                batch.extend(self.balanced(group, per_group.min(size - batch.len()), rng));
                if batch.len() >= size {
                    break;
                }
            }
            drawn += batch.len();
            batches.push(batch);
        }
        batches
    }

    /// `count` frames of `group`, each pose equally likely.
    fn balanced(&self, group: &Group, count: usize, rng: &mut StdRng) -> Vec<usize> {
        let mut keys: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for &member in &group.members {
            keys.entry(&self.records[member].key)
                .or_default()
                .push(member);
        }
        let keys: Vec<&Vec<usize>> = keys.values().collect();
        (0..count)
            .map(|_| {
                let key = keys[rng.random_range(0..keys.len())];
                key[rng.random_range(0..key.len())]
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn sample(pose: &str, targets: [f32; 12], cheeks: bool) -> Sample {
        Sample {
            index: 0,
            step: 0,
            pose: pose.into(),
            targets,
            cheeks_labelled: cheeks,
            native: None,
            moving: false,
            face: Default::default(),
            anchor: None,
            identity: None,
        }
    }

    #[test]
    fn recordings_label_what_they_know() {
        let mut out = [0.0; 12];
        out[0] = 1.0;
        out[1] = 1.0;
        out[2] = -1.0;
        let (values, labelled) = labels(&sample("Tongue left", out, true));
        assert_eq!(values[..3], [1.0, 1.0, -1.0]);
        assert!(labelled[..8].iter().all(|known| *known), "{labelled:?}");
        let jaw = target_index("jaw_open").unwrap();
        assert!(!labelled[jaw], "nothing labels the jaw");
        assert!(!labelled[super::super::BROW_START], "nor the brows");

        let (values, labelled) = labels(&sample(SUCK_POSE, [0.0; 12], true));
        let suck = target_index("cheek_suck_left").unwrap();
        assert_eq!((values[suck], labelled[suck]), (1.0, true));
        assert!(!labelled[1], "a hidden tongue has no direction");

        let (_, labelled) = labels(&sample("Neutral", [0.0; 12], false));
        assert!(
            !labelled[suck],
            "recordings before cheek labels say nothing of them"
        );

        let mut rendered = sample("Brows up", [0.0; 12], true);
        rendered.face.insert("brow_inner_up_left".into(), 0.8);
        rendered.face.insert("jaw_open".into(), 0.3);
        let (values, labelled) = labels(&rendered);
        assert_eq!(values[super::super::BROW_START], 0.8);
        assert!(labelled[jaw] && values[jaw] == 0.3);
    }

    #[test]
    fn batches_hold_at_most_two_faces() {
        let records = (0..30)
            .map(|index| FaceRecord {
                labels: [0.0; OUTPUTS],
                labelled: [false; OUTPUTS],
                group: index % 3,
                slot: None,
                key: (index % 4).to_string(),
                synthetic: false,
                native: None,
                upper_face: true,
            })
            .collect::<Vec<_>>();
        let groups = (0..3)
            .map(|group| Group {
                name: group.to_string(),
                members: (0..30).filter(|i| i % 3 == group).collect(),
                anchors: Default::default(),
            })
            .collect();
        let frames = FaceFrames {
            records,
            images: vec![],
            size: 0,
            groups,
        };
        let mut rng = StdRng::seed_from_u64(1);
        let batches = frames.batches(8, &mut rng);
        assert!(batches.iter().map(Vec::len).sum::<usize>() >= 30);
        for batch in &batches {
            assert!(batch.len() <= 8);
            let mut faces: Vec<usize> = batch.iter().map(|&i| frames.records[i].group).collect();
            faces.sort();
            faces.dedup();
            assert!(faces.len() <= 2);
        }
    }
}
