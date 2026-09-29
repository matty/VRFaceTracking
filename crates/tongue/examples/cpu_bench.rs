//! Times one folded gate + direction pass on the CPU and GPU backends. Developer tool:
//! cargo run -p vrft-tongue --release --example cpu_bench -- <model-dir>
use burn::tensor::backend::Backend;
use burn::tensor::{Tensor, TensorData};
use std::time::Instant;
use vrft_tongue::model::TongueNet;
use vrft_tongue::{Checkpoint, Role};

fn bench<B: Backend>(name: &str, dir: &std::path::Path) {
    let device = B::Device::default();
    let nets: Vec<(TongueNet<B>, usize)> = [Role::Gate, Role::Direction]
        .iter()
        .map(|role| {
            let c = Checkpoint::load(&role.find(dir).unwrap()).unwrap();
            (
                TongueNet::from_weights(c.weights, &device).unwrap().fold(),
                c.metadata.image_size,
            )
        })
        .collect();
    let run = || {
        for (net, size) in &nets {
            let input = Tensor::<B, 4>::from_data(
                TensorData::new(vec![0.5f32; 2 * size * size], [1, 2, *size, *size]),
                &device,
            );
            net.forward(input).into_data();
        }
    };
    for _ in 0..3 {
        run();
    }
    let started = Instant::now();
    for _ in 0..10 {
        run();
    }
    println!("{name}: {:?} per frame", started.elapsed() / 10);
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    bench::<burn::backend::Flex>("flex", &dir);
    bench::<burn::backend::Wgpu>("wgpu", &dir);
}
