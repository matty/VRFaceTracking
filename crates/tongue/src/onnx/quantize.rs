//! int8 quantization of a model graph, calibrated on real frames, as ONNX
//! Runtime's static QDQ quantization does it, so its int8 kernels run the
//! convolutions:
//!
//! - Every float tensor in the convolutional part of the graph (the views
//!   in, through to the pooling) gets a QuantizeLinear/DequantizeLinear pair,
//!   uint8 with a zero point, scaled to the range it took on the calibration
//!   frames.
//! - Convolution weights are int8 per output channel, symmetric, and their
//!   biases int32 at the input's scale times the weight's.
//! - The pooling, the linear layers and the heads stay float.
//!
//! The int8 graph is saved beside the checkpoint (`<file>.int8.onnx`) with the
//! weights' fingerprint (`<file>.int8.json`), and only used for those weights.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::proto::{Attribute, DataType, Graph, Initializer, Node, ValueInfo};
use crate::model::Weights;

/// Ops whose float outputs are quantized: the convolutional part.
const QUANTIZED: [&str; 9] = [
    "Conv", "Sigmoid", "Mul", "Add", "Sub", "Abs", "Concat", "Slice", "Reshape",
];
/// Where the convolutional part ends: these read quantized tensors and give
/// float ones.
const ENDS: [&str; 4] = ["AveragePool", "GlobalAveragePool", "GlobalMaxPool", "Gemm"];

/// The frames calibration reads before quantizing.
pub const CALIBRATION_FRAMES: usize = 48;

/// The tensors quantization covers: the graph's `views` input and every
/// float output of a convolutional-part node, in graph order, except those
/// whose names start with one of `float` (see [`super::TONGUE_SCOPE`]).
pub fn tensors(graph: &Graph, float: &[&str]) -> Vec<String> {
    let mut region: HashSet<String> = HashSet::from(["views".to_string()]);
    let mut out = vec!["views".to_string()];
    for node in &graph.nodes {
        if ENDS.contains(&node.op_type.as_str()) || !QUANTIZED.contains(&node.op_type.as_str()) {
            continue;
        }
        if float
            .iter()
            .any(|prefix| node.outputs[0].starts_with(prefix))
        {
            continue;
        }
        // In the region when it reads a region tensor (constants aside).
        if node.inputs.iter().any(|input| region.contains(input)) {
            for output in &node.outputs {
                if graph.outputs.iter().any(|o| &o.name == output) {
                    continue;
                }
                region.insert(output.clone());
                out.push(output.clone());
            }
        }
    }
    out
}

/// `graph` with every tensor in `names` also an output, to read their range.
pub fn calibration_graph(graph: &Graph, names: &[String]) -> Graph {
    let mut graph = graph.clone();
    for name in names {
        if graph.outputs.iter().all(|o| &o.name != name) {
            graph.outputs.push(ValueInfo {
                name: name.clone(),
                data_type: DataType::Float,
                dims: vec![],
            });
        }
    }
    graph
}

/// Running minimum and maximum per tensor.
#[derive(Default)]
pub struct Ranges(HashMap<String, (f32, f32)>);

impl Ranges {
    pub fn observe(&mut self, name: &str, values: &[f32]) {
        let (low, high) = values
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
                (lo.min(v), hi.max(v))
            });
        let entry = self.0.entry(name.to_string()).or_insert((low, high));
        entry.0 = entry.0.min(low);
        entry.1 = entry.1.max(high);
    }
}

/// uint8 scale and zero point for a range, which always covers 0.
fn activation_params(low: f32, high: f32) -> (f32, u8) {
    let (low, high) = (low.min(0.0), high.max(0.0));
    let scale = ((high - low) / 255.0).max(1e-8);
    let zero = (-low / scale).round().clamp(0.0, 255.0) as u8;
    (scale, zero)
}

