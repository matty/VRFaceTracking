//! Universal face models that earlier versions trained still load, enroll
//! a face setup and predict, on the CPU, from a freshly initialised 64 px
//! model.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use vrft_quest_pro_protocol::{CameraLayout, STRIP_BYTES};
use vrft_tongue::backend::Cpu;
use vrft_tongue::universal::net::FaceNet;
use vrft_tongue::universal::{
    Enrollment, FaceCheckpoint, FaceMetadata, FaceModel, ANCHOR_SLOTS, FACE_TARGETS, FILE_NAME,
};
use vrft_tongue::{Accelerator, TARGETS};

const SIZE: usize = 64;

fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One frame's labels: the tongue's twelve, then `face` labels and slot.
struct Pose {
    name: &'static str,
    targets: [f32; 12],
    face: Value,
    anchor: Option<&'static str>,
}

fn tongue(visible: f32, h: f32, v: f32) -> [f32; 12] {
    let mut targets = [0.0; 12];
    targets[0] = visible;
    targets[1] = visible;
    targets[2] = h;
    targets[3] = v;
    targets
}

fn poses() -> Vec<Pose> {
    let pose = |name, targets, face: Value, anchor| Pose {
        name,
        targets,
        face,
        anchor,
    };
    let mut puffed = tongue(0., 0., 0.);
    puffed[10] = 1.0;
    puffed[11] = 1.0;
    vec![
        pose(
            "Neutral",
            tongue(0., 0., 0.),
            json!({"jaw_open": 0.0, "brow_inner_up_left": 0.0, "brow_inner_up_right": 0.0}),
            Some("neutral"),
        ),
        pose(
            "Jaw open",
            tongue(0., 0., 0.),
            json!({"jaw_open": 1.0, "brow_inner_up_left": 0.0, "brow_inner_up_right": 0.0}),
            Some("jaw_open"),
        ),
        pose(
            "Kiss",
            tongue(0., 0., 0.),
            json!({"jaw_open": 0.0, "brow_inner_up_left": 0.0, "brow_inner_up_right": 0.0}),
            Some("pucker"),
        ),
        pose("Puff", puffed, json!({}), Some("puff")),
        pose(
            "Suck",
            tongue(0., 0., 0.),
            json!({"cheek_suck_left": 1.0, "cheek_suck_right": 1.0}),
            Some("suck"),
        ),
        pose(
            "Brows up",
            tongue(0., 0., 0.),
            json!({"jaw_open": 0.0, "brow_inner_up_left": 1.0, "brow_inner_up_right": 1.0}),
            None,
        ),
        pose(
            "Tongue out",
            tongue(1., 0., 0.),
            json!({}),
            Some("tongue_out"),
        ),
        pose(
            "Tongue up",
            tongue(1., 0., 1.),
            json!({}),
            Some("tongue_up"),
        ),
        pose(
            "Tongue down",
            tongue(1., 0., -1.),
            json!({}),
            Some("tongue_down"),
        ),
        pose(
            "Tongue left",
            tongue(1., -1., 0.),
            json!({}),
            Some("tongue_left"),
        ),
        pose(
            "Tongue right",
            tongue(1., 1., 0.),
            json!({}),
            Some("tongue_right"),
        ),
    ]
}

/// A five-camera set in `layout`, `per_pose` frames of each pose for each
/// of `identities`, each frame a flat gray level per camera that depends on
/// the pose.
fn write_set(dir: &Path, layout: &CameraLayout, identities: &[&str], per_pose: usize) {
    std::fs::create_dir_all(dir).unwrap();
    let mut metadata = layout.metadata();
    metadata["format"] = "vrft-tongue-capture-v1".into();
    metadata["mode"] = "synthetic".into();
    metadata["targets"] = json!(TARGETS);
    std::fs::write(dir.join("metadata.json"), metadata.to_string()).unwrap();
    let mut frames = vec![];
    let mut lines = String::new();
    let mut index = 0;
    for (person, identity) in identities.iter().enumerate() {
        for (step, pose) in poses().iter().enumerate() {
            for repeat in 0..per_pose {
                let mut frame = vec![0u8; layout.frame_bytes()];
                let width = layout.width();
                for (row, line) in frame.chunks_mut(width).enumerate() {
                    for (x, pixel) in line.iter_mut().enumerate() {
                        let camera = x / layout.view;
                        *pixel =
                            (20 + step * 19 + camera * 7 + person * 3 + repeat + row % 3) as u8;
                    }
                }
                frames.extend(frame);
                let mut line = json!({"index": index, "step": step + person * 100, "pose": pose.name,
                    "targets": pose.targets, "face": pose.face, "identity": identity});
                if let Some(anchor) = pose.anchor {
                    line["anchor"] = anchor.into();
                }
                lines += &format!("{line}\n");
                index += 1;
            }
        }
    }
    std::fs::write(dir.join("frames.gray8"), frames).unwrap();
    std::fs::write(dir.join("samples.jsonl"), lines).unwrap();
}

#[test]
fn loads_enrolls_and_predicts() {
    let root = temp("universal");
    let path = root.join(FILE_NAME);
    FaceCheckpoint {
        metadata: FaceMetadata::new(SIZE),
        weights: FaceNet::<Cpu>::init(&Default::default()).weights(),
    }
    .save(&path)
    .unwrap();
    let checkpoint = FaceCheckpoint::load(&path).unwrap();
    assert_eq!(checkpoint.metadata.image_size, SIZE);
    assert_eq!(checkpoint.metadata.outputs, FACE_TARGETS);

    // A face setup at the headset's size: every camera, 400 px.
    let setup = root.join("setup");
    write_set(&setup, &CameraLayout::all(), &["wearer"], 2);
    let enrollment = Enrollment::from_recording(&setup).unwrap();
    assert!(ANCHOR_SLOTS
        .iter()
        .all(|slot| enrollment.frames.contains_key(*slot)));
    let mut model = FaceModel::load(&path, Accelerator::Cpu).unwrap();
    let blank = vec![90u8; STRIP_BYTES];
    let before = model.predict(&blank).unwrap();
    model.enroll(&enrollment).unwrap();
    assert_eq!(model.info().enrolled.len(), ANCHOR_SLOTS.len());
    assert!(model.info().tongue_map, "the held tongue poses fit a map");
    let after = model.predict(&blank).unwrap();
    for (index, name) in FACE_TARGETS.iter().enumerate() {
        let value = after.values[index];
        assert!(value.is_finite() && value.abs() <= 1.0, "{name} = {value}");
        if !model.enabled(index) {
            assert_eq!(value, 0.0, "{name} isn't trained");
        }
    }
    // Enrolling changes what the anchor-conditioned heads read.
    assert_ne!(before.values[4..], after.values[4..]);
    std::fs::remove_dir_all(root).unwrap();
}
