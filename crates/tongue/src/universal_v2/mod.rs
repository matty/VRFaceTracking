//! Universal face models in the `universal-face-v2` format: an ONNX graph
//! that reads the raw five-camera strip (`<name>.area.onnx`) and NumPy heads
//! with metadata pinned to the graph (`<name>.npz`), as QFT+ made them. They
//! run on VRFT's ONNX Runtime with QFT+'s per-frame logic, ported from its
//! `universal_face.py` and `face_events.py` (MIT), so a QFT+ model gives in
//! VRFT what it gives in QFT+.
//!
//! QFT+'s own model is opt-in only. Its weights were trained on Ava-256 (CC BY-NC 4.0) and
//! private MetaHuman renders: VRFT never ships or downloads them by itself.
//! The user points VRFT at a copy (`VRFT_FACE_MODEL`, the `.npz`; its
//! `.area.onnx` beside it), fetched from QFT+'s release with
//! `tools/benchmark/qftplus.py --fetch`.
//!
//! Per frame:
//! 1. The graph reads the raw 2000 x 400 strip and gives the mouth embedding
//!    `q`, the tongue head `t` and the brow embedding `w`.
//! 2. The mouth head (43 VRCFT outputs) reads `q` against the face setup's
//!    anchors; the brow head reads `w` against the neutral face.
//! 3. The tongue's direction is the face setup's ridge fit on `q`, else
//!    `tanh` of the tongue head. Its visibility and extension are Meta's own
//!    TongueOut through the event layer, as in QFT+.
//! 4. Cheek puffs split between the sides by how far `q` moved toward each
//!    one-sided puff. Inner and outer brow raises are Meta's own, split by the
//!    brow head's share.
//! 5. The event layer ([`events`]).

pub mod events;
pub mod npz;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::backend::Accelerator;
use crate::onnx::runtime::{Input, Session};
use events::{Event, FaceEvents, BROW, PUFF, SUCK, TONGUE};
use npz::Array;

pub const SCHEMA: &str = "universal-face-v2";
pub const SLOTS: [&str; 6] = [
    "neutral",
    "jaw_open",
    "pucker",
    "puff",
    "tongue_out",
    "suck",
];
const SIDES: [&str; 2] = ["puff_left", "puff_right"];
pub const CHEEKS: [&str; 4] = [
    "CheekPuffLeft",
    "CheekPuffRight",
    "CheekSuckLeft",
    "CheekSuckRight",
];
pub const BROWS: [&str; 8] = [
    "BrowInnerUpLeft",
    "BrowInnerUpRight",
    "BrowOuterUpLeft",
    "BrowOuterUpRight",
    "BrowLowererLeft",
    "BrowLowererRight",
    "BrowPinchLeft",
    "BrowPinchRight",
];
/// The raises QFT+ takes from Meta, split by the brow head's share.
const NATIVE_RAISE: [(&str, [&str; 2]); 2] = [
    ("BrowInnerUp", ["InnerBrowRaiserL", "InnerBrowRaiserR"]),
    ("BrowOuterUp", ["OuterBrowRaiserL", "OuterBrowRaiserR"]),
];
const NATIVE_PUFF: [&str; 2] = ["CheekPuffL", "CheekPuffR"];
const MAX_UNMIX_COND: f64 = 10.0;
const SHARE_SECONDS: f64 = 0.15;
const TONGUE_DIRECTION: [(&str, [f64; 2]); 5] = [
    ("tongue_out", [0.0, 0.0]),
    ("tongue_up", [0.0, 1.0]),
    ("tongue_down", [0.0, -1.0]),
    ("tongue_left", [-1.0, 0.0]),
    ("tongue_right", [1.0, 0.0]),
];
const RIDGE: f64 = 2.0;
const TONGUE_REACH: f64 = 0.15;
const TONGUE_GAIN: f64 = 2.0;
/// How old a native sample may be and still count (`label_capture.FRESH_NS`).
pub const FRESH_NS: i64 = 100_000_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    schema: String,
    names: Vec<String>,
    slots: Vec<String>,
    brow_names: Vec<String>,
    image_size: usize,
    approved_for_output: bool,
    allows_no_enrollment: bool,
    provenance: String,
    graph_sha256: String,
}

