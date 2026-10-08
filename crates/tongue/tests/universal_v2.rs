//! Fine-tuning a `universal-face-v2` model end to end, on a tiny stand-in for
//! QFT+'s (its graph pools the strip, its heads are random) and synthetic
//! five-camera recordings. Needs ONNX Runtime's library, as `tests/onnx.rs`
//! does; without it the test says so and passes.

use std::path::{Path, PathBuf};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::json;
use sha2::{Digest, Sha256};
use vrft_quest_pro_protocol::{
    CameraLayout, TrainerArchitecture, TrainerRequest, TrainingDevice, TrainingReport, STRIP_BYTES,
    STRIP_WIDTH,
};
use vrft_tongue::onnx::proto::{Attribute, DataType, Graph, Initializer, Node, ValueInfo};
use vrft_tongue::train::{run, Options};
use vrft_tongue::universal::Enrollment;
use vrft_tongue::universal_v2::npz::{self, Array};
use vrft_tongue::universal_v2::{UniversalV2, FILE_NAME, GRAPH_FILE};
use vrft_tongue::{Accelerator, TARGETS};

/// The strip pools to 5 x 25 cells of 80 px, five across each camera.
const CELLS: usize = 125;

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

fn floats(name: &str, dims: &[usize], values: &[f32]) -> Initializer {
    Initializer {
        name: name.into(),
        dims: dims.iter().map(|&d| d as i64).collect(),
        data_type: DataType::Float,
        raw: values.iter().flat_map(|v| v.to_le_bytes()).collect(),
    }
}

fn shape(name: &str, dims: &[i64]) -> Initializer {
    Initializer {
        name: name.into(),
        dims: vec![dims.len() as i64],
        data_type: DataType::Int64,
        raw: dims.iter().flat_map(|v| v.to_le_bytes()).collect(),
    }
}

fn node(op: &str, inputs: &[&str], output: &str, attributes: Vec<(&str, Attribute)>) -> Node {
    Node {
        op_type: op.into(),
        inputs: inputs.iter().map(|s| s.to_string()).collect(),
        outputs: vec![output.into()],
        attributes: attributes
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    }
}

fn random(rng: &mut StdRng, count: usize, spread: f32) -> Vec<f32> {
    (0..count)
        .map(|_| rng.random_range(-spread..spread))
        .collect()
}

/// A graph of QFT+'s inputs and outputs: the strip's pooled cells, times
/// random weights.
fn graph(rng: &mut StdRng) -> Vec<u8> {
    let mut nodes = vec![
        node(
            "Cast",
            &["cameras"],
            "pixels",
            vec![("to", Attribute::Int(1))],
        ),
        node("Reshape", &["pixels", "image_shape"], "image", vec![]),
        node(
            "AveragePool",
            &["image"],
            "pooled",
            vec![
                ("kernel_shape", Attribute::Ints(vec![80, 80])),
                ("strides", Attribute::Ints(vec![80, 80])),
            ],
        ),
        node("Reshape", &["pooled", "cells_shape"], "cells", vec![]),
    ];
    let mut initializers = vec![
        shape("image_shape", &[1, 1, 400, STRIP_WIDTH as i64]),
        shape("cells_shape", &[1, CELLS as i64]),
    ];
    let mut outputs = vec![];
    for (name, width) in [("mouth", 512), ("tongue", 7), ("brows", 480)] {
        let weights = format!("{name}_weights");
        initializers.push(floats(
            &weights,
            &[CELLS, width],
            &random(rng, CELLS * width, 0.02),
        ));
        nodes.push(node("MatMul", &["cells", &weights], name, vec![]));
        outputs.push(ValueInfo {
            name: name.into(),
            data_type: DataType::Float,
            dims: vec![1, width as i64],
        });
    }
    Graph {
        name: "universal-face-v2-test".into(),
        nodes,
        initializers,
        inputs: vec![ValueInfo {
            name: "cameras".into(),
            data_type: DataType::Uint8,
            dims: vec![400, STRIP_WIDTH as i64],
        }],
        outputs,
    }
    .to_model()
}

const NAMES: [&str; 5] = [
    "CheekPuffLeft",
    "CheekPuffRight",
    "CheekSuckLeft",
    "CheekSuckRight",
    "TongueOut",
];

