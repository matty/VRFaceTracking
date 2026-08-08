//! Binary parameter encoding with dynamic bit discovery and delta checking.

use super::{ParamType, Parameter};
use rosc::{OscMessage, OscType};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use vrft_common::UnifiedTrackingData;

const DEFAULT_PREFIX: &str = "/avatar/parameters/";

/// Text following `{name}` for every way `addr` could refer to a parameter of
/// that name, applying the prefix and nested-version rules once so that bit
/// discovery, negative-parameter discovery and relevancy checks all agree.
///
/// An empty result means `addr` does not refer to `name` at all. A `"1"`,
/// `"2"`, `"4"`... suffix is a binary bit, `"Negative"` is the sign companion,
/// and anything else belongs to a different parameter that merely shares this
/// one's opening text.
pub(crate) fn name_suffixes<'a>(name: &str, addr: &'a str) -> Vec<&'a str> {
    let mut suffixes = Vec::new();

    let Some(stripped) = addr.strip_prefix(DEFAULT_PREFIX) else {
        return suffixes;
    };

    // Exact match: "{name}{suffix}"
    if stripped.len() > name.len() && stripped.starts_with(name) {
        suffixes.push(&stripped[name.len()..]);
    }

    // Suffix match: ".../{name}{suffix}" with any prefix
    let sep = format!("/{name}");
    if let Some(idx) = stripped.find(sep.as_str()) {
        // Reject nested version prefixes (e.g., /v1/v2/Name)
        if idx >= 2 {
            let before = &stripped[..idx];
            let bytes = before.as_bytes();
            if bytes[bytes.len() - 1].is_ascii_digit()
                && bytes.len() >= 2
                && bytes[bytes.len() - 2] == b'v'
            {
                return suffixes;
            }
        }
        suffixes.push(&stripped[idx + sep.len()..]);
    }

    suffixes
}

/// Returns shift count if index is power of 2 (1, 2, 4, 8...), None otherwise.
pub fn get_binary_steps(index: u32) -> Option<usize> {
    let mut curr_seq_item = 1u32;
    for i in 0..32 {
        if curr_seq_item == index {
            return Some(i);
        }
        curr_seq_item = curr_seq_item.saturating_mul(2);
    }
    None
}

/// Binary parameter with dynamic bit discovery
pub struct BinaryBaseParameter {
    pub name: String,
    pub bit_params: Vec<(String, usize)>,
    pub negative_param: Option<String>,
    pub max_binary_int: u32,
    pub relevant: bool,
    get_value: Arc<dyn Fn(&UnifiedTrackingData) -> f32 + Send + Sync>,
    last_bits: HashMap<String, bool>,
    negative_relevant: bool,
    send_on_load: bool,
    needs_initial_send: bool,
}

impl BinaryBaseParameter {
    pub fn new(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> f32 + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            bit_params: Vec::new(),
            negative_param: None,
            max_binary_int: 0,
            relevant: false,
            get_value: Arc::new(get_value),
            last_bits: HashMap::new(),
            negative_relevant: false,
            send_on_load: false,
            needs_initial_send: false,
        }
    }

    /// Create a parameter that sends all bit values immediately when it becomes relevant
    pub fn new_with_send_on_load(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> f32 + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            bit_params: Vec::new(),
            negative_param: None,
            max_binary_int: 0,
            relevant: false,
            get_value: Arc::new(get_value),
            last_bits: HashMap::new(),
            negative_relevant: false,
            send_on_load: true,
            needs_initial_send: false,
        }
    }

    /// Matches binary parameter patterns:
    /// - `/avatar/parameters/{name}N` (exact)
    /// - `/avatar/parameters/{prefix}/{name}N` (any prefix)
    fn matches_binary_pattern(&self, addr: &str) -> Option<u32> {
        name_suffixes(&self.name, addr)
            .into_iter()
            .find_map(|suffix| suffix.parse::<u32>().ok())
    }

    /// Whether `addr` is this parameter's `{name}Negative` companion.
    fn matches_negative_pattern(&self, addr: &str) -> bool {
        name_suffixes(&self.name, addr).contains(&"Negative")
    }

    fn process_binary(&self, value: f32, binary_index: usize) -> bool {
        let mut val = value;

        if !self.negative_relevant && val < 0.0 {
            return false;
        }
        val = val.abs();

        if val > 0.99999 {
            return true;
        }

        let big_value = (val * self.max_binary_int as f32) as u32;
        ((big_value >> binary_index) & 1) == 1
    }
}

