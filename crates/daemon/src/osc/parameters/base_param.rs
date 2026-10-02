//! Float and bool parameter types with relevancy tracking and delta checking.

use super::{ends_with_version_segment, ParamType, Parameter, DEFAULT_PREFIX};
use rosc::{OscMessage, OscType};
use std::collections::{HashMap, HashSet};
use vrft_common::UnifiedTrackingData;

/// Matches parameter addresses with flexible prefix support.
///
/// This matches:
/// - Exact parameter name after stripping `/avatar/parameters/`
/// - Any address ending with `/{name}` (e.g., `FT/`, `OSCm/Float/FT/`, custom prefixes)
///
/// Rejects nested version prefixes (e.g., `/v1/v2/EyeLeftX`)
pub(crate) fn matches_address(name: &str, addr: &str) -> bool {
    let Some(stripped) = addr.strip_prefix(DEFAULT_PREFIX) else {
        return false;
    };

    if stripped == name {
        return true;
    }

    // Suffix match: "{prefix}/{name}"
    stripped
        .strip_suffix(name)
        .and_then(|rest| rest.strip_suffix('/'))
        .is_some_and(|prefix| !ends_with_version_segment(prefix))
}

/// A value a [`BaseParam`] can carry.
pub trait ParamValue: Copy + Send + Sync + 'static {
    /// The avatar parameter type this value is sent to.
    const TYPE: ParamType;

    fn to_osc(self) -> OscType;

    /// Whether this value differs enough from the last one sent to send again.
    fn changed(self, last: Self) -> bool;

    /// Whether this value may be sent at all.
    fn sendable(self) -> bool {
        true
    }
}

impl ParamValue for f32 {
    const TYPE: ParamType = ParamType::Float;

    fn to_osc(self) -> OscType {
        OscType::Float(self)
    }

    fn changed(self, last: Self) -> bool {
        (self - last).abs() > 0.00001
    }

    /// A NaN or infinity is never sent or remembered. Remembering one froze
    /// the parameter for good, since nothing compares as changed from NaN, and
    /// sending one hands the avatar's animator a value it cannot use. The last
    /// finite value stays in place until a usable one arrives.
    fn sendable(self) -> bool {
        self.is_finite()
    }
}

impl ParamValue for bool {
    const TYPE: ParamType = ParamType::Bool;

    fn to_osc(self) -> OscType {
        OscType::Bool(self)
    }

    fn changed(self, last: Self) -> bool {
        self != last
    }
}

/// Parameter with relevancy tracking, sent to every matching avatar address
pub struct BaseParam<T: ParamValue> {
    pub name: String,
    pub addresses: Vec<String>,
    pub relevant: bool,
    get_value: Box<dyn Fn(&UnifiedTrackingData) -> T + Send + Sync>,
    last_value: Option<T>,
}

/// Float parameter with relevancy tracking
pub type FloatParam = BaseParam<f32>;

/// Bool parameter with relevancy tracking
pub type BoolParam = BaseParam<bool>;

impl<T: ParamValue> BaseParam<T> {
    pub fn new(
        name: &str,
        get_value: impl Fn(&UnifiedTrackingData) -> T + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            addresses: vec![format!("{}{}", DEFAULT_PREFIX, name)],
            relevant: false,
            get_value: Box::new(get_value),
            last_value: None,
        }
    }
}

impl<T: ParamValue> Parameter for BaseParam<T> {
    fn reset(
        &mut self,
        avatar_params: &HashSet<String>,
        param_types: &HashMap<String, ParamType>,
    ) -> usize {
        // Forgetting the last value is what makes the first process() after
        // an avatar change send, whatever the value is.
        self.last_value = None;

        self.addresses = avatar_params
            .iter()
            .filter(|addr| {
                matches_address(&self.name, addr)
                    && param_types.get(*addr).is_none_or(|t| *t == T::TYPE)
            })
            .cloned()
            .collect();
        self.relevant = !self.addresses.is_empty();

        // Add /FT/ fallback if not already present
        if self.relevant && !self.addresses.iter().any(|a| a.contains("/FT/")) {
            self.addresses
                .push(format!("{}FT/{}", DEFAULT_PREFIX, self.name));
        }

        usize::from(self.relevant)
    }

    fn process(&mut self, data: &UnifiedTrackingData) -> Vec<OscMessage> {
        if !self.relevant {
            return vec![];
        }

        let value = (self.get_value)(data);

        // Delta check
        if !value.sendable() || self.last_value.is_some_and(|last| !value.changed(last)) {
            return vec![];
        }

        self.last_value = Some(value);

        self.addresses
            .iter()
            .map(|addr| OscMessage {
                addr: addr.clone(),
                args: vec![value.to_osc()],
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

    /// Only a whole `v{digits}` segment marks a nested version. The old check
    /// read the last two characters, so it rejected `Dev2/` and let `v10/` in.
    #[test]
    fn test_matches_address_version_check_is_per_segment() {
        assert!(matches_address(
            "JawOpen",
            "/avatar/parameters/Dev2/JawOpen"
        ));
        assert!(matches_address(
            "v2/JawOpen",
            "/avatar/parameters/Av3/v2/JawOpen"
        ));
        assert!(!matches_address(
            "v2/EyeLeftX",
            "/avatar/parameters/v10/v2/EyeLeftX"
        ));
        assert!(!matches_address("JawOpen", "/avatar/parameters/v2/JawOpen"));
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
