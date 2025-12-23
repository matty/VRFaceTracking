use common::CalibrationParameter;

#[test]
fn test_step_gating_and_progress() {
    let mut p = CalibrationParameter::new();
    let dt = 0.1;

    p.update_calibration(0.01, false, dt);
    assert_eq!(p.samples_seen(), 1);
    assert!((p.progress - (1.0 / 64.0)).abs() < 1e-6);

    p.update_calibration(0.02, false, dt);
    assert_eq!(p.samples_seen(), 1);

    p.update_calibration(0.03, false, dt);
    assert_eq!(p.samples_seen(), 2);
}

#[test]
fn test_progress_saturates_at_1() {
    let mut p = CalibrationParameter::new();
    let dt = 0.1;

    for i in 0..70 {
        p.update_calibration(i as f32 * 0.1, false, dt);
    }

    assert_eq!(p.samples_seen(), 64);
    assert!((p.progress - 1.0).abs() < 1e-6);
}

#[test]
fn test_calculate_parameter_matches_formula() {
    let mut p = CalibrationParameter::new();
    p.max = 0.8;
    p.progress = 1.0;

    let current_value: f32 = 0.4;
    let k = 1.0;

    let confidence = k * p.progress;
    let expected = confidence * (current_value / p.max) + (1.0 - confidence) * current_value;

    let got = p.calculate_parameter(current_value, k);
    assert!((got - expected).abs() < 1e-6);
}

#[test]
fn test_calculate_parameter_nan_passthrough() {
    let p = CalibrationParameter::new();
    let got = p.calculate_parameter(f32::NAN, 0.0);
    assert!(got.is_nan());
}
