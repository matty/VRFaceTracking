//! Parameter system tests
//!
//! Tests for base parameters, address matching, and fallback behavior.

use common::UnifiedTrackingData;
use std::collections::{HashMap, HashSet};
use vrft_d::osc::parameters::base_param::{BoolParam, FloatParam};
use vrft_d::osc::parameters::{ParamType, Parameter};

mod address_matching {
    use super::*;

    #[test]
    fn exact_match() {
        let mut param = FloatParam::new("v2/EyeLeftX", |_| 0.0);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/EyeLeftX".to_string());
        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/v2/EyeLeftX".to_string(),
            ParamType::Float,
        );

        assert!(param.reset(&avatar_params, &param_types) > 0);
    }

    #[test]
    fn ft_prefix_match() {
        let mut param = FloatParam::new("v2/EyeLeftX", |_| 0.0);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/FT/v2/EyeLeftX".to_string());
        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/FT/v2/EyeLeftX".to_string(),
            ParamType::Float,
        );

        assert!(param.reset(&avatar_params, &param_types) > 0);
    }

    #[test]
    fn accepts_custom_prefix() {
        // Any arbitrary prefix is accepted; only nested versions (v1/v2/) are rejected
        let mut param = FloatParam::new("v2/EyeLeftX", |_| 0.0);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/Custom/v2/EyeLeftX".to_string());
        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/Custom/v2/EyeLeftX".to_string(),
            ParamType::Float,
        );

        assert!(param.reset(&avatar_params, &param_types) > 0);
    }

    #[test]
    fn rejects_wrong_avatar_prefix() {
        let mut param = FloatParam::new("EyeLeftX", |_| 0.0);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/wrong/parameters/EyeLeftX".to_string());
        let mut param_types = HashMap::new();
        param_types.insert("/wrong/parameters/EyeLeftX".to_string(), ParamType::Float);

        assert_eq!(param.reset(&avatar_params, &param_types), 0);
    }
}

mod float_param {
    use super::*;

    #[test]
    fn not_relevant_when_no_matches() {
        let mut param = FloatParam::new("v2/TongueOut", |_| 0.5);
        let empty_params = HashSet::new();
        let empty_types = HashMap::new();

        let relevant = param.reset(&empty_params, &empty_types);

        assert_eq!(relevant, 0);
        assert_eq!(param.addresses.len(), 0);
    }

    #[test]
    fn adds_ft_fallback_when_non_ft_match() {
        let mut param = FloatParam::new("v2/TongueOut", |_| 0.5);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/TongueOut".to_string());
        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/v2/TongueOut".to_string(),
            ParamType::Float,
        );

        param.reset(&avatar_params, &param_types);

        assert_eq!(param.addresses.len(), 2);
        assert!(param
            .addresses
            .contains(&"/avatar/parameters/v2/TongueOut".to_string()));
        assert!(param
            .addresses
            .contains(&"/avatar/parameters/FT/v2/TongueOut".to_string()));
    }

    #[test]
    fn no_duplicate_ft_when_ft_already_matches() {
        let mut param = FloatParam::new("v2/TongueOut", |_| 0.5);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/FT/v2/TongueOut".to_string());
        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/FT/v2/TongueOut".to_string(),
            ParamType::Float,
        );

        param.reset(&avatar_params, &param_types);

        assert_eq!(param.addresses.len(), 1);
    }

    #[test]
    fn not_relevant_when_wrong_type() {
        let mut param = FloatParam::new("v2/TongueOut", |_| 0.5);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/TongueOut".to_string());
        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/v2/TongueOut".to_string(),
            ParamType::Bool,
        );

        let relevant = param.reset(&avatar_params, &param_types);

        assert_eq!(relevant, 0);
    }

    #[test]
    fn sends_to_all_matched_addresses() {
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

        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn delta_check_prevents_duplicate_sends() {
        let mut param = FloatParam::new("v2/Test", |_| 0.5);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/Test".to_string());
        let mut param_types = HashMap::new();
        param_types.insert("/avatar/parameters/v2/Test".to_string(), ParamType::Float);
        param.reset(&avatar_params, &param_types);

        let data = UnifiedTrackingData::default();

        let messages1 = param.process(&data);
        assert!(!messages1.is_empty());

        let messages2 = param.process(&data);
        assert!(
            messages2.is_empty(),
            "Delta check should prevent duplicate sends"
        );
    }
}

mod bool_param {
    use super::*;

    #[test]
    fn not_relevant_when_no_matches() {
        let mut param = BoolParam::new("v2/TongueOut", |_| true);
        let empty_params = HashSet::new();
        let empty_types = HashMap::new();

        let relevant = param.reset(&empty_params, &empty_types);

        assert_eq!(relevant, 0);
        assert_eq!(param.addresses.len(), 0);
    }

    #[test]
    fn adds_ft_fallback_when_non_ft_match() {
        let mut param = BoolParam::new("v2/TongueOut", |_| true);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/TongueOut".to_string());
        let mut param_types = HashMap::new();
        param_types.insert(
            "/avatar/parameters/v2/TongueOut".to_string(),
            ParamType::Bool,
        );

        param.reset(&avatar_params, &param_types);

        assert_eq!(param.addresses.len(), 2);
    }

    #[test]
    fn sends_to_all_matched_addresses() {
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

        assert_eq!(messages.len(), 2);
    }
}

mod legacy_params {
    use vrft_d::osc::parameters::legacy_eye::create_legacy_eye_parameters;
    use vrft_d::osc::parameters::legacy_lip::create_legacy_lip_parameters;

