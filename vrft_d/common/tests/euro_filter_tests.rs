use common::EuroFilter;

#[test]
fn test_euro_filter_initialization() {
    let mut filter = EuroFilter::new();
    let first_val = 100.0;
    let filtered = filter.filter(first_val);
    assert_eq!(
        filtered, first_val,
        "First value should be passed through exactly"
    );
}

#[test]
fn test_euro_filter_derivative_no_spike() {
    let mut filter = EuroFilter::new();
    // First value
    filter.filter(0.0);

    // Second value
    // If dx was calculated against 0.0 (initialized) it would be fine.
    // If dx was calculated against uninitialized garbage or 0.0 incorrectly, we might see issues.
    // But mainly we want to ensure smoothness.

    let val2 = 1.0;
    let filtered2 = filter.filter(val2);

    // With default params (min_cutoff=1.0, beta=0.5, d_cutoff=0.1, hz=10.0)
    // dx = (1.0 - 0.0) * 10.0 = 10.0
    // edx low pass: alpha(10, 0.1) is small.
    // cutoff = 1.0 + 0.5 * |edx|
    // alpha(10, cutoff)

    // Just ensure it's not NaN and is reasonable.
    assert!(filtered2 > 0.0);
    assert!(filtered2 <= 1.0);
}

#[test]
fn test_euro_filter_nan_handling() {
    let mut filter = EuroFilter::new();
    let res = filter.filter(f32::NAN);
    assert_eq!(res, 0.0);
}
