//! Live inference with the universal face model, and the wearer's
//! enrollment: the one-minute face setup's frames, encoded once when the
//! model loads.
//!
//! Enrollment gives the mouth head its anchors (the mean mouth embedding of
//! each slot's frames), the brow head its neutral (the mean brow embedding
//! of the neutral frames), and fits how this wearer's tongue reads each way:
//! a ridge regression from the mouth embedding of the five held tongue
//! poses to their directions, as QFT+ does, which then replaces the tongue
//! head's horizontal and vertical.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use burn::tensor::backend::Backend;
use burn::tensor::{Tensor, TensorData};
use log::warn;

use super::checkpoint::FaceCheckpoint;
use super::net::{activate, FaceNet};
use super::{ANCHOR_SLOTS, BROW_EMBEDDING, CAMERAS, FACE_TARGETS, MOUTH_EMBEDDING};
use crate::backend::{Accelerator, Cpu, Gpu, GPU_NAME};
use crate::infer::guarded;
use crate::onnx::{self, live::LiveSession, live::Plan};
use crate::preprocess::{AreaResize, VIEW};
use crate::recordings::Recording;
use vrft_quest_pro_protocol::{STRIP_BYTES, STRIP_WIDTH};

const OUTPUTS: usize = FACE_TARGETS.len();
/// The held tongue poses and the direction each asks for.
pub const TONGUE_HOLDS: [(&str, [f64; 2]); 5] = [
    ("tongue_out", [0.0, 0.0]),
    ("tongue_up", [0.0, 1.0]),
    ("tongue_down", [0.0, -1.0]),
    ("tongue_left", [-1.0, 0.0]),
    ("tongue_right", [1.0, 0.0]),
];
/// Ridge strength per embedding dimension: lambda = 2 * dim.
const RIDGE: f64 = 2.0;
/// A direction whose held pose reads less than this far gets no gain...
const TONGUE_REACH: f64 = 0.15;
/// ...and none gets more than this.
const TONGUE_GAIN: f64 = 2.0;

/// The face setup's frames, by slot name: the six anchor slots, the
/// one-sided puffs and the tongue's held directions.
#[derive(Default)]
pub struct Enrollment {
    pub frames: BTreeMap<String, Vec<Vec<u8>>>,
}

impl Enrollment {
    /// Reads every frame of a face setup recording that shows a slot. Its
    /// frames must hold all five cameras or the mouth pair at the headset's
    /// size; mouth-only frames get blank eye and brow views.
    pub fn from_recording(dir: &Path) -> Result<Self> {
        let recording = Recording::open(dir)?;
        if recording.layout.view != VIEW {
            bail!("{} isn't a headset recording", dir.display());
        }
        let mut indices: Vec<(usize, String)> = recording
            .samples
            .iter()
            .filter_map(|sample| Some((sample.index, sample.anchor.clone()?)))
            .collect();
        indices.sort();
        let mut frames: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
        let mut slots = indices.iter().map(|(_, slot)| slot);
        let layout = recording.layout.clone();
        recording.read_whole(
            &indices.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            |frame| {
                let slot = slots.next().expect("a slot per frame").clone();
                let mut strip = vec![0u8; STRIP_BYTES];
                for (position, &camera) in layout.cameras.iter().enumerate() {
                    for row in 0..VIEW {
                        strip[row * STRIP_WIDTH as usize + camera as usize * VIEW..][..VIEW]
                            .copy_from_slice(
                                &frame[row * layout.width() + position * VIEW..][..VIEW],
                            );
                    }
                }
                frames.entry(slot).or_default().push(strip);
            },
        )?;
        if frames.is_empty() {
            bail!("{} has no face setup poses", dir.display());
        }
        Ok(Self { frames })
    }
}

/// How the wearer's tongue reads each way, from their held poses.
#[derive(Clone, Debug)]
pub struct TongueMap {
    mean: Vec<f64>,
    scale: Vec<f64>,
    /// `[dim][2]`: horizontal then vertical.
    weights: Vec<[f64; 2]>,
    /// Left, right, down, up.
    gains: [f64; 4],
}

