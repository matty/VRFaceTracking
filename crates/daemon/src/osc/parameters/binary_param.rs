//! Binary parameter encoding with dynamic bit discovery and delta checking.

use super::{ends_with_version_segment, ParamType, Parameter, DEFAULT_PREFIX};
use rosc::{OscMessage, OscType};
use std::collections::{HashMap, HashSet};
use vrft_common::UnifiedTrackingData;

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
        if ends_with_version_segment(&stripped[..idx]) {
            return suffixes;
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
    /// Every `{name}Negative` address the avatar has; each gets the sign.
    pub negative_params: Vec<String>,
    pub max_binary_int: u32,
    pub relevant: bool,
    get_value: Box<dyn Fn(&UnifiedTrackingData) -> f32 + Send + Sync>,
    last_bits: HashMap<String, bool>,
}

impl BinaryBaseParameter {
    pub fn new(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> f32 + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            bit_params: Vec::new(),
            negative_params: Vec::new(),
            max_binary_int: 0,
            relevant: false,
            get_value: Box::new(get_value),
            last_bits: HashMap::new(),
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

        if self.negative_params.is_empty() && val < 0.0 {
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
        // Forgetting the last bits is what makes the first process() after an
        // avatar change send every bit, whatever the value is.
        self.bit_params.clear();
        self.last_bits.clear();

        // Find the negative param addresses, applying the same matching rules
        // as bit discovery so a parameter whose name merely ends with this one
        // cannot claim them. An avatar can carry several (`XNegative` and
        // `FT/XNegative`), and every one of them needs the sign.
        self.negative_params = avatar_params
            .iter()
            .filter(|a| {
                self.matches_negative_pattern(a)
                    && param_types.get(*a).is_some_and(|t| *t == ParamType::Bool)
            })
            .cloned()
            .collect();
        self.negative_params.sort();

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
            // No binary bits, but a negative param still makes it relevant
            self.relevant = !self.negative_params.is_empty();
            return self.negative_params.len();
        }

        // Scale by distinct bits, not addresses: `X1` and `FT/X1` are the same
        // bit sent twice and must not widen the range.
        let distinct_bits: HashSet<usize> = params_to_create.values().copied().collect();
        self.max_binary_int = 2u32.pow(distinct_bits.len() as u32);
        self.bit_params = params_to_create.into_iter().collect();
        self.bit_params.sort_by_key(|(_, shift)| *shift);

        log::debug!(
            "BinaryParam '{}': {} bit params",
            self.name,
            self.bit_params.len()
        );

        self.relevant = true;

        self.bit_params.len() + self.negative_params.len()
    }

    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage> {
        if !self.relevant {
            return vec![];
        }

        let value = (self.get_value)(data);
        let mut messages = Vec::new();

        for addr in &self.negative_params {
            send_if_changed(&mut self.last_bits, &mut messages, addr, value < 0.0);
        }

        for (addr, shift_index) in &self.bit_params {
            let bit_value = self.process_binary(value, *shift_index);
            send_if_changed(&mut self.last_bits, &mut messages, addr, bit_value);
        }

        messages
    }
}

/// Queues `bit` for `addr` unless it is the value last sent there.
fn send_if_changed(
    last_bits: &mut HashMap<String, bool>,
    messages: &mut Vec<OscMessage>,
    addr: &str,
    bit: bool,
) {
    if last_bits.get(addr) == Some(&bit) {
        return;
    }
    last_bits.insert(addr.to_string(), bit);
    messages.push(OscMessage {
        addr: addr.to_string(),
        args: vec![OscType::Bool(bit)],
    });
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
        assert!(param.negative_params.is_empty());

        let mut bool_types = HashMap::new();
        bool_types.insert(addr.clone(), ParamType::Bool);
        assert_eq!(param.reset(&avatar_params, &bool_types), 1);
        assert_eq!(param.negative_params, vec![addr]);
    }

    fn bool_avatar(addrs: &[&str]) -> (HashSet<String>, HashMap<String, ParamType>) {
        let set: HashSet<String> = addrs
            .iter()
            .map(|a| format!("/avatar/parameters/{a}"))
            .collect();
        let types = set.iter().map(|a| (a.clone(), ParamType::Bool)).collect();
        (set, types)
    }

    /// `X1` and `FT/X1` are one bit at two addresses. Counting addresses made
    /// the range 2^4 instead of 2^2, so 0.5 came out as 8 and read as zero in
    /// the two bits that exist.
    #[test]
    fn duplicate_addresses_do_not_widen_the_range() {
        let mut param = BinaryBaseParameter::new("X", |_| 0.5);
        let (set, types) = bool_avatar(&["X1", "X2", "FT/X1", "FT/X2"]);
        assert_eq!(param.reset(&set, &types), 4);
        assert_eq!(param.max_binary_int, 4);

        // 0.5 * 4 = 2 = 0b10: bit 2 on, bit 1 off, at both addresses
        let mut sent: Vec<(String, bool)> = param
            .process(&UnifiedTrackingData::default())
            .into_iter()
            .map(|m| (m.addr, m.args[0] == OscType::Bool(true)))
            .collect();
        sent.sort();
        assert_eq!(
            sent,
            vec![
                ("/avatar/parameters/FT/X1".to_string(), false),
                ("/avatar/parameters/FT/X2".to_string(), true),
                ("/avatar/parameters/X1".to_string(), false),
                ("/avatar/parameters/X2".to_string(), true),
            ]
        );
    }

    /// Only one arbitrary `Negative` address used to get the sign; an avatar
    /// reading the other one never saw the value go negative.
    #[test]
    fn every_negative_address_gets_the_sign() {
        let mut param = BinaryBaseParameter::new("X", |_| -0.5);
        let (set, types) = bool_avatar(&["X1", "XNegative", "FT/XNegative"]);
        assert_eq!(param.reset(&set, &types), 3);

        let negatives: Vec<_> = param
            .process(&UnifiedTrackingData::default())
            .into_iter()
            .filter(|m| m.addr.ends_with("Negative"))
            .collect();
        assert_eq!(negatives.len(), 2);
        assert!(negatives.iter().all(|m| m.args[0] == OscType::Bool(true)));
    }

    #[test]
    fn first_process_after_reset_sends_every_bit() {
        let mut param = BinaryBaseParameter::new("X", |_| 0.0);
        let (set, types) = bool_avatar(&["X1", "X2", "XNegative"]);
        let data = UnifiedTrackingData::default();

        param.reset(&set, &types);
        assert_eq!(param.process(&data).len(), 3);
        assert!(
            param.process(&data).is_empty(),
            "unchanged bits are not resent"
        );

        param.reset(&set, &types);
        assert_eq!(
            param.process(&data).len(),
            3,
            "an avatar change sends again"
        );
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
            negative_params: Vec::new(),
            max_binary_int: 16,
            relevant: true,
            get_value: Box::new(|_| 0.5),
            last_bits: HashMap::new(),
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
