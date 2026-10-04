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

struct Engine<B: Backend> {
    net: FaceNet<B>,
    resize: AreaResize,
    anchors: Tensor<B, 3>,
    present: Tensor<B, 2>,
    brow_neutral: Tensor<B, 2>,
    brow_present: Tensor<B, 2>,
    tongue_map: Option<TongueMap>,
    device: B::Device,
}

impl<B: Backend> Engine<B> {
    fn new(checkpoint: &FaceCheckpoint, device: B::Device) -> Result<Self> {
        let net = FaceNet::from_weights(checkpoint.weights.clone(), false, &device)?.fold();
        let slots = ANCHOR_SLOTS.len();
        Ok(Self {
            net,
            resize: AreaResize::new(checkpoint.metadata.image_size),
            anchors: Tensor::zeros([1, slots, MOUTH_EMBEDDING], &device),
            present: Tensor::zeros([1, slots], &device),
            brow_neutral: Tensor::zeros([1, BROW_EMBEDDING], &device),
            brow_present: Tensor::zeros([1, 1], &device),
            tongue_map: None,
            device,
        })
    }

    /// `strips` (each 2000 x 400) as `[n, 5, size, size]` in 0..1.
    fn views(&self, strips: &[&[u8]]) -> Tensor<B, 4> {
        let size = self.resize.size();
        let mut pixels = Vec::with_capacity(strips.len() * CAMERAS * size * size);
        let mut view = vec![0u8; size * size];
        for strip in strips {
            for camera in 0..CAMERAS {
                self.resize
                    .view_of(strip, STRIP_WIDTH as usize, camera, &mut view);
                pixels.extend(view.iter().map(|&value| value as f32 / 255.0));
            }
        }
        Tensor::from_data(
            TensorData::new(pixels, [strips.len(), CAMERAS, size, size]),
            &self.device,
        )
    }

    fn enroll(&mut self, enrollment: &Enrollment) -> Result<(Vec<String>, bool)> {
        let mut anchors = vec![0f32; ANCHOR_SLOTS.len() * MOUTH_EMBEDDING];
        let mut present = vec![0f32; ANCHOR_SLOTS.len()];
        let mut holds: BTreeMap<&str, Vec<Vec<f32>>> = BTreeMap::new();
        let mut enrolled = vec![];
        for (slot, strips) in &enrollment.frames {
            let strips: Vec<&[u8]> = strips.iter().map(Vec::as_slice).collect();
            let mut mouth = vec![];
            let mut brow = vec![];
            for chunk in strips.chunks(16) {
                let embeddings = self.net.embed(self.views(chunk));
                mouth.extend(to_rows(embeddings.mouth, MOUTH_EMBEDDING)?);
                brow.extend(to_rows(embeddings.brow, BROW_EMBEDDING)?);
            }
            if let Some(index) = ANCHOR_SLOTS.iter().position(|name| name == slot) {
                let mean = mean_rows(&mouth);
                anchors[index * MOUTH_EMBEDDING..][..MOUTH_EMBEDDING].copy_from_slice(&mean);
                present[index] = 1.0;
                enrolled.push(slot.clone());
                if index == 0 {
                    self.brow_neutral = Tensor::from_data(
                        TensorData::new(mean_rows(&brow), [1, BROW_EMBEDDING]),
                        &self.device,
                    );
                    self.brow_present = Tensor::ones([1, 1], &self.device);
                }
            }
            if let Some((name, _)) = TONGUE_HOLDS.iter().find(|(name, _)| name == slot) {
                holds.insert(name, mouth);
            }
        }
        self.anchors = Tensor::from_data(
            TensorData::new(anchors, [1, ANCHOR_SLOTS.len(), MOUTH_EMBEDDING]),
            &self.device,
        );
        self.present = Tensor::from_data(
            TensorData::new(present, [1, ANCHOR_SLOTS.len()]),
            &self.device,
        );
        self.tongue_map = TongueMap::fit(&holds);
        Ok((enrolled, self.tongue_map.is_some()))
    }

    fn predict(&self, strip: &[u8]) -> Result<[f32; OUTPUTS]> {
        let embeddings = self.net.embed(self.views(&[strip]));
        let q = to_rows(embeddings.mouth.clone(), MOUTH_EMBEDDING)?.remove(0);
        let raw = self.net.raw(
            &embeddings,
            self.anchors.clone(),
            self.present.clone(),
            self.brow_neutral.clone(),
            self.brow_present.clone(),
        );
        let mut values: [f32; OUTPUTS] = activate(raw)
            .into_data()
            .to_vec::<f32>()
            .map_err(|error| anyhow!("{error:?}"))?
            .try_into()
            .map_err(|_| anyhow!("face model returned the wrong number of values"))?;
        if let Some(map) = &self.tongue_map {
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

enum Engines {
    Gpu(Box<Engine<Gpu>>),
    Cpu(Box<Engine<Cpu>>),
}

pub struct FaceModel {
    engine: Engines,
    info: FaceInfo,
    enabled: [bool; OUTPUTS],
}

impl FaceModel {
    /// Loads a checkpoint and proves it runs on the chosen device. `Auto`
    /// uses the GPU when one works and falls back to the CPU.
    pub fn load(path: &Path, accelerator: Accelerator) -> Result<Self> {
        let checkpoint = FaceCheckpoint::load(path)?;
        let blank = vec![0u8; STRIP_BYTES];
        let engine = match accelerator {
            Accelerator::Cpu => None,
            Accelerator::Auto | Accelerator::Gpu => {
                let started = guarded(|| {
                    let engine = Engine::<Gpu>::new(&checkpoint, Default::default())?;
                    engine.predict(&blank)?;
                    Ok(engine)
                });
                match started {
                    Ok(engine) => Some(Engines::Gpu(Box::new(engine))),
                    Err(error) if accelerator == Accelerator::Auto => {
                        warn!("Face model: GPU unavailable ({error:#}); using the CPU");
                        None
                    }
                    Err(error) => return Err(error.context("the GPU could not run the face model")),
                }
            }
        };
        let engine = match engine {
            Some(engine) => engine,
            None => Engines::Cpu(Box::new(Engine::new(&checkpoint, Default::default())?)),
        };
        let metadata = &checkpoint.metadata;
        let info = FaceInfo {
            device: match engine {
                Engines::Gpu(_) => GPU_NAME.into(),
                Engines::Cpu(_) => "CPU".into(),
            },
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

    /// Whether output `index` of [`FACE_TARGETS`] was trained.
    pub fn enabled(&self, index: usize) -> bool {
        self.enabled[index]
    }

    /// Encodes the wearer's face setup; until then, or without one, the
    /// model reads every anchor as missing.
    pub fn enroll(&mut self, enrollment: &Enrollment) -> Result<()> {
        let (enrolled, tongue_map) = guarded(|| match &mut self.engine {
            Engines::Gpu(engine) => engine.enroll(enrollment),
            Engines::Cpu(engine) => engine.enroll(enrollment),
        })?;
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
        let mut values = guarded(|| match &self.engine {
            Engines::Gpu(engine) => engine.predict(strip),
            Engines::Cpu(engine) => engine.predict(strip),
        })?;
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
