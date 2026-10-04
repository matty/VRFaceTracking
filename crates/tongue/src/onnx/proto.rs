//! The few ONNX protobuf messages a model needs, written by hand: ONNX's
//! schema is proto2, and only these fields are used.

/// ONNX `TensorProto.DataType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataType {
    Float = 1,
    Uint8 = 2,
    Int8 = 3,
    Int32 = 6,
    Int64 = 7,
    Bool = 9,
}

/// A constant tensor (`TensorProto`).
#[derive(Clone, Debug)]
pub struct Initializer {
    pub name: String,
    pub dims: Vec<i64>,
    pub data_type: DataType,
    /// Little-endian values.
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum Attribute {
    Int(i64),
    Float(f32),
    Ints(Vec<i64>),
}

#[derive(Clone, Debug)]
pub struct Node {
    pub op_type: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub attributes: Vec<(String, Attribute)>,
}

/// A graph input or output: a tensor of a fixed shape, or of an unknown one
/// when `dims` is empty.
#[derive(Clone, Debug)]
pub struct ValueInfo {
    pub name: String,
    pub data_type: DataType,
    pub dims: Vec<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub name: String,
    pub nodes: Vec<Node>,
    pub initializers: Vec<Initializer>,
    pub inputs: Vec<ValueInfo>,
    pub outputs: Vec<ValueInfo>,
}

/// ONNX IR version 8 and opset 17: what ONNX Runtime 1.14 and later read.
const IR_VERSION: i64 = 8;
pub const OPSET: i64 = 17;

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn key(out: &mut Vec<u8>, field: u32, wire: u32) {
    varint(out, u64::from(field << 3 | wire));
}

fn int(out: &mut Vec<u8>, field: u32, value: i64) {
    key(out, field, 0);
    varint(out, value as u64);
}

fn bytes(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    key(out, field, 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn string(out: &mut Vec<u8>, field: u32, value: &str) {
    bytes(out, field, value.as_bytes());
}

fn float(out: &mut Vec<u8>, field: u32, value: f32) {
    key(out, field, 5);
    out.extend_from_slice(&value.to_le_bytes());
}

impl Initializer {
    fn encode(&self) -> Vec<u8> {
        let mut out = vec![];
        for &dim in &self.dims {
            int(&mut out, 1, dim);
        }
        int(&mut out, 2, self.data_type as i64);
        string(&mut out, 8, &self.name);
        bytes(&mut out, 9, &self.raw);
        out
    }
}

impl Node {
    fn encode(&self) -> Vec<u8> {
        let mut out = vec![];
        for input in &self.inputs {
            string(&mut out, 1, input);
        }
        for output in &self.outputs {
            string(&mut out, 2, output);
        }
        string(&mut out, 3, &self.outputs[0]);
        string(&mut out, 4, &self.op_type);
        for (name, value) in &self.attributes {
            let mut attribute = vec![];
            string(&mut attribute, 1, name);
            match value {
                Attribute::Float(value) => {
                    float(&mut attribute, 2, *value);
                    int(&mut attribute, 20, 1);
                }
                Attribute::Int(value) => {
                    int(&mut attribute, 3, *value);
                    int(&mut attribute, 20, 2);
                }
                Attribute::Ints(values) => {
                    for &value in values {
                        int(&mut attribute, 8, value);
                    }
                    int(&mut attribute, 20, 7);
                }
            }
            bytes(&mut out, 5, &attribute);
        }
        out
    }
}

impl ValueInfo {
    fn encode(&self) -> Vec<u8> {
        let mut shape = vec![];
        for &dim in &self.dims {
            let mut dimension = vec![];
            int(&mut dimension, 1, dim);
            bytes(&mut shape, 1, &dimension);
        }
        let mut tensor = vec![];
        int(&mut tensor, 1, self.data_type as i64);
        // No dims: shape unknown (never a scalar here).
        if !self.dims.is_empty() {
            bytes(&mut tensor, 2, &shape);
        }
        let mut type_proto = vec![];
        bytes(&mut type_proto, 1, &tensor);
        let mut out = vec![];
        string(&mut out, 1, &self.name);
        bytes(&mut out, 2, &type_proto);
        out
    }
}

impl Graph {
    fn encode(&self) -> Vec<u8> {
        let mut out = vec![];
        for node in &self.nodes {
            bytes(&mut out, 1, &node.encode());
        }
        string(&mut out, 2, &self.name);
        for initializer in &self.initializers {
            bytes(&mut out, 5, &initializer.encode());
        }
        for input in &self.inputs {
            bytes(&mut out, 11, &input.encode());
        }
        for output in &self.outputs {
            bytes(&mut out, 12, &output.encode());
        }
        out
    }

    /// The serialised `ModelProto`.
    pub fn to_model(&self) -> Vec<u8> {
        let mut out = vec![];
        int(&mut out, 1, IR_VERSION);
        string(&mut out, 2, "vrft-tongue");
        bytes(&mut out, 7, &self.encode());
        let mut opset = vec![];
        string(&mut opset, 1, "");
        int(&mut opset, 2, OPSET);
        bytes(&mut out, 8, &opset);
        out
    }
}
