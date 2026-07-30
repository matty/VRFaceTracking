use vrft_common::EuroFilter;

// Simulated dt at 60fps
const DT_60FPS: f32 = 1.0 / 60.0;

#[test]
fn test_euro_filter_initialization() {
    let mut filter = EuroFilter::new();
    let first_val = 100.0;
    let filtered = filter.filter(first_val, DT_60FPS);
    assert_eq!(
        filtered, first_val,
        "First value should be passed through exactly"
    );
}

#[test]
fn test_euro_filter_derivative_no_spike() {
    let mut filter = EuroFilter::new();
    // First value
    filter.filter(0.0, DT_60FPS);

    // Second value
    let val2 = 1.0;
    let filtered2 = filter.filter(val2, DT_60FPS);

    // Just ensure it's not NaN and is reasonable.
    assert!(filtered2 > 0.0);
    assert!(filtered2 <= 1.0);
}

#[test]
fn test_euro_filter_nan_handling() {
    let mut filter = EuroFilter::new();
    let res = filter.filter(f32::NAN, DT_60FPS);
    assert_eq!(res, 0.0);
}

#[test]
fn test_euro_filter_recovers_after_infinite_sample() {
    let mut filter = EuroFilter::new();
    filter.filter(0.5, DT_60FPS);
    filter.filter(f32::INFINITY, DT_60FPS);

    // Every subsequent output must stay finite; an accepted Inf would make the
    // next sample compute Inf - Inf and latch NaN permanently.
    for _ in 0..10 {
        let out = filter.filter(0.5, DT_60FPS);
        assert!(out.is_finite(), "filter output went non-finite: {}", out);
    }
}

#[test]
fn test_euro_filter_holds_last_value_on_non_finite_input() {
    let mut filter = EuroFilter::new();
    filter.filter(0.25, DT_60FPS);
    let good = filter.filter(0.25, DT_60FPS);

    assert_eq!(filter.filter(f32::NAN, DT_60FPS), good);
    assert_eq!(filter.filter(f32::INFINITY, DT_60FPS), good);
    assert_eq!(filter.filter(f32::NEG_INFINITY, DT_60FPS), good);
}

#[test]
fn test_euro_filter_survives_extreme_dt() {
    let mut filter = EuroFilter::new();
    filter.filter(0.0, DT_60FPS);

    // A near-zero dt drives hz high enough to overflow the derivative term.
    let out = filter.filter(1.0, f32::MIN_POSITIVE);
    assert!(out.is_finite(), "extreme dt produced {}", out);

    let recovered = filter.filter(0.5, DT_60FPS);
    assert!(
        recovered.is_finite(),
        "filter did not recover: {}",
        recovered
    );
}

#[test]
fn test_euro_filter_hz_derived_from_dt() {
    let mut filter = EuroFilter::new();
    filter.filter(0.5, DT_60FPS);
    filter.filter(0.6, DT_60FPS);
    // After filtering with 60fps dt, internal hz should be ~60
    // We can't read hz directly, but we can verify behavior differs from 10Hz default
    // by checking that a small step at high hz produces less smoothing (higher alpha)
    // than the same step would at low hz

    let mut filter_fast = EuroFilter::new();
    let mut filter_slow = EuroFilter::new();

    // Initialize both
    filter_fast.filter(0.0, 1.0 / 120.0); // 120 Hz
    filter_slow.filter(0.0, 1.0 / 10.0); // 10 Hz

    // Same input step
    let fast_result = filter_fast.filter(1.0, 1.0 / 120.0);
    let slow_result = filter_slow.filter(1.0, 1.0 / 10.0);

    // At higher hz, alpha is smaller, so the filter responds more slowly
    // (more smoothing per sample, but more samples per second)
    assert!(
        fast_result < slow_result,
        "Higher sample rate should produce more per-sample smoothing: fast={}, slow={}",
        fast_result,
        slow_result
    );
}