impl TongueMap {
    /// Fits the map from each held pose's mouth embeddings; `None` unless
    /// all five poses are there.
    pub fn fit(holds: &BTreeMap<&str, Vec<Vec<f32>>>) -> Option<Self> {
        if !TONGUE_HOLDS
            .iter()
            .all(|(slot, _)| holds.get(slot).is_some_and(|q| !q.is_empty()))
        {
            return None;
        }
        let mut x: Vec<Vec<f64>> = vec![];
        let mut y: Vec<[f64; 2]> = vec![];
        for (slot, direction) in TONGUE_HOLDS {
            for q in &holds[slot] {
                x.push(q.iter().map(|&v| f64::from(v)).collect());
                y.push(direction);
            }
        }
        let (n, dim) = (x.len(), x[0].len());
        let mean: Vec<f64> = (0..dim)
            .map(|d| x.iter().map(|row| row[d]).sum::<f64>() / n as f64)
            .collect();
        let scale: Vec<f64> = (0..dim)
            .map(|d| {
                let variance =
                    x.iter().map(|row| (row[d] - mean[d]).powi(2)).sum::<f64>() / n as f64;
                variance.sqrt() + 1e-3
            })
            .collect();
        let xn: Vec<Vec<f64>> = x
            .iter()
            .map(|row| (0..dim).map(|d| (row[d] - mean[d]) / scale[d]).collect())
            .collect();
        // Dual form: weights = xn^T (xn xn^T + lambda I)^-1 y, an n x n solve.
        let lambda = RIDGE * dim as f64;
        let mut gram = vec![0f64; n * n];
        for i in 0..n {
            for j in 0..n {
                gram[i * n + j] = xn[i].iter().zip(&xn[j]).map(|(a, b)| a * b).sum();
            }
            gram[i * n + i] += lambda;
        }
        let alpha = solve(&gram, n, &y)?;
        let weights: Vec<[f64; 2]> = (0..dim)
            .map(|d| {
                let mut w = [0.0; 2];
                for (row, a) in xn.iter().zip(&alpha) {
                    w[0] += row[d] * a[0];
                    w[1] += row[d] * a[1];
                }
                w
            })
            .collect();
        let mut map = Self {
            mean,
            scale,
            weights,
            gains: [1.0; 4],
        };
        let held = |slot: &str| -> [f64; 2] {
            let mut values: Vec<[f64; 2]> = holds[slot].iter().map(|q| map.raw(q)).collect();
            [0, 1].map(|axis| {
                values.sort_by(|a, b| a[axis].total_cmp(&b[axis]));
                values[values.len() / 2][axis]
            })
        };
        let gain = |reach: f64| {
            if reach >= TONGUE_REACH {
                (1.0 / reach).min(TONGUE_GAIN)
            } else {
                1.0
            }
        };
        map.gains = [
            gain(-held("tongue_left")[0]),
            gain(held("tongue_right")[0]),
            gain(-held("tongue_down")[1]),
            gain(held("tongue_up")[1]),
        ];
        Some(map)
    }

    fn raw(&self, q: &[f32]) -> [f64; 2] {
        let mut out = [0.0; 2];
        for (d, value) in q.iter().enumerate() {
            let x = (f64::from(*value) - self.mean[d]) / self.scale[d];
            out[0] += x * self.weights[d][0];
            out[1] += x * self.weights[d][1];
        }
        out
    }

    /// Horizontal and vertical in -1..1: positive is the wearer's right, up.
    pub fn direction(&self, q: &[f32]) -> [f32; 2] {
        let [h, v] = self.raw(q);
        let [left, right, down, up] = self.gains;
        let h = h * if h > 0.0 { right } else { left };
        let v = v * if v > 0.0 { up } else { down };
        [h.clamp(-1.0, 1.0) as f32, v.clamp(-1.0, 1.0) as f32]
    }
}

