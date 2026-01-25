use super::base_param::{BoolParam, FloatParam};
use super::binary_param::BinaryBaseParameter;
use super::{ParamType, Parameter};
use common::UnifiedTrackingData;
use rosc::OscMessage;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Container that creates bool + float + binary params for one expression
pub struct EParam {
    bool_param: BoolParam,
    float_param: FloatParam,
    binary_param: Option<BinaryBaseParameter>,
}

impl EParam {
    /// Create a new EParam with float getter
    pub fn new(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> f32 + Send + Sync + Clone + 'static,
        min_bool_threshold: f32,
        skip_binary: bool,
    ) -> Self {
        let get_value_arc = Arc::new(get_value.clone());
        let get_value_arc2 = get_value_arc.clone();

        let bool_param =
            BoolParam::new(name, move |data| (get_value_arc)(data) < min_bool_threshold);

        let float_param = FloatParam::new(name, move |data| (get_value_arc2)(data));

        let binary_param = if skip_binary {
            None
        } else {
            Some(BinaryBaseParameter::new(name, get_value))
        };

        Self {
            bool_param,
            float_param,
            binary_param,
        }
    }

    /// Simplified constructor with default threshold of 0.5 and binary enabled
    pub fn simple(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> f32 + Send + Sync + Clone + 'static,
    ) -> Self {
        Self::new(name, get_value, 0.5, false)
    }

    /// Constructor for expression-based params with 0.0 threshold
    pub fn expression(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> f32 + Send + Sync + Clone + 'static,
    ) -> Self {
        Self::new(name, get_value, 0.0, false)
    }
}

impl Parameter for EParam {
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> bool {
        let bool_relevant = self.bool_param.reset(avatar_params, param_types);
        let float_relevant = self.float_param.reset(avatar_params, param_types);
        let binary_relevant = self
            .binary_param
            .as_mut()
            .map_or(false, |b| b.reset(avatar_params, param_types));

        bool_relevant || float_relevant || binary_relevant
    }

    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage> {
        let mut messages = Vec::new();

        messages.extend(self.bool_param.process(data));
        messages.extend(self.float_param.process(data));

        if let Some(binary) = &mut self.binary_param {
            messages.extend(binary.process(data));
        }

        messages
    }
}