/// A dense layer, `[out, in]` row-major.
struct Layer {
    weight: Vec<f32>,
    bias: Vec<f32>,
    inputs: usize,
}

impl Layer {
    fn apply(&self, x: &[f32]) -> Vec<f32> {
        self.bias
            .iter()
            .enumerate()
            .map(|(row, b)| {
                let w = &self.weight[row * self.inputs..][..self.inputs];
                b + w.iter().zip(x).map(|(w, x)| w * x).sum::<f32>()
            })
            .collect()
    }
}

fn silu(x: f32) -> f32 {
    x / (1.0 + (-x.clamp(-60.0, 60.0)).exp())
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x.clamp(-60.0, 60.0)).exp())
}

/// What one frame gives, after the event layer.
#[derive(Clone, Debug, Default)]
pub struct V2Frame {
    /// CheekPuffLeft/Right, CheekSuckLeft/Right.
    pub cheeks: [f64; 4],
    /// BrowLowererLeft/Right, BrowPinchLeft/Right.
    pub brows: [f64; 4],
    /// BrowInnerUpLeft/Right, BrowOuterUpLeft/Right: Meta's raises split by
    /// the brow head's share; None without fresh native values.
    pub raises: Option<[f64; 4]>,
    pub tongue_visible: bool,
    /// Sent while visible: Meta's TongueOut, at least 0.4.
    pub tongue_extension: f64,
    pub tongue_horizontal: f64,
    pub tongue_vertical: f64,
}

/// Meta's own expression values, by their OpenXR names, and when they
/// arrived (nanoseconds on the caller's monotonic clock).
#[derive(Clone, Debug, Default)]
pub struct Native {
    pub arrival: i64,
    pub values: HashMap<String, f64>,
}

struct TongueMapV2 {
    mean: Vec<f32>,
    scale: Vec<f32>,
    /// `[dim, 2]`.
    weights: Vec<f32>,
    /// left, right, down, up.
    gains: [f64; 4],
}

pub struct UniversalV2 {
    session: Session,
    names: Vec<String>,
    head: [Layer; 3],
    head_missing: Vec<f32>,
    brow: [Layer; 2],
    brow_missing: Vec<f32>,
    pub provenance: String,
    pub allows_no_enrollment: bool,
    anchors: Vec<f32>,
    present: Vec<f32>,
    brow_neutral: Vec<f32>,
    brow_present: f32,
    /// Neutral `[2, 256]`, axis `[2, 256]`, unmix.
    puff_axis: Option<(Vec<f32>, Vec<f32>, Option<[[f64; 2]; 2]>)>,
    tongue_map: Option<TongueMapV2>,
    events: FaceEvents,
    share: BTreeMap<&'static str, f64>,
    share_at: Option<i64>,
}

fn take(arrays: &HashMap<String, Array>, name: &str) -> Result<(Vec<usize>, Vec<f32>)> {
    let (shape, values) = arrays
        .get(name)
        .with_context(|| format!("the model has no {name}"))?
        .floats(name)?;
    Ok((shape.to_vec(), values.to_vec()))
}

fn layer(arrays: &HashMap<String, Array>, prefix: &str, index: usize) -> Result<Layer> {
    let (shape, weight) = take(arrays, &format!("{prefix}w{index}"))?;
    let (_, bias) = take(arrays, &format!("{prefix}b{index}"))?;
    Ok(Layer {
        weight,
        bias,
        inputs: shape[1],
    })
}