/// Solves `a x = b` for symmetric positive definite `a` (n x n) and two
/// right-hand sides, by Cholesky.
fn solve(a: &[f64], n: usize, b: &[[f64; 2]]) -> Option<Vec<[f64; 2]>> {
    let mut l = vec![0f64; n * n];
    for j in 0..n {
        let diagonal = a[j * n + j] - (0..j).map(|k| l[j * n + k].powi(2)).sum::<f64>();
        if diagonal.is_nan() || diagonal <= 0.0 {
            return None;
        }
        l[j * n + j] = diagonal.sqrt();
        for i in j + 1..n {
            let dot: f64 = (0..j).map(|k| l[i * n + k] * l[j * n + k]).sum();
            l[i * n + j] = (a[i * n + j] - dot) / l[j * n + j];
        }
    }
    // Forward then back substitution, for one right-hand side.
    let substitute = |mut x: Vec<f64>| {
        for i in 0..n {
            let dot: f64 = (0..i).map(|k| l[i * n + k] * x[k]).sum();
            x[i] = (x[i] - dot) / l[i * n + i];
        }
        for i in (0..n).rev() {
            let dot: f64 = (i + 1..n).map(|k| l[k * n + i] * x[k]).sum();
            x[i] = (x[i] - dot) / l[i * n + i];
        }
        x
    };
    let [first, second] = [0, 1].map(|axis| substitute(b.iter().map(|row| row[axis]).collect()));
    let x: Vec<[f64; 2]> = first.into_iter().zip(second).map(|(a, b)| [a, b]).collect();
    x.iter().flatten().all(|v| v.is_finite()).then_some(x)
}

/// What the loaded model reports, for the daemon's log and status.
#[derive(Clone, Debug)]
pub struct FaceInfo {
    pub device: String,
    pub image_size: usize,
    pub camera_weight: f32,
    pub threshold: f32,
    pub disabled_targets: Vec<String>,
    /// The anchor slots enrolled; empty without enrollment.
    pub enrolled: Vec<String>,
    /// Whether the wearer's tongue directions were fitted.
    pub tongue_map: bool,
}

/// One frame's outputs in `FACE_TARGETS` order, 0 for those not trained.
#[derive(Clone, Copy, Debug)]
pub struct FacePrediction {
    pub values: [f32; OUTPUTS],
}

/// The wearer's face setup as the model reads it.
struct State {
    /// `[6 * 512]`, a slot's mean mouth embedding where `present`.
    anchors: Vec<f32>,
    present: Vec<f32>,
    /// `[480]`, the neutral frames' mean brow embedding.
    brow_neutral: Vec<f32>,
    brow_present: f32,
    tongue_map: Option<TongueMap>,
}

impl State {
    fn empty() -> Self {
        Self {
            anchors: vec![0.0; ANCHOR_SLOTS.len() * MOUTH_EMBEDDING],
            present: vec![0.0; ANCHOR_SLOTS.len()],
            brow_neutral: vec![0.0; BROW_EMBEDDING],
            brow_present: 0.0,
            tongue_map: None,
        }
    }
}

/// Embeddings, one row per strip.
type Rows = Vec<Vec<f32>>;

/// Runs the network: Burn on a device, or ONNX Runtime.
trait Runner {
    /// Each strip's mouth embedding `q` and brow embedding `w`.
    fn embed(&mut self, strips: &[&[u8]]) -> Result<(Rows, Rows)>;
    /// One strip's activated outputs and its mouth embedding.
    fn predict(&mut self, strip: &[u8], state: &State) -> Result<([f32; OUTPUTS], Vec<f32>)>;
    /// Where it runs now, when that can change (ONNX Runtime's switch to
    /// int8).
    fn device(&self) -> Option<String> {
        None
    }
    fn settled(&self) -> bool {
        true
    }
}

/// `strips` (each 2000 x 400) as `n * 5` views of `size` px in 0..1.
fn views(resize: &AreaResize, strips: &[&[u8]]) -> Vec<f32> {
    let size = resize.size();
    let mut pixels = Vec::with_capacity(strips.len() * CAMERAS * size * size);
    let mut view = vec![0u8; size * size];
    for strip in strips {
        for camera in 0..CAMERAS {
            resize.view_of(strip, STRIP_WIDTH as usize, camera, &mut view);
            pixels.extend(view.iter().map(|&value| value as f32 / 255.0));
        }
    }
    pixels
}

struct BurnRunner<B: Backend> {
    net: FaceNet<B>,
    resize: AreaResize,
    device: B::Device,
}

impl<B: Backend> BurnRunner<B> {
    fn new(checkpoint: &FaceCheckpoint, device: B::Device) -> Result<Self> {
        Ok(Self {
            net: FaceNet::from_weights(checkpoint.weights.clone(), false, &device)?.fold(),
            resize: AreaResize::new(checkpoint.metadata.image_size),
            device,
        })
    }

