//! The Quest Pro stereo tongue network, `spatial-stereo-resnet-v2` from
//! Qpro-Enhanced-FT (MIT license), in Burn.
//!
//! Layers are written out rather than taken from `burn::nn` so that
//! BatchNorm always uses its running statistics: personal training
//! fine-tunes on tiny batches and must not rewrite the population
//! statistics, exactly as the reference trainer freezes them. Weights are
//! named as in the PyTorch state dict, so checkpoints map one to one.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use burn::module::{Module, Param, RunningState};
use burn::nn::{Dropout, DropoutConfig, Linear, LinearConfig};
use burn::tensor::activation::{sigmoid, silu, tanh};
use burn::tensor::backend::Backend;
use burn::tensor::module::conv2d;
use burn::tensor::ops::ConvOptions;
use burn::tensor::{Bool, Tensor, TensorData};

use crate::{TARGETS, TONGUE_TARGETS};

pub const ARCHITECTURE: &str = "spatial-stereo-resnet-v2";
const BATCH_NORM_EPSILON: f32 = 1e-5;
/// Heads with a signed range (tanh); every other head is a sigmoid.
const SIGNED: [&str; 3] = ["horizontal", "vertical", "twist"];

/// Named weights, as in a PyTorch state dict.
pub type Weights = HashMap<String, TensorData>;

struct Reader<'a> {
    weights: &'a mut Weights,
    /// Make up freshly initialised weights instead of reading them.
    fresh: bool,
}

impl Reader<'_> {
    fn take<B: Backend, const D: usize>(
        &mut self,
        name: &str,
        shape: [usize; D],
        device: &B::Device,
    ) -> Result<Tensor<B, D>> {
        if self.fresh {
            return Ok(fresh(shape, device));
        }
        let data = self
            .weights
            .remove(name)
            .with_context(|| format!("checkpoint has no {name}"))?;
        if data.shape.as_slice() != shape.as_slice() {
            bail!(
                "{name} has shape {:?}, expected {:?}",
                data.shape.as_slice(),
                shape
            );
        }
        let data = data.convert::<f32>();
        Ok(Tensor::from_data(data, device))
    }
}

/// PyTorch's default initialisation for a convolution or linear weight or
/// bias: uniform within 1/sqrt(fan in).
fn fresh<B: Backend, const D: usize>(shape: [usize; D], device: &B::Device) -> Tensor<B, D> {
    use burn::tensor::Distribution;
    let fan_in: usize = if D == 1 {
        192
    } else {
        shape.iter().skip(1).product()
    };
    let bound = 1.0 / (fan_in as f64).sqrt();
    Tensor::random(shape, Distribution::Uniform(-bound, bound), device)
}

fn save<B: Backend, const D: usize>(out: &mut Weights, name: String, tensor: Tensor<B, D>) {
    out.insert(name, tensor.into_data().convert::<f32>());
}

#[derive(Module, Debug)]
pub struct Conv<B: Backend> {
    weight: Param<Tensor<B, 4>>,
    /// Only set once a BatchNorm has been folded in for inference.
    bias: Option<Param<Tensor<B, 1>>>,
    stride: usize,
    padding: usize,
}

impl<B: Backend> Conv<B> {
    fn load(
        r: &mut Reader,
        name: &str,
        [input, output, kernel]: [usize; 3],
        stride: usize,
        device: &B::Device,
    ) -> Result<Self> {
        let weight = r.take(
            &format!("{name}.weight"),
            [output, input, kernel, kernel],
            device,
        )?;
        Ok(Self {
            weight: Param::from_tensor(weight),
            bias: None,
            stride,
            padding: kernel / 2,
        })
    }

    fn save(&self, name: &str, out: &mut Weights) {
        save(out, format!("{name}.weight"), self.weight.val());
    }

    fn forward(&self, input: Tensor<B, 4>) -> Tensor<B, 4> {
        let options = ConvOptions::new([self.stride; 2], [self.padding; 2], [1, 1], 1);
        conv2d(
            input,
            self.weight.val(),
            self.bias.as_ref().map(Param::val),
            options,
        )
    }
}

/// BatchNorm2d that always normalises with its running statistics.
#[derive(Module, Debug)]
pub struct Norm<B: Backend> {
    gamma: Param<Tensor<B, 1>>,
    beta: Param<Tensor<B, 1>>,
    mean: RunningState<Tensor<B, 1>>,
    var: RunningState<Tensor<B, 1>>,
}