impl UniversalV2 {
    /// `path` is the `.npz`; the graph is `<stem>.area.onnx` beside it.
    pub fn load(path: &Path, accelerator: Accelerator) -> Result<Self> {
        let arrays = npz::read(path)?;
        let Some(Array::Text(meta)) = arrays.get("meta") else {
            bail!("{} has no metadata", path.display());
        };
        let meta: Meta = serde_json::from_str(meta).context("QFT+ model metadata")?;
        let graph_path = path.with_extension("area.onnx");
        let graph = std::fs::read(&graph_path)
            .with_context(|| format!("reading {}", graph_path.display()))?;
        let sha = format!("{:x}", Sha256::digest(&graph));
        if meta.schema != SCHEMA
            || meta.slots != SLOTS
            || meta.brow_names != BROWS
            || ![64, 96, 128].contains(&meta.image_size)
            || sha != meta.graph_sha256
        {
            bail!("{} is not a QFT+ universal face model", path.display());
        }
        if !meta.approved_for_output {
            bail!("{} hasn't been approved for output by QFT+", path.display());
        }
        if !CHEEKS
            .iter()
            .all(|cheek| meta.names.iter().any(|name| name == cheek))
        {
            bail!("{} lacks the cheek outputs", path.display());
        }
        let session = Session::new(&graph, accelerator)?;
        let head = [
            layer(&arrays, "head_", 1)?,
            layer(&arrays, "head_", 2)?,
            layer(&arrays, "head_", 3)?,
        ];
        let brow = [layer(&arrays, "brow_", 1)?, layer(&arrays, "brow_", 2)?];
        let (_, head_missing) = take(&arrays, "head_missing")?;
        let (_, brow_missing) = take(&arrays, "brow_missing")?;
        let mut model = Self {
            session,
            names: meta.names,
            head,
            head_missing,
            brow,
            brow_missing,
            provenance: meta.provenance,
            allows_no_enrollment: meta.allows_no_enrollment,
            anchors: vec![0.0; 6 * 512],
            present: vec![0.0; 6],
            brow_neutral: vec![0.0; 480],
            brow_present: 0.0,
            puff_axis: None,
            tongue_map: None,
            events: FaceEvents::new(vec![], HashMap::new(), HashMap::new(), 0.0),
            share: BTreeMap::new(),
            share_at: None,
        };
        model.enroll(&BTreeMap::new())?;
        Ok(model)
    }

    pub fn device(&self) -> &str {
        &self.session.device
    }