    fn input(&self, strips: &[&[u8]]) -> Tensor<B, 4> {
        let size = self.resize.size();
        Tensor::from_data(
            TensorData::new(
                views(&self.resize, strips),
                [strips.len(), CAMERAS, size, size],
            ),
            &self.device,
        )
    }
}

impl<B: Backend> Runner for BurnRunner<B> {
    fn embed(&mut self, strips: &[&[u8]]) -> Result<(Rows, Rows)> {
        let (mut mouth, mut brow) = (vec![], vec![]);
        for chunk in strips.chunks(16) {
            let embeddings = self.net.embed(self.input(chunk));
            mouth.extend(to_rows(embeddings.mouth, MOUTH_EMBEDDING)?);
            brow.extend(to_rows(embeddings.brow, BROW_EMBEDDING)?);
        }
        Ok((mouth, brow))
    }

    fn predict(&mut self, strip: &[u8], state: &State) -> Result<([f32; OUTPUTS], Vec<f32>)> {
        let slots = ANCHOR_SLOTS.len();
        let embeddings = self.net.embed(self.input(&[strip]));
        let q = to_rows(embeddings.mouth.clone(), MOUTH_EMBEDDING)?.remove(0);
        let tensor = |values: &[f32], shape: [usize; 2]| {
            Tensor::<B, 2>::from_data(TensorData::new(values.to_vec(), shape), &self.device)
        };
        let raw = self.net.raw(
            &embeddings,
            Tensor::from_data(
                TensorData::new(state.anchors.clone(), [1, slots, MOUTH_EMBEDDING]),
                &self.device,
            ),
            tensor(&state.present, [1, slots]),
            tensor(&state.brow_neutral, [1, BROW_EMBEDDING]),
            tensor(&[state.brow_present], [1, 1]),
        );
        let values: [f32; OUTPUTS] = activate(raw)
            .into_data()
            .to_vec::<f32>()
            .map_err(|error| anyhow!("{error:?}"))?
            .try_into()
            .map_err(|_| anyhow!("face model returned the wrong number of values"))?;
        Ok((values, q))
    }
}

struct OnnxRunner {
    session: LiveSession,
    resize: AreaResize,
}

impl OnnxRunner {
    fn run(&mut self, strip: &[u8], state: &State, outputs: &[&str]) -> Result<Vec<Vec<f32>>> {
        let size = self.resize.size();
        let slots = ANCHOR_SLOTS.len();
        self.session.run(
            &[
                (
                    "views",
                    &[1, CAMERAS, size, size],
                    &views(&self.resize, &[strip]),
                ),
                ("anchors", &[1, slots, MOUTH_EMBEDDING], &state.anchors),
                ("present", &[1, slots], &state.present),
                ("brow_neutral", &[1, BROW_EMBEDDING], &state.brow_neutral),
                ("brow_present", &[1, 1], &[state.brow_present]),
            ],
            outputs,
        )
    }
}

impl Runner for OnnxRunner {
    fn embed(&mut self, strips: &[&[u8]]) -> Result<(Rows, Rows)> {
        let empty = State::empty();
        let (mut mouth, mut brow) = (vec![], vec![]);
        for strip in strips {
            let mut out = self.run(strip, &empty, &["q", "w"])?;
            brow.push(out.pop().unwrap_or_default());
            mouth.push(out.pop().unwrap_or_default());
        }
        Ok((mouth, brow))
    }

    fn predict(&mut self, strip: &[u8], state: &State) -> Result<([f32; OUTPUTS], Vec<f32>)> {
        let mut out = self.run(strip, state, &["values", "q"])?;
        let q = out.pop().unwrap_or_default();
        let values = out
            .pop()
            .unwrap_or_default()
            .try_into()
            .map_err(|_| anyhow!("face model returned the wrong number of values"))?;
        Ok((values, q))
    }

    fn device(&self) -> Option<String> {
        Some(self.session.device())
    }

    fn settled(&self) -> bool {
        self.session.settled()
    }
}

struct Engine {
    runner: Box<dyn Runner + Send>,
    state: State,
}

