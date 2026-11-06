//! The tongue models as ONNX graphs, for inference on ONNX Runtime
//! (`runtime`). Each graph is built from a checkpoint's weights when the
//! model loads, with every BatchNorm folded into its convolution, and can be
//! quantized to int8 (`quantize`).
//!
//! - The stereo pair's gate or direction model: `views` `[1, 2, size, size]`
//!   in 0..1 in, `values` `[1, 12]` out, activated as `TongueNet::forward`.

pub mod live;
pub mod proto;
pub mod quantize;
pub mod runtime;

use anyhow::{anyhow, bail, Context, Result};

use crate::model::{Weights, BATCH_NORM_EPSILON, ENCODER, FUSION, HEAD, SIGNED};
use crate::TARGETS;
use proto::{Attribute, DataType, Graph, Initializer, Node, ValueInfo};

/// Builds a graph node by node, naming every value.
pub(crate) struct Builder<'w> {
    weights: &'w Weights,
    graph: Graph,
    next: usize,
}

impl<'w> Builder<'w> {
    fn new(weights: &'w Weights, name: &str) -> Self {
        Self {
            weights,
            graph: Graph {
                name: name.into(),
                ..Graph::default()
            },
            next: 0,
        }
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}_{}", self.next)
    }

    fn input(&mut self, name: &str, dims: &[usize]) -> String {
        self.graph.inputs.push(ValueInfo {
            name: name.into(),
            data_type: DataType::Float,
            dims: dims.iter().map(|&d| d as i64).collect(),
        });
        name.into()
    }

    fn output(&mut self, value: &str, name: &str, dims: &[usize]) {
        self.op_named("Identity", &[value], name, vec![]);
        self.graph.outputs.push(ValueInfo {
            name: name.into(),
            data_type: DataType::Float,
            dims: dims.iter().map(|&d| d as i64).collect(),
        });
    }

    fn constant(
        &mut self,
        prefix: &str,
        data_type: DataType,
        dims: &[usize],
        raw: Vec<u8>,
    ) -> String {
        let name = self.fresh(prefix);
        self.graph.initializers.push(Initializer {
            name: name.clone(),
            dims: dims.iter().map(|&d| d as i64).collect(),
            data_type,
            raw,
        });
        name
    }

    fn floats(&mut self, prefix: &str, dims: &[usize], values: &[f32]) -> String {
        let raw = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.constant(prefix, DataType::Float, dims, raw)
    }

    fn ints(&mut self, values: &[i64]) -> String {
        let raw = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.constant("shape", DataType::Int64, &[values.len()], raw)
    }

    fn bools(&mut self, values: &[bool]) -> String {
        let raw = values.iter().map(|&v| u8::from(v)).collect();
        self.constant("mask", DataType::Bool, &[1, values.len()], raw)
    }

    fn op_named(
        &mut self,
        op: &str,
        inputs: &[&str],
        output: &str,
        attributes: Vec<(&str, Attribute)>,
    ) {
        self.graph.nodes.push(Node {
            op_type: op.into(),
            inputs: inputs.iter().map(|&i| i.into()).collect(),
            outputs: vec![output.into()],
            attributes: attributes.into_iter().map(|(n, a)| (n.into(), a)).collect(),
        });
    }

    fn op(&mut self, op: &str, inputs: &[&str], attributes: Vec<(&str, Attribute)>) -> String {
        let output = self.fresh(&op.to_ascii_lowercase());
        self.op_named(op, inputs, &output, attributes);
        output
    }

    fn weight(&self, name: &str) -> Result<(Vec<f32>, Vec<usize>)> {
        let data = self
            .weights
            .get(name)
            .with_context(|| format!("the checkpoint has no {name}"))?;
        let values = data
            .to_vec::<f32>()
            .map_err(|error| anyhow!("{name}: {error:?}"))?;
        Ok((values, data.shape.to_vec()))
    }

    fn reshape(&mut self, value: &str, dims: &[i64]) -> String {
        let shape = self.ints(dims);
        self.op("Reshape", &[value, &shape], vec![])
    }

    /// `value[start..end]` along `axis`.
    fn slice(&mut self, value: &str, axis: i64, start: i64, end: i64) -> String {
        let (s, e, a) = (self.ints(&[start]), self.ints(&[end]), self.ints(&[axis]));
        self.op("Slice", &[value, &s, &e, &a], vec![])
    }

    fn concat(&mut self, values: &[&str], axis: i64) -> String {
        self.op("Concat", values, vec![("axis", Attribute::Int(axis))])
    }

    fn silu(&mut self, value: &str) -> String {
        let gate = self.op("Sigmoid", &[value], vec![]);
        self.op("Mul", &[value, &gate], vec![])
    }

    /// A convolution and its BatchNorm, folded into one convolution with a
    /// bias, as `Norm::fold_into` does.
    fn conv_norm(&mut self, input: &str, conv: &str, norm: &str, stride: usize) -> Result<String> {
        let (weight, shape) = self.weight(&format!("{conv}.weight"))?;
        let (gamma, _) = self.weight(&format!("{norm}.weight"))?;
        let (beta, _) = self.weight(&format!("{norm}.bias"))?;
        let (mean, _) = self.weight(&format!("{norm}.running_mean"))?;
        let (var, _) = self.weight(&format!("{norm}.running_var"))?;
        let [out, _, kernel, _] = shape[..] else {
            bail!("{conv}.weight isn't a 2-d convolution");
        };
        let per_channel = weight.len() / out;
        let mut folded = weight;
        let mut bias = vec![0f32; out];
        for c in 0..out {
            let scale = gamma[c] / (var[c] + BATCH_NORM_EPSILON).sqrt();
            for value in &mut folded[c * per_channel..][..per_channel] {
                *value *= scale;
            }
            bias[c] = beta[c] - mean[c] * scale;
        }
        let w = self.floats("weight", &shape, &folded);
        let b = self.floats("bias", &[out], &bias);
        let pad = (kernel / 2) as i64;
        Ok(self.op(
            "Conv",
            &[input, &w, &b],
            vec![
                ("kernel_shape", Attribute::Ints(vec![kernel as i64; 2])),
                ("strides", Attribute::Ints(vec![stride as i64; 2])),
                ("pads", Attribute::Ints(vec![pad; 4])),
            ],
        ))
    }

    /// `Stage::forward`: a strided ConvNorm and SiLU, then a residual block.
    fn stage(&mut self, input: &str, prefix: &str, start: usize, stride: usize) -> Result<String> {
        let down = self.conv_norm(
            input,
            &format!("{prefix}.{start}"),
            &format!("{prefix}.{}", start + 1),
            stride,
        )?;
        let x = self.silu(&down);
        let r = format!("{prefix}.{}.network", start + 3);
        let first = self.conv_norm(&x, &format!("{r}.0"), &format!("{r}.1"), 1)?;
        let inner = self.silu(&first);
        let second = self.conv_norm(&inner, &format!("{r}.3"), &format!("{r}.4"), 1)?;
        let sum = self.op("Add", &[&x, &second], vec![]);
        Ok(self.silu(&sum))
    }

    /// A linear layer stored PyTorch's way, `[out, in]`.
    fn linear(&mut self, input: &str, name: &str) -> Result<String> {
        let (weight, shape) = self.weight(&format!("{name}.weight"))?;
        let (bias, bias_shape) = self.weight(&format!("{name}.bias"))?;
        let w = self.floats("weight", &shape, &weight);
        let b = self.floats("bias", &bias_shape, &bias);
        Ok(self.op(
            "Gemm",
            &[input, &w, &b],
            vec![("transB", Attribute::Int(1))],
        ))
    }

    /// Sigmoid, or tanh where `signed`.
    fn activate(&mut self, raw: &str, signed: &[bool]) -> String {
        let mask = self.bools(signed);
        let tanh = self.op("Tanh", &[raw], vec![]);
        let sigmoid = self.op("Sigmoid", &[raw], vec![]);
        self.op("Where", &[&mask, &tanh, &sigmoid], vec![])
    }

    fn global_mean(&mut self, value: &str, flat: i64) -> String {
        let pooled = self.op("GlobalAveragePool", &[value], vec![]);
        self.reshape(&pooled, &[1, flat])
    }

    fn finish(self) -> Graph {
        self.graph
    }
}

