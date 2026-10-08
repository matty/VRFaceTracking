//! The universal face network in Burn. Weights are named like a PyTorch
//! state dict: the front and tails keep the v8 encoder's `nn.Sequential`
//! indices, so `encoder.network.0-11` maps to `front.0-11` and
//! `encoder.network.12-15` to each tail's `12-15`.

use anyhow::{bail, Result};
use burn::module::{Module, Param};
use burn::nn::{Linear, LinearConfig};
use burn::tensor::activation::{sigmoid, silu, tanh};
use burn::tensor::backend::Backend;
use burn::tensor::module::adaptive_avg_pool2d;
use burn::tensor::{Bool, Tensor};

use super::{
    ANCHOR_SLOTS, BROW_EMBEDDING, BROW_OUTPUTS, CAMERAS, FACE_TARGETS, MOUTH_EMBEDDING,
    MOUTH_OUTPUTS, TONGUE_OUTPUTS,
};
use crate::model::{save, Reader, Stage, Weights, ENCODER};

/// The v8 encoder's last stage, which each tail copies.
const TAIL: (usize, [usize; 3], usize) = ENCODER[3];
const TAIL_CHANNELS: usize = 160;
/// The mouth tail pools to this many cells a side before its linear layer.
const MOUTH_GRID: usize = 4;
const MOUTH_PROJECTION: usize = MOUTH_EMBEDDING / 2;
pub const TAILS: [&str; 3] = ["mouth_tail", "tongue_tail", "brow_tail"];
/// The mouth head's input: q, every anchor, q minus the neutral anchor, and
/// a present flag per anchor.
const MOUTH_HEAD_INPUT: usize = MOUTH_EMBEDDING * (ANCHOR_SLOTS.len() + 2) + ANCHOR_SLOTS.len();
const MOUTH_HEAD: [(usize, usize); 3] = [
    (MOUTH_HEAD_INPUT, 512),
    (512, 256),
    (256, MOUTH_OUTPUTS.len()),
];
const BROW_HEAD_INPUT: usize = BROW_EMBEDDING * 2 + 1;
const BROW_HEAD: [(usize, usize); 2] = [(BROW_HEAD_INPUT, 128), (128, BROW_OUTPUTS.len())];

fn load_linear<B: Backend>(
    r: &mut Reader,
    name: &str,
    (input, output): (usize, usize),
    device: &B::Device,
) -> Result<Linear<B>> {
    let mut linear = LinearConfig::new(input, output).init(device);
    // PyTorch stores [out, in]; Burn multiplies by [in, out].
    let weight: Tensor<B, 2> = r.take(&format!("{name}.weight"), [output, input], device)?;
    linear.weight = Param::from_tensor(weight.transpose());
    linear.bias = Some(Param::from_tensor(r.take(
        &format!("{name}.bias"),
        [output],
        device,
    )?));
    Ok(linear)
}

fn save_linear<B: Backend>(linear: &Linear<B>, name: &str, out: &mut Weights) {
    save(
        out,
        format!("{name}.weight"),
        linear.weight.val().transpose(),
    );
    if let Some(bias) = &linear.bias {
        save(out, format!("{name}.bias"), bias.val());
    }
}

/// Starts at zero, rather than at random, unless the state dict has it.
fn zeros_unless<B: Backend, const D: usize>(
    r: &mut Reader,
    name: &str,
    shape: [usize; D],
    device: &B::Device,
) -> Result<Tensor<B, D>> {
    if r.fresh && !r.weights.contains_key(name) {
        return Ok(Tensor::zeros(shape, device));
    }
    r.take(name, shape, device)
}

/// What one pass over the cameras gives, before the anchor-conditioned heads.
pub struct Embeddings<B: Backend> {
    /// `[n, 512]`.
    pub mouth: Tensor<B, 2>,
    /// `[n, 4]`, before visibility's and extension's sigmoid and the
    /// directions' tanh.
    pub tongue: Tensor<B, 2>,
    /// `[n, 480]`.
    pub brow: Tensor<B, 2>,
}

#[derive(Module, Debug)]
pub struct FaceNet<B: Backend> {
    front: Vec<Stage<B>>,
    mouth_tail: Stage<B>,
    tongue_tail: Stage<B>,
    brow_tail: Stage<B>,
    mouth_projection: Linear<B>,
    /// Added to each mouth camera's projection: `[2, 256]`.
    mouth_offset: Param<Tensor<B, 2>>,
    tongue_head: Linear<B>,
    /// Stands in for each anchor not enrolled: `[6, 512]`.
    mouth_missing: Param<Tensor<B, 2>>,
    mouth_head: Vec<Linear<B>>,
    /// Stands in for the brow neutral when it isn't enrolled: `[480]`.
    brow_missing: Param<Tensor<B, 1>>,
    brow_head: Vec<Linear<B>>,
}