/// A stand-in for QFT+'s model in `dir`.
fn base_model(dir: &Path, rng: &mut StdRng) {
    std::fs::create_dir_all(dir).unwrap();
    let graph = graph(rng);
    std::fs::write(dir.join(GRAPH_FILE), &graph).unwrap();
    let meta = json!({
        "schema": "universal-face-v2", "names": NAMES,
        "slots": ["neutral", "jaw_open", "pucker", "puff", "tongue_out", "suck"],
        "browNames": ["BrowInnerUpLeft", "BrowInnerUpRight", "BrowOuterUpLeft",
            "BrowOuterUpRight", "BrowLowererLeft", "BrowLowererRight", "BrowPinchLeft",
            "BrowPinchRight"],
        "imageSize": 128, "approvedForOutput": true, "allowsNoEnrollment": true,
        "provenance": "test", "graphSha256": format!("{:x}", Sha256::digest(&graph)),
    });
    let mut float = |shape: Vec<usize>, spread: f32| {
        let count = shape.iter().product();
        Array::Float {
            shape,
            values: random(rng, count, spread),
        }
    };
    let arrays = vec![
        ("meta".to_string(), Array::Text(meta.to_string())),
        ("head_missing".into(), float(vec![6, 512], 0.1)),
        ("head_w1".into(), float(vec![16, 4102], 0.05)),
        ("head_b1".into(), float(vec![16], 0.1)),
        ("head_w2".into(), float(vec![8, 16], 0.3)),
        ("head_b2".into(), float(vec![8], 0.1)),
        ("head_w3".into(), float(vec![NAMES.len(), 8], 0.3)),
        ("head_b3".into(), float(vec![NAMES.len()], 0.1)),
        ("brow_missing".into(), float(vec![480], 0.1)),
        ("brow_w1".into(), float(vec![8, 961], 0.05)),
        ("brow_b1".into(), float(vec![8], 0.1)),
        ("brow_w2".into(), float(vec![8, 8], 0.3)),
        ("brow_b2".into(), float(vec![8], 0.1)),
    ];
    npz::write(&dir.join(FILE_NAME), &arrays).unwrap();
}

/// One pose: its name, the tongue and cheek labels, face labels and slot.
type Pose = (
    &'static str,
    [f32; 12],
    serde_json::Value,
    Option<&'static str>,
);

fn targets(visible: f32, h: f32, v: f32, puffs: [f32; 2]) -> [f32; 12] {
    let mut targets = [0.0; 12];
    targets[..4].copy_from_slice(&[visible, visible, h, v]);
    targets[10..].copy_from_slice(&puffs);
    targets
}

/// A strip that shows the pose: each camera's cells brighten with what it
/// would see.
fn strip(rng: &mut StdRng, labels: &[f32; 12], face: &serde_json::Value) -> Vec<u8> {
    let value = |name: &str| face[name].as_f64().unwrap_or(0.0) as f32;
    // Per camera, the brightness of its left and right halves.
    let levels = [
        [value("brow_inner_up_left"), value("brow_lowerer_left")],
        [value("brow_inner_up_right"), value("brow_lowerer_right")],
        [labels[10], labels[0] * (1.0 + labels[2])],
        [labels[11], labels[0] * (1.0 + labels[3])],
        [value("cheek_suck_left"), value("cheek_suck_right")],
    ];
    let mut strip = vec![0u8; STRIP_BYTES];
    for (row, line) in strip.chunks_mut(STRIP_WIDTH as usize).enumerate() {
        for (column, pixel) in line.iter_mut().enumerate() {
            let [left, right] = levels[column / 400];
            let level = if column % 400 < 200 { left } else { right };
            let noise: f32 = rng.random_range(-12.0..12.0);
            *pixel = (60.0 + 80.0 * level + (row % 40) as f32 + noise).clamp(0.0, 255.0) as u8;
        }
    }
    strip
}