    /// The graph on one strip: `q` (512), `t` (7), `w` (480).
    pub fn run(&mut self, strip: &[u8]) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>)> {
        let mut out = self.session.run_inputs(
            &[("cameras", &[400, 2000], Input::U8(strip))],
            &["mouth", "tongue", "brows"],
        )?;
        let w = out.pop().unwrap_or_default();
        let t = out.pop().unwrap_or_default();
        let q = out.pop().unwrap_or_default();
        Ok((q, t, w))
    }

    /// `head_forward`: the mouth head's 43 probabilities.
    fn head_forward(&self, q: &[f32], anchors: &[f32], present: &[f32]) -> Vec<f32> {
        let mut z = Vec::with_capacity(4102);
        z.extend_from_slice(q);
        let mut a = vec![0f32; 6 * 512];
        for slot in 0..6 {
            let source = if present[slot] > 0.0 {
                anchors
            } else {
                &self.head_missing
            };
            a[slot * 512..][..512].copy_from_slice(&source[slot * 512..][..512]);
        }
        z.extend_from_slice(&a);
        z.extend(q.iter().zip(&a[..512]).map(|(q, n)| q - n));
        z.extend_from_slice(present);
        let z: Vec<f32> = self.head[0].apply(&z).into_iter().map(silu).collect();
        let z: Vec<f32> = self.head[1].apply(&z).into_iter().map(silu).collect();
        self.head[2].apply(&z).into_iter().map(sigmoid).collect()
    }

    /// `brow_forward`: the 8 brows.
    fn brow_forward(&self, w: &[f32], neutral: &[f32], present: f32) -> Vec<f32> {
        let n = if present > 0.0 {
            neutral
        } else {
            &self.brow_missing
        };
        let mut z = Vec::with_capacity(961);
        z.extend_from_slice(w);
        z.extend(w.iter().zip(n).map(|(w, n)| w - n));
        z.push(present);
        let z: Vec<f32> = self.brow[0].apply(&z).into_iter().map(silu).collect();
        self.brow[1].apply(&z).into_iter().map(sigmoid).collect()
    }

    fn index(&self, name: &str) -> usize {
        self.names.iter().position(|n| n == name).unwrap_or(0)
    }

    /// Encodes the face setup, by slot name (QFT+'s slots, one-sided puffs and
    /// tongue holds, as VRFT's face setup records them), as QFT+'s
    /// `encode_enrollment` does, and sets the event layer up from it.
    pub fn enroll(&mut self, frames: &BTreeMap<String, Vec<Vec<u8>>>) -> Result<()> {
        let mut anchors = vec![0f32; 6 * 512];
        let mut present = vec![0f32; 6];
        let mut sides: HashMap<&str, Vec<f32>> = HashMap::new();
        let (mut brow_neutral, mut brow_present) = (vec![0f32; 480], 0f32);
        let mut brow_rest: HashMap<String, f64> = HashMap::new();
        let mut holds: HashMap<&str, Vec<Vec<f32>>> = HashMap::new();
        let order = SLOTS.iter().chain(&SIDES).chain(
            TONGUE_DIRECTION
                .iter()
                .map(|(s, _)| s)
                .filter(|s| !SLOTS.contains(s)),
        );
        for &slot in order {
            let Some(strips) = frames.get(slot).filter(|s| !s.is_empty()) else {
                continue;
            };
            let (mut qs, mut ws) = (vec![], vec![]);
            for strip in strips {
                let (q, _, w) = self.run(strip)?;
                qs.push(q);
                ws.push(w);
            }
            if TONGUE_DIRECTION.iter().any(|(s, _)| *s == slot) {
                holds.insert(slot, qs.clone());
                if slot != "tongue_out" {
                    continue;
                }
            }
            if let Some(side) = SIDES.iter().find(|s| **s == slot) {
                sides.insert(side, mean(&qs));
                continue;
            }
            let index = SLOTS.iter().position(|s| *s == slot).unwrap();
            anchors[index * 512..][..512].copy_from_slice(&mean(&qs));
            present[index] = 1.0;
            if slot == "neutral" {
                brow_neutral = mean(&ws);
                brow_present = 1.0;
                let outputs: Vec<Vec<f32>> = ws
                    .iter()
                    .map(|w| self.brow_forward(w, &brow_neutral, 1.0))
                    .collect();
                for (k, name) in BROWS.iter().enumerate() {
                    let mut column: Vec<f32> = outputs.iter().map(|o| o[k]).collect();
                    brow_rest.insert(name.to_string(), median(&mut column));
                }
            }
        }
        self.tongue_map = tongue_map(&holds);
        self.anchors = anchors;
        self.present = present;
        self.brow_neutral = brow_neutral;
        self.brow_present = brow_present;

        // Which way each one-sided puff moves q, to split a puff's amount.
        self.puff_axis = None;
        let mut reach: HashMap<String, f64> = HashMap::new();
        if self.present[0] > 0.0 && self.present[3] > 0.0 {
            let n = self.anchors[..512].to_vec();
            let d: Vec<f32> = self.anchors[3 * 512..][..512]
                .iter()
                .zip(&n)
                .map(|(p, n)| p - n)
                .collect();
            let mut axis = d.clone();
            for camera in 0..2 {
                let row = &d[camera * 256..][..256];
                let norm = row.iter().map(|v| v * v).sum::<f32>().max(1e-6);
                for v in &mut axis[camera * 256..][..256] {
                    *v /= norm;
                }
            }
            let mut unmix = None;
            if sides.len() == 2 {
                let left = puff_depth(&sides["puff_left"], &n, &axis);
                let right = puff_depth(&sides["puff_right"], &n, &axis);
                let mix = [[left[0], right[0]], [left[1], right[1]]];
                if condition(mix) < MAX_UNMIX_COND {
                    unmix = inverse(mix);
                }
            }
            self.puff_axis = Some((n, axis, unmix));
        }
        let mut cheek_rest: HashMap<String, f64> = HashMap::new();
        if self.present[0] > 0.0 {
            let level = |model: &Self, slot: usize, cheeks: &[&str]| -> f64 {
                let p = model.head_forward(
                    &model.anchors[slot * 512..][..512],
                    &model.anchors,
                    &model.present,
                );
                cheeks
                    .iter()
                    .map(|c| p[model.index(c)] as f64)
                    .fold(f64::NEG_INFINITY, f64::max)
            };
            let puff_rest = level(self, 0, &CHEEKS[..2]);
            let suck_rest = level(self, 0, &CHEEKS[2..]);
            for (k, cheek) in CHEEKS.iter().enumerate() {
                cheek_rest.insert(cheek.to_string(), if k < 2 { puff_rest } else { suck_rest });
            }
            for (slot, cheeks) in [(3, &CHEEKS[..2]), (5, &CHEEKS[2..])] {
                if self.present[slot] > 0.0 {
                    let full = level(self, slot, cheeks);
                    if full - cheek_rest[cheeks[0]] >= 0.2 {
                        for cheek in cheeks {
                            reach.insert(cheek.to_string(), full);
                        }
                    }
                }
            }
        }
        brow_rest.retain(|name, _| !NATIVE_RAISE.iter().any(|(base, _)| name.starts_with(base)));
        let mut neutral = brow_rest;
        neutral.extend(cheek_rest);
        let mut config: Vec<(String, Event)> = vec![
            ("CheekPuffLeft".into(), PUFF),
            ("CheekPuffRight".into(), PUFF),
            ("CheekSuckLeft".into(), SUCK),
            ("CheekSuckRight".into(), SUCK),
            ("TongueOut".into(), TONGUE),
        ];
        config.extend(BROWS.iter().map(|b| (b.to_string(), BROW)));
        self.events = FaceEvents::new(config, neutral, reach, 0.0);
        self.share.clear();
        self.share_at = None;
        if !self.present.iter().any(|&p| p > 0.0) && !self.allows_no_enrollment {
            bail!("this model needs the face setup");
        }
        Ok(())
    }

    /// QFT+'s `update`: one strip, the latest native sample (if any), now.
    pub fn update(&mut self, strip: &[u8], native: Option<&Native>, now: i64) -> Result<V2Frame> {
        let (q, t, w) = self.run(strip)?;
        let probabilities = self.head_forward(&q, &self.anchors, &self.present);
        let mut p: HashMap<String, f64> = self
            .names
            .iter()
            .cloned()
            .zip(probabilities.iter().map(|&v| v as f64))
            .collect();
        let brows = self.brow_forward(&w, &self.brow_neutral, self.brow_present);
        for (name, value) in BROWS.iter().zip(&brows) {
            p.insert(name.to_string(), *value as f64);
        }
        let (horizontal, vertical) = match &self.tongue_map {
            Some(map) => tongue_direction(&q, map),
            None => ((t[1] as f64).tanh(), (t[2] as f64).tanh()),
        };
        let fresh = native.filter(|n| (n.arrival - now).abs() <= FRESH_NS);
        let empty = HashMap::new();
        let nv = fresh.map_or(&empty, |n| &n.values);
        let native_puff = NATIVE_PUFF.map(|n| nv.get(n).copied().unwrap_or(0.0));
        if let Some((n, axis, unmix)) = &self.puff_axis {
            let d = puff_depth(&q, n, axis);
            let share = match unmix {
                None => {
                    let top = d[0].max(d[1]).max(1e-6);
                    [(d[0] / top).powi(2), (d[1] / top).powi(2)]
                }
                Some(m) => {
                    let u = [
                        (m[0][0] * d[0] + m[0][1] * d[1]).max(0.0),
                        (m[1][0] * d[0] + m[1][1] * d[1]).max(0.0),
                    ];
                    let top = u[0].max(u[1]).max(1e-6);
                    [u[0] / top, u[1] / top]
                }
            };
            let amount = p["CheekPuffLeft"]
                .max(p["CheekPuffRight"])
                .max(native_puff[0])
                .max(native_puff[1]);
            p.insert("CheekPuffLeft".into(), amount * share[0]);
            p.insert("CheekPuffRight".into(), amount * share[1]);
        } else {
            p.insert(
                "CheekPuffLeft".into(),
                p["CheekPuffLeft"].max(native_puff[0]),
            );
            p.insert(
                "CheekPuffRight".into(),
                p["CheekPuffRight"].max(native_puff[1]),
            );
        }
        let dt = self
            .share_at
            .map_or(0.0, |at| ((now - at) as f64 / 1e9).max(0.0));
        self.share_at = Some(now);
        let mut raises = [0.0; 4];
        let mut have_raises = true;
        for (k, (base, [left, right])) in NATIVE_RAISE.iter().enumerate() {
            let a = p[&format!("{base}Left")] + 0.05;
            let b = p[&format!("{base}Right")] + 0.05;
            let previous = self.share.get(base).copied();
            let rate = if previous.is_none() {
                1.0
            } else {
                1.0 - (-dt / SHARE_SECONDS).exp()
            };
            let old = previous.unwrap_or(0.0);
            let share = old + rate * (a / (a + b) - old);
            self.share.insert(base, share);
            match (nv.get(*left), nv.get(*right)) {
                (Some(l), Some(r)) => {
                    let amp = (l + r) / 2.0;
                    raises[2 * k] = (amp * 2.0 * share).min(1.0);
                    raises[2 * k + 1] = (amp * 2.0 * (1.0 - share)).min(1.0);
                }
                _ => have_raises = false,
            }
        }
        let tongue_native = nv.get("TongueOut").copied().unwrap_or(0.0);
        let mut values: Vec<(String, f64)> = CHEEKS
            .iter()
            .chain(&BROWS)
            .map(|name| (name.to_string(), p[*name]))
            .collect();
        values.push(("TongueOut".into(), tongue_native));
        let out = self.events.step(
            &values,
            native.map(|n| n.arrival),
            fresh.map(|n| &n.values),
            now,
        );
        let get = |name: &str| out.iter().find(|(n, _)| n == name).map_or(0.0, |&(_, v)| v);
        Ok(V2Frame {
            cheeks: CHEEKS.map(get),
            brows: [
                "BrowLowererLeft",
                "BrowLowererRight",
                "BrowPinchLeft",
                "BrowPinchRight",
            ]
            .map(get),
            raises: have_raises.then_some(raises),
            tongue_visible: self.events.on["TongueOut"],
            tongue_extension: tongue_native.max(0.4),
            tongue_horizontal: horizontal,
            tongue_vertical: vertical,
        })
    }
}

