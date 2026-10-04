//! A model's ONNX session as the daemon runs it: on the CPU it starts on the
//! float graph, reads the range of every quantized tensor over the first
//! [`CALIBRATION_FRAMES`] frames it is given, then builds the int8 graph in
//! the background and switches to it. The int8 graph is saved beside the
//! checkpoint, so later loads start on it. On the GPU the float graph stays.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use anyhow::Result;
use log::{info, warn};

use super::proto::Graph;
use super::quantize::{self, Ranges, CALIBRATION_FRAMES};
use super::runtime::Session;
use crate::backend::Accelerator;
use crate::model::Weights;

/// How much of a model runs in int8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// All float.
    Float,
    /// int8, except the values whose names start with one of these.
    Int8(&'static [&'static str]),
}

impl Plan {
    /// `default`, unless `VRFT_ONNX_INT8` says otherwise: `0` (or `off`)
    /// keeps every model float, `all` quantizes every model whole.
    pub fn with_override(default: Plan) -> Plan {
        match std::env::var("VRFT_ONNX_INT8").as_deref() {
            Ok("0" | "off") => Plan::Float,
            Ok("all") => Plan::Int8(&[]),
            _ => default,
        }
    }

    fn float(self) -> &'static [&'static str] {
        match self {
            Plan::Float => &[],
            Plan::Int8(float) => float,
        }
    }
}

struct Calibrating {
    graph: Graph,
    tensors: Vec<String>,
    ranges: Ranges,
    frames: usize,
    float: &'static [&'static str],
}

pub struct LiveSession {
    session: Session,
    calibrating: Option<Calibrating>,
    pending: Option<Receiver<Result<Session>>>,
    checkpoint: PathBuf,
    weights: Weights,
    /// "int8" or "fp32".
    pub precision: &'static str,
}

impl LiveSession {
    /// `graph` is the float graph of the weights in `checkpoint`; `plan`
    /// says how much of it may run in int8 on the CPU.
    pub fn new(
        graph: Graph,
        checkpoint: &Path,
        weights: &Weights,
        accelerator: Accelerator,
        plan: Plan,
    ) -> Result<Self> {
        let plan = Plan::with_override(plan);
        let saved = match plan {
            Plan::Float => None,
            Plan::Int8(float) => quantize::saved(checkpoint, weights, float)?,
        };
        if let Some(model) = saved {
            match Session::new(&model, accelerator) {
                Ok(session) => {
                    return Ok(Self {
                        session,
                        calibrating: None,
                        pending: None,
                        checkpoint: checkpoint.into(),
                        weights: Weights::new(),
                        precision: "int8",
                    })
                }
                Err(error) => warn!(
                    "{}: the saved int8 model failed ({error:#})",
                    checkpoint.display()
                ),
            }
        }
        // Only the CPU calibrates: the GPU keeps the float graph, without
        // the calibration's extra outputs.
        let on_cpu = accelerator == Accelerator::Cpu && plan != Plan::Float;
        let tensors = quantize::tensors(&graph, plan.float());
        let model = if on_cpu {
            quantize::calibration_graph(&graph, &tensors).to_model()
        } else {
            graph.to_model()
        };
        let session = Session::new(&model, accelerator)?;
        Ok(Self {
            session,
            calibrating: on_cpu.then(|| Calibrating {
                graph,
                tensors,
                ranges: Ranges::default(),
                frames: 0,
                float: plan.float(),
            }),
            pending: None,
            checkpoint: checkpoint.into(),
            weights: if on_cpu {
                weights.clone()
            } else {
                Weights::new()
            },
            precision: "fp32",
        })
    }

    /// Whether it has stopped changing: no calibration or int8 build left.
    pub fn settled(&self) -> bool {
        self.calibrating.is_none() && self.pending.is_none()
    }

    pub fn device(&self) -> String {
        format!("{}, {}", self.session.device, self.precision)
    }

    /// Runs one frame, as [`Session::run`] does, calibrating and switching
    /// to int8 along the way.
    pub fn run(
        &mut self,
        inputs: &[(&str, &[usize], &[f32])],
        outputs: &[&str],
    ) -> Result<Vec<Vec<f32>>> {
        self.switch();
        let Some(calibrating) = &mut self.calibrating else {
            return self.session.run(inputs, outputs);
        };
        let mut wanted: Vec<&str> = outputs.to_vec();
        wanted.extend(calibrating.tensors.iter().map(String::as_str));
        let mut values = self.session.run(inputs, &wanted)?;
        for (name, values) in calibrating.tensors.iter().zip(&values[outputs.len()..]) {
            calibrating.ranges.observe(name, values);
        }
        // The views input isn't an output; its range is the input's.
        if let Some((_, _, views)) = inputs.iter().find(|(name, ..)| *name == "views") {
            calibrating.ranges.observe("views", views);
        }
        calibrating.frames += 1;
        if calibrating.frames >= CALIBRATION_FRAMES {
            self.quantize_in_background();
        }
        values.truncate(outputs.len());
        Ok(values)
    }

    fn quantize_in_background(&mut self) {
        let Some(calibrating) = self.calibrating.take() else {
            return;
        };
        let (send, receive) = mpsc::channel();
        let checkpoint = self.checkpoint.clone();
        let weights = std::mem::take(&mut self.weights);
        std::thread::Builder::new()
            .name("onnx-quantize".into())
            .spawn(move || {
                let built = (|| {
                    let graph = quantize::quantize(
                        &calibrating.graph,
                        &calibrating.ranges,
                        calibrating.float,
                    )?;
                    let model = graph.to_model();
                    let session = Session::new(&model, Accelerator::Cpu)?;
                    let saved = quantize::save(
                        &checkpoint,
                        &weights,
                        calibrating.float,
                        &model,
                        calibrating.frames,
                    );
                    if let Err(error) = saved {
                        warn!("{error:#}");
                    }
                    Ok(session)
                })();
                let _ = send.send(built);
            })
            .ok();
        self.pending = Some(receive);
    }

    fn switch(&mut self) {
        let Some(pending) = &self.pending else {
            return;
        };
        match pending.try_recv() {
            Ok(Ok(session)) => {
                info!("{}: now running int8", self.checkpoint.display());
                self.session = session;
                self.precision = "int8";
                self.pending = None;
            }
            Ok(Err(error)) => {
                warn!(
                    "{}: int8 quantization failed ({error:#}); staying on fp32",
                    self.checkpoint.display()
                );
                self.pending = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.pending = None,
        }
    }
}
