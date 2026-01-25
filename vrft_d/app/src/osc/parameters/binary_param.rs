//! Binary parameter encoding with dynamic bit discovery and delta checking.

use super::{ParamType, Parameter};
use common::UnifiedTrackingData;
use rosc::{OscMessage, OscType};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const DEFAULT_PREFIX: &str = "/avatar/parameters/";

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
        }
    }

    /// Matches `{prefix}{name}N` or `{prefix}FT/{name}N` where N is a number.
    fn matches_binary_pattern(&self, addr: &str) -> Option<u32> {
        let stripped = addr.strip_prefix(DEFAULT_PREFIX)?;

        let after_prefix = stripped.strip_prefix("FT/").unwrap_or(stripped);

        if !after_prefix.starts_with(&self.name) {
            return None;
        }

        let suffix = &after_prefix[self.name.len()..];
        suffix.parse::<u32>().ok()
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
    ) -> bool {
        self.bit_params.clear();
        self.last_bits.clear();
        self.negative_relevant = false;

        let neg_name = format!("{}Negative", self.name);
        let neg_addr = format!("{}{}", DEFAULT_PREFIX, neg_name);
        if avatar_params.contains(&neg_addr) || avatar_params.iter().any(|a| a.ends_with(&neg_name))
        {
            self.negative_param = Some(neg_addr.clone());
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
            self.relevant = false;
            return false;
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
        true
    }

    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage> {
        if !self.relevant {
            return vec![];
        }

        let value = (self.get_value)(data);
        let mut messages = Vec::new();

        if let Some(neg_addr) = &self.negative_param {
            if self.negative_relevant {
                let is_negative = value < 0.0;
                let last_neg = self.last_bits.get(neg_addr).copied();

                if last_neg != Some(is_negative) {
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

            if last_bit != Some(bit_value) {
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
    fn test_matches_binary_pattern_rejects_custom_prefix() {
        let param = BinaryBaseParameter::new("v2/SmileFrown", |_| 0.0);
        assert_eq!(
            param.matches_binary_pattern("/avatar/parameters/Custom/v2/SmileFrown1"),
            None
        );
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