fn mean(rows: &[Vec<f32>]) -> Vec<f32> {
    let mut total = vec![0f64; rows[0].len()];
    for row in rows {
        for (t, v) in total.iter_mut().zip(row) {
            *t += *v as f64;
        }
    }
    total
        .iter()
        .map(|t| (*t / rows.len() as f64) as f32)
        .collect()
}

fn median(values: &mut [f32]) -> f64 {
    values.sort_by(f32::total_cmp);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2] as f64
    } else {
        (values[n / 2 - 1] as f64 + values[n / 2] as f64) / 2.0
    }
}

/// Per mouth camera, how far `q` moved from neutral toward the both-cheek
/// puff (1 = all the way).
fn puff_depth(q: &[f32], neutral: &[f32], axis: &[f32]) -> [f64; 2] {
    let mut d = [0f64; 2];
    for (camera, depth) in d.iter_mut().enumerate() {
        let range = camera * 256..(camera + 1) * 256;
        let sum: f32 = q[range.clone()]
            .iter()
            .zip(&neutral[range.clone()])
            .zip(&axis[range])
            .map(|((q, n), a)| (q - n) * a)
            .sum();
        *depth = (sum as f64).max(0.0);
    }
    d
}

/// The 2-norm condition number of a 2 x 2 matrix.
fn condition(m: [[f64; 2]; 2]) -> f64 {
    let (a, b, c, d) = (m[0][0], m[0][1], m[1][0], m[1][1]);
    let s = a * a + b * b + c * c + d * d;
    let det = (a * d - b * c).abs();
    let root = (s * s - 4.0 * det * det).max(0.0).sqrt();
    let (high, low) = (
        ((s + root) / 2.0).sqrt(),
        ((s - root) / 2.0).max(0.0).sqrt(),
    );
    if low == 0.0 {
        f64::INFINITY
    } else {
        high / low
    }
}

