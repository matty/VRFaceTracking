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
    assert!(!is_calibrating, "Calibration should not start when disabled");
    
    // manual check of state
    match mutator.calibration_state {
        CalibrationState::Uncalibrated => {},
        _ => panic!("State should be Uncalibrated"),
    }

    // Test 2: Switch Profile should be ignored
    // We can check if the profile ID changes. Default is typically "default".
    // Note: The new profile ID we try to switch to should be different.
    let initial_profile = mutator.calibration_manager.current_profile_id.clone();
    let _ = mutator.switch_profile("new_profile");
    assert_eq!(mutator.calibration_manager.current_profile_id, initial_profile, "Profile switching should be disabled");

    // Test 3: Load Calibration should be ignored
    // This is harder to test without a file, but the function returns early so it shouldn't error on missing file if it returns early.
    // If it didn't return early, it would try to load "default" profile from disk and likely fail if we don't set up the env, 
    // or succeed if it finds one. But purely based on logic, we expect it to return Ok(()) immediately.
    let result = mutator.load_calibration(&PathBuf::from("non_existent_path"));
    assert!(result.is_ok(), "Load calibration should return Ok (ignored) when disabled");
}

#[test]
fn test_calibration_enabled_behavior_control() {
    // Control test to ensure our test setup works and strict mode isn't ALWAYS on for some reason
    let mut config = MutationConfig::default();
    config.calibration.enabled = true;

    // We need to use a temp dir for storage to avoid messing with real files during tests?
    // UnifiedTrackingMutator::new hardcodes "." as path in the current impl (line 187 of mutator.rs).
    // This makes it a bit messy for unit tests writing files. 
    // Ideally we'd modify new() to take a path or use a temp dir.
    // For now, we just test start_calibration which is in-memory.

    let mut mutator = UnifiedTrackingMutator::new(config);

    mutator.start_calibration(10.0);
    let (is_calibrating, _, _, _) = mutator.calibration_status();
    assert!(is_calibrating, "Calibration SHOULD start when enabled");
}