impl Engine {
    fn enroll(&mut self, enrollment: &Enrollment) -> Result<(Vec<String>, bool)> {
        let mut state = State::empty();
        let mut holds: BTreeMap<&str, Vec<Vec<f32>>> = BTreeMap::new();
        let mut enrolled = vec![];
        for (slot, strips) in &enrollment.frames {
            let strips: Vec<&[u8]> = strips.iter().map(Vec::as_slice).collect();
            let (mouth, brow) = self.runner.embed(&strips)?;
            if let Some(index) = ANCHOR_SLOTS.iter().position(|name| name == slot) {
                let mean = mean_rows(&mouth);
                state.anchors[index * MOUTH_EMBEDDING..][..MOUTH_EMBEDDING].copy_from_slice(&mean);
                state.present[index] = 1.0;
                enrolled.push(slot.clone());
                if index == 0 {
                    state.brow_neutral = mean_rows(&brow);
                    state.brow_present = 1.0;
                }
            }
            if let Some((name, _)) = TONGUE_HOLDS.iter().find(|(name, _)| name == slot) {
                holds.insert(name, mouth);
            }
        }
        state.tongue_map = TongueMap::fit(&holds);
        let fitted = state.tongue_map.is_some();
        self.state = state;
        Ok((enrolled, fitted))
    }

    fn predict(&mut self, strip: &[u8]) -> Result<[f32; OUTPUTS]> {
        let (mut values, q) = self.runner.predict(strip, &self.state)?;
        if let Some(map) = &self.state.tongue_map {
            let [horizontal, vertical] = map.direction(&q);
            values[2] = horizontal;
            values[3] = vertical;
        }
        Ok(values)
    }
}

fn to_rows<B: Backend>(tensor: Tensor<B, 2>, width: usize) -> Result<Vec<Vec<f32>>> {
    Ok(tensor
        .into_data()
        .to_vec::<f32>()
        .map_err(|error| anyhow!("{error:?}"))?
        .chunks(width)
        .map(<[f32]>::to_vec)
        .collect())
}

fn mean_rows(rows: &[Vec<f32>]) -> Vec<f32> {
    let mut mean = vec![0f32; rows[0].len()];
    for row in rows {
        for (total, value) in mean.iter_mut().zip(row) {
            *total += value / rows.len() as f32;
        }
    }
    mean
}

pub struct FaceModel {
    engine: Engine,
    info: FaceInfo,
    enabled: [bool; OUTPUTS],
}

impl FaceModel {
    /// Loads a checkpoint and proves it runs on the chosen device.
    ///
    /// ONNX Runtime runs it when its library is there (see
    /// [`onnx::runtime`]), unless `VRFT_INFERENCE=burn`: on the CPU for
    /// `Auto` and `Cpu`, through DirectML for `Gpu`. Otherwise Burn runs it,
    /// where `Auto` uses the GPU when one works and falls back to the CPU.
    pub fn load(path: &Path, accelerator: Accelerator) -> Result<Self> {
        let checkpoint = FaceCheckpoint::load(path)?;
        let blank = vec![0u8; STRIP_BYTES];
        let prove = |mut engine: Engine| -> Result<Engine> {
            engine.predict(&blank)?;
            Ok(engine)
        };
        let mut device = String::new();
        let mut engine = None;
        if onnx::runtime::wanted() {
            let started = guarded(|| {
                let graph = onnx::face_graph(&checkpoint.weights, checkpoint.metadata.image_size)?;
                let placement = onnx::runtime::placement(accelerator);
                let plan = Plan::Int8(&[onnx::TONGUE_SCOPE]);
                let session = LiveSession::new(graph, path, &checkpoint.weights, placement, plan)?;
                let runner = OnnxRunner {
                    session,
                    resize: AreaResize::new(checkpoint.metadata.image_size),
                };
                prove(Engine {
                    runner: Box::new(runner),
                    state: State::empty(),
                })
            });
            match started {
                Ok(started) => {
                    device = started.runner.device().unwrap_or_default();
                    engine = Some(started);
                }
                Err(error) => warn!("Face model: ONNX Runtime unavailable ({error:#}); using Burn"),
            }
        }
        if engine.is_none() && accelerator != Accelerator::Cpu {
            let started = guarded(|| {
                prove(Engine {
                    runner: Box::new(BurnRunner::<Gpu>::new(&checkpoint, Default::default())?),
                    state: State::empty(),
                })
            });
            match started {
                Ok(started) => {
                    device = GPU_NAME.into();
                    engine = Some(started);
                }
                Err(error) if accelerator == Accelerator::Auto => {
                    warn!("Face model: GPU unavailable ({error:#}); using the CPU");
                }
                Err(error) => return Err(error.context("the GPU could not run the face model")),
            }
        }
        let engine = match engine {
            Some(engine) => engine,
            None => {
                device = "CPU".into();
                Engine {
                    runner: Box::new(BurnRunner::<Cpu>::new(&checkpoint, Default::default())?),
                    state: State::empty(),
                }
            }
        };
        let metadata = &checkpoint.metadata;
        let info = FaceInfo {
            device,
            image_size: metadata.image_size,
            camera_weight: metadata.visibility_gate.camera_weight as f32,
            threshold: metadata.visibility_gate.threshold as f32,
            disabled_targets: metadata.disabled_targets.clone(),
            enrolled: vec![],
            tongue_map: false,
        };
        Ok(Self {
            engine,
            enabled: FACE_TARGETS.map(|name| metadata.enabled(name)),
            info,
        })
    }