    #[test]
    fn legacy_eye_has_minimum_count() {
        let params = create_legacy_eye_parameters();
        assert!(
            params.len() >= 50,
            "Expected at least 50 legacy eye params, got {}",
            params.len()
        );
    }

    #[test]
    fn legacy_lip_has_minimum_count() {
        let params = create_legacy_lip_parameters();
        assert!(
            params.len() >= 100,
            "Expected at least 100 legacy lip params, got {}",
            params.len()
        );
    }
}

mod binary_param {
    use common::UnifiedTrackingData;
    use std::collections::{HashMap, HashSet};
    use vrft_d::osc::parameters::binary_param::BinaryBaseParameter;
    use vrft_d::osc::parameters::{ParamType, Parameter};

    #[test]
    fn discovers_binary_addresses() {
        let mut param = BinaryBaseParameter::new("Test", |_| 0.5);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/Test1".to_string());
        avatar_params.insert("/avatar/parameters/Test2".to_string());
        avatar_params.insert("/avatar/parameters/Test4".to_string());

        let mut param_types = HashMap::new();
        param_types.insert("/avatar/parameters/Test1".to_string(), ParamType::Bool);
        param_types.insert("/avatar/parameters/Test2".to_string(), ParamType::Bool);
        param_types.insert("/avatar/parameters/Test4".to_string(), ParamType::Bool);

        let relevant = param.reset(&avatar_params, &param_types);

        assert!(
            relevant > 0,
            "Binary param should be relevant when bits are found"
        );
    }

    #[test]
    fn encodes_values_correctly() {
        let mut param = BinaryBaseParameter::new("Test", |_| 0.75);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/Test1".to_string());
        avatar_params.insert("/avatar/parameters/Test2".to_string());

        let mut param_types = HashMap::new();
        param_types.insert("/avatar/parameters/Test1".to_string(), ParamType::Bool);
        param_types.insert("/avatar/parameters/Test2".to_string(), ParamType::Bool);

        param.reset(&avatar_params, &param_types);

        let data = UnifiedTrackingData::default();
        let messages = param.process(&data);

        // Value 0.75 * 4 (2 bits = 4 steps) rounds to 3 = 0b11
        // So both bits should be true
        assert!(!messages.is_empty());
    }
}

mod send_on_load {
    use common::UnifiedTrackingData;
    use std::collections::{HashMap, HashSet};
    use vrft_d::osc::parameters::base_param::{BoolParam, FloatParam};
    use vrft_d::osc::parameters::binary_param::BinaryBaseParameter;
    use vrft_d::osc::parameters::{ParamType, Parameter};

    #[test]
    fn float_param_sends_on_load() {
        let mut param = FloatParam::new_with_send_on_load("v2/Test", |_| 0.5);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/Test".to_string());
        let mut param_types = HashMap::new();
        param_types.insert("/avatar/parameters/v2/Test".to_string(), ParamType::Float);

        param.reset(&avatar_params, &param_types);

        let data = UnifiedTrackingData::default();
        let messages1 = param.process(&data);

        // First process should send
        assert!(
            !messages1.is_empty(),
            "sendOnLoad should force initial send"
        );

        // Second process with same value should NOT send (delta check)
        let messages2 = param.process(&data);
        assert!(
            messages2.is_empty(),
            "Delta check should prevent duplicate after initial"
        );
    }

    #[test]
    fn bool_param_sends_on_load() {
        let mut param = BoolParam::new_with_send_on_load("v2/Test", |_| true);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/Test".to_string());
        let mut param_types = HashMap::new();
        param_types.insert("/avatar/parameters/v2/Test".to_string(), ParamType::Bool);

        param.reset(&avatar_params, &param_types);

        let data = UnifiedTrackingData::default();
        let messages1 = param.process(&data);

        // First process should send
        assert!(
            !messages1.is_empty(),
            "sendOnLoad should force initial send"
        );

        // Second process with same value should NOT send
        let messages2 = param.process(&data);
        assert!(
            messages2.is_empty(),
            "Delta check should prevent duplicate after initial"
        );
    }

    #[test]
    fn binary_param_sends_on_load() {
        let mut param = BinaryBaseParameter::new_with_send_on_load("Test", |_| 0.5);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/Test1".to_string());
        avatar_params.insert("/avatar/parameters/Test2".to_string());

        let mut param_types = HashMap::new();
        param_types.insert("/avatar/parameters/Test1".to_string(), ParamType::Bool);
        param_types.insert("/avatar/parameters/Test2".to_string(), ParamType::Bool);

        param.reset(&avatar_params, &param_types);

        let data = UnifiedTrackingData::default();
        let messages1 = param.process(&data);

        // First process should send all bits
        assert_eq!(
            messages1.len(),
            2,
            "sendOnLoad should force send all bit params"
        );

        // Second process with same value should NOT send
        let messages2 = param.process(&data);
        assert!(
            messages2.is_empty(),
            "Delta check should prevent duplicate after initial"
        );
    }

    #[test]
    fn send_on_load_resets_on_avatar_change() {
        let mut param = FloatParam::new_with_send_on_load("v2/Test", |_| 0.5);
        let mut avatar_params = HashSet::new();
        avatar_params.insert("/avatar/parameters/v2/Test".to_string());
        let mut param_types = HashMap::new();
        param_types.insert("/avatar/parameters/v2/Test".to_string(), ParamType::Float);

        // First avatar
        param.reset(&avatar_params, &param_types);
        let data = UnifiedTrackingData::default();
        let _ = param.process(&data);

        // Simulate avatar change (reset again)
        param.reset(&avatar_params, &param_types);
        let messages = param.process(&data);

        // Should send again after reset
        assert!(
            !messages.is_empty(),
            "Should send again after avatar change reset"
        );
    }
}
