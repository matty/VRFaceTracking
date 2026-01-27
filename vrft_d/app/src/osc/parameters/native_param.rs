//! Native tracking parameters that bypass VRChat's parameter system.
//! These send directly to /tracking/ endpoints and are only relevant when
//! the avatar doesn't already have equivalent parameters.

use super::{ParamType, Parameter};
use common::UnifiedTrackingData;
use rosc::{OscMessage, OscType};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Type alias for value getter function to reduce type complexity
type ValueGetterFn = Arc<dyn Fn(&UnifiedTrackingData) -> Vec<f32> + Send + Sync>;

/// Type alias for condition function to reduce type complexity
type ConditionFn = Arc<dyn Fn(&HashSet<String>) -> bool + Send + Sync>;

/// Native parameter with conditional relevancy.
/// Only activates when the condition function returns true (e.g., when avatar lacks eye params).
pub struct NativeParameter {
    address: String,
    get_value: ValueGetterFn,
    /// Returns true if this native param should be relevant given the avatar's params
    condition: ConditionFn,
    relevant: bool,
    last_values: Option<Vec<f32>>,
}

impl NativeParameter {
    /// Create a native float parameter
    pub fn new_float(
        address: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> f32 + Send + Sync + 'static,
        condition: impl Fn(&HashSet<String>) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            address: address.to_string(),
            get_value: Arc::new(move |d| vec![get_value(d)]),
            condition: Arc::new(condition),
            relevant: false,
            last_values: None,
        }
    }

    /// Create a native Vector4 parameter (sends as 4 floats)
    pub fn new_vector4(
        address: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> [f32; 4] + Send + Sync + 'static,
        condition: impl Fn(&HashSet<String>) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            address: address.to_string(),
            get_value: Arc::new(move |d| get_value(d).to_vec()),
            condition: Arc::new(condition),
            relevant: false,
            last_values: None,
        }
    }
}

impl Parameter for NativeParameter {
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        _param_types: &HashMap<String, ParamType>,
    ) -> usize {
        self.last_values = None;

        // Check if condition is met (e.g., avatar lacks eye params)
        self.relevant = (self.condition)(avatar_params);

        if self.relevant {
            log::debug!("NativeParam '{}' is relevant", self.address);
        }

        if self.relevant {
            1
        } else {
            0
        }
    }

    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage> {
        if !self.relevant {
            return vec![];
        }

        let values = (self.get_value)(data);

        // Delta check
        let should_send = match &self.last_values {
            Some(last) => values
                .iter()
                .zip(last.iter())
                .any(|(a, b)| (a - b).abs() > 0.00001),
            None => true,
        };

        if !should_send {
            return vec![];
        }

        self.last_values = Some(values.clone());

        // Send as array of floats
        vec![OscMessage {
            addr: self.address.clone(),
            args: values.iter().map(|v| OscType::Float(*v)).collect(),
        }]
    }
}

/// Checks if avatar has any eye X/Y parameters
pub fn has_eye_xy_params(avatar_params: &HashSet<String>) -> bool {
    avatar_params.iter().any(|p| {
        let lower = p.to_lowercase();
        (lower.contains("eye") || lower.contains("eyes"))
            && (lower.ends_with("x") || lower.ends_with("y"))
    })
}

/// Checks if avatar has any eye openness/lid parameters
pub fn has_eye_lid_params(avatar_params: &HashSet<String>) -> bool {
    avatar_params.iter().any(|p| {
        let lower = p.to_lowercase();
        lower.contains("eye") && (lower.contains("open") || lower.contains("lid"))
    })
}

/// Creates all native tracking parameters
pub fn create_native_parameters() -> Vec<Box<dyn Parameter>> {
    vec![
        // Vector4: Left Pitch/Yaw, Right Pitch/Yaw
        // Only relevant if avatar lacks EyeX/EyeY params
        Box::new(NativeParameter::new_vector4(
            "/tracking/eye/LeftRightPitchYaw",
            |d| {
                // Convert gaze X/Y to pitch/yaw
                // Gaze convention: X = yaw (left-right), Y = pitch (up-down)
                [
                    d.eye.left.gaze.y,  // left pitch (up/down)
                    d.eye.left.gaze.x,  // left yaw (left/right)
                    d.eye.right.gaze.y, // right pitch
                    d.eye.right.gaze.x, // right yaw
                ]
            },
            |params| !has_eye_xy_params(params),
        )),
        // Float: Combined eye closed amount
        // Only relevant if avatar lacks eye open/lid params
        Box::new(NativeParameter::new_float(
            "/tracking/eye/EyesClosedAmount",
            |d| 1.0 - (d.eye.left.openness + d.eye.right.openness) / 2.0,
            |params| !has_eye_lid_params(params),
        )),
    ]
}
