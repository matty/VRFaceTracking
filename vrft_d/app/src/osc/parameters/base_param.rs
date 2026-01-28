//! Float and bool parameter types with relevancy tracking and delta checking.

use super::{ParamType, Parameter};
use common::UnifiedTrackingData;
use rosc::{OscMessage, OscType};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const DEFAULT_PREFIX: &str = "/avatar/parameters/";

/// Matches parameter addresses with flexible prefix support.
///
/// This matches:
/// - Exact parameter name after stripping `/avatar/parameters/`
/// - Any address ending with `/{name}` (e.g., `FT/`, `OSCm/Float/FT/`, custom prefixes)
///
/// Rejects nested version prefixes (e.g., `/v1/v2/EyeLeftX`)
fn matches_address(name: &str, addr: &str) -> bool {
    let stripped = match addr.strip_prefix(DEFAULT_PREFIX) {
        Some(s) => s,
        None => return false,
    };

    // Check for exact match
    if stripped == name {
        return true;
    }

    // Check for suffix match: ends with /{name}
    let suffix = format!("/{}", name);
    if stripped.ends_with(&suffix) {
        // Reject nested version prefixes like /v1/v2/Name
        // Get the character before the suffix
        let prefix_len = stripped.len() - suffix.len();
        if prefix_len >= 2 {
            let before_suffix = &stripped[..prefix_len];
            // Check if it ends with /v{digit}
            if before_suffix.len() >= 2 {
                let bytes = before_suffix.as_bytes();
                let last = bytes[bytes.len() - 1];
                if last.is_ascii_digit() && bytes.len() >= 2 && bytes[bytes.len() - 2] == b'v' {
                    return false;
                }
            }
        }
        return true;
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
    send_on_load: bool,
    needs_initial_send: bool,
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
            send_on_load: false,
            needs_initial_send: false,
        }
    }

    /// Create a parameter that sends its value immediately when it becomes relevant
    pub fn new_with_send_on_load(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> f32 + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            addresses: vec![format!("{}{}", DEFAULT_PREFIX, name)],
            relevant: false,
            get_value: Arc::new(get_value),
            last_value: None,
            send_on_load: true,
            needs_initial_send: false,
        }
    }
}

impl Parameter for FloatParam {
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> usize {
        self.addresses.clear();
        self.last_value = None;
        self.needs_initial_send = false;

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

            // Mark for initial send if sendOnLoad is enabled
            if self.send_on_load {
                self.needs_initial_send = true;
            }
        } else {
            // No matches found - mark as irrelevant
            self.relevant = false;
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

        let value = (self.get_value)(data);

        // Force send on first call after reset if sendOnLoad is enabled
        if self.needs_initial_send {
            self.needs_initial_send = false;
            self.last_value = Some(value);
            return self
                .addresses
                .iter()
                .map(|addr| OscMessage {
                    addr: addr.clone(),
                    args: vec![OscType::Float(value)],
                })
                .collect();
        }

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
    send_on_load: bool,
    needs_initial_send: bool,
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
            send_on_load: false,
            needs_initial_send: false,
        }
    }

    /// Create a parameter that sends its value immediately when it becomes relevant
    pub fn new_with_send_on_load(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            addresses: vec![format!("{}{}", DEFAULT_PREFIX, name)],
            relevant: false,
            get_value: Arc::new(get_value),
            last_value: None,
            send_on_load: true,
            needs_initial_send: false,
        }
    }
}

