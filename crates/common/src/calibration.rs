use crate::UnifiedExpressions;
use serde::{Deserialize, Serialize};

// Constants

/// Rolling data window size for calibration samples.
pub const POINTS: usize = 64;

/// Step delta: minimum change required to accept a new sample (noise filter).
pub const S_DELTA: f32 = 0.15;

/// Confidence delta: minimum confidence improvement to update statistics.
pub const C_DELTA: f32 = 0.1;

/// Expected mean-to-stdDev ratio for typical bounded expression data.
/// CV ≈ 0.58 is common for 0-1 normalized expression values.
pub const EXPECTED_CV_RATIO: f32 = 1.732; // ≈ √3

/// StdDev of uniform distribution on [0,1]: 1/√12 ≈ 0.2887
pub const MAX_REASONABLE_STDDEV: f32 = 0.2887;

/// Reciprocal of MAX_REASONABLE_STDDEV: √12 ≈ 3.464
pub const STDDEV_QUALITY_FACTOR: f32 = 3.464;

/// Sigmoid midpoint: below this value, prefer raw data.
pub const SIGMOID_MIDPOINT: f32 = 0.05;

/// Sigmoid steepness for blend transition.
pub const SIGMOID_STEEPNESS: f32 = 40.0;

fn default_points_array() -> [f32; POINTS] {
    [0.0; POINTS]
}

// CalibrationState

#[derive(Debug, Clone, PartialEq)]
pub enum CalibrationState {
    Uncalibrated,
    Collecting { timer: f32, duration: f32 },
    Calibrated,
}

// CalibrationParameter

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

    /// Maximum observed value (retained for backwards compatibility).
    pub max: f32,

    /// Running mean of observed values.
    pub mean: f32,

    /// Running standard deviation.
    pub std_dev: f32,

    /// Multi-factor confidence score (0-1).
    pub confidence: f32,

    /// Peak confidence achieved (prevents regression).
    pub max_confidence: f32,
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
            mean: 0.0,
            std_dev: 0.0,
            confidence: 0.0,
            max_confidence: 0.0,
        }
    }
}

impl CalibrationParameter {
    /// Update calibration with a new sample value.
    pub fn update_calibration(&mut self, current_value: f32, continuous: bool, d_t: f32) {
        let difference = (current_value - self.current_step).abs();

        // Accept sample only if it differs significantly (noise filter)
        if self.current_step.is_nan() || difference >= S_DELTA * d_t {
            if self.fixed_index < self.data_points.len() {
                self.fixed_index += 1;
                self.progress = self.fixed_index as f32 / self.data_points.len() as f32;
            } else if !self.finished {
                self.finished = true;
            }

            self.data_points[self.rolling_index] = current_value;

            if !self.finished || continuous {
                self.rolling_index = (self.rolling_index + 1) % self.data_points.len();
                self.calculate_stats();
            }
        }

        self.current_step = self.clamp_step(current_value, S_DELTA * d_t);
    }

    fn clamp_step(&self, value: f32, factor: f32) -> f32 {
        (value / factor).floor() * factor
    }

