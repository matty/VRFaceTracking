//! The ONNX graphs compute what the Burn models compute. Needs ONNX Runtime's
//! library (`VRFT_ONNXRUNTIME`, beside the test binary, or
//! `.local/onnxruntime/` under the working directory); without it each test
//! says so and passes.

use burn::tensor::{Tensor, TensorData};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use vrft_tongue::backend::{Accelerator, Cpu};
use vrft_tongue::model::{TongueNet, Weights};
use vrft_tongue::onnx::{self, runtime::Session};
use vrft_tongue::universal::net::{activate, FaceNet};

fn runtime() -> bool {
    match onnx::runtime::available() {
        Ok(()) => true,
        Err(error) => {
            eprintln!("skipped: {error:#}");
            false
        }
    }
}

/// Random BatchNorm statistics, so folding them into the convolutions is
/// tested, and random values in 0..1.
fn scramble(weights: &mut Weights, rng: &mut StdRng) {
    for (name, data) in weights.iter_mut() {
        let shape = data.shape.clone();
        let values: Vec<f32> = data.to_vec::<f32>().unwrap();
        let values = if name.ends_with("running_var") {
            values.iter().map(|_| rng.random_range(0.5..2.0)).collect()
        } else if name.ends_with("running_mean")
            || (name.ends_with(".bias") && !name.contains("head"))
        {
            values.iter().map(|_| rng.random_range(-0.3..0.3)).collect()
        } else if name.ends_with(".weight") && shape.num_dims() == 1 {
            values.iter().map(|_| rng.random_range(0.7..1.3)).collect()
        } else {
            values
        };
        *data = TensorData::new(values, shape);
    }
}

fn pixels(rng: &mut StdRng, count: usize) -> Vec<f32> {
    (0..count).map(|_| rng.random_range(0.0..1.0)).collect()
}

fn close(name: &str, a: &[f32], b: &[f32], tolerance: f32) {
    assert_eq!(a.len(), b.len(), "{name} length");
    let worst = a
        .iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0f32, f32::max);
    assert!(
        worst < tolerance,
        "{name}: ONNX differs from Burn by {worst}"
    );
}

#[test]
fn the_pair_graph_matches_burn() {
    if !runtime() {
        return;
    }
    let device = Default::default();
    let mut rng = StdRng::seed_from_u64(7);
    let mut weights = TongueNet::<Cpu>::init(&device).weights();
    scramble(&mut weights, &mut rng);
    let size = 96;
    let net = TongueNet::<Cpu>::from_weights(weights.clone(), &device)
        .unwrap()
        .fold();
    let mut session = Session::new(
        &onnx::pair_graph(&weights, size).unwrap().to_model(),
        Accelerator::Cpu,
    )
    .unwrap();
    for _ in 0..3 {
        let views = pixels(&mut rng, 2 * size * size);
        let burn = net
            .forward(Tensor::<Cpu, 4>::from_data(
                TensorData::new(views.clone(), [1, 2, size, size]),
                &device,
            ))
            .into_data()
            .to_vec::<f32>()
            .unwrap();
        let ort = session
            .run(&[("views", &[1, 2, size, size], &views)], &["values"])
            .unwrap();
        close("values", &burn, &ort[0], 1e-4);
    }
}