/// The stereo pair's gate or direction model at `size` px.
pub fn pair_graph(weights: &Weights, size: usize) -> Result<Graph> {
    let mut b = Builder::new(weights, "spatial-stereo-resnet-v2");
    let views = b.input("views", &[1, 2, size, size]);
    let mut x = b.reshape(&views, &[2, 1, size as i64, size as i64]);
    for &(start, _, stride) in &ENCODER {
        x = b.stage(&x, "encoder.network", start, stride)?;
    }
    let left = b.slice(&x, 0, 0, 1);
    let right = b.slice(&x, 0, 1, 2);
    let difference = b.op("Sub", &[&left, &right], vec![]);
    let distance = b.op("Abs", &[&difference], vec![]);
    let product = b.op("Mul", &[&left, &right], vec![]);
    let mut fused = b.concat(&[&left, &right, &distance, &product], 1);
    for &(start, _, stride) in &FUSION {
        fused = b.stage(&fused, "stereo_fusion", start, stride)?;
    }
    let channels = FUSION[FUSION.len() - 1].1[1] as i64;
    let mean = b.global_mean(&fused, channels);
    let max = b.op("GlobalMaxPool", &[&fused], vec![]);
    let max = b.reshape(&max, &[1, channels]);
    let mut values = b.concat(&[&mean, &max], 1);
    for (index, &(layer, ..)) in HEAD.iter().enumerate() {
        values = b.linear(&values, &format!("head.{layer}"))?;
        if index + 1 < HEAD.len() {
            values = b.silu(&values);
        }
    }
    let signed = TARGETS.map(|name| SIGNED.contains(&name));
    let values = b.activate(&values, &signed);
    b.output(&values, "values", &[1, TARGETS.len()]);
    Ok(b.finish())
}
