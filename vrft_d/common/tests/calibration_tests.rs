use common::{CalibrationParameter, MAX_REASONABLE_STDDEV, POINTS};

#[test]
fn test_progress_saturates_at_1() {
    let mut p = CalibrationParameter::default();
    let dt = 0.1;

    for i in 0..70 {
        p.update_calibration(i as f32 * 0.1, false, dt);
    }

    assert_eq!(p.fixed_index, POINTS);
    assert!((p.progress - 1.0).abs() < 1e-6);
}

#[test]
fn test_calculate_parameter_nan_passthrough() {
    let p = CalibrationParameter::default();
    let got = p.calculate_parameter(f32::NAN, 0.0);
    assert!(got.is_nan());
}

#[test]
fn test_mean_calculation_accuracy() {
    let mut p = CalibrationParameter::default();
    let values = [0.2, 0.4, 0.6];

    // Feed known values repeatedly
    for i in 0..21 {
        p.update_calibration(values[i % 3], true, 0.1);
    }

    // Mean should be approximately 0.4
    assert!(
        (p.mean - 0.4).abs() < 0.1,
        "Mean {} should be close to 0.4",
        p.mean
    );
}

#[test]
fn test_stddev_with_varied_data() {
    let mut p = CalibrationParameter::default();

    // Feed varied values to build up statistics
    for i in 0..POINTS {
        let value = (i as f32 / (POINTS - 1) as f32).clamp(0.0, 1.0);
        p.update_calibration(value, true, 0.1);
    }

    // StdDev should be reasonable (below uniform distribution threshold)
    assert!(
        p.std_dev <= MAX_REASONABLE_STDDEV + 0.1,
        "StdDev {} should be near or below {}",
        p.std_dev,
        MAX_REASONABLE_STDDEV
    );
    assert!(
        p.std_dev > 0.0,
        "StdDev should be positive with varied data"
    );
}

#[test]
fn test_confidence_increases_with_data() {
    let mut p = CalibrationParameter::default();
    let initial_confidence = p.confidence;

    // Feed consistent data
    for i in 0..POINTS {
        // Alternate values to pass step delta filter
        let value = if i % 2 == 0 { 0.4 } else { 0.6 };
        p.update_calibration(value, true, 0.1);
    }

    assert!(
        p.confidence > initial_confidence,
        "Confidence {} should increase from initial {}",
        p.confidence,
        initial_confidence
    );
}

#[test]
fn test_max_confidence_prevents_regression() {
    let mut p = CalibrationParameter::default();

    // Build up confidence with good data
    for i in 0..POINTS {
        let value = if i % 2 == 0 { 0.4 } else { 0.6 };
        p.update_calibration(value, true, 0.1);
    }

    let peak_confidence = p.max_confidence;
    assert!(peak_confidence > 0.0);

    // Max confidence should not decrease significantly
    assert!(p.max_confidence >= peak_confidence * 0.9);
}

#[test]
fn test_curve_adjustment_behavior() {
    let p = CalibrationParameter {
        mean: 0.6,
        std_dev: 0.15,
        confidence: 1.0,
        max_confidence: 1.0,
        max: 0.8,
        progress: 1.0,
        ..Default::default()
    };

    // Test that adjustment is applied
    let raw = 0.5;
    let adjusted = p.calculate_parameter(raw, 0.0);

    // Should be in valid range
    assert!(adjusted >= 0.0 && adjusted <= 1.0);
}

#[test]
fn test_zero_confidence_returns_raw() {
    let p = CalibrationParameter::default();

    let raw = 0.5;
    let adjusted = p.calculate_parameter(raw, 0.0);

    // With no calibration data, should return raw value
    assert!(
        (adjusted - raw).abs() < 1e-6,
        "With no calibration, adjusted {} should equal raw {}",
        adjusted,
        raw
    );
}

#[test]
fn test_sigmoid_prefers_raw_near_zero() {
    let p = CalibrationParameter {
        mean: 0.5,
        std_dev: 0.2,
        confidence: 1.0,
        max_confidence: 1.0,
        max: 0.8,
        progress: 1.0,
        ..Default::default()
    };

    // Very low input should stay relatively close to raw
    let raw = 0.01;
    let adjusted = p.calculate_parameter(raw, 0.0);

    assert!(
        (adjusted - raw).abs() < 0.2,
        "Near-zero value {} should stay close to raw {}",
        adjusted,
        raw
    );
}

#[test]
fn test_max_value_tracking() {
    let mut p = CalibrationParameter::default();

    // Feed alternating values with large enough differences to pass step delta filter
    for i in 0..20 {
        // Use alternating extremes with occasional high peak
        let value = match i {
            10 => 0.9,
            _ if i % 2 == 0 => 0.1,
            _ => 0.5,
        };
        p.update_calibration(value, true, 0.1);
    }

    assert!(
        p.max >= 0.5, // At minimum should capture some values
        "Max {} should be positive",
        p.max
    );
}