fn floats(initializer: &Initializer) -> Vec<f32> {
    initializer
        .raw
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn initializer(name: String, data_type: DataType, dims: Vec<i64>, raw: Vec<u8>) -> Initializer {
    Initializer {
        name,
        dims,
        data_type,
        raw,
    }
}

/// `graph` with QDQ pairs and int8 convolution weights, except for the
/// values `float` leaves out (as [`tensors`] takes it).
pub fn quantize(graph: &Graph, ranges: &Ranges, float: &[&str]) -> Result<Graph> {
    let names = tensors(graph, float);
    let mut out = graph.clone();
    let mut nodes = Vec::with_capacity(graph.nodes.len() * 3);
    let mut params: HashMap<String, (f32, u8)> = HashMap::new();
    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut extra: Vec<Initializer> = vec![];
    let quantized: HashSet<&String> = names.iter().collect();

    let mut qdq = Pairs {
        ranges,
        params: &mut params,
        renamed: &mut renamed,
    };
    qdq.add("views", &mut nodes, &mut extra)?;
    let by_name: HashMap<String, Initializer> = graph
        .initializers
        .iter()
        .map(|i| (i.name.clone(), i.clone()))
        .collect();
    let mut replaced: HashSet<String> = HashSet::new();
    for original in &graph.nodes {
        let mut current = original.clone();
        for input in &mut current.inputs {
            if let Some(dq) = qdq.renamed.get(input.as_str()) {
                *input = dq.clone();
            }
        }
        let left_float = float
            .iter()
            .any(|prefix| original.outputs[0].starts_with(prefix));
        if current.op_type == "Conv" && !left_float && quantized.contains(&original.inputs[0]) {
            let x_scale = qdq
                .params
                .get(&original.inputs[0])
                .map(|p| p.0)
                .with_context(|| format!("{} isn't quantized", original.inputs[0]))?;
            let weight = &by_name[&original.inputs[1]];
            let bias = &by_name[&original.inputs[2]];
            let (w, b) = (floats(weight), floats(bias));
            let channels = weight.dims[0] as usize;
            let per = w.len() / channels;
            let mut w_q = Vec::with_capacity(w.len());
            let mut w_scales = Vec::with_capacity(channels);
            let mut b_q = Vec::with_capacity(channels);
            for c in 0..channels {
                let row = &w[c * per..][..per];
                let scale = (row.iter().fold(0f32, |m, v| m.max(v.abs())) / 127.0).max(1e-10);
                w_scales.push(scale);
                w_q.extend(
                    row.iter()
                        .map(|v| (v / scale).round().clamp(-127.0, 127.0) as i8 as u8),
                );
                b_q.push(
                    ((b[c] / (x_scale * scale)).round() as f64)
                        .clamp(i32::MIN as f64, i32::MAX as f64) as i32,
                );
            }
            let wn = &original.inputs[1];
            let bn = &original.inputs[2];
            let channel = vec![channels as i64];
            extra.push(initializer(
                format!("{wn}_q"),
                DataType::Int8,
                weight.dims.clone(),
                w_q,
            ));
            extra.push(initializer(
                format!("{wn}_scale"),
                DataType::Float,
                channel.clone(),
                w_scales.iter().flat_map(|v| v.to_le_bytes()).collect(),
            ));
            extra.push(initializer(
                format!("{wn}_zero"),
                DataType::Int8,
                channel.clone(),
                vec![0; channels],
            ));
            extra.push(initializer(
                format!("{bn}_q"),
                DataType::Int32,
                channel.clone(),
                b_q.iter().flat_map(|v| v.to_le_bytes()).collect(),
            ));
            extra.push(initializer(
                format!("{bn}_scale"),
                DataType::Float,
                channel.clone(),
                w_scales
                    .iter()
                    .flat_map(|s| (s * x_scale).to_le_bytes())
                    .collect(),
            ));
            extra.push(initializer(
                format!("{bn}_zero"),
                DataType::Int32,
                channel,
                vec![0; channels * 4],
            ));
            let axis = vec![("axis", Attribute::Int(0))];
            nodes.push(node(
                "DequantizeLinear",
                &[
                    &format!("{wn}_q"),
                    &format!("{wn}_scale"),
                    &format!("{wn}_zero"),
                ],
                &format!("{wn}_dq"),
                axis.clone(),
            ));
            nodes.push(node(
                "DequantizeLinear",
                &[
                    &format!("{bn}_q"),
                    &format!("{bn}_scale"),
                    &format!("{bn}_zero"),
                ],
                &format!("{bn}_dq"),
                axis,
            ));
            current.inputs[1] = format!("{wn}_dq");
            current.inputs[2] = format!("{bn}_dq");
            replaced.insert(wn.clone());
            replaced.insert(bn.clone());
        }
        let outputs = current.outputs.clone();
        nodes.push(current);
        for output in outputs {
            if quantized.contains(&output) {
                qdq.add(&output, &mut nodes, &mut extra)?;
            }
        }
    }
    out.nodes = nodes;
    out.initializers.retain(|i| !replaced.contains(&i.name));
    out.initializers.extend(extra);
    if out.nodes.iter().filter(|n| n.op_type == "Conv").count()
        != graph.nodes.iter().filter(|n| n.op_type == "Conv").count()
    {
        bail!("quantization lost a convolution");
    }
    Ok(out)
}

/// QDQ pairs after tensors; consumers then read `<tensor>_dq`.
struct Pairs<'a> {
    ranges: &'a Ranges,
    params: &'a mut HashMap<String, (f32, u8)>,
    renamed: &'a mut HashMap<String, String>,
}

