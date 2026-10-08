//! The universal face model's labels for a recorded frame, for scoring
//! models of it (`examples/evaluate_face.rs`).

use super::{target_index, FACE_TARGETS};
use crate::recordings::Sample;
use crate::CHEEK_COLUMNS;

const OUTPUTS: usize = FACE_TARGETS.len();
/// The recorded pose that labels both cheeks sucked in.
const SUCK_POSE: &str = "Cheeks sucked in";

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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pose: &str, targets: [f32; 12], cheeks: bool) -> Sample {
        Sample {
            index: 0,
            step: 0,
            pose: pose.into(),
            targets,
            cheeks_labelled: cheeks,
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
}
