use api::{UnifiedExpressionShape, UnifiedExpressions, UnifiedTrackingData};
use common::{MutationConfig, UnifiedTrackingMutator};

#[test]
fn test_continuous_calibration_updates_and_applies() {
    let config = MutationConfig {
        calibration_enabled: true,
        calibration_continuous: true,
        mutator_enabled: true,
        ..Default::default()
    };

    let mut mutator = UnifiedTrackingMutator::new(config);

    let mut data = UnifiedTrackingData {
        shapes: vec![UnifiedExpressionShape::default(); UnifiedExpressions::Max as usize],
        ..Default::default()
    };

    // Feed a sequence that forces multiple accepted samples (toggle crosses step threshold).
    for i in 0..7 {
        data.shapes[0].weight = if i % 2 == 0 { 1.0 } else { 0.0 };
        mutator.mutate(&mut data, 0.01);
    }

    // Progress should have advanced (1/64 increments per accepted sample).
    assert!(mutator.calibration_manager.data.shapes[0].progress > 0.0);

    // After enough accepted samples, progress should have become non-zero.
    assert!(mutator.calibration_manager.data.shapes[0].progress > 0.0);

    // Now verify that calibration is being applied (output is not identical to raw).
    data.shapes[0].weight = 0.4;
    mutator.mutate(&mut data, 0.01);
    assert!(
        (data.shapes[0].weight - 0.4).abs() > 1e-6,
        "Expected calibrated output to differ from raw once confidence is non-zero"
    );
}