impl<B: Backend> FaceNet<B> {
    /// A freshly initialised network.
    pub fn init(device: &B::Device) -> Self {
        Self::from_weights(Weights::new(), true, device).expect("fresh weights always fit")
    }

    /// Builds the network from a state dict. With `fresh`, anything missing
    /// is freshly initialised, such as everything but the layers taken from
    /// the v8 encoder; without, every weight must be there. Either way every
    /// weight given must be used.
    pub fn from_weights(mut weights: Weights, fresh: bool, device: &B::Device) -> Result<Self> {
        weights.retain(|name, _| !name.ends_with("num_batches_tracked"));
        let mut r = Reader {
            weights: &mut weights,
            fresh,
        };
        let front = ENCODER[..3]
            .iter()
            .map(|&(start, shape, stride)| {
                Stage::load(&mut r, "front", start, shape, stride, device)
            })
            .collect::<Result<_>>()?;
        let (start, shape, stride) = TAIL;
        let mut tail = |name| Stage::load(&mut r, name, start, shape, stride, device);
        let [mouth_tail, tongue_tail, brow_tail] =
            [tail(TAILS[0])?, tail(TAILS[1])?, tail(TAILS[2])?];
        let projection_input = TAIL_CHANNELS * MOUTH_GRID * MOUTH_GRID;
        let mouth_projection = load_linear(
            &mut r,
            "mouth_projection",
            (projection_input, MOUTH_PROJECTION),
            device,
        )?;
        let mouth_offset = zeros_unless(&mut r, "mouth_offset", [2, MOUTH_PROJECTION], device)?;
        let tongue_head = load_linear(
            &mut r,
            "tongue_head",
            (2 * TAIL_CHANNELS, TONGUE_OUTPUTS.len()),
            device,
        )?;
        let mouth_missing = zeros_unless(
            &mut r,
            "mouth_missing",
            [ANCHOR_SLOTS.len(), MOUTH_EMBEDDING],
            device,
        )?;
        let mouth_head = MOUTH_HEAD
            .iter()
            .enumerate()
            .map(|(index, &shape)| {
                load_linear(&mut r, &format!("mouth_head.{index}"), shape, device)
            })
            .collect::<Result<_>>()?;
        let brow_missing = zeros_unless(&mut r, "brow_missing", [BROW_EMBEDDING], device)?;
        let brow_head = BROW_HEAD
            .iter()
            .enumerate()
            .map(|(index, &shape)| {
                load_linear(&mut r, &format!("brow_head.{index}"), shape, device)
            })
            .collect::<Result<_>>()?;
        if let Some(name) = weights.keys().next() {
            bail!("checkpoint has an unexpected weight {name}");
        }
        Ok(Self {
            front,
            mouth_tail,
            tongue_tail,
            brow_tail,
            mouth_projection,
            mouth_offset: Param::from_tensor(mouth_offset),
            tongue_head,
            mouth_missing: Param::from_tensor(mouth_missing),
            mouth_head,
            brow_missing: Param::from_tensor(brow_missing),
            brow_head,
        })
    }

    /// The weights in state dict naming.
    pub fn weights(&self) -> Weights {
        let mut out = Weights::new();
        for (stage, &(start, ..)) in self.front.iter().zip(&ENCODER) {
            stage.save("front", start, &mut out);
        }
        for (stage, name) in [&self.mouth_tail, &self.tongue_tail, &self.brow_tail]
            .into_iter()
            .zip(TAILS)
        {
            stage.save(name, TAIL.0, &mut out);
        }
        save_linear(&self.mouth_projection, "mouth_projection", &mut out);
        save(&mut out, "mouth_offset".into(), self.mouth_offset.val());
        save_linear(&self.tongue_head, "tongue_head", &mut out);
        save(&mut out, "mouth_missing".into(), self.mouth_missing.val());
        for (index, linear) in self.mouth_head.iter().enumerate() {
            save_linear(linear, &format!("mouth_head.{index}"), &mut out);
        }
        save(&mut out, "brow_missing".into(), self.brow_missing.val());
        for (index, linear) in self.brow_head.iter().enumerate() {
            save_linear(linear, &format!("brow_head.{index}"), &mut out);
        }
        out
    }

    /// The same network with every BatchNorm folded into its convolution,
    /// for inference.
    pub fn fold(self) -> Self {
        Self {
            front: self.front.into_iter().map(Stage::fold).collect(),
            mouth_tail: self.mouth_tail.fold(),
            tongue_tail: self.tongue_tail.fold(),
            brow_tail: self.brow_tail.fold(),
            ..self
        }
    }

