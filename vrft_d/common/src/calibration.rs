use serde::{Deserialize, Serialize};
use crate::UnifiedExpressions;

pub const POINTS: usize = 64;
pub const S_DELTA: f32 = 0.15;

fn default_points_array() -> [f32; POINTS] {
    [0.0; POINTS]
}

#[derive(Debug, Clone, PartialEq)]
pub enum CalibrationState {
    Uncalibrated,
    Collecting { timer: f32, duration: f32 },
    Calibrated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationParameter {
    pub name: String,
    #[serde(skip)]
    pub rolling_index: usize,
    #[serde(skip)]
    pub fixed_index: usize,
    #[serde(skip)]
    pub finished: bool,
    #[serde(skip, default = "default_points_array")]
    pub data_points: [f32; POINTS],
    pub progress: f32,
    #[serde(skip)]
    pub current_step: f32,
    pub max: f32,
}

impl Default for CalibrationParameter {
    fn default() -> Self {
        Self {
            name: String::new(),
            rolling_index: 0,
            fixed_index: 0,
            finished: false,
            data_points: [0.0; POINTS],
            progress: 0.0,
            current_step: f32::NAN,
            max: 0.0,
        }
    }
}

impl CalibrationParameter {
    pub fn update_calibration(&mut self, current_value: f32, continuous: bool, d_t: f32) {
        let difference = (current_value - self.current_step).abs();
        if self.current_step.is_nan() || difference >= S_DELTA * d_t {
            if self.fixed_index < self.data_points.len() {
                self.fixed_index += 1;
                self.progress = self.fixed_index as f32 / self.data_points.len() as f32;
            } else if !self.finished {
                self.finished = true;
            }

            self.data_points[self.rolling_index] = current_value;
            if !self.finished || (self.finished && continuous) {
                self.rolling_index = (self.rolling_index + 1) % self.data_points.len();
                self.calculate_stats();
            }
        }
        self.current_step = self.clamp_step(current_value, S_DELTA * d_t);
    }

    fn clamp_step(&self, value: f32, factor: f32) -> f32 {
        (value / factor).floor() * factor
    }

    pub fn calculate_stats(&mut self) {
        if self.fixed_index as f32 >= 0.1 * self.data_points.len() as f32 {
            let mut current_max = 0.0f32;
            for &p in &self.data_points {
                if p > current_max {
                    current_max = p;
                }
            }

            if current_max > self.max {
                self.max = current_max;
            }
        }
    }

    fn normalize(&self, current_value: f32) -> f32 {
        if self.max == 0.0 {
            return current_value;
        }
        current_value / self.max
    }

    pub fn calculate_parameter(&self, current_value: f32, k: f32) -> f32 {
        if current_value.is_nan() {
            return current_value;
        }

        let confidence = k * self.progress;
        let adjusted_value = confidence * self.normalize(current_value) + (1.0 - confidence) * current_value;

        if adjusted_value.is_nan() {
            return current_value;
        }

        adjusted_value
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationData {
    pub shapes: Vec<CalibrationParameter>,
}

impl Default for CalibrationData {
    fn default() -> Self {
        let mut shapes = Vec::with_capacity(UnifiedExpressions::Max as usize);
        for i in 0..UnifiedExpressions::Max as usize {
            let expr: UnifiedExpressions = unsafe { std::mem::transmute(i) };
            shapes.push(CalibrationParameter {
                name: format!("{:?}", expr),
                max: 0.0,
                ..Default::default()
            });
        }
        Self { shapes }
    }
}

impl CalibrationData {
    pub fn clear(&mut self) {
        for i in 0..self.shapes.len() {
            let expr: UnifiedExpressions = unsafe { std::mem::transmute(i) };
            self.shapes[i] = CalibrationParameter {
                name: format!("{:?}", expr),
                max: 0.0,
                ..Default::default()
            };
        }
    }
}