impl<B: Backend> Norm<B> {
    fn load(r: &mut Reader, name: &str, channels: usize, device: &B::Device) -> Result<Self> {
        if r.fresh {
            let ones = || Tensor::ones([channels], device);
            let zeros = || Tensor::zeros([channels], device);
            return Ok(Self {
                gamma: Param::from_tensor(ones()),
                beta: Param::from_tensor(zeros()),
                mean: RunningState::new(zeros()),
                var: RunningState::new(ones()),
            });
        }
        Ok(Self {
            gamma: Param::from_tensor(r.take(&format!("{name}.weight"), [channels], device)?),
            beta: Param::from_tensor(r.take(&format!("{name}.bias"), [channels], device)?),
            mean: RunningState::new(r.take(&format!("{name}.running_mean"), [channels], device)?),
            var: RunningState::new(r.take(&format!("{name}.running_var"), [channels], device)?),
        })
    }

    fn save(&self, name: &str, out: &mut Weights) {
        save(out, format!("{name}.weight"), self.gamma.val());
        save(out, format!("{name}.bias"), self.beta.val());
        save(out, format!("{name}.running_mean"), self.mean.value());
        save(out, format!("{name}.running_var"), self.var.value());
    }

    /// The per-channel scale and shift this BatchNorm applies.
    fn affine(&self) -> (Tensor<B, 1>, Tensor<B, 1>) {
        let scale = self.gamma.val() / self.var.value().add_scalar(BATCH_NORM_EPSILON).sqrt();
        let shift = self.beta.val() - self.mean.value() * scale.clone();
        (scale, shift)
    }

    fn forward(&self, input: Tensor<B, 4>) -> Tensor<B, 4> {
        let channels = self.gamma.dims()[0];
        let (scale, shift) = self.affine();
        input * scale.reshape([1, channels, 1, 1]) + shift.reshape([1, channels, 1, 1])
    }

    /// `conv` followed by this BatchNorm, as one convolution with a bias.
    fn fold_into(&self, conv: Conv<B>) -> Conv<B> {
        let channels = self.gamma.dims()[0];
        let (scale, shift) = self.affine();
        let weight = conv.weight.val() * scale.reshape([channels, 1, 1, 1]);
        Conv {
            weight: Param::from_tensor(weight),
            bias: Some(Param::from_tensor(shift)),
            ..conv
        }
    }
}

/// A convolution and its BatchNorm; once folded for inference, one
/// convolution with a bias.
#[derive(Module, Debug)]
pub struct ConvNorm<B: Backend> {
    conv: Conv<B>,
    norm: Option<Norm<B>>,
}

impl<B: Backend> ConvNorm<B> {
    /// `conv` and `norm` are the two layers' state dict names.
    fn load(
        r: &mut Reader,
        [conv, norm]: [&str; 2],
        shape: [usize; 3],
        stride: usize,
        device: &B::Device,
    ) -> Result<Self> {
        Ok(Self {
            conv: Conv::load(r, conv, shape, stride, device)?,
            norm: Some(Norm::load(r, norm, shape[1], device)?),
        })
    }

    fn save(&self, [conv, norm]: [&str; 2], out: &mut Weights) {
        self.conv.save(conv, out);
        self.norm
            .as_ref()
            .expect("a folded tongue model is inference only")
            .save(norm, out);
    }

    fn fold(self) -> Self {
        match self.norm {
            Some(norm) => Self {
                conv: norm.fold_into(self.conv),
                norm: None,
            },
            None => self,
        }
    }

    /// Without the activation.
    fn forward(&self, input: Tensor<B, 4>) -> Tensor<B, 4> {
        let output = self.conv.forward(input);
        match &self.norm {
            Some(norm) => norm.forward(output),
            None => output,
        }
    }
}

#[derive(Module, Debug)]
pub struct Residual<B: Backend> {
    first: ConvNorm<B>,
    second: ConvNorm<B>,
}

impl<B: Backend> Residual<B> {
    fn names(name: &str) -> [[String; 2]; 2] {
        [0, 3].map(|start| {
            [
                format!("{name}.network.{start}"),
                format!("{name}.network.{}", start + 1),
            ]
        })
    }

    fn load(r: &mut Reader, name: &str, channels: usize, device: &B::Device) -> Result<Self> {
        let shape = [channels, channels, 3];
        let [first, second] = Self::names(name);
        Ok(Self {
            first: ConvNorm::load(r, [&first[0], &first[1]], shape, 1, device)?,
            second: ConvNorm::load(r, [&second[0], &second[1]], shape, 1, device)?,
        })
    }

