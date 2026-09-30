//! End-to-end personal training on small synthetic recordings, on the CPU,
//! from freshly initialised 32 px base models.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use vrft_tongue::backend::Cpu;
use vrft_tongue::checkpoint::{Metadata, VisibilityGate};
use vrft_tongue::model::{TongueNet, Trainable, Weights, ARCHITECTURE};
use vrft_tongue::preprocess::FRAME_BYTES;
use vrft_tongue::train::{run, Options};
use vrft_tongue::{Accelerator, Checkpoint, Role, TongueModel, TARGETS, TONGUE_TARGETS};

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

fn target(visible: f32, extension: f32, horizontal: f32, vertical: f32) -> [f32; 12] {
    [
        visible, extension, horizontal, vertical, 0., 0., 0., 0., 0., 0., 0., 0.,
    ]
}

/// A hidden tongue with these cheek puffs.
fn cheeks(left: f32, right: f32) -> [f32; 12] {
    let mut targets = target(0., 0., 0., 0.);
    targets[10] = left;
    targets[11] = right;
    targets
}

type Label<'a> = (u64, &'a str, [f32; 12], Option<[f32; 2]>);

/// Frames of one gray level each, labelled with `labels`.
fn recording(dir: &Path, labels: &[Label], first: u8) {
    write_recording(dir, labels, first, TARGETS.len());
}

/// A recording from before the cheek heads, with tongue labels alone.
fn legacy_recording(dir: &Path, labels: &[Label], first: u8) {
    write_recording(dir, labels, first, TONGUE_TARGETS);
}

fn write_recording(dir: &Path, labels: &[Label], first: u8, heads: usize) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("metadata.json"),
        json!({"format": "vrft-tongue-capture-v1", "bytesPerFrame": FRAME_BYTES,
            "targets": TARGETS[..heads]})
        .to_string(),
    )
    .unwrap();
    let mut frames = vec![];
    let mut lines = String::new();
    for (index, (step, pose, targets, dot)) in labels.iter().enumerate() {
        frames.extend(std::iter::repeat_n(
            first.wrapping_add(index as u8),
            FRAME_BYTES,
        ));
        let mut line = json!({"index": index, "step": step, "pose": pose,
            "targets": targets[..heads], "native_tongue_out": targets[0]});
        if let Some(dot) = dot {
            line["dot"] = json!(dot);
        }
        lines += &format!("{line}\n");
    }
    std::fs::write(dir.join("frames.gray8"), frames).unwrap();
    std::fs::write(dir.join("samples.jsonl"), lines).unwrap();
}

/// 64 frames in eight 8-frame poses: three hidden, five graded visible.
fn graded(dir: &Path, first: u8) {
    recording(dir, &graded_labels(), first);
}

fn graded_labels() -> Vec<Label<'static>> {
    let poses = [
        target(0., 0., 0., 0.),
        target(0., 0., 0., 0.),
        target(0., 0., 0., 0.),
        target(1., 0.5, 0.5, 0.),
        target(1., 1., -1., 0.),
        target(1., 0.75, 0., 0.5),
        target(1., 1., 0.7, -0.7),
        target(1., 0.25, 0., 0.),
    ];
    const NAMES: [&str; 8] = [
        "Pose 0", "Pose 1", "Pose 2", "Pose 3", "Pose 4", "Pose 5", "Pose 6", "Pose 7",
    ];
    (0..64)
        .map(|i| (i as u64 / 8, NAMES[i / 8], poses[i / 8], None))
        .collect()
}

fn write_request(root: &Path, base: &Path, recordings: &[PathBuf]) -> PathBuf {
    let request = root.join("request.json");
    std::fs::write(
        &request,
        json!({"device": "cpu", "base_model_dir": base, "recordings": recordings}).to_string(),
    )
    .unwrap();
    request
}