fn inverse(m: [[f64; 2]; 2]) -> Option<[[f64; 2]; 2]> {
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    (det != 0.0).then(|| {
        [
            [m[1][1] / det, -m[0][1] / det],
            [-m[1][0] / det, m[0][0] / det],
        ]
    })
}

/// QFT+'s `tongue_map`: a ridge fit from the held poses' mouth embeddings to
/// their directions, with a gain per direction.
fn tongue_map(holds: &HashMap<&str, Vec<Vec<f32>>>) -> Option<TongueMapV2> {
    if !TONGUE_DIRECTION.iter().all(|(s, _)| holds.contains_key(s)) {
        return None;
    }
    let mut x: Vec<Vec<f64>> = vec![];
    let mut y: Vec<[f64; 2]> = vec![];
    for (slot, direction) in TONGUE_DIRECTION {
        for q in &holds[slot] {
            x.push(q.iter().map(|&v| v as f64).collect());
            y.push(direction);
        }
    }
    let (rows, dim) = (x.len(), x[0].len());
    let mean: Vec<f64> = (0..dim)
        .map(|j| x.iter().map(|r| r[j]).sum::<f64>() / rows as f64)
        .collect();
    let scale: Vec<f64> = (0..dim)
        .map(|j| {
            (x.iter().map(|r| (r[j] - mean[j]).powi(2)).sum::<f64>() / rows as f64).sqrt() + 1e-3
        })
        .collect();
    let xn: Vec<Vec<f64>> = x
        .iter()
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(j, v)| (v - mean[j]) / scale[j])
                .collect()
        })
        .collect();
    // weights = xn' (xn xn' + RIDGE * dim * I)^-1 y
    let mut gram = vec![vec![0f64; rows]; rows];
    for i in 0..rows {
        for k in 0..rows {
            gram[i][k] = xn[i].iter().zip(&xn[k]).map(|(a, b)| a * b).sum::<f64>()
                + if i == k { RIDGE * dim as f64 } else { 0.0 };
        }
    }
    let alpha = solve(gram, y.clone())?;
    let mut weights = vec![0f64; dim * 2];
    for j in 0..dim {
        for i in 0..rows {
            weights[j * 2] += xn[i][j] * alpha[i][0];
            weights[j * 2 + 1] += xn[i][j] * alpha[i][1];
        }
    }
    let held = |slot: &str| -> [f64; 2] {
        let mut h: Vec<f64> = vec![];
        let mut v: Vec<f64> = vec![];
        for q in &holds[slot] {
            let (a, b) = (0..dim).fold((0.0, 0.0), |(a, b), j| {
                let n = (q[j] as f64 - mean[j]) / scale[j];
                (a + n * weights[j * 2], b + n * weights[j * 2 + 1])
            });
            h.push(a);
            v.push(b);
        }
        let mid = |values: &mut Vec<f64>| {
            values.sort_by(f64::total_cmp);
            let n = values.len();
            if n % 2 == 1 {
                values[n / 2]
            } else {
                (values[n / 2 - 1] + values[n / 2]) / 2.0
            }
        };
        [mid(&mut h), mid(&mut v)]
    };
    let gain = |reach: f64| {
        if reach >= TONGUE_REACH {
            (1.0 / reach).min(TONGUE_GAIN)
        } else {
            1.0
        }
    };
    let gains = [
        gain(-held("tongue_left")[0]),
        gain(held("tongue_right")[0]),
        gain(-held("tongue_down")[1]),
        gain(held("tongue_up")[1]),
    ];
    Some(TongueMapV2 {
        mean: mean.iter().map(|&v| v as f32).collect(),
        scale: scale.iter().map(|&v| v as f32).collect(),
        weights: weights.iter().map(|&v| v as f32).collect(),
        gains,
    })
}

fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<[f64; 2]>) -> Option<Vec<[f64; 2]>> {
    let n = a.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            for k in col..n {
                a[row][k] -= f * a[col][k];
            }
            b[row][0] -= f * b[col][0];
            b[row][1] -= f * b[col][1];
        }
    }
    let mut x = vec![[0f64; 2]; n];
    for row in (0..n).rev() {
        for c in 0..2 {
            let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k][c]).sum();
            x[row][c] = (b[row][c] - s) / a[row][row];
        }
    }
    Some(x)
}

/// QFT+'s `tongue_direction`, in float32 as it computes it.
fn tongue_direction(q: &[f32], map: &TongueMapV2) -> (f64, f64) {
    let (mut h, mut v) = (0f32, 0f32);
    for (j, &value) in q.iter().enumerate() {
        let n = (value - map.mean[j]) / map.scale[j];
        h += n * map.weights[j * 2];
        v += n * map.weights[j * 2 + 1];
    }
    let [left, right, down, up] = map.gains;
    let (h, v) = (h as f64, v as f64);
    (
        (h * if h > 0.0 { right } else { left }).clamp(-1.0, 1.0),
        (v * if v > 0.0 { up } else { down }).clamp(-1.0, 1.0),
    )
}
