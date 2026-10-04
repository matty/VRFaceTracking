//! The universal face model end to end on a tiny fixture, on the CPU: one
//! training pass at 64 px over two rendered-style faces with enrollment
//! poses, then loading the result, enrolling a face setup and predicting.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use vrft_quest_pro_protocol::{CameraLayout, STRIP_BYTES};
use vrft_tongue::backend::Cpu;
use vrft_tongue::checkpoint::{Metadata, VisibilityGate};
use vrft_tongue::model::{TongueNet, ARCHITECTURE};
use vrft_tongue::train::{run, Options};
use vrft_tongue::universal::{
    Enrollment, FaceCheckpoint, FaceModel, ANCHOR_SLOTS, FACE_TARGETS, FILE_NAME,
};
use vrft_tongue::{Accelerator, Checkpoint, Role, TARGETS};

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

fn base_models(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    for role in [Role::Gate, Role::Direction] {
        Checkpoint {
            metadata: Metadata {
                architecture: ARCHITECTURE.into(),
                image_size: 32,
                visibility_gate: VisibilityGate::default(),
                disabled_targets: vec![],
                personal_training: None,
            },
            weights: TongueNet::<Cpu>::init(&Default::default()).weights(),
        }
        .save(&role.safetensors(dir))
        .unwrap();
    }
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
fn trains_loads_enrolls_and_predicts() {
    let root = temp("universal");
    let base = root.join("base");
    base_models(&base);
    let packed = CameraLayout {
        cameras: vec![0, 1, 2, 3, 4],
        view: SIZE,
    };
    let set = root.join("set");
    write_set(&set, &packed, &["id-a", "id-b"], 8);
    let request = root.join("request.json");
    std::fs::write(
        &request,
        json!({"name": "Faces", "device": "cpu", "base_model_dir": base,
            "recordings": [set], "architecture": "universal-face-v1"})
        .to_string(),
    )
    .unwrap();
    let output = root.join("output");
    let options = Options {
        epochs: 1,
        batch_size: 8,
        image_size: Some(SIZE),
        ..Options::default()
    };
    run(&request, &output, &options).unwrap();

    let report: Value =
        serde_json::from_slice(&std::fs::read(output.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["name"], "Faces");
    assert_eq!(report["frames"], 2 * 11 * 8);
    assert_eq!(report["coverage"]["faces"], 2);
    assert_eq!(report["coverage"]["facesWithSlot"]["neutral"], 2);
    let supported: Vec<&str> = report["supported_targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    for trained in [
        "visibility",
        "horizontal",
        "vertical",
        "cheek_puff_left",
        "cheek_suck_right",
        "jaw_open",
        "brow_inner_up_left",
    ] {
        assert!(
            supported.contains(&trained),
            "{trained} trains: {supported:?}"
        );
    }
    // Nothing labels the outer brows or the frown.
    assert!(report["disabled_targets"]
        .as_array()
        .unwrap()
        .contains(&json!("brow_pinch_left")));
    let progress: Value =
        serde_json::from_slice(&std::fs::read(output.join("progress.json")).unwrap()).unwrap();
    assert_eq!(progress["stage"], "complete");

    let checkpoint = FaceCheckpoint::load(&output.join(FILE_NAME)).unwrap();
    assert_eq!(checkpoint.metadata.image_size, SIZE);
    assert_eq!(checkpoint.metadata.outputs, FACE_TARGETS);
    assert_eq!(checkpoint.metadata.training.unwrap()["epochs"], 1);

    // A face setup at the headset's size: every camera, 400 px.
    let setup = root.join("setup");
    write_set(&setup, &CameraLayout::all(), &["wearer"], 2);
    let enrollment = Enrollment::from_recording(&setup).unwrap();
    assert!(ANCHOR_SLOTS
        .iter()
        .all(|slot| enrollment.frames.contains_key(*slot)));
    let mut model = FaceModel::load(&output.join(FILE_NAME), Accelerator::Cpu).unwrap();
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

#[test]
fn mouth_only_recordings_need_no_upper_face() {
    let root = temp("universal-mouth");
    let base = root.join("base");
    base_models(&base);
    let mouth = CameraLayout {
        cameras: vec![2, 3],
        view: SIZE,
    };
    let set = root.join("set");
    write_set(&set, &mouth, &["only"], 8);
    let request = root.join("request.json");
    std::fs::write(
        &request,
        json!({"device": "cpu", "base_model_dir": base, "recordings": [set],
            "architecture": "universal-face-v1"})
        .to_string(),
    )
    .unwrap();
    let output = root.join("output");
    let options = Options {
        epochs: 1,
        batch_size: 8,
        image_size: Some(SIZE),
        ..Options::default()
    };
    run(&request, &output, &options).unwrap();
    let report: Value =
        serde_json::from_slice(&std::fs::read(output.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["coverage"]["upperFace"], 0);
    std::fs::remove_dir_all(root).unwrap();
}