    fn save(&self, name: &str, out: &mut Weights) {
        let [first, second] = Self::names(name);
        self.first.save([&first[0], &first[1]], out);
        self.second.save([&second[0], &second[1]], out);
    }

    fn fold(self) -> Self {
        Self {
            first: self.first.fold(),
            second: self.second.fold(),
        }
    }

    fn forward(&self, input: Tensor<B, 4>) -> Tensor<B, 4> {
        let inner = silu(self.first.forward(input.clone()));
        silu(input + self.second.forward(inner))
    }
}

/// A strided ConvNorm followed by a residual block, as one item of an
/// `nn.Sequential` that starts at `start`.
#[derive(Module, Debug)]
pub struct Stage<B: Backend> {
    down: ConvNorm<B>,
    residual: Residual<B>,
}

impl<B: Backend> Stage<B> {
    fn names(prefix: &str, start: usize) -> [String; 3] {
        [
            format!("{prefix}.{start}"),
            format!("{prefix}.{}", start + 1),
            format!("{prefix}.{}", start + 3),
        ]
    }

    fn load(
        r: &mut Reader,
        prefix: &str,
        start: usize,
        shape: [usize; 3],
        stride: usize,
        device: &B::Device,
    ) -> Result<Self> {
        let [conv, norm, residual] = Self::names(prefix, start);
        Ok(Self {
            down: ConvNorm::load(r, [&conv, &norm], shape, stride, device)?,
            residual: Residual::load(r, &residual, shape[1], device)?,
        })
    }

    fn save(&self, prefix: &str, start: usize, out: &mut Weights) {
        let [conv, norm, residual] = Self::names(prefix, start);
        self.down.save([&conv, &norm], out);
        self.residual.save(&residual, out);
    }

    fn fold(self) -> Self {
        Self {
            down: self.down.fold(),
            residual: self.residual.fold(),
        }
    }

    fn forward(&self, input: Tensor<B, 4>) -> Tensor<B, 4> {
        self.residual.forward(silu(self.down.forward(input)))
    }
}

/// Encoder stages: (first nn.Sequential index, [in, out, kernel], stride).
const ENCODER: [(usize, [usize; 3], usize); 4] = [
    (0, [1, 32, 5], 2),
    (4, [32, 64, 3], 2),
    (8, [64, 96, 3], 2),
    (12, [96, 160, 3], 2),
];
const FUSION: [(usize, [usize; 3], usize); 2] = [(0, [640, 224, 1], 1), (4, [224, 256, 3], 2)];
/// Head linear layers: (nn.Sequential index, in, out).
const HEAD: [(usize, usize, usize); 3] = [
    (0, 512, 384),
    (3, 384, FEATURES),
    (6, FEATURES, TARGETS.len()),
];
/// Width of the features the last head layer maps to the outputs.
pub const FEATURES: usize = 192;
/// State dict names of the last head layer.
pub const OUTPUT_WEIGHT: &str = "head.6.weight";
pub const OUTPUT_BIAS: &str = "head.6.bias";
/// Where an added head starts before training: sigmoid(-4) is about 0.02.
const ADDED_HEAD_BIAS: f32 = -4.0;
const HEAD_DROPOUT: [f64; 2] = [0.18, 0.08];

/// Gives weights saved before the cheek heads existed a row for each, and
/// says whether it did. The added rows read nothing, so their heads rest
/// near zero until training fits them.
pub fn add_missing_heads(weights: &mut Weights) -> Result<bool> {
    let Some(weight) = weights.get(OUTPUT_WEIGHT) else {
        return Ok(false);
    };
    let rows = weight.shape.as_slice()[0];
    if rows == TARGETS.len() {
        return Ok(false);
    }
    if rows != TONGUE_TARGETS || weight.shape.as_slice() != [TONGUE_TARGETS, FEATURES] {
        bail!("{OUTPUT_WEIGHT} has shape {:?}", weight.shape.as_slice());
    }
    let added = TARGETS.len() - TONGUE_TARGETS;
    let mut values = weight
        .clone()
        .convert::<f32>()
        .to_vec::<f32>()
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    values.extend(std::iter::repeat_n(0.0, added * FEATURES));
    weights.insert(
        OUTPUT_WEIGHT.into(),
        TensorData::new(values, [TARGETS.len(), FEATURES]),
    );
    let bias = weights
        .get(OUTPUT_BIAS)
        .with_context(|| format!("checkpoint has no {OUTPUT_BIAS}"))?;
    let mut values = bias
        .clone()
        .convert::<f32>()
        .to_vec::<f32>()
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    if values.len() != TONGUE_TARGETS {
        bail!("{OUTPUT_BIAS} has {} values", values.len());
    }
    values.extend(std::iter::repeat_n(ADDED_HEAD_BIAS, added));
    weights.insert(OUTPUT_BIAS.into(), TensorData::new(values, [TARGETS.len()]));
    Ok(true)
}

