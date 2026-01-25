pub mod base_param;
pub mod binary_param;
pub mod eparam;
pub mod registry;

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
    /// Reset parameter state based on new avatar's parameters
    /// Returns true if this parameter is now relevant
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> bool;

    /// Process tracking data and return OSC messages to send
    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage>;
}