impl Parameter for BoolParam {
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> usize {
        self.addresses.clear();
        self.last_value = None;
        self.needs_initial_send = false;

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

            // Mark for initial send if sendOnLoad is enabled
            if self.send_on_load {
                self.needs_initial_send = true;
            }
        } else {
            // No matches found - mark as irrelevant
            self.relevant = false;
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

        let value = (self.get_value)(data);

        // Force send on first call after reset if sendOnLoad is enabled
        if self.needs_initial_send {
            self.needs_initial_send = false;
            self.last_value = Some(value);
            return self
                .addresses
                .iter()
                .map(|addr| OscMessage {
                    addr: addr.clone(),
                    args: vec![OscType::Bool(value)],
                })
                .collect();
        }

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

/// Int parameter with relevancy tracking
pub struct IntParam {
    pub name: String,
    pub addresses: Vec<String>,
    pub relevant: bool,
    get_value: Arc<dyn Fn(&UnifiedTrackingData) -> i32 + Send + Sync>,
    last_value: Option<i32>,
    send_on_load: bool,
    needs_initial_send: bool,
}

impl IntParam {
    pub fn new(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> i32 + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            addresses: vec![format!("{}{}", DEFAULT_PREFIX, name)],
            relevant: false,
            get_value: Arc::new(get_value),
            last_value: None,
            send_on_load: false,
            needs_initial_send: false,
        }
    }

    /// Create a parameter that sends its value immediately when it becomes relevant
    pub fn new_with_send_on_load(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> i32 + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            addresses: vec![format!("{}{}", DEFAULT_PREFIX, name)],
            relevant: false,
            get_value: Arc::new(get_value),
            last_value: None,
            send_on_load: true,
            needs_initial_send: false,
        }
    }
}

impl Parameter for IntParam {
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> usize {
        self.addresses.clear();
        self.last_value = None;
        self.needs_initial_send = false;

        let compatible: Vec<_> = avatar_params
            .iter()
            .filter(|addr| {
                matches_address(&self.name, addr)
                    && param_types.get(*addr).is_none_or(|t| *t == ParamType::Int)
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

            // Mark for initial send if sendOnLoad is enabled
            if self.send_on_load {
                self.needs_initial_send = true;
            }
        } else {
            // No matches found - mark as irrelevant
            self.relevant = false;
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

        let value = (self.get_value)(data);

        // Force send on first call after reset if sendOnLoad is enabled
        if self.needs_initial_send {
            self.needs_initial_send = false;
            self.last_value = Some(value);
            return self
                .addresses
                .iter()
                .map(|addr| OscMessage {
                    addr: addr.clone(),
                    args: vec![OscType::Int(value)],
                })
                .collect();
        }

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
                args: vec![OscType::Int(value)],
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
    fn test_matches_address_custom_prefix_matches() {
        // Arbitrary prefixes are accepted, not just known VRChat patterns
        assert!(matches_address(
            "v2/EyeLeftX",
            "/avatar/parameters/Custom/v2/EyeLeftX"
        ));
        assert!(matches_address(
            "SmileFrown",
            "/avatar/parameters/VF/SmileFrown"
        ));
    }

    #[test]
    fn test_matches_address_rejects_nested_versions() {
        // The negative lookbehind rejects nested version prefixes like /v1/v2/Name
        assert!(!matches_address(
            "v2/EyeLeftX",
            "/avatar/parameters/v1/v2/EyeLeftX"
        ));
        assert!(!matches_address(
            "v2/SmileFrown",
            "/avatar/parameters/v3/v2/SmileFrown"
        ));
    }

    #[test]
    fn test_matches_address_rejects_wrong_prefix() {
        assert!(!matches_address("EyeLeftX", "/wrong/parameters/EyeLeftX"));
        assert!(!matches_address("EyeLeftX", "EyeLeftX"));
    }

    #[test]
    fn test_matches_address_oscmooth_float_prefix() {
        assert!(matches_address(
            "v2/SmileFrown",
            "/avatar/parameters/OSCm/Float/FT/v2/SmileFrown"
        ));
        assert!(matches_address(
            "v2/EyeLeftX",
            "/avatar/parameters/OSCm/Float/v2/EyeLeftX"
        ));
    }

    #[test]
    fn test_matches_address_oscmooth_bool_prefix() {
        assert!(matches_address(
            "v2/SmileFrown",
            "/avatar/parameters/OSCm/Bool/FT/v2/SmileFrown"
        ));
        assert!(matches_address(
            "v2/EyeLeftX",
            "/avatar/parameters/OSCm/Bool/v2/EyeLeftX"
        ));
    }
}