    /// `views` is `[n, 5, size, size]` in 0..1, the cameras in order.
    pub fn embed(&self, views: Tensor<B, 4>) -> Embeddings<B> {
        let [n, cameras, height, width] = views.dims();
        assert_eq!(cameras, CAMERAS, "the face model takes all five cameras");
        let mut features = views.reshape([n * CAMERAS, 1, height, width]);
        for stage in &self.front {
            features = stage.forward(features);
        }
        let [_, channels, h, w] = features.dims();
        let features = features.reshape([n, CAMERAS, channels, h, w]);
        let pick = |first: usize, count: usize| {
            features
                .clone()
                .narrow(1, first, count)
                .reshape([n * count, channels, h, w])
        };
        let mouth_views = pick(2, 2);

        let mouth = self.mouth_tail.forward(mouth_views.clone());
        let mouth = adaptive_avg_pool2d(mouth, [MOUTH_GRID, MOUTH_GRID])
            .reshape([n * 2, TAIL_CHANNELS * MOUTH_GRID * MOUTH_GRID]);
        let mouth = self
            .mouth_projection
            .forward(mouth)
            .reshape([n, 2, MOUTH_PROJECTION])
            + self.mouth_offset.val().unsqueeze_dim::<3>(0);
        let mouth = mouth.reshape([n, MOUTH_EMBEDDING]);

        let tongue =
            global_mean(self.tongue_tail.forward(mouth_views)).reshape([n, 2 * TAIL_CHANNELS]);
        let tongue = self.tongue_head.forward(tongue);

        let brow_views = Tensor::cat(
            vec![features.clone().narrow(1, 0, 2), features.narrow(1, 4, 1)],
            1,
        )
        .reshape([n * 3, channels, h, w]);
        let brow = global_mean(self.brow_tail.forward(brow_views)).reshape([n, BROW_EMBEDDING]);
        Embeddings {
            mouth,
            tongue,
            brow,
        }
    }

    /// The mouth head's values before their sigmoid, `[n, 5]`. `anchors` is
    /// `[n, 6, 512]` and `present` `[n, 6]`, 1 where that anchor is
    /// enrolled; the others read the learned missing vectors.
    pub fn mouth_raw(
        &self,
        q: Tensor<B, 2>,
        anchors: Tensor<B, 3>,
        present: Tensor<B, 2>,
    ) -> Tensor<B, 2> {
        let [n, _] = q.dims();
        let slots = ANCHOR_SLOTS.len();
        let mask: Tensor<B, 3, Bool> = present
            .clone()
            .greater_elem(0.5)
            .unsqueeze_dim::<3>(2)
            .expand([n, slots, MOUTH_EMBEDDING]);
        let anchors = self
            .mouth_missing
            .val()
            .unsqueeze_dim::<3>(0)
            .expand([n, slots, MOUTH_EMBEDDING])
            .mask_where(mask, anchors);
        let neutral = anchors
            .clone()
            .narrow(1, 0, 1)
            .reshape([n, MOUTH_EMBEDDING]);
        let mut values = Tensor::cat(
            vec![
                q.clone(),
                anchors.reshape([n, slots * MOUTH_EMBEDDING]),
                q - neutral,
                present,
            ],
            1,
        );
        let last = self.mouth_head.len() - 1;
        for (index, linear) in self.mouth_head.iter().enumerate() {
            values = linear.forward(values);
            if index < last {
                values = silu(values);
            }
        }
        values
    }

    /// The brow head's values before their sigmoid, `[n, 8]`. `neutral` is
    /// `[n, 480]` and `present` `[n, 1]`.
    pub fn brow_raw(
        &self,
        w: Tensor<B, 2>,
        neutral: Tensor<B, 2>,
        present: Tensor<B, 2>,
    ) -> Tensor<B, 2> {
        let [n, _] = w.dims();
        let mask: Tensor<B, 2, Bool> = present
            .clone()
            .greater_elem(0.5)
            .expand([n, BROW_EMBEDDING]);
        let neutral = self
            .brow_missing
            .val()
            .unsqueeze_dim::<2>(0)
            .expand([n, BROW_EMBEDDING])
            .mask_where(mask, neutral);
        let values = Tensor::cat(vec![w.clone(), w - neutral, present], 1);
        let hidden = silu(self.brow_head[0].forward(values));
        self.brow_head[1].forward(hidden)
    }

    /// Every output before activation, `[n, 17]` in `FACE_TARGETS` order.
    pub fn raw(
        &self,
        embeddings: &Embeddings<B>,
        anchors: Tensor<B, 3>,
        present: Tensor<B, 2>,
        brow_neutral: Tensor<B, 2>,
        brow_present: Tensor<B, 2>,
    ) -> Tensor<B, 2> {
        let mouth = self.mouth_raw(embeddings.mouth.clone(), anchors, present);
        let brows = self.brow_raw(embeddings.brow.clone(), brow_neutral, brow_present);
        Tensor::cat(vec![embeddings.tongue.clone(), mouth, brows], 1)
    }
}

