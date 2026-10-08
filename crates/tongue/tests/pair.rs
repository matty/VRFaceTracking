//! Tongue pairs that earlier versions trained still load and predict, on
//! the CPU, from freshly initialised 32 px models.

use std::path::{Path, PathBuf};

use vrft_tongue::backend::Cpu;
use vrft_tongue::checkpoint::{Metadata, VisibilityGate};
use vrft_tongue::model::{TongueNet, ARCHITECTURE};
use vrft_tongue::preprocess::FRAME_BYTES;
use vrft_tongue::{Accelerator, Checkpoint, Role, TongueModel, TONGUE_TARGETS};

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
                tongue_out: None,
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
    assert!(values.iter().all(|value| value.is_finite()));
    assert_eq!(values[10..], [0.0; 2]);
    std::fs::remove_dir_all(dir).unwrap();
}
