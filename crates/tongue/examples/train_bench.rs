//! Times one training step and one inference pass per GPU backend.
//! Developer tool: cargo run -p vrft-tongue --release --example train_bench
use burn::backend::Autodiff;
use burn::optim::{AdamWConfig, GradientsParams, Optimizer};
use burn::tensor::backend::Backend;
use burn::tensor::{Tensor, TensorData};
use std::time::Instant;
use vrft_tongue::model::TongueNet;

fn bench<G: Backend>(name: &str, batch: usize) {
    type A<G> = Autodiff<G>;
    let size = 224;
    let pixels = vec![0.4f32; batch * 2 * size * size];
    let device: G::Device = Default::default();
    let mut model = TongueNet::<A<G>>::init(&device);
    let mut optimizer = AdamWConfig::new().init::<A<G>, TongueNet<A<G>>>();
    let mut times = vec![];
    for i in 0..6 {
        let t = Instant::now();
        let input = Tensor::<A<G>, 4>::from_data(
            TensorData::new(pixels.clone(), [batch, 2, size, size]),
            &device,
        );
        let loss = model.forward(input).mean();
        let grads = GradientsParams::from_grads(loss.backward(), &model);
        model = Optimizer::<TongueNet<A<G>>, A<G>>::step(&mut optimizer, 1e-4, model, grads);
        let _ = model
            .forward(Tensor::zeros([1, 2, 32, 32], &device))
            .sum()
            .into_data();
        if i >= 3 {
            times.push(t.elapsed());
        }
    }
    let infer = TongueNet::<G>::init(&device).fold();
    let one = vec![0.4f32; 2 * size * size];
    let mut infer_times = vec![];
    for i in 0..8 {
        let t = Instant::now();
        let input =
            Tensor::<G, 4>::from_data(TensorData::new(one.clone(), [1, 2, size, size]), &device);
        let _ = infer.forward(input).into_data();
        if i >= 3 {
            infer_times.push(t.elapsed());
        }
    }
    println!(
        "{name}: train step (batch {batch}) {:?}; inference {:?}",
        times,
        infer_times.iter().min().unwrap()
    );
}

fn main() {
    for batch in [1, 4, 12, 24] {
        bench::<burn::backend::Wgpu>("wgpu", batch);
    }
}