/// Each output's sigmoid, or tanh for the signed ones.
pub fn activate<B: Backend>(raw: Tensor<B, 2>) -> Tensor<B, 2> {
    let [n, outputs] = raw.dims();
    assert_eq!(outputs, FACE_TARGETS.len());
    let device = raw.device();
    let signed: Vec<f32> = (0..outputs)
        .map(|index| {
            if super::SIGNED.contains(&index) {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let signed: Tensor<B, 2, Bool> = Tensor::<B, 1>::from_floats(signed.as_slice(), &device)
        .reshape([1, outputs])
        .greater_elem(0.5)
        .expand([n, outputs]);
    sigmoid(raw.clone()).mask_where(signed, tanh(raw))
}

fn global_mean<B: Backend>(features: Tensor<B, 4>) -> Tensor<B, 2> {
    let [n, channels, h, w] = features.dims();
    features
        .reshape([n, channels, h * w])
        .mean_dim(2)
        .reshape([n, channels])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Cpu;

    #[test]
    fn shapes_follow_the_reference_layout() {
        let device = Default::default();
        let net = FaceNet::<Cpu>::init(&device);
        let views = Tensor::<Cpu, 4>::zeros([2, 5, 64, 64], &device);
        let embeddings = net.embed(views);
        assert_eq!(embeddings.mouth.dims(), [2, 512]);
        assert_eq!(embeddings.tongue.dims(), [2, 4]);
        assert_eq!(embeddings.brow.dims(), [2, 480]);
        let anchors = Tensor::<Cpu, 3>::zeros([2, 6, 512], &device);
        let present = Tensor::<Cpu, 2>::zeros([2, 6], &device);
        let neutral = Tensor::<Cpu, 2>::zeros([2, 480], &device);
        let brow_present = Tensor::<Cpu, 2>::zeros([2, 1], &device);
        let raw = net.raw(&embeddings, anchors, present, neutral, brow_present);
        assert_eq!(raw.dims(), [2, FACE_TARGETS.len()]);
        let values = activate(raw).into_data().to_vec::<f32>().unwrap();
        assert!(values
            .iter()
            .all(|value| value.is_finite() && value.abs() <= 1.0));
    }

    #[test]
    fn weights_round_trip_and_folding_keeps_the_outputs() {
        let device = Default::default();
        let net = FaceNet::<Cpu>::init(&device);
        let weights = net.weights();
        assert_eq!(weights["mouth_head.0.weight"].shape.as_slice(), [512, 4102]);
        assert_eq!(weights["brow_head.0.weight"].shape.as_slice(), [128, 961]);
        assert_eq!(
            weights["mouth_projection.weight"].shape.as_slice(),
            [256, 2560]
        );
        assert_eq!(weights["tongue_head.weight"].shape.as_slice(), [4, 320]);
        let again = FaceNet::<Cpu>::from_weights(weights.clone(), false, &device).unwrap();
        let views = Tensor::<Cpu, 4>::random(
            [1, 5, 64, 64],
            burn::tensor::Distribution::Uniform(0.0, 1.0),
            &device,
        );
        let before = net
            .embed(views.clone())
            .mouth
            .into_data()
            .to_vec::<f32>()
            .unwrap();
        let after = again
            .fold()
            .embed(views)
            .mouth
            .into_data()
            .to_vec::<f32>()
            .unwrap();
        for (a, b) in before.iter().zip(&after) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
        let mut missing = weights;
        missing.remove("brow_missing");
        assert!(FaceNet::<Cpu>::from_weights(missing, false, &device).is_err());
    }

    #[test]
    fn missing_anchors_read_the_learned_vectors() {
        let device = Default::default();
        let net = FaceNet::<Cpu>::init(&device);
        let q = Tensor::<Cpu, 2>::ones([1, 512], &device);
        let anchors = Tensor::<Cpu, 3>::ones([1, 6, 512], &device) * 5.0;
        let absent = Tensor::<Cpu, 2>::zeros([1, 6], &device);
        // Absent anchors are ignored, whatever their values.
        let a = net.mouth_raw(q.clone(), anchors, absent.clone());
        let b = net.mouth_raw(q, Tensor::zeros([1, 6, 512], &device), absent);
        let (a, b) = (
            a.into_data().to_vec::<f32>().unwrap(),
            b.into_data().to_vec::<f32>().unwrap(),
        );
        assert_eq!(a, b);
    }
}