fn record(dir: &Path, mode: &str, poses: &[Pose], frames: usize, rng: &mut StdRng) {
    std::fs::create_dir_all(dir).unwrap();
    let mut metadata = CameraLayout::all().metadata();
    metadata["format"] = "vrft-tongue-capture-v1".into();
    metadata["mode"] = mode.into();
    metadata["targets"] = json!(TARGETS);
    std::fs::write(dir.join("metadata.json"), metadata.to_string()).unwrap();
    let (mut lines, mut pixels, mut index) = (String::new(), vec![], 0);
    for (step, (name, labels, face, anchor)) in poses.iter().enumerate() {
        for _ in 0..frames {
            let mut line = json!({"index": index, "step": step, "pose": name,
                "targets": labels, "face": face});
            if let Some(anchor) = anchor {
                line["anchor"] = json!(anchor);
            }
            lines += &format!("{line}\n");
            pixels.extend(strip(rng, labels, face));
            index += 1;
        }
    }
    std::fs::write(dir.join("samples.jsonl"), lines).unwrap();
    std::fs::write(dir.join("frames.gray8"), pixels).unwrap();
}

fn face_setup() -> Vec<Pose> {
    let calm = json!({"cheek_suck_left": 0.0, "cheek_suck_right": 0.0,
        "brow_inner_up_left": 0.0, "brow_inner_up_right": 0.0,
        "brow_lowerer_left": 0.0, "brow_lowerer_right": 0.0});
    let none = json!({});
    vec![
        (
            "Relax",
            targets(0., 0., 0., [0., 0.]),
            calm,
            Some("neutral"),
        ),
        (
            "Puff",
            targets(0., 0., 0., [1., 1.]),
            none.clone(),
            Some("puff"),
        ),
        (
            "Out",
            targets(1., 0., 0., [0., 0.]),
            none.clone(),
            Some("tongue_out"),
        ),
        (
            "Up",
            targets(1., 0., 1., [0., 0.]),
            none.clone(),
            Some("tongue_up"),
        ),
        (
            "Down",
            targets(1., 0., -1., [0., 0.]),
            none.clone(),
            Some("tongue_down"),
        ),
        (
            "Left",
            targets(1., -1., 0., [0., 0.]),
            none.clone(),
            Some("tongue_left"),
        ),
        (
            "Right",
            targets(1., 1., 0., [0., 0.]),
            none,
            Some("tongue_right"),
        ),
    ]
}

fn face_recording() -> Vec<Pose> {
    let face = |pairs: &[(&str, f32)]| {
        let mut face = json!({"cheek_suck_left": 0.0, "cheek_suck_right": 0.0});
        for (name, value) in pairs {
            face[*name] = json!(value);
        }
        face
    };
    let brows_at = |inner: f32, lower: f32| {
        face(&[
            ("brow_inner_up_left", inner),
            ("brow_inner_up_right", inner),
            ("brow_lowerer_left", lower),
            ("brow_lowerer_right", lower),
        ])
    };
    let none = face(&[]);
    vec![
        (
            "Relax",
            targets(0., 0., 0., [0., 0.]),
            brows_at(0., 0.),
            None,
        ),
        (
            "Puff left",
            targets(0., 0., 0., [1., 0.]),
            none.clone(),
            None,
        ),
        (
            "Puff right",
            targets(0., 0., 0., [0., 1.]),
            none.clone(),
            None,
        ),
        (
            "Suck",
            targets(0., 0., 0., [0., 0.]),
            face(&[("cheek_suck_left", 1.0), ("cheek_suck_right", 1.0)]),
            None,
        ),
        (
            "Brows up",
            targets(0., 0., 0., [0., 0.]),
            brows_at(1., 0.),
            None,
        ),
        (
            "Frown",
            targets(0., 0., 0., [0., 0.]),
            brows_at(0., 1.),
            None,
        ),
        (
            "Tongue left",
            targets(1., -1., 0., [0., 0.]),
            none.clone(),
            None,
        ),
        (
            "Tongue right",
            targets(1., 1., 0., [0., 0.]),
            none.clone(),
            None,
        ),
        (
            "Tongue up",
            targets(1., 0., 1., [0., 0.]),
            none.clone(),
            None,
        ),
        ("Tongue down", targets(1., 0., -1., [0., 0.]), none, None),
    ]
}

