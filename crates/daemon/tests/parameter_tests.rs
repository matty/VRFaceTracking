//! Parameter system tests
//!
//! Tests for base parameters, address matching, and fallback behavior.

use std::collections::{HashMap, HashSet};
use vrft_common::UnifiedTrackingData;
use vrft_daemon::osc::parameters::base_param::{BoolParam, FloatParam};
use vrft_daemon::osc::parameters::{ParamType, Parameter};

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
    use vrft_daemon::osc::parameters::legacy_eye::create_legacy_eye_parameters;
    use vrft_daemon::osc::parameters::legacy_lip::create_legacy_lip_parameters;

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
    use rosc::OscType;
    use std::collections::{HashMap, HashSet};
    use vrft_common::UnifiedTrackingData;
    use vrft_daemon::osc::parameters::binary_param::BinaryBaseParameter;
    use vrft_daemon::osc::parameters::{ParamType, Parameter};

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

        assert_eq!(param.reset(&avatar_params, &param_types), 3);
        assert_eq!(param.max_binary_int, 8);
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

        // Value 0.75 * 4 (2 bits = 4 steps) = 3 = 0b11, so both bits are true
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|m| m.args == vec![OscType::Bool(true)]));
    }
}

mod avatar_change {
    use std::collections::{HashMap, HashSet};
    use vrft_common::UnifiedTrackingData;
    use vrft_daemon::osc::parameters::base_param::{BoolParam, FloatParam};
    use vrft_daemon::osc::parameters::{ParamType, Parameter};

    fn avatar(addr: &str, t: ParamType) -> (HashSet<String>, HashMap<String, ParamType>) {
        let addr = format!("/avatar/parameters/{addr}");
        (
            [addr.clone()].into_iter().collect(),
            [(addr, t)].into_iter().collect(),
        )
    }

    /// An unchanged value is quiet until the avatar changes, then it is sent
    /// again so the new avatar starts from the current state.
    #[test]
    fn float_param_resends_after_avatar_change() {
        let mut param = FloatParam::new("v2/Test", |_| 0.5);
        let (params, types) = avatar("v2/Test", ParamType::Float);
        let data = UnifiedTrackingData::default();

        param.reset(&params, &types);
        assert_eq!(param.process(&data).len(), 2);
        assert!(param.process(&data).is_empty());

        param.reset(&params, &types);
        assert_eq!(param.process(&data).len(), 2);
    }

    #[test]
    fn bool_param_resends_after_avatar_change() {
        let mut param = BoolParam::new("ExpressionTrackingActive", |_| false);
        let (params, types) = avatar("ExpressionTrackingActive", ParamType::Bool);
        let data = UnifiedTrackingData::default();

        param.reset(&params, &types);
        assert_eq!(param.process(&data).len(), 2);
        assert!(param.process(&data).is_empty());

        param.reset(&params, &types);
        assert_eq!(param.process(&data).len(), 2);
    }
}

mod non_finite_values {
    use rosc::OscType;
    use std::collections::{HashMap, HashSet};
    use vrft_common::UnifiedTrackingData;
    use vrft_daemon::osc::parameters::base_param::FloatParam;
    use vrft_daemon::osc::parameters::{ParamType, Parameter};

    fn param() -> FloatParam {
        let mut param = FloatParam::new("v2/Test", |d| d.head.head_yaw);
        let addr = "/avatar/parameters/FT/v2/Test".to_string();
        let params: HashSet<String> = [addr.clone()].into_iter().collect();
        let types: HashMap<String, ParamType> = [(addr, ParamType::Float)].into_iter().collect();
        param.reset(&params, &types);
        param
    }

    fn frame(yaw: f32) -> UnifiedTrackingData {
        let mut data = UnifiedTrackingData::default();
        data.head.head_yaw = yaw;
        data
    }

    /// Nothing compares as changed from NaN, so a NaN first value used to be
    /// remembered and the parameter never sent again.
    #[test]
    fn nan_first_value_does_not_freeze_the_parameter() {
        let mut param = param();

        assert!(
            param.process(&frame(f32::NAN)).is_empty(),
            "NaN is not sent"
        );

        let messages = param.process(&frame(0.5));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].args, vec![OscType::Float(0.5)]);
        assert!(param.process(&frame(0.5)).is_empty());
    }

    #[test]
    fn non_finite_values_are_skipped_and_the_last_value_kept() {
        let mut param = param();

        assert_eq!(param.process(&frame(0.25)).len(), 1);
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(param.process(&frame(v)).is_empty(), "{v} is not sent");
        }
        assert!(
            param.process(&frame(0.25)).is_empty(),
            "0.25 was already sent and is still current"
        );
        assert_eq!(param.process(&frame(0.75)).len(), 1);
    }
}