/// Which layers fine-tuning may change; the rest keep the base model's
/// weights.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Trainable {
    #[default]
    All,
    /// The fully connected head: the convolutional layers that see the
    /// images stay as they are.
    Head,
    /// Only the last layer, which maps features to outputs.
    Output,
}

impl Trainable {
    pub fn name(self) -> &'static str {
        match self {
            Trainable::All => "all",
            Trainable::Head => "head",
            Trainable::Output => "output",
        }
    }
}

impl std::str::FromStr for Trainable {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        [Trainable::All, Trainable::Head, Trainable::Output]
            .into_iter()
            .find(|layers| layers.name() == name)
            .ok_or_else(|| format!("unknown layers {name}; use all, head or output"))
    }
}

#[derive(Module, Debug)]
pub struct TongueNet<B: Backend> {
    encoder: Vec<Stage<B>>,
    fusion: Vec<Stage<B>>,
    head: Vec<Linear<B>>,
    dropout: Vec<Dropout>,
    signed: RunningState<Tensor<B, 1>>,
}

impl<B: Backend> TongueNet<B> {
    /// A freshly initialised network, for tests.
    pub fn init(device: &B::Device) -> Self {
        Self::from_weights(Weights::new(), device).expect("fresh weights always fit")
    }

    /// Builds the network from a PyTorch-named state dict. Every weight must
    /// be used; `signed_mask` and BatchNorm batch counters are ignored. An
    /// empty state dict gives a freshly initialised network.
    pub fn from_weights(mut weights: Weights, device: &B::Device) -> Result<Self> {
        weights.remove("signed_mask");
        weights.retain(|name, _| !name.ends_with("num_batches_tracked"));
        let fresh = weights.is_empty();
        let mut r = Reader {
            weights: &mut weights,
            fresh,
        };
        let encoder = ENCODER
            .iter()
            .map(|&(start, shape, stride)| {
                Stage::load(&mut r, "encoder.network", start, shape, stride, device)
            })
            .collect::<Result<_>>()?;
        let fusion = FUSION
            .iter()
            .map(|&(start, shape, stride)| {
                Stage::load(&mut r, "stereo_fusion", start, shape, stride, device)
            })
            .collect::<Result<_>>()?;
        let head = HEAD
            .iter()
            .map(|&(index, input, output)| {
                let mut linear = LinearConfig::new(input, output).init(device);
                // PyTorch stores [out, in]; Burn multiplies by [in, out].
                let weight: Tensor<B, 2> =
                    r.take(&format!("head.{index}.weight"), [output, input], device)?;
                linear.weight = Param::from_tensor(weight.transpose());
                linear.bias = Some(Param::from_tensor(r.take(
                    &format!("head.{index}.bias"),
                    [output],
                    device,
                )?));
                Ok(linear)
            })
            .collect::<Result<_>>()?;
        if let Some(name) = weights.keys().next() {
            bail!("checkpoint has an unexpected weight {name}");
        }
        let signed: Vec<f32> = TARGETS
            .iter()
            .map(|name| if SIGNED.contains(name) { 1.0 } else { 0.0 })
            .collect();
        Ok(Self {
            encoder,
            fusion,
            head,
            dropout: HEAD_DROPOUT
                .iter()
                .map(|&p| DropoutConfig::new(p).init())
                .collect(),
            signed: RunningState::new(Tensor::from_floats(signed.as_slice(), device)),
        })
    }

    /// The same network with every BatchNorm folded into its convolution:
    /// identical outputs, less work per frame, and no longer trainable or
    /// saveable.
    pub fn fold(self) -> Self {
        Self {
            encoder: self.encoder.into_iter().map(Stage::fold).collect(),
            fusion: self.fusion.into_iter().map(Stage::fold).collect(),
            ..self
        }
    }

