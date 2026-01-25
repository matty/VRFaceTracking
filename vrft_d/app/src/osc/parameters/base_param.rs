//! Float and bool parameter types with relevancy tracking and delta checking.

use super::{ParamType, Parameter};
use common::UnifiedTrackingData;
use rosc::{OscMessage, OscType};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const DEFAULT_PREFIX: &str = "/avatar/parameters/";

/// Matches `/avatar/parameters/{name}` or `/avatar/parameters/FT/{name}`
fn matches_address(name: &str, addr: &str) -> bool {
    let stripped = match addr.strip_prefix(DEFAULT_PREFIX) {
        Some(s) => s,
        None => return false,
    };

    if stripped == name {
        return true;
    }

    if let Some(after_ft) = stripped.strip_prefix("FT/") {
        if after_ft == name {
            return true;
        }
    }

    false
}

/// Float parameter with relevancy tracking
pub struct FloatParam {
    pub name: String,
    pub addresses: Vec<String>,
    pub relevant: bool,
    get_value: Arc<dyn Fn(&UnifiedTrackingData) -> f32 + Send + Sync>,
    last_value: Option<f32>,
}

impl FloatParam {
    pub fn new(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> f32 + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            addresses: vec![format!("{}{}", DEFAULT_PREFIX, name)],
            relevant: false,
            get_value: Arc::new(get_value),
            last_value: None,
        }
    }
}

impl Parameter for FloatParam {
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> bool {
        self.addresses.clear();
        self.last_value = None;

        let compatible: Vec<_> = avatar_params
            .iter()
            .filter(|addr| {
                matches_address(&self.name, addr)
                    && param_types
                        .get(*addr)
                        .is_none_or(|t| *t == ParamType::Float)
            })
            .cloned()
            .collect();

        if !compatible.is_empty() {
            self.addresses.extend(compatible);
            self.relevant = true;
        } else {
            self.addresses
                .push(format!("{}FT/{}", DEFAULT_PREFIX, self.name));
            self.relevant = false;
        }

        self.relevant
    }

    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage> {
        if !self.relevant {
            return vec![];
        }

        let value = (self.get_value)(data);

        // Delta check
        let should_send = match self.last_value {
            Some(last) => (value - last).abs() > 0.00001,
            None => true,
        };

        if !should_send {
            return vec![];
        }

        self.last_value = Some(value);

        self.addresses
            .iter()
            .map(|addr| OscMessage {
                addr: addr.clone(),
                args: vec![OscType::Float(value)],
            })
            .collect()
    }
}

/// Bool parameter with relevancy tracking
pub struct BoolParam {
    pub name: String,
    pub addresses: Vec<String>,
    pub relevant: bool,
    get_value: Arc<dyn Fn(&UnifiedTrackingData) -> bool + Send + Sync>,
    last_value: Option<bool>,
}

impl BoolParam {
    pub fn new(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            addresses: vec![format!("{}{}", DEFAULT_PREFIX, name)],
            relevant: false,
            get_value: Arc::new(get_value),
            last_value: None,
        }
    }
}

impl Parameter for BoolParam {
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> bool {
        self.addresses.clear();
        self.last_value = None;

        let compatible: Vec<_> = avatar_params
            .iter()
            .filter(|addr| {
                matches_address(&self.name, addr)
                    && param_types.get(*addr).is_none_or(|t| *t == ParamType::Bool)
            })
            .cloned()
            .collect();

        if !compatible.is_empty() {
            self.addresses.extend(compatible);
            self.relevant = true;
        } else {
            self.addresses
                .push(format!("{}FT/{}", DEFAULT_PREFIX, self.name));
            self.relevant = false;
        }

        self.relevant
    }

    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage> {
        if !self.relevant {
            return vec![];
        }

        let value = (self.get_value)(data);

        let should_send = match self.last_value {
            Some(last) => value != last,
            None => true,
        };

        if !should_send {
            return vec![];
        }

        self.last_value = Some(value);

        self.addresses
            .iter()
            .map(|addr| OscMessage {
                addr: addr.clone(),
                args: vec![OscType::Bool(value)],
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_address_exact_match() {
        assert!(matches_address(
            "v2/EyeLeftX",
            "/avatar/parameters/v2/EyeLeftX"
        ));
        assert!(matches_address("EyeLeftX", "/avatar/parameters/EyeLeftX"));
    }

    #[test]
    fn test_matches_address_ft_prefix() {
        assert!(matches_address(
            "v2/EyeLeftX",
            "/avatar/parameters/FT/v2/EyeLeftX"
        ));
        assert!(matches_address(
            "EyeLeftX",
            "/avatar/parameters/FT/EyeLeftX"
        ));
    }

    #[test]
    fn test_matches_address_rejects_custom_prefix() {
        assert!(!matches_address(
            "v2/EyeLeftX",
            "/avatar/parameters/Custom/v2/EyeLeftX"
        ));
        assert!(!matches_address(
            "EyeLeftX",
            "/avatar/parameters/Custom/EyeLeftX"
        ));
        assert!(!matches_address(
            "SmileFrown",
            "/avatar/parameters/VF/SmileFrown"
        ));
    }

    #[test]
    fn test_matches_address_rejects_suffix_match() {
        assert!(!matches_address(
            "EyeLeftX",
            "/avatar/parameters/Something/Extra/EyeLeftX"
        ));
        assert!(!matches_address(
            "v2/EyeX",
            "/avatar/parameters/Random/v2/EyeX"
        ));
    }

    #[test]
    fn test_matches_address_rejects_wrong_prefix() {
        assert!(!matches_address("EyeLeftX", "/wrong/parameters/EyeLeftX"));
        assert!(!matches_address("EyeLeftX", "EyeLeftX"));
    }
}