impl Parameter for BinaryBaseParameter {
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> usize {
        self.bit_params.clear();
        self.last_bits.clear();
        self.negative_relevant = false;

        // Find the negative param address, applying the same matching rules as
        // bit discovery so a parameter whose name merely ends with this one
        // cannot claim it.
        let neg_addr = avatar_params
            .iter()
            .find(|a| {
                self.matches_negative_pattern(a)
                    && param_types.get(*a).is_some_and(|t| *t == ParamType::Bool)
            })
            .cloned();

        if let Some(addr) = neg_addr {
            self.negative_param = Some(addr);
            self.negative_relevant = true;
        } else {
            self.negative_param = None;
        }

        let mut params_to_create: HashMap<String, usize> = HashMap::new();

        for param_addr in avatar_params.iter() {
            let is_bool = param_types
                .get(param_addr)
                .is_some_and(|t| *t == ParamType::Bool);
            if !is_bool {
                continue;
            }

            if let Some(index) = self.matches_binary_pattern(param_addr) {
                if let Some(binary_index) = get_binary_steps(index) {
                    params_to_create.insert(param_addr.clone(), binary_index);
                }
            }
        }

        if params_to_create.is_empty() {
            if self.negative_relevant {
                // No binary bits but negative param exists — still relevant for the negative bool
                self.relevant = true;
                if self.send_on_load {
                    self.needs_initial_send = true;
                }
                return 1;
            }
            self.relevant = false;
            return 0;
        }

        self.max_binary_int = 2u32.pow(params_to_create.len() as u32);
        self.bit_params = params_to_create.into_iter().collect();
        self.bit_params.sort_by_key(|(_, shift)| *shift);

        log::debug!(
            "BinaryParam '{}': {} bit params",
            self.name,
            self.bit_params.len()
        );

        self.relevant = true;

        // Mark for initial send if sendOnLoad is enabled
        if self.send_on_load {
            self.needs_initial_send = true;
        }
        // Count: number of bit params + 1 for negative param if present
        self.bit_params.len() + if self.negative_relevant { 1 } else { 0 }
    }

    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage> {
        if !self.relevant {
            return vec![];
        }

        let value = (self.get_value)(data);
        let mut messages = Vec::new();

        // Force send all bits on first call after reset if sendOnLoad is enabled
        let force_send = self.needs_initial_send;
        if self.needs_initial_send {
            self.needs_initial_send = false;
        }

        if let Some(neg_addr) = &self.negative_param {
            if self.negative_relevant {
                let is_negative = value < 0.0;
                let last_neg = self.last_bits.get(neg_addr).copied();

                if force_send || last_neg != Some(is_negative) {
                    messages.push(OscMessage {
                        addr: neg_addr.clone(),
                        args: vec![OscType::Bool(is_negative)],
                    });
                    self.last_bits.insert(neg_addr.clone(), is_negative);
                }
            }
        }

        for (addr, shift_index) in &self.bit_params {
            let bit_value = self.process_binary(value, *shift_index);
            let last_bit = self.last_bits.get(addr).copied();

            if force_send || last_bit != Some(bit_value) {
                messages.push(OscMessage {
                    addr: addr.clone(),
                    args: vec![OscType::Bool(bit_value)],
                });
                self.last_bits.insert(addr.clone(), bit_value);
            }
        }

        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_param_requires_the_same_boundary_rules_as_bits() {
        let param = BinaryBaseParameter::new("SmileSadRight", |_| 0.0);

        assert!(param.matches_negative_pattern("/avatar/parameters/SmileSadRightNegative"));
        assert!(param.matches_negative_pattern("/avatar/parameters/FT/SmileSadRightNegative"));

        // A v2 parameter that merely ends with this parameter's name must not
        // be claimed by it.
        assert!(!param.matches_negative_pattern("/avatar/parameters/v2/SmileSadRightNegative"));

        // Neither may an unrelated parameter that happens to share the suffix.
        assert!(!param.matches_negative_pattern("/avatar/parameters/MySmileSadRightNegative"));
    }

    #[test]
    fn negative_param_does_not_match_bare_name_or_bits() {
        let param = BinaryBaseParameter::new("JawOpen", |_| 0.0);

        assert!(!param.matches_negative_pattern("/avatar/parameters/JawOpen"));
        assert!(!param.matches_negative_pattern("/avatar/parameters/JawOpen1"));
        assert!(!param.matches_negative_pattern("/avatar/parameters/JawOpenNegativeExtra"));
    }

    #[test]
    fn negative_param_must_be_declared_bool() {
        let mut param = BinaryBaseParameter::new("JawOpen", |_| 0.0);

        let addr = "/avatar/parameters/JawOpenNegative".to_string();
        let avatar_params: HashSet<String> = [addr.clone()].into_iter().collect();

        let mut float_types = HashMap::new();
        float_types.insert(addr.clone(), ParamType::Float);
        assert_eq!(param.reset(&avatar_params, &float_types), 0);
        assert_eq!(param.negative_param, None);

        let mut bool_types = HashMap::new();
        bool_types.insert(addr.clone(), ParamType::Bool);
        assert_eq!(param.reset(&avatar_params, &bool_types), 1);
        assert_eq!(param.negative_param, Some(addr));
    }

    #[test]
    fn test_get_binary_steps() {
        assert_eq!(get_binary_steps(1), Some(0));
        assert_eq!(get_binary_steps(2), Some(1));
        assert_eq!(get_binary_steps(4), Some(2));
        assert_eq!(get_binary_steps(8), Some(3));
        assert_eq!(get_binary_steps(16), Some(4));
        assert_eq!(get_binary_steps(3), None);
        assert_eq!(get_binary_steps(5), None);
        assert_eq!(get_binary_steps(0), None);
    }

    #[test]
    fn test_matches_binary_pattern_exact_match() {
        let param = BinaryBaseParameter::new("v2/SmileFrown", |_| 0.0);
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/v2/SmileFrown1"),
            Some(1)
        );
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/v2/SmileFrown2"),
            Some(2)
        );
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/v2/SmileFrown4"),
            Some(4)
        );
    }

    #[test]
    fn test_matches_binary_pattern_ft_prefix() {
        let param = BinaryBaseParameter::new("v2/SmileFrown", |_| 0.0);
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/FT/v2/SmileFrown1"),
            Some(1)
        );
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/FT/v2/SmileFrown2"),
            Some(2)
        );
    }

    #[test]
    fn test_matches_binary_pattern_accepts_custom_prefix() {
        let param = BinaryBaseParameter::new("v2/SmileFrown", |_| 0.0);
        // Custom prefixes are now accepted (same as float/bool params)
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/Custom/v2/SmileFrown1"),
            Some(1)
        );
        // Wrong base name still rejected
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/VF/SmileFrown1"),
            None
        );
    }

    #[test]
    fn test_matches_binary_pattern_rejects_wrong_base_name() {
        let param = BinaryBaseParameter::new("v2/SmileFrown", |_| 0.0);
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/v2/EyeX1"),
            None
        );
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/FT/v2/JawOpen1"),
            None
        );
    }

    #[test]
    fn test_matches_binary_pattern_no_suffix() {
        let param = BinaryBaseParameter::new("v2/SmileFrown", |_| 0.0);
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/v2/SmileFrown"),
            None
        );
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/FT/v2/SmileFrown"),
            None
        );
    }

    #[test]
    fn test_process_binary_encoding() {
        let param = BinaryBaseParameter {
            name: "Test".to_string(),
            bit_params: vec![
                ("Test1".to_string(), 0),
                ("Test2".to_string(), 1),
                ("Test4".to_string(), 2),
                ("Test8".to_string(), 3),
            ],
            negative_param: None,
            max_binary_int: 16,
            relevant: true,
            get_value: Arc::new(|_| 0.5),
            last_bits: HashMap::new(),
            negative_relevant: false,
            send_on_load: false,
            needs_initial_send: false,
        };

        // 0.5 * 16 = 8 = 1000 in binary
        assert!(!param.process_binary(0.5, 0));
        assert!(!param.process_binary(0.5, 1));
        assert!(!param.process_binary(0.5, 2));
        assert!(param.process_binary(0.5, 3));

        // 1.0 = all bits true
        assert!(param.process_binary(1.0, 0));
        assert!(param.process_binary(1.0, 1));
        assert!(param.process_binary(1.0, 2));
        assert!(param.process_binary(1.0, 3));
    }
}
