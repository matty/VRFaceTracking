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
            // Add /FT/ fallback if not already present
            let has_ft = compatible.iter().any(|a| a.contains("/FT/"));
            self.addresses.extend(compatible);
            if !has_ft {
                self.addresses
                    .push(format!("{}FT/{}", DEFAULT_PREFIX, self.name));
            }
            self.relevant = true;
        } else {
            // No match - still send to /FT/ as fallback
            self.addresses
                .push(format!("{}FT/{}", DEFAULT_PREFIX, self.name));
            self.relevant = true;
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
            // Add /FT/ fallback if not already present
            let has_ft = compatible.iter().any(|a| a.contains("/FT/"));
            self.addresses.extend(compatible);
            if !has_ft {
                self.addresses
                    .push(format!("{}FT/{}", DEFAULT_PREFIX, self.name));
            }
            self.relevant = true;
        } else {
            // No match - still send to /FT/ as fallback
            self.addresses
                .push(format!("{}FT/{}", DEFAULT_PREFIX, self.name));
            self.relevant = true;
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

    // ===== FloatParam Fallback Tests =====

    #[test]
    fn test_float_param_fallback_is_relevant_when_no_matches() {
        let mut param = FloatParam::new("v2/TongueOut", |_| 0.5);
        let empty_params = HashSet::new();
        let empty_types = HashMap::new();

        let relevant = param.reset(&empty_params, &empty_types);

        assert!(relevant, "Fallback should be marked relevant");
        assert_eq!(param.addresses.len(), 1, "Should have exactly one fallback");
        assert!(
            param.addresses[0].contains("/FT/"),
            "Fallback should use /FT/ prefix"
        );
        assert_eq!(
            param.addresses[0],
            "/avatar/parameters/FT/v2/TongueOut",
            "Fallback address should be correct"
        );
    }

    #[test]
    fn test_float_param_adds_ft_fallback_when_non_ft_match() {
        let mut param = FloatParam::new("v2/TongueOut", |_| 0.5);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/TongueOut".to_string());

        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/v2/TongueOut".to_string(),
            ParamType::Float,
        );

        let relevant = param.reset(&avatar_params, &param_types);

        assert!(relevant, "Should be relevant when matched");
        assert_eq!(
            param.addresses.len(),
            2,
            "Should have matched + FT fallback"
        );
        assert!(
            param.addresses.contains(&"/avatar/parameters/v2/TongueOut".to_string()),
            "Should include matched address"
        );
        assert!(
            param.addresses.contains(&"/avatar/parameters/FT/v2/TongueOut".to_string()),
            "Should include /FT/ fallback"
        );
    }

    #[test]
    fn test_float_param_no_duplicate_ft_when_ft_already_matches() {
        let mut param = FloatParam::new("v2/TongueOut", |_| 0.5);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/FT/v2/TongueOut".to_string());

        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/FT/v2/TongueOut".to_string(),
            ParamType::Float,
        );

        let relevant = param.reset(&avatar_params, &param_types);

        assert!(relevant, "Should be relevant when FT matched");
        assert_eq!(
            param.addresses.len(),
            1,
            "Should NOT add duplicate /FT/ address"
        );
        assert!(
            param.addresses[0].contains("/FT/"),
            "Should contain the FT match"
        );
    }

    #[test]
    fn test_float_param_skips_wrong_type() {
        let mut param = FloatParam::new("v2/TongueOut", |_| 0.5);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/TongueOut".to_string());

        let mut param_types = HashMap::new();
        // Mark as Bool, not Float - should not match
        param_types.insert(
            "/avatar/parameters/v2/TongueOut".to_string(),
            ParamType::Bool,
        );

        let relevant = param.reset(&avatar_params, &param_types);

        // Should still be relevant via fallback
        assert!(relevant, "Fallback should still be relevant");
        assert_eq!(param.addresses.len(), 1, "Only fallback address");
        assert!(
            param.addresses[0].contains("/FT/"),
            "Should use /FT/ fallback"
        );
    }

    #[test]
    fn test_float_param_process_sends_to_all_addresses() {
        let mut param = FloatParam::new("v2/TongueOut", |_| 0.75);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/TongueOut".to_string());

        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/v2/TongueOut".to_string(),
            ParamType::Float,
        );

        param.reset(&avatar_params, &param_types);

        let data = UnifiedTrackingData::default();
        let messages = param.process(&data);

        // Should send to both addresses
        assert_eq!(messages.len(), 2, "Should send to both addresses");
        let addrs: Vec<_> = messages.iter().map(|m| m.addr.as_str()).collect();
        assert!(addrs.contains(&"/avatar/parameters/v2/TongueOut"));
        assert!(addrs.contains(&"/avatar/parameters/FT/v2/TongueOut"));
    }

    // ===== BoolParam Fallback Tests =====

    #[test]
    fn test_bool_param_fallback_is_relevant_when_no_matches() {
        let mut param = BoolParam::new("v2/TongueOut", |_| true);
        let empty_params = HashSet::new();
        let empty_types = HashMap::new();

        let relevant = param.reset(&empty_params, &empty_types);

        assert!(relevant, "Fallback should be marked relevant");
        assert_eq!(param.addresses.len(), 1, "Should have exactly one fallback");
        assert!(
            param.addresses[0].contains("/FT/"),
            "Fallback should use /FT/ prefix"
        );
    }

    #[test]
    fn test_bool_param_adds_ft_fallback_when_non_ft_match() {
        let mut param = BoolParam::new("v2/TongueOut", |_| true);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/TongueOut".to_string());

        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/v2/TongueOut".to_string(),
            ParamType::Bool,
        );

        let relevant = param.reset(&avatar_params, &param_types);

        assert!(relevant, "Should be relevant when matched");
        assert_eq!(
            param.addresses.len(),
            2,
            "Should have matched + FT fallback"
        );
    }

    #[test]
    fn test_bool_param_process_sends_to_all_addresses() {
        let mut param = BoolParam::new("v2/TongueOut", |_| true);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/TongueOut".to_string());

        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/v2/TongueOut".to_string(),
            ParamType::Bool,
        );

        param.reset(&avatar_params, &param_types);

        let data = UnifiedTrackingData::default();
        let messages = param.process(&data);

        // Should send to both addresses
        assert_eq!(messages.len(), 2, "Should send to both addresses");
    }

    // ===== Delta Check Tests =====

    #[test]
    fn test_float_param_delta_check_prevents_duplicate_sends() {
        let mut param = FloatParam::new("v2/Test", |_| 0.5);
        let empty_params = HashSet::new();
        let empty_types = HashMap::new();
        param.reset(&empty_params, &empty_types);

        let data = UnifiedTrackingData::default();

        // First send
        let messages1 = param.process(&data);
        assert!(!messages1.is_empty(), "First send should have messages");

        // Second send with same value
        let messages2 = param.process(&data);
        assert!(messages2.is_empty(), "Second send should be empty (delta check)");
    }
}
