pub mod base_param;
pub mod binary_param;
pub mod eparam;
pub mod legacy_eye;
pub mod legacy_lip;
pub mod native_param;
pub mod registry;
pub mod unified_expressions;

use common::UnifiedTrackingData;
use rosc::OscMessage;
use std::collections::{HashMap, HashSet};

/// Parameter type information from avatar
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    Float,
    Bool,
    Int,
}

/// Trait for all parameter types
pub trait Parameter: Send + Sync {
    /// Reset parameter state based on avatar's available parameters.
    /// Returns the count of individual addresses/sub-parameters that are now relevant.
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> usize;

    /// Process tracking data and return OSC messages to send
    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage>;
}