    /// Calculate statistical properties from data buffer.
    pub fn calculate_stats(&mut self) {
        let min_samples = (0.1 * self.data_points.len() as f32) as usize;
        if self.fixed_index < min_samples {
            return;
        }

        let sample_count = self.fixed_index.min(POINTS);
        let valid_data: Vec<f32> = self.data_points[..sample_count]
            .iter()
            .copied()
            .filter(|v| !v.is_nan())
            .collect();

        if valid_data.is_empty() {
            return;
        }

        // Calculate mean
        let new_mean = valid_data.iter().sum::<f32>() / valid_data.len() as f32;

        // Calculate stdDev
        let variance_sum: f32 = valid_data.iter().map(|v| (v - new_mean).powi(2)).sum();
        let new_std_dev = (variance_sum / valid_data.len() as f32).sqrt();

        // Multi-factor confidence calculation
        let distribution_penalty =
            (1.0 - (EXPECTED_CV_RATIO * new_std_dev - new_mean).abs()).max(0.0);
        let std_dev_limit = (1.0 - (new_std_dev - MAX_REASONABLE_STDDEV).max(0.0)).powi(3);
        let mean_limit = 1.0 - (new_mean - 0.5).max(0.0);
        let mean_pusher = (2.0 * new_mean).powf(0.2);

        let new_confidence =
            (distribution_penalty * std_dev_limit * mean_limit * mean_pusher).clamp(0.0, 1.0);

        // Only update if confidence improved (prevents regression)
        if new_confidence >= self.max_confidence - C_DELTA {
            // Weight new stats less the more confident we are
            let lerp = 1.0 - self.confidence.powi(2);

            if !new_mean.is_nan() {
                self.mean = new_mean * lerp + self.mean * (1.0 - lerp);
            }
            if !new_std_dev.is_nan() {
                self.std_dev = new_std_dev * lerp + self.std_dev * (1.0 - lerp);
            }
            if !new_confidence.is_nan() {
                self.confidence = new_confidence * lerp + self.confidence * (1.0 - lerp);
            }
            if new_confidence > self.max_confidence {
                self.max_confidence = self.max_confidence * lerp + new_confidence * (1.0 - lerp);
            }
        }

        // Track max for backwards compatibility
        if let Some(&max_val) = valid_data
            .iter()
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        {
            if max_val > self.max {
                self.max = max_val;
            }
        }
    }

    /// Curve adjustment formula: 2v/(1+v) - expands lower range more aggressively.
    fn curve_adjusted_range(&self, value: f32) -> f32 {
        if value <= 0.0 {
            return 1.0; // No adjustment when value is zero or negative
        }
        2.0 * value / (1.0 + value)
    }

    /// Calculate adjusted parameter value using learned calibration.
    pub fn calculate_parameter(&self, current_value: f32, k: f32) -> f32 {
        if current_value.is_nan() {
            return current_value;
        }

        // Skip calibration if no data collected yet
        if self.confidence == 0.0 && self.max == 0.0 {
            return current_value;
        }

        // Calculate adjusted max based on learned statistics
        let adjusted_max = self.mean + k * self.std_dev;

        // Apply power curve for range expansion
        let curve_exponent = self.curve_adjusted_range(adjusted_max);
        let curved_value = current_value.powf(curve_exponent);

        // Sigmoid blend (prefer raw values near zero)
        let sigmoid = 1.0 / (1.0 + (-SIGMOID_STEEPNESS * (current_value - SIGMOID_MIDPOINT)).exp());

        // StdDev quality factor
        let quality = (STDDEV_QUALITY_FACTOR * self.std_dev - 1.0)
            .abs()
            .powf(0.2)
            .max(0.0);

        let blend = self.confidence * sigmoid * quality;
        let adjusted_value = blend * curved_value.clamp(0.0, 1.0) + (1.0 - blend) * current_value;

        if adjusted_value.is_nan() {
            current_value
        } else {
            adjusted_value
        }
    }
}

// CalibrationData

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationData {
    pub shapes: Vec<CalibrationParameter>,
}

impl Default for CalibrationData {
    fn default() -> Self {
        let mut shapes = Vec::with_capacity(UnifiedExpressions::Max as usize);
        for i in 0..UnifiedExpressions::Max as usize {
            let expr = UnifiedExpressions::try_from(i).expect("index within valid enum range");
            shapes.push(CalibrationParameter {
                name: format!("{:?}", expr),
                ..Default::default()
            });
        }
        Self { shapes }
    }
}

impl CalibrationData {
    pub fn clear(&mut self) {
        for i in 0..self.shapes.len() {
            let expr = UnifiedExpressions::try_from(i).expect("index within valid enum range");
            self.shapes[i] = CalibrationParameter {
                name: format!("{:?}", expr),
                ..Default::default()
            };
        }
    }
}