#[test]
fn the_face_graph_matches_burn() {
    if !runtime() {
        return;
    }
    let device = Default::default();
    let mut rng = StdRng::seed_from_u64(11);
    let mut weights = FaceNet::<Cpu>::init(&device).weights();
    scramble(&mut weights, &mut rng);
    for name in ["mouth_missing", "brow_missing", "mouth_offset"] {
        let data = &weights[name];
        let values = pixels(&mut rng, data.num_elements());
        let shape = data.shape.clone();
        weights.insert(name.into(), TensorData::new(values, shape));
    }
    let size = 64;
    let net = FaceNet::<Cpu>::from_weights(weights.clone(), false, &device)
        .unwrap()
        .fold();
    let mut session = Session::new(
        &onnx::face_graph(&weights, size).unwrap().to_model(),
        Accelerator::Cpu,
    )
    .unwrap();
    // Without a face setup, then with some anchors and the brow neutral.
    for (present, brow_present) in [([0.0; 6], 0.0), ([1.0, 0.0, 1.0, 1.0, 0.0, 1.0], 1.0)] {
        let views = pixels(&mut rng, 5 * size * size);
        let anchors = pixels(&mut rng, 6 * 512);
        let neutral = pixels(&mut rng, 480);
        let tensor = |values: &[f32], shape: Vec<usize>| {
            Tensor::<Cpu, 2>::from_data(TensorData::new(values.to_vec(), shape), &device)
        };
        let embeddings = net.embed(Tensor::from_data(
            TensorData::new(views.clone(), [1, 5, size, size]),
            &device,
        ));
        let raw = net.raw(
            &embeddings,
            Tensor::from_data(TensorData::new(anchors.clone(), [1, 6, 512]), &device),
            tensor(&present, vec![1, 6]),
            tensor(&neutral, vec![1, 480]),
            tensor(&[brow_present], vec![1, 1]),
        );
        let burn_values = activate(raw).into_data().to_vec::<f32>().unwrap();
        let burn_q = embeddings.mouth.into_data().to_vec::<f32>().unwrap();
        let burn_w = embeddings.brow.into_data().to_vec::<f32>().unwrap();
        let ort = session
            .run(
                &[
                    ("views", &[1, 5, size, size], &views),
                    ("anchors", &[1, 6, 512], &anchors),
                    ("present", &[1, 6], &present),
                    ("brow_neutral", &[1, 480], &neutral),
                    ("brow_present", &[1, 1], &[brow_present]),
                ],
                &["values", "q", "w"],
            )
            .unwrap();
        close("values", &burn_values, &ort[0], 1e-4);
        close("q", &burn_q, &ort[1], 1e-3);
        close("w", &burn_w, &ort[2], 1e-3);
    }
}

#[test]
fn a_session_calibrates_switches_to_int8_and_saves_it() {
    use vrft_tongue::onnx::live::{LiveSession, Plan};
    use vrft_tongue::onnx::quantize::CALIBRATION_FRAMES;
    if !runtime() {
        return;
    }
    let device = Default::default();
    let mut rng = StdRng::seed_from_u64(3);
    let mut weights = TongueNet::<Cpu>::init(&device).weights();
    scramble(&mut weights, &mut rng);
    let size = 96;
    let folder = std::env::temp_dir().join(format!("vrft-onnx-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let checkpoint = folder.join("direction.safetensors");
    let graph = onnx::pair_graph(&weights, size).unwrap();
    let mut float = Session::new(&graph.to_model(), Accelerator::Cpu).unwrap();
    let mut live = LiveSession::new(
        graph.clone(),
        &checkpoint,
        &weights,
        Accelerator::Cpu,
        Plan::Int8(&[]),
    )
    .unwrap();
    assert_eq!(live.precision, "fp32");
    let frames: Vec<Vec<f32>> = (0..8).map(|_| pixels(&mut rng, 2 * size * size)).collect();
    for i in 0..CALIBRATION_FRAMES {
        live.run(
            &[("views", &[1, 2, size, size], &frames[i % frames.len()])],
            &["values"],
        )
        .unwrap();
    }
    let started = std::time::Instant::now();
    while live.precision != "int8" {
        assert!(started.elapsed().as_secs() < 60, "never switched to int8");
        std::thread::sleep(std::time::Duration::from_millis(50));
        live.run(&[("views", &[1, 2, size, size], &frames[0])], &["values"])
            .unwrap();
    }
    // int8 stays close to float on frames like the calibration's.
    for frame in &frames {
        let a = float
            .run(&[("views", &[1, 2, size, size], frame)], &["values"])
            .unwrap();
        let b = live
            .run(&[("views", &[1, 2, size, size], frame)], &["values"])
            .unwrap();
        close("int8 values", &a[0], &b[0], 0.1);
    }
    // The next load starts on the saved int8 graph, but not for other weights.
    let again = LiveSession::new(
        graph.clone(),
        &checkpoint,
        &weights,
        Accelerator::Cpu,
        Plan::Int8(&[]),
    )
    .unwrap();
    assert_eq!(again.precision, "int8");
    let mut other = weights.clone();
    scramble(&mut other, &mut rng);
    let changed = LiveSession::new(
        graph,
        &checkpoint,
        &other,
        Accelerator::Cpu,
        Plan::Int8(&[]),
    )
    .unwrap();
    assert_eq!(changed.precision, "fp32");
    std::fs::remove_dir_all(&folder).ok();
}