    /// The same network with gradients switched off outside `trainable`, so
    /// an optimizer leaves those layers alone.
    pub fn freeze(self, trainable: Trainable) -> Self {
        let frozen_head = match trainable {
            Trainable::All => return self,
            Trainable::Head => 0,
            Trainable::Output => HEAD.len() - 1,
        };
        Self {
            encoder: self.encoder.no_grad(),
            fusion: self.fusion.no_grad(),
            head: self
                .head
                .into_iter()
                .enumerate()
                .map(|(index, linear)| {
                    if index < frozen_head {
                        linear.no_grad()
                    } else {
                        linear
                    }
                })
                .collect(),
            ..self
        }
    }

    /// The weights in PyTorch state dict naming.
    pub fn weights(&self) -> Weights {
        let mut out = Weights::new();
        for (stage, &(start, ..)) in self.encoder.iter().zip(&ENCODER) {
            stage.save("encoder.network", start, &mut out);
        }
        for (stage, &(start, ..)) in self.fusion.iter().zip(&FUSION) {
            stage.save("stereo_fusion", start, &mut out);
        }
        for (linear, &(index, ..)) in self.head.iter().zip(&HEAD) {
            save(
                &mut out,
                format!("head.{index}.weight"),
                linear.weight.val().transpose(),
            );
            if let Some(bias) = &linear.bias {
                save(&mut out, format!("head.{index}.bias"), bias.val());
            }
        }
        out
    }

    fn encode(&self, mut images: Tensor<B, 4>) -> Tensor<B, 4> {
        for stage in &self.encoder {
            images = stage.forward(images);
        }
        images
    }

    /// The features the last head layer reads, `[batch, FEATURES]`, for
    /// `cameras` as [`forward`](Self::forward) takes them.
    pub fn features(&self, cameras: Tensor<B, 4>) -> Tensor<B, 2> {
        let [batch, views, height, width] = cameras.dims();
        assert_eq!(views, 2, "the tongue model takes two camera views");
        // One encoder pass over both views: BatchNorm uses running
        // statistics, so each image is encoded independently anyway.
        let encoded = self.encode(cameras.reshape([batch * 2, 1, height, width]));
        let [_, channels, h, w] = encoded.dims();
        let encoded = encoded.reshape([batch, 2, channels, h, w]);
        let left = encoded
            .clone()
            .narrow(1, 0, 1)
            .reshape([batch, channels, h, w]);
        let right = encoded.narrow(1, 1, 1).reshape([batch, channels, h, w]);
        let mut fused = Tensor::cat(
            vec![
                left.clone(),
                right.clone(),
                (left.clone() - right.clone()).abs(),
                left * right,
            ],
            1,
        );
        for stage in &self.fusion {
            fused = stage.forward(fused);
        }
        let [_, channels, h, w] = fused.dims();
        let flat = fused.reshape([batch, channels, h * w]);
        let mut values = Tensor::cat(
            vec![
                flat.clone().mean_dim(2).reshape([batch, channels]),
                flat.max_dim(2).reshape([batch, channels]),
            ],
            1,
        );
        // Every head layer but the last, each followed by its dropout.
        for (linear, dropout) in self.head.iter().zip(&self.dropout) {
            values = dropout.forward(silu(linear.forward(values)));
        }
        values
    }

    /// `cameras` is `[batch, 2, size, size]` in 0..1: the left then the right
    /// mouth camera. Returns `[batch, TARGETS.len()]` in `TARGETS` order.
    pub fn forward(&self, cameras: Tensor<B, 4>) -> Tensor<B, 2> {
        self.activate(self.forward_raw(cameras))
    }

    /// The heads' values before their sigmoid or tanh, as
    /// [`forward`](Self::forward) would return them after [`activate`](Self::activate).
    pub fn forward_raw(&self, cameras: Tensor<B, 4>) -> Tensor<B, 2> {
        let features = self.features(cameras);
        self.head[self.head.len() - 1].forward(features)
    }

    /// Each head's sigmoid, or tanh for the signed heads.
    pub fn activate(&self, values: Tensor<B, 2>) -> Tensor<B, 2> {
        let [batch, _] = values.dims();
        let signed: Tensor<B, 2, Bool> = self
            .signed
            .value()
            .reshape([1, TARGETS.len()])
            .greater_elem(0.5)
            .expand([batch, TARGETS.len()]);
        sigmoid(values.clone()).mask_where(signed, tanh(values))
    }
}