fn one_pass() -> Options {
    Options {
        epochs: 1,
        ..Options::default()
    }
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

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn full_training_report_and_inference_contract() {
    let root = temp("training");
    let base = root.join("base");
    base_models(&base);
    let recordings = [root.join("recording-0"), root.join("recording-1")];
    graded(&recordings[0], 1);
    graded(&recordings[1], 65);
    let follow = root.join("follow");
    let labels: Vec<Label> = (0..24)
        .map(|i| {
            let (v, h) = (i as f32 / 4.0).sin_cos();
            (0, "Follow the dot", target(1., 1., h, v), Some([h, v]))
        })
        .collect();
    recording(&follow, &labels, 130);
    let request = root.join("request.json");
    std::fs::write(
        &request,
        json!({"name": "Synthetic check", "device": "cpu", "base_model_dir": base,
            "recordings": [recordings[0], recordings[1], follow, recordings[0]]})
        .to_string(),
    )
    .unwrap();
    let output = root.join("output");
    let options = Options {
        epochs: 1,
        ..Options::default()
    };
    run(&request, &output, &options).unwrap();

    let report = read_json(&output.join("report.json"));
    assert_eq!(report["name"], "Synthetic check");
    assert_eq!(report["frames"], 152, "each recording is used once");
    assert_eq!(
        report["coverage"]["sources"],
        json!({"follow": 24, "poses": 128})
    );
    assert_eq!(report["epochs"], 1);
    assert_eq!(report["supported_targets"], json!(TARGETS[..4]));
    assert_eq!(report["disabled_targets"], json!(TARGETS[4..]));
    assert_eq!(report["calibration"]["camera_weight"], 0.8);
    let threshold = report["calibration"]["threshold"].as_f64().unwrap();
    assert!((0.3..=0.8).contains(&threshold));
    let progress = read_json(&output.join("progress.json"));
    assert_eq!(progress["stage"], "complete");
    assert_eq!(progress["fraction"], 1.0);

    for role in [Role::Gate, Role::Direction] {
        let saved = Checkpoint::load(&role.safetensors(&output)).unwrap();
        assert_eq!(saved.metadata.disabled_targets, TARGETS[4..]);
        assert_eq!(saved.metadata.personal_training.unwrap()["epochs"], 1);
        assert_eq!(saved.metadata.visibility_gate.threshold, threshold);
    }
    let mut model = TongueModel::load(&output, Accelerator::Cpu).unwrap();
    assert_eq!(model.info().disabled_targets, TARGETS[4..]);
    let values = model.predict(&vec![0u8; FRAME_BYTES]).unwrap();
    assert!(values.iter().all(|v| v.is_finite()));
    assert_eq!(values[4..], [0.0; 8]);

    // A finished run is never overwritten.
    assert!(run(&request, &output, &options).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cheek_puffs_train_on_hidden_tongue_frames() {
    let root = temp("cheeks");
    let base = root.join("base");
    base_models(&base);
    let dir = root.join("recording");
    let mut labels = graded_labels();
    let puffs = [
        ("Left cheek puffed", cheeks(1., 0.)),
        ("Right cheek puffed", cheeks(0., 1.)),
        ("Both cheeks puffed", cheeks(1., 1.)),
    ];
    for (step, (pose, targets)) in puffs.into_iter().enumerate() {
        labels.extend((0..8).map(|_| (8 + step as u64, pose, targets, None)));
    }
    recording(&dir, &labels, 1);
    let output = root.join("output");
    run(&write_request(&root, &base, &[dir]), &output, &one_pass()).unwrap();

    let report = read_json(&output.join("report.json"));
    let supported = report["supported_targets"].as_array().unwrap();
    assert!(supported.contains(&json!("cheek_puff_left")), "{report}");
    assert!(supported.contains(&json!("cheek_puff_right")), "{report}");
    let coverage = &report["coverage"]["targets"]["cheek_puff_left"];
    assert_eq!(coverage["positive"], 16, "tongue in or out: {coverage}");
    let model = TongueModel::load(&output, Accelerator::Cpu).unwrap();
    assert!(!model
        .info()
        .disabled_targets
        .iter()
        .any(|name| name.starts_with("cheek")));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn locked_layers_keep_the_base_weights() {
    let root = temp("locked");
    let base = root.join("base");
    base_models(&base);
    let dir = root.join("recording");
    graded(&dir, 1);
    let output = root.join("output");
    let options = Options {
        trainable: Trainable::Output,
        ..one_pass()
    };
    run(&write_request(&root, &base, &[dir]), &output, &options).unwrap();

    let values = |weights: &Weights, name: &str| weights[name].clone().to_vec::<f32>().unwrap();
    for role in [Role::Gate, Role::Direction] {
        let before = Checkpoint::load(&role.safetensors(&base)).unwrap().weights;
        let after = Checkpoint::load(&role.safetensors(&output)).unwrap();
        assert_eq!(
            after.metadata.personal_training.unwrap()["layers"],
            "output"
        );
        for name in before.keys() {
            let changed = values(&before, name) != values(&after.weights, name);
            assert_eq!(changed, name.starts_with("head.6."), "{role:?} {name}");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn old_recordings_leave_the_cheeks_untrained() {
    let root = temp("legacy");
    let base = root.join("base");
    base_models(&base);
    let dir = root.join("recording");
    legacy_recording(&dir, &graded_labels(), 1);
    let output = root.join("output");
    run(&write_request(&root, &base, &[dir]), &output, &one_pass()).unwrap();
    let report = read_json(&output.join("report.json"));
    let disabled = report["disabled_targets"].as_array().unwrap();
    assert!(disabled.contains(&json!("cheek_puff_left")), "{report}");
    assert_eq!(
        report["coverage"]["targets"]["cheek_puff_left"]["positive"],
        0
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn models_from_before_the_cheek_heads_load_with_them_disabled() {
    let dir = temp("old-model");
    let mut weights = TongueNet::<Cpu>::init(&Default::default()).weights();
    for (name, rows) in [("head.6.weight", 192), ("head.6.bias", 1)] {
        let values: Vec<f32> = weights[name].to_vec().unwrap();
        let shape = if rows == 1 {
            vec![TONGUE_TARGETS]
        } else {
            vec![TONGUE_TARGETS, rows]
        };
        weights.insert(
            name.into(),
            burn::tensor::TensorData::new(values[..TONGUE_TARGETS * rows].to_vec(), shape),
        );
    }
    for role in [Role::Gate, Role::Direction] {
        let checkpoint = Checkpoint {
            metadata: Metadata {
                architecture: ARCHITECTURE.into(),
                image_size: 32,
                visibility_gate: VisibilityGate::default(),
                disabled_targets: vec!["roll".into()],
                personal_training: None,
            },
            weights: weights.clone(),
        };
        checkpoint.save(&role.safetensors(&dir)).unwrap();
    }
    let loaded = Checkpoint::load(&Role::Direction.safetensors(&dir)).unwrap();
    assert_eq!(
        loaded.metadata.disabled_targets,
        ["roll", "cheek_puff_left", "cheek_puff_right"]
    );
    let mut model = TongueModel::load(&dir, Accelerator::Cpu).unwrap();
    let values = model.predict(&vec![90u8; FRAME_BYTES]).unwrap();
    assert_eq!(values[10..], [0.0; 2]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn missing_directions_are_named_in_progress() {
    let root = temp("missing");
    let base = root.join("base");
    base_models(&base);
    let recording_dir = root.join("recording");
    let labels: Vec<Label> = (0..48)
        .map(|i| {
            let targets = if i < 24 {
                target(0., 0., 0., 0.)
            } else {
                target(1., 1., 1., 0.)
            };
            (i as u64 / 8, "Pose", targets, None)
        })
        .collect();
    recording(&recording_dir, &labels, 1);
    let request = root.join("request.json");
    std::fs::write(
        &request,
        json!({"device": "cpu", "base_model_dir": base, "recordings": [recording_dir]}).to_string(),
    )
    .unwrap();
    let output = root.join("output");
    let error = format!(
        "{:#}",
        run(&request, &output, &Options::default()).unwrap_err()
    );
    assert!(
        error.contains("No usable tongue left, up, down poses"),
        "{error}"
    );
    assert_eq!(read_json(&output.join("progress.json"))["stage"], "failed");
    std::fs::remove_dir_all(root).unwrap();
}