#[test]
fn qftplus_heads_fine_tune_into_a_model_qfts_loader_reads() {
    if let Err(error) = vrft_tongue::onnx::runtime::available() {
        eprintln!("skipped: {error:#}");
        return;
    }
    let root = temp("universal-v2");
    let mut rng = StdRng::seed_from_u64(7);
    let base = root.join("qftplus");
    base_model(&base, &mut rng);
    let setup = root.join("setup");
    record(&setup, "enrollment", &face_setup(), 4, &mut rng);
    let recording = root.join("face");
    record(&recording, "face", &face_recording(), 12, &mut rng);
    let request = TrainerRequest {
        name: Some("Mine".into()),
        device: TrainingDevice::Cpu,
        base_model_dir: base.clone(),
        recordings: vec![recording.clone()],
        architecture: TrainerArchitecture::UniversalFaceV2,
        face_setup: Some(setup.clone()),
    };
    std::fs::write(
        root.join("request.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    let output = root.join("run");
    let options = Options {
        epochs: 4,
        learning_rate: Some(1e-3),
    };
    run(&root.join("request.json"), &output, &options).unwrap();

    let report: TrainingReport =
        serde_json::from_slice(&std::fs::read(output.join("report.json")).unwrap()).unwrap();
    assert_eq!(report.name, "Mine");
    assert_eq!(
        report.face_setup.as_deref(),
        Some(&*setup.display().to_string())
    );
    assert_eq!(report.frames, Some(120));
    assert_eq!(report.held_out_frames, 30, "the last fifth of each pose");
    let face = report
        .kept
        .iter()
        .find(|kept| kept.focus == "face")
        .unwrap();
    assert_eq!(face.scores.len(), 5, "QFT+'s heads, then each pass");
    assert!(face.scores[face.epoch as usize] <= face.scores[0]);
    for name in ["CheekPuffLeft", "CheekSuckRight", "BrowLowererLeft"] {
        assert!(report.supported_targets.iter().any(|n| n == name), "{name}");
    }
    assert!(report
        .disabled_targets
        .iter()
        .any(|name| name == "BrowPinchLeft"));
    assert_eq!(
        std::fs::read(output.join(GRAPH_FILE)).unwrap(),
        std::fs::read(base.join(GRAPH_FILE)).unwrap()
    );

    let before = npz::read(&base.join(FILE_NAME)).unwrap();
    let after = npz::read(&output.join(FILE_NAME)).unwrap();
    let meta = |arrays: &std::collections::HashMap<String, Array>| -> serde_json::Value {
        match &arrays["meta"] {
            Array::Text(text) => serde_json::from_str(text).unwrap(),
            _ => panic!("meta is text"),
        }
    };
    let (old, new) = (meta(&before), meta(&after));
    assert_eq!(new["graphSha256"], old["graphSha256"]);
    assert_eq!(
        new.as_object().unwrap().keys().collect::<Vec<_>>(),
        old.as_object().unwrap().keys().collect::<Vec<_>>(),
        "QFT+'s loader takes exactly its own keys"
    );
    let values = |arrays: &std::collections::HashMap<String, Array>, name: &str| {
        arrays[name].floats(name).unwrap().1.to_vec()
    };
    // The weights that read the anchors and presence flags stay QFT+'s.
    let (w1, new_w1) = (values(&before, "head_w1"), values(&after, "head_w1"));
    for (row, new_row) in w1.chunks(4102).zip(new_w1.chunks(4102)) {
        assert_eq!(row[512..3584], new_row[512..3584]);
        assert_eq!(row[4096..], new_row[4096..]);
    }
    assert_ne!(w1, new_w1, "the part that reads q trains");
    assert_eq!(
        values(&before, "head_missing"),
        values(&after, "head_missing")
    );
    let tongue = report.kept.iter().find(|kept| kept.focus == "tongue");
    assert_eq!(
        after.contains_key("tongue_weights"),
        tongue.is_some_and(|kept| kept.epoch == 1)
    );

    let mut model = UniversalV2::load(&output.join(FILE_NAME), Accelerator::Cpu).unwrap();
    model
        .enroll(&Enrollment::from_recording(&setup).unwrap().frames)
        .unwrap();
    assert!(model.has_tongue_map());
    std::fs::remove_dir_all(root).unwrap();
}