impl Pairs<'_> {
    fn add(
        &mut self,
        tensor: &str,
        nodes: &mut Vec<Node>,
        extra: &mut Vec<Initializer>,
    ) -> Result<()> {
        let &(low, high) = self
            .ranges
            .0
            .get(tensor)
            .with_context(|| format!("no calibration range for {tensor}"))?;
        let (scale, zero) = activation_params(low, high);
        self.params.insert(tensor.to_string(), (scale, zero));
        let s = format!("{tensor}_scale");
        let z = format!("{tensor}_zero");
        extra.push(initializer(
            s.clone(),
            DataType::Float,
            vec![],
            scale.to_le_bytes().to_vec(),
        ));
        extra.push(initializer(z.clone(), DataType::Uint8, vec![], vec![zero]));
        let (q, dq) = (format!("{tensor}_q"), format!("{tensor}_dq"));
        nodes.push(node("QuantizeLinear", &[tensor, &s, &z], &q, vec![]));
        nodes.push(node("DequantizeLinear", &[&q, &s, &z], &dq, vec![]));
        self.renamed.insert(tensor.to_string(), dq);
        Ok(())
    }
}

fn node(op: &str, inputs: &[&str], output: &str, attributes: Vec<(&str, Attribute)>) -> Node {
    Node {
        op_type: op.into(),
        inputs: inputs.iter().map(|i| i.to_string()).collect(),
        outputs: vec![output.into()],
        attributes: attributes.into_iter().map(|(n, a)| (n.into(), a)).collect(),
    }
}

/// A fingerprint of the weights an int8 graph was made from.
pub fn fingerprint(weights: &Weights) -> String {
    let mut names: Vec<&String> = weights.keys().collect();
    names.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for name in names {
        name.hash(&mut hasher);
        weights[name].as_bytes().hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

#[derive(Serialize, Deserialize)]
struct Saved {
    fingerprint: String,
    /// The values left float.
    #[serde(default)]
    float: Vec<String>,
    calibration_frames: usize,
}

fn paths(checkpoint: &Path) -> (PathBuf, PathBuf) {
    let name = checkpoint
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (
        checkpoint.with_file_name(format!("{name}.int8.onnx")),
        checkpoint.with_file_name(format!("{name}.int8.json")),
    )
}

/// The int8 graph saved beside `checkpoint`, if it was made from `weights`
/// with the same values left `float`.
pub fn saved(checkpoint: &Path, weights: &Weights, float: &[&str]) -> Result<Option<Vec<u8>>> {
    let (model, info) = paths(checkpoint);
    let Ok(text) = std::fs::read_to_string(&info) else {
        return Ok(None);
    };
    let Ok(saved) = serde_json::from_str::<Saved>(&text) else {
        return Ok(None);
    };
    if saved.fingerprint != fingerprint(weights) || saved.float != float {
        return Ok(None);
    }
    Ok(std::fs::read(model).ok())
}

/// Saves an int8 graph beside `checkpoint`. A folder that can't be written
/// to only costs a calibration next time.
pub fn save(
    checkpoint: &Path,
    weights: &Weights,
    float: &[&str],
    model: &[u8],
    frames: usize,
) -> Result<()> {
    let (path, info) = paths(checkpoint);
    std::fs::write(&path, model).with_context(|| format!("writing {}", path.display()))?;
    let saved = Saved {
        fingerprint: fingerprint(weights),
        float: float.iter().map(|value| value.to_string()).collect(),
        calibration_frames: frames,
    };
    std::fs::write(&info, serde_json::to_string_pretty(&saved)?)
        .with_context(|| format!("writing {}", info.display()))?;
    Ok(())
}