    pub fn info(&self) -> &FaceInfo {
        &self.info
    }

    /// Whether the model has stopped changing how it runs (ONNX Runtime's
    /// calibration and switch to int8 are done).
    pub fn settled(&self) -> bool {
        self.engine.runner.settled()
    }

    /// Whether output `index` of [`FACE_TARGETS`] was trained.
    pub fn enabled(&self, index: usize) -> bool {
        self.enabled[index]
    }

    /// Encodes the wearer's face setup; until then, or without one, the
    /// model reads every anchor as missing.
    pub fn enroll(&mut self, enrollment: &Enrollment) -> Result<()> {
        let (enrolled, tongue_map) = guarded(|| self.engine.enroll(enrollment))?;
        self.info.enrolled = enrolled;
        self.info.tongue_map = tongue_map;
        Ok(())
    }

    /// One five-camera strip (2000 x 400 gray8) in; smoothing is the
    /// caller's job.
    pub fn predict(&mut self, strip: &[u8]) -> Result<FacePrediction> {
        if strip.len() != STRIP_BYTES {
            bail!("face frames must be 2000x400 gray8");
        }
        let mut values = guarded(|| self.engine.predict(strip))?;
        if let Some(device) = self.engine.runner.device() {
            self.info.device = device;
        }
        for (value, enabled) in values.iter_mut().zip(self.enabled) {
            if !enabled {
                *value = 0.0;
            }
        }
        if values.iter().any(|value| !value.is_finite()) {
            bail!("face model returned non-finite values");
        }
        Ok(FacePrediction { values })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tongue_map_reads_held_poses_back() {
        // Embeddings where two dimensions carry the direction, with noise.
        let mut holds: BTreeMap<&str, Vec<Vec<f32>>> = BTreeMap::new();
        let mut seed = 7u64;
        let mut noise = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f32 / u32::MAX as f32 - 0.25) * 0.02
        };
        for (slot, [h, v]) in TONGUE_HOLDS {
            let frames = (0..6)
                .map(|_| {
                    let mut q: Vec<f32> = (0..32).map(|_| noise()).collect();
                    q[0] += h as f32 * 0.4;
                    q[1] += v as f32 * 0.4;
                    q
                })
                .collect();
            holds.insert(slot, frames);
        }
        let map = TongueMap::fit(&holds).unwrap();
        for (slot, [h, v]) in TONGUE_HOLDS {
            let [rh, rv] = map.direction(&holds[slot][0]);
            assert!((f64::from(rh) - h).abs() < 0.35, "{slot}: {rh} vs {h}");
            assert!((f64::from(rv) - v).abs() < 0.35, "{slot}: {rv} vs {v}");
        }
        assert!(map
            .gains
            .iter()
            .all(|gain| (1.0..=TONGUE_GAIN).contains(gain)));
        holds.remove("tongue_up");
        assert!(TongueMap::fit(&holds).is_none(), "every pose is needed");
    }

    #[test]
    fn cholesky_solves_a_small_system() {
        let a = [4.0, 1.0, 1.0, 3.0];
        let x = solve(&a, 2, &[[1.0, 0.0], [2.0, 1.0]]).unwrap();
        assert!((4.0 * x[0][0] + x[1][0] - 1.0).abs() < 1e-12);
        assert!((x[0][0] + 3.0 * x[1][0] - 2.0).abs() < 1e-12);
        assert!(solve(&[0.0], 1, &[[1.0, 1.0]]).is_none());
    }
}
