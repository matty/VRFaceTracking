//! The wearer's face setup, as frames a face model reads against.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Result};
use vrft_quest_pro_protocol::{STRIP_BYTES, STRIP_WIDTH};

use crate::preprocess::VIEW;
use crate::recordings::Recording;

/// The face setup's frames, by slot name: the six anchor slots, the
/// one-sided puffs and the tongue's held directions.
#[derive(Default)]
pub struct Enrollment {
    pub frames: BTreeMap<String, Vec<Vec<u8>>>,
}

impl Enrollment {
    /// Reads every frame of a face setup recording that shows a slot. Its
    /// frames must hold all five cameras or the mouth pair at the headset's
    /// size; mouth-only frames get blank eye and brow views.
    pub fn from_recording(dir: &Path) -> Result<Self> {
        let recording = Recording::open(dir)?;
        if recording.layout.view != VIEW {
            bail!("{} isn't a headset recording", dir.display());
        }
        let mut indices: Vec<(usize, String)> = recording
            .samples
            .iter()
            .filter_map(|sample| Some((sample.index, sample.anchor.clone()?)))
            .collect();
        indices.sort();
        let mut frames: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
        let mut slots = indices.iter().map(|(_, slot)| slot);
        let layout = recording.layout.clone();
        recording.read_whole(
            &indices.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            |frame| {
                let slot = slots.next().expect("a slot per frame").clone();
                let mut strip = vec![0u8; STRIP_BYTES];
                for (position, &camera) in layout.cameras.iter().enumerate() {
                    for row in 0..VIEW {
                        strip[row * STRIP_WIDTH as usize + camera as usize * VIEW..][..VIEW]
                            .copy_from_slice(
                                &frame[row * layout.width() + position * VIEW..][..VIEW],
                            );
                    }
                }
                frames.entry(slot).or_default().push(strip);
            },
        )?;
        if frames.is_empty() {
            bail!("{} has no face setup poses", dir.display());
        }
        Ok(Self { frames })
    }
}
