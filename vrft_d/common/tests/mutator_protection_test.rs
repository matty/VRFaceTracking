use common::{CalibrationState, MutationConfig, UnifiedTrackingMutator};
use std::path::PathBuf;

#[test]
fn test_calibration_disabled_behavior() {
    let mut config = MutationConfig::default();
    config.calibration.enabled = false;

    let mut mutator = UnifiedTrackingMutator::new(config);

    // Test 1: Start Calibration should be ignored
    mutator.start_calibration(10.0);
    let (is_calibrating, _, _, _) = mutator.calibration_status();
    assert!(
        !is_calibrating,
        "Calibration should not start when disabled"
    );

    match mutator.get_calibration_state() {
        CalibrationState::Uncalibrated => {}
        _ => panic!("State should be Uncalibrated"),
    }

    // Test 2: Load Calibration should be ignored when disabled
    let result = mutator.load_calibration(&PathBuf::from("non_existent_path"));
    assert!(
        result.is_ok(),
        "Load calibration should return Ok (ignored) when disabled"
    );
}

#[test]
fn test_calibration_enabled_behavior_control() {
    let mut config = MutationConfig::default();
    config.calibration.enabled = true;

    let mut mutator = UnifiedTrackingMutator::new(config);

    mutator.start_calibration(10.0);
    let (is_calibrating, _, _, _) = mutator.calibration_status();
    assert!(is_calibrating, "Calibration SHOULD start when enabled");
}
